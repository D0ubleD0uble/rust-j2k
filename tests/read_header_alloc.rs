//! `read_header` allocates nothing sized by the image: a header declaring an
//! 8000x8000 image (64 M samples, inside the decode guard, so `decode` would
//! allocate its 256 MiB plane) is read with a few kilobytes of heap.
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

const SIDE: u32 = 8000;

/// The committed one-component codestream with its SIZ image and tile set to
/// `SIDE` x `SIDE`, cut right after its first `SOT` marker.
fn big_header() -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/regular_latlon_lossy.j2k"
    );
    let mut bytes = std::fs::read(path).expect("read the fixture");
    // SOC, then SIZ: marker, Lsiz, Rsiz, then Xsiz at 8, Ysiz, XOsiz, YOsiz,
    // XTsiz, YTsiz, XTOsiz, YTOsiz.
    assert_eq!(&bytes[2..4], &[0xFF, 0x51]);
    for at in [8, 12, 24, 28] {
        bytes[at..at + 4].copy_from_slice(&SIDE.to_be_bytes());
    }
    for at in [16, 20, 32, 36] {
        bytes[at..at + 4].copy_from_slice(&0u32.to_be_bytes());
    }
    let sot = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    bytes.truncate(sot + 2);
    bytes
}

/// The same codestream in a minimal JP2 file with a matching `ihdr`.
fn in_jp2(codestream: &[u8]) -> Vec<u8> {
    let bx = |kind: &[u8; 4], payload: &[u8]| {
        let mut out = (8 + payload.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    };
    let mut ihdr = SIDE.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&SIDE.to_be_bytes());
    ihdr.extend_from_slice(&[0, 1, 7, 7, 0, 0]);
    let jp2h = [bx(b"ihdr", &ihdr), bx(b"colr", &[1, 0, 0, 0, 0, 0, 17])].concat();
    [
        vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A],
        bx(b"ftyp", b"jp2 \0\0\0\0jp2 "),
        bx(b"jp2h", &jp2h),
        bx(b"jp2c", codestream),
    ]
    .concat()
}

#[test]
fn read_header_allocates_nothing_sized_by_the_image() {
    let codestream = big_header();
    let file = in_jp2(&codestream);
    for (what, bytes) in [("codestream", &codestream), ("JP2 file", &file)] {
        let before = CURRENT.load(Ordering::SeqCst);
        PEAK.store(before, Ordering::SeqCst);
        let header = rust_j2k::read_header(bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
        let peak = PEAK.load(Ordering::SeqCst) - before;
        assert_eq!((header.width, header.height), (SIDE, SIDE), "{what}");
        assert!(
            peak < 64 * 1024,
            "{what}: read_header peaked at {peak} bytes"
        );
    }
}
