//! What the work limits do on a 32-bit target, where a tile that a 64-bit
//! target refuses as over the sample limit is past the address space.
//!
//! A tile of 520 x 512 in 1x1 precincts holds 266,240 precincts: past the
//! 2^18 floor of the precinct cap on every target, and past what a 32-bit
//! target can address for its precinct bookkeeping. There the refusal is
//! `AddressSpace` with nothing to retry with, under every sample limit,
//! rather than the `Precincts` refusal that `SampleLimit::Raised` admits on a
//! 64-bit target. This file tests both: the first on a 32-bit target (run in
//! CI on `wasm32-wasip1`), the second on a 64-bit one.

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{
    AmountUnit, DecodeOptions, Error, LimitKind, SampleLimit, decode_with, read_header_with,
};

/// One 520 x 512 tile with no wavelet levels and 1x1 precincts: 266,240 of
/// them, and an empty packet each.
fn precinct_tile() -> Vec<u8> {
    let (width, height) = (520, 512);
    synth::empty_tiles(
        [width, height, 0, 0, width, height, 0, 0],
        0,
        0,
        &[0],
        &[(width * height) as usize],
        0,
    )
}

#[cfg(not(target_pointer_width = "64"))]
#[test]
fn a_tile_over_the_address_space_is_refused_with_nothing_to_retry() {
    let bytes = precinct_tile();
    for limit in [
        SampleLimit::Default,
        SampleLimit::Raised(4),
        SampleLimit::AtMost(u64::MAX),
        SampleLimit::Unlimited,
    ] {
        let options = DecodeOptions::default().with_sample_limit(limit.clone());
        for result in [
            read_header_with(&bytes, &options).map(|_| ()),
            decode_with(&bytes, &options).map(|_| ()),
        ] {
            let Err(Error::Limit(refusal)) = result else {
                panic!("{limit:?}: {result:?}");
            };
            assert_eq!(refusal.kind(), LimitKind::AddressSpace, "{refusal}");
            assert!(!refusal.kind().is_adjustable(), "{refusal}");
            assert_eq!(refusal.retry_with(), None, "{refusal}");
            let at = refusal.refused_at().expect("the tile and its precincts");
            assert_eq!(
                (at.tile(), at.amount(), at.unit()),
                (0, 266_240, AmountUnit::Precincts),
                "{refusal}"
            );
        }
    }
}

/// A JP2 file of two tiles, the second over the code-block cap: 7200 x 7168
/// samples in 4x4 code-blocks is 3,225,600 of them. The first tile is 16
/// samples wide, so the image stays within the default sample limit on a
/// 64-bit target.
fn two_tiles_the_second_over_the_code_block_cap() -> Vec<u8> {
    let (side, height) = (7200, 7168);
    let codestream = synth::empty_tiles(
        [2 * side, height, side - 16, 0, side, height, 0, 0],
        0,
        0,
        &[],
        &[1, 1],
        0,
    );
    synth::jp2_file(
        &[
            synth::ihdr_for(&codestream),
            synth::bx(b"colr", &synth::colr_enum(17)),
        ],
        &[],
        &codestream,
        &[],
    )
}

/// The tile, its amount and the unit come as data on every target: on a
/// 64-bit one as `TileCodeBlocks`, on a 32-bit one as `AddressSpace`, whose
/// tile cap never rises above its floor.
#[test]
fn a_tile_over_the_code_block_cap_gives_its_tile_amount_and_unit() {
    let bytes = two_tiles_the_second_over_the_code_block_cap();
    let options = DecodeOptions::default();
    for result in [
        read_header_with(&bytes, &options).map(|_| ()),
        decode_with(&bytes, &options).map(|_| ()),
    ] {
        let Err(Error::Limit(refusal)) = result else {
            panic!("{result:?}");
        };
        let kind = if cfg!(target_pointer_width = "64") {
            LimitKind::TileCodeBlocks
        } else {
            LimitKind::AddressSpace
        };
        assert_eq!(refusal.kind(), kind, "{refusal}");
        let at = refusal.refused_at().expect("the tile and its code-blocks");
        assert_eq!(
            (at.tile(), at.amount(), at.unit()),
            (1, 3_225_600, AmountUnit::CodeBlocks),
            "{refusal}"
        );
    }
}

/// On a 64-bit target the same tile is only over the default cap: both
/// calls refuse it as `TilePrecincts`, and `Raised(2)` admits it.
#[cfg(target_pointer_width = "64")]
#[test]
fn the_same_tile_is_a_sample_limit_refusal_on_a_64_bit_target() {
    let bytes = precinct_tile();
    let options = DecodeOptions::default();
    for result in [
        read_header_with(&bytes, &options).map(|_| ()),
        decode_with(&bytes, &options).map(|_| ()),
    ] {
        let Err(Error::Limit(refusal)) = result else {
            panic!("the default admitted it: {result:?}");
        };
        assert_eq!(refusal.kind(), LimitKind::TilePrecincts, "{refusal}");
        assert_eq!(refusal.retry_with(), Some(rust_j2k::Raise::Raised(2)));
    }
    let raised = DecodeOptions::default().with_sample_limit(SampleLimit::Raised(2));
    read_header_with(&bytes, &raised).expect("Raised(2) admits the header");
    decode_with(&bytes, &raised).expect("Raised(2) admits the decode");
}
