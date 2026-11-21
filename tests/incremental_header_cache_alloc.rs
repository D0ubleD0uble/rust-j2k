//! `IncrementalDecoder::header` keeps nothing between calls for a file
//! that has no tile over a cap, whatever its main header holds, and the
//! decoder itself stays small.
//!
//! The file is the worst main header for the parsed form: one tile of
//! 64 x 64 samples and 256 `PPM` markers of about 64 KiB each, 16.8 MB in
//! all, holding about 4.19 million `Nppm` records of no packet headers.
//! Parsed, each record is a `Vec` of 24 bytes, so a copy of the parsed
//! header would be 100 MB, six times the input.
//!
//! The bytes are counted with a counting global allocator, so this file
//! holds a single test: a second one running in parallel would count into
//! the same totals. The shim is in `tests/support/alloc.rs`.

#[path = "support/alloc.rs"]
mod alloc;
#[path = "support/synth.rs"]
mod synth;
use alloc::CURRENT;
use std::sync::atomic::Ordering;

use rust_j2k::{DecodeOptions, IncrementalDecoder, read_header_with};

#[global_allocator]
static ALLOCATOR: alloc::PeakAlloc = alloc::PeakAlloc;

fn ppm_file() -> Vec<u8> {
    let siz = synth::siz_body([64, 64, 0, 0, 64, 64, 0, 0], &[[7, 1, 1]]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0)),
        synth::seg(0xFF5C, &synth::qcd_none_body(0, &[8])),
    ]
    .concat();
    for index in 0..=255u8 {
        // `Zppm`, then records of an `Nppm` of 0 and no bytes.
        let mut body = vec![index];
        body.resize(65531, 0);
        bytes.extend(synth::seg(0xFF60, &body));
    }
    bytes.extend(synth::tile_part(0, 0, 1, &[], &[]));
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

#[test]
fn header_keeps_nothing_for_a_file_with_no_tile_over_a_cap() {
    // The decoder was 448 bytes before the cache, and 896 with it inline.
    let size = size_of::<IncrementalDecoder>();
    println!("size_of IncrementalDecoder: {size}");
    let bytes = ppm_file();
    assert!(bytes.len() > 16_000_000, "{} bytes", bytes.len());
    let lenient = DecodeOptions::default().with_strict(false);
    let want = read_header_with(&bytes, &lenient).expect("a header");
    let mut decoder = IncrementalDecoder::new(lenient).unwrap();
    decoder.feed(&bytes);
    let base = CURRENT.load(Ordering::SeqCst);
    // Measured first and printed after: the captured output is heap too.
    let mut helds = [0; 3];
    for (call, slot) in helds.iter_mut().enumerate() {
        let got = decoder.header().expect("a header").expect("a header");
        assert_eq!(got, want, "call {call}");
        *slot = CURRENT.load(Ordering::SeqCst) - base;
    }
    for (call, held) in helds.into_iter().enumerate() {
        println!("call {call}: {held} bytes held after the call");
        assert!(held <= 64, "call {call}: {held} bytes held");
    }
    assert!(size <= 512, "{size} bytes");
}
