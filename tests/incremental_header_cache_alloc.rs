//! `IncrementalDecoder::header` keeps nothing between calls for a file whose
//! tiles' sizes show them within the caps, whatever its main header holds,
//! only the counts of a file whose tiles' sizes do not, and the decoder
//! itself stays small.
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

use rust_j2k::{DecodeOptions, Header, IncrementalDecoder, read_header_with};

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
        // `Zppm`, then records of an `Nppm` of 0 and no bytes: 16,382 whole
        // records, as a segment may not end inside an `Nppm` (A.7.4).
        let mut body = vec![index];
        body.resize(1 + 4 * 16_382, 0);
        bytes.extend(synth::seg(0xFF60, &body));
    }
    bytes.extend(synth::tile_part(0, 0, 1, &[], &[]));
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// One tile of 1024 x 1024 samples and no wavelet levels: its size does not
/// show it within the caps, so its counts under the main header are kept.
fn big_tile_file() -> Vec<u8> {
    let siz = synth::siz_body([1024, 1024, 0, 0, 1024, 1024, 0, 0], &[[7, 1, 1]]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0)),
        synth::seg(0xFF5C, &synth::qcd_none_body(0, &[8])),
    ]
    .concat();
    bytes.extend(synth::tile_part(0, 0, 1, &[], &[]));
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// A decoder whose `header_kept` mutex has been locked once, with nothing fed.
/// Where std's `Mutex` is pthread-based (macOS, for one), it boxes its
/// `pthread_mutex_t` on the first lock (64 bytes on aarch64 macOS), which is
/// not the cache's, so it is taken before the heap is measured.
fn warm_decoder(options: DecodeOptions) -> IncrementalDecoder {
    let decoder = IncrementalDecoder::new(options).unwrap();
    assert!(decoder.header().expect("no bytes, no error").is_none());
    decoder
}

/// The heap `decoder` holds after each of 3 calls of `header()`, over what
/// it held before the first, with each header checked against `want`.
fn held_after_calls(decoder: &IncrementalDecoder, want: &Header) -> [usize; 3] {
    let base = CURRENT.load(Ordering::SeqCst);
    let mut helds = [0; 3];
    for (call, slot) in helds.iter_mut().enumerate() {
        let got = decoder.header().expect("a header").expect("a header");
        assert_eq!(&got, want, "call {call}");
        // The header returned is heap too.
        drop(got);
        *slot = CURRENT.load(Ordering::SeqCst).saturating_sub(base);
    }
    helds
}

#[test]
fn header_holds_what_the_files_tiles_need() {
    // The decoder was 448 bytes before the cache, and 896 with it inline.
    let size = size_of::<IncrementalDecoder>();
    assert!(size <= 512, "{size} bytes");
    let lenient = DecodeOptions::default().with_strict(false);

    // Tiles whose sizes show them within the caps: nothing is kept, though
    // the main header is 16.8 MB of `PPM` data.
    let bytes = ppm_file();
    assert!(bytes.len() > 16_000_000, "{} bytes", bytes.len());
    let want = read_header_with(&bytes, &lenient).expect("a header");
    let mut decoder = warm_decoder(lenient.clone());
    decoder.feed(&bytes);
    let none = held_after_calls(&decoder, &want);
    drop(decoder);

    // A tile that is not: its counts are kept, a few hundred bytes.
    let bytes = big_tile_file();
    let want = read_header_with(&bytes, &lenient).expect("a header");
    let mut decoder = warm_decoder(lenient);
    decoder.feed(&bytes);
    let counts = held_after_calls(&decoder, &want);

    // Printed after the measures: the captured output is heap too.
    println!("size {size}; held {none:?} (PPM file), {counts:?} (big tile)");
    assert_eq!(none, [0; 3]);
    for held in counts {
        assert!((1..=4096).contains(&held), "{held} bytes held");
    }
}
