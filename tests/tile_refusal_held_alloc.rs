//! A tile refusal an incremental decoder holds does not buy the refused
//! work: while `refine` waits on it (`Waiting::TileError`) and `header`
//! holds it back, neither builds the refused tile's geometry or decodes it.
//!
//! The file is one tile of 1000 x 1000 whose own `COD` gives no levels and
//! 1x1 precincts: 10^6 precincts, over the precinct cap of
//! `SampleLimit::AtMost(2^24)`. It has no `EOC`, so the refusal is held for
//! as long as bytes may come. A decode under a ceiling that admits the tile
//! shows what building it costs: about 264 MB, where the calls that hold the
//! refusal peak at about 67 KB (2026-11-22).
//!
//! The peak is measured with a counting global allocator, so this file holds
//! a single test. The shim in `tests/support/alloc.rs` forwards every call to
//! `System` unchanged (see `tests/sample_limit_at_most_alloc.rs`).

#[path = "support/alloc.rs"]
mod alloc;
#[path = "support/synth.rs"]
mod synth;
use alloc::{CURRENT, PEAK};
use std::sync::atomic::Ordering;

use rust_j2k::{DecodeOptions, IncrementalDecoder, Refined, SampleLimit, Waiting, decode_with};

#[global_allocator]
static ALLOCATOR: alloc::PeakAlloc = alloc::PeakAlloc;

const SIDE: u32 = 1000;

/// The output image, 4 bytes a sample: what the calls that hold the
/// refusal stay well under.
const IMAGE: usize = 4 * (SIDE as usize) * (SIDE as usize);

/// The file: the one tile's tile-part, and a second of `filler` bytes of
/// data that is not yet whole, so the codestream stays open.
fn file(filler: usize) -> Vec<u8> {
    let siz = synth::siz_body([SIDE, SIDE, 0, 0, SIDE, SIDE, 0, 0], &[[7, 1, 1]]);
    let mut own = synth::cod_body(1, 0, 0, 0);
    own[0] = 1;
    own.push(0x00);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0)),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
    ]
    .concat();
    bytes.extend(synth::tile_part(0, 0, 2, &synth::seg(0xFF52, &own), &[0]));
    let mut last = synth::tile_part(0, 1, 2, &[], &vec![0; filler]);
    // A `Psot` past the end: the tile-part is still coming in.
    let psot = u32::try_from(last.len() + 1000).expect("a short tile-part");
    last[6..10].copy_from_slice(&psot.to_be_bytes());
    bytes.extend(last);
    bytes
}

/// The heap `run` holds at its peak, over what was live before.
fn peak<T>(run: impl FnOnce() -> T) -> (usize, T) {
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let out = run();
    (PEAK.load(Ordering::SeqCst) - before, out)
}

#[test]
fn a_held_tile_refusal_does_not_build_the_refused_tile() {
    let bytes = file(1 << 16);
    let options = DecodeOptions::default().with_sample_limit(SampleLimit::AtMost(1 << 24));
    let mut decoder = IncrementalDecoder::new(options).expect("valid options");
    let mut most = 0;
    let mut waits = 0;
    for piece in bytes.chunks(4096) {
        decoder.feed(piece);
        let (held, header) = peak(|| decoder.header());
        assert!(matches!(header, Ok(Some(_))), "{header:?}");
        let (refined, refine) = peak(|| decoder.refine());
        match refine {
            Ok(Refined::Waiting(Waiting::TileError { .. })) => waits += 1,
            Ok(Refined::Waiting(Waiting::MoreBytes { .. })) => {}
            other => panic!("{other:?}"),
        }
        most = most.max(held).max(refined);
    }
    // Each doubling of the bytes fed decodes, from 4 KiB up.
    assert!(waits >= 5, "{waits} waits");
    // Not a precinct of the refused tile, nor its image.
    assert!(most < IMAGE / 16, "{most} bytes at the peak");

    // Under a ceiling that admits the tile, its precincts are built.
    let admitted = DecodeOptions::default()
        .with_strict(false)
        .with_sample_limit(SampleLimit::AtMost(1 << 40));
    let (built, decoded) = peak(|| decode_with(&bytes, &admitted));
    assert!(decoded.is_ok(), "{:?}", decoded.err());
    assert!(
        built > 16 * IMAGE,
        "{built} bytes to build the tile, {most} held"
    );
    eprintln!("held: {most} bytes at the peak; the tile built: {built}");
}
