//! A small file that declares a huge image is refused without allocating
//! anything sized by the image: over the default sample limit, and, under
//! `WorkLimit::Unlimited`, past what the target can address.
//!
//! The peak is measured with a counting global allocator, so this file holds a
//! single test: a second one running in parallel would count into the same
//! peak.
//!
//! A global allocator can only be written with `unsafe`. This one is a shim in
//! a test binary that forwards every call to `System` unchanged, which the
//! project conventions allow (`.claude/rules/conventions.md`, "Stay
//! pure-Rust"). The library itself has no `unsafe`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use rust_j2k::{DecodeOptions, Error, WorkLimit, decode_with};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call forwards to `System` unchanged; the counters only
// observe the sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let now = CURRENT.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
        PEAK.fetch_max(now, Ordering::SeqCst);
        // SAFETY: the caller's contract for `alloc` is passed through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Ordering::SeqCst);
        // SAFETY: the caller's contract for `dealloc` is passed through.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The committed one-component codestream with its SIZ image and tile set to
/// `side` x `side`, cut right after its first `SOT` marker.
fn header(side: u32) -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/regular_latlon_lossy.j2k"
    );
    let mut bytes = std::fs::read(path).expect("read the fixture");
    // SOC, then SIZ: marker, Lsiz, Rsiz, then Xsiz at 8, Ysiz, XOsiz, YOsiz,
    // XTsiz, YTsiz, XTOsiz, YTOsiz.
    assert_eq!(&bytes[2..4], &[0xFF, 0x51]);
    for at in [8, 12, 24, 28] {
        bytes[at..at + 4].copy_from_slice(&side.to_be_bytes());
    }
    for at in [16, 20, 32, 36] {
        bytes[at..at + 4].copy_from_slice(&0u32.to_be_bytes());
    }
    let sot = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    bytes.truncate(sot + 2);
    bytes
}

#[test]
fn a_huge_declared_image_is_refused_without_allocating_it() {
    let cases = [
        // 20000^2 = 4e8 samples, 1.6 GB as i32: over the default, and
        // addressable on a 32-bit target too.
        (
            "over the default",
            header(20_000),
            WorkLimit::Default,
            "sample limit",
        ),
        // (2^32 - 1)^2 samples: more than any target can address.
        (
            "past the address space",
            header(u32::MAX),
            WorkLimit::Unlimited,
            "address",
        ),
    ];
    for (what, bytes, limit, says) in cases {
        let options = DecodeOptions::default().with_sample_limit(limit);
        let before = CURRENT.load(Ordering::SeqCst);
        PEAK.store(before, Ordering::SeqCst);
        let result = decode_with(&bytes, &options);
        let peak = PEAK.load(Ordering::SeqCst) - before;
        match result {
            Err(Error::Limit(m)) => assert!(m.contains(says), "{what}: {m}"),
            other => panic!("{what}: {:?}", other.map(|i| (i.width, i.height))),
        }
        assert!(
            peak < 64 * 1024,
            "{what}: the refusal peaked at {peak} bytes"
        );
    }
}
