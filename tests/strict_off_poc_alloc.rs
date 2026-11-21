//! With `strict` off, the packets a `POC` sequence reaches past the end of
//! the data are counted without being read. They take no memory of their
//! own: a raised packet work bound lets the walk count more of them, which
//! takes longer, but the decode's heap stays what a strict decode's is.
//!
//! The peak is measured with a counting global allocator, so this file holds a
//! single test: a second one running in parallel would count into the same
//! peak.
//!
//! A global allocator can only be written with `unsafe`. The shim in
//! `tests/support/alloc.rs` forwards every call to `System` unchanged, which
//! the project conventions allow (`.claude/rules/conventions.md`, "Stay
//! pure-Rust"). The library itself has no `unsafe`.

#[path = "support/alloc.rs"]
mod alloc;
use alloc::{CURRENT, PEAK};
use std::sync::atomic::Ordering;

use rust_j2k::{DecodeOptions, Error, Raise, WorkLimit, decode_with};

#[global_allocator]
static ALLOCATOR: alloc::PeakAlloc = alloc::PeakAlloc;

/// The heap `run` peaks at beyond what was held before it.
fn peak_of<T>(run: impl FnOnce() -> T) -> (T, usize) {
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let out = run();
    (out, PEAK.load(Ordering::SeqCst) - before)
}

/// `pocspent_2tiles_60000_559.j2k`: the second tile holds no data, and its
/// two identical LRCP volumes reach 3.354e7 packets, more than the packet
/// bitmap holds. Under `Raised(3)` a strict decode refuses the tile as
/// cut short; with `strict` off the walk counts the 3.354e7 packets past
/// the end and is refused for work while skipping the second volume's.
/// Kept in a set, those packets took about 800 MB; the strict decode peaks
/// at about 115 KB, and with `strict` off it now peaks within 1 KB of that.
#[test]
fn packets_past_the_end_take_no_memory_under_a_raised_bound() {
    const TIMES: usize = 2;
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/work_bound/pocspent_2tiles_60000_559.j2k"
    ))
    .expect("read the fixture");
    let options = DecodeOptions::default().with_packet_work_limit(WorkLimit::Raised(3));

    let (strict, strict_peak) = peak_of(|| decode_with(&bytes, &options));
    assert!(matches!(strict, Err(Error::Codestream(_))), "{strict:?}");

    let lenient = options.with_strict(false);
    let (result, peak) = peak_of(|| decode_with(&bytes, &lenient));
    match result {
        Err(Error::Limit(detail)) => {
            assert_eq!(detail.retry_with(), Some(Raise::Raised(6)), "{detail}")
        }
        other => panic!("{other:?}"),
    }
    assert!(
        peak <= TIMES * strict_peak,
        "with `strict` off the decode peaked at {peak} bytes, strict at {strict_peak}"
    );
}
