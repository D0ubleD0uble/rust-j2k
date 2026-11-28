//! `IncrementalDecoder::header` copies no ICC profile bytes for a `colr` of
//! `METH` 2 after the codestream.
//!
//! Whether such a box passes depends only on its method and on the profile
//! being non-empty, so the walk after the codestream decides that from the
//! first bytes. The box here has length 0, so it runs to the end of the
//! input and is read again by each call. A profile of 1 MiB copied on each
//! call would cost O(n) a call and O(n^2) over a feed.
//!
//! The bytes are counted with a counting global allocator, so this file
//! holds a single test: a second one running in parallel would count into
//! the same totals. The shim is in `tests/support/alloc.rs`.

#[path = "support/alloc.rs"]
mod alloc;
#[path = "support/synth.rs"]
mod synth;
use alloc::{CURRENT, PEAK};
use std::sync::atomic::Ordering;

use rust_j2k::{DecodeOptions, IncrementalDecoder};

#[global_allocator]
static ALLOCATOR: alloc::PeakAlloc = alloc::PeakAlloc;

const PROFILE: usize = 1 << 20;

/// A gray JP2 file of one 4 x 4 component, then a `colr` of length 0 whose
/// `METH` 2 profile is `PROFILE` bytes.
fn file() -> Vec<u8> {
    let siz = synth::siz_body([4, 4, 0, 0, 4, 4, 0, 0], &[[7, 1, 1]]);
    let mut codestream = vec![0xFF, 0x4F];
    codestream.extend(synth::seg(0xFF51, &siz));
    codestream.extend(synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0)));
    codestream.extend(synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])));
    codestream.extend(synth::tile_part(0, 0, 1, &[], &[0]));
    codestream.extend([0xFF, 0xD9]);
    // A `colr` of `METH` 3 is ignored, so none has been taken before the
    // one after the codestream: that is the one a copy would be made for.
    let ignored = synth::bx(b"colr", &[3, 0, 0]);
    let mut bytes = synth::jp2_file(
        &[synth::ihdr_for(&codestream), ignored],
        &[],
        &codestream,
        &[],
    );
    bytes.extend([0, 0, 0, 0]);
    bytes.extend(b"colr");
    bytes.extend(synth::colr_icc(&vec![7; PROFILE]));
    bytes
}

#[test]
fn a_header_call_copies_no_profile_after_the_codestream() {
    let bytes = file();
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes);
    // A first call, so what a later one reuses is in place.
    decoder.header().expect("the file reads");
    let mut extra = Vec::new();
    for _ in 0..3 {
        let base = CURRENT.load(Ordering::SeqCst);
        PEAK.store(base, Ordering::SeqCst);
        let header = decoder.header().expect("the file reads");
        assert!(header.is_some());
        extra.push(PEAK.load(Ordering::SeqCst) - base);
    }
    println!("extra live bytes at the peak of a call: {extra:?}");
    for bytes in extra {
        assert!(
            bytes < PROFILE / 16,
            "a call held {bytes} more bytes at its peak, on a profile of {PROFILE}"
        );
    }
}
