//! What the work limits do on a 32-bit target, where a tile that a 64-bit
//! target refuses as over the sample limit is past the address space.
//!
//! A tile of 520 x 512 in 1x1 precincts holds 266,240 precincts: past the
//! 2^18 floor of the precinct cap on every target, and past what a 32-bit
//! target can address for its precinct bookkeeping. There the refusal is
//! `AddressSpace` with nothing to retry with, under every sample limit,
//! rather than the `Precincts` refusal that `WorkLimit::Raised` admits on a
//! 64-bit target. This file tests both: the first on a 32-bit target (run in
//! CI on `wasm32-wasip1`), the second on a 64-bit one.

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{DecodeOptions, Error, LimitKind, WorkLimit, decode_with, read_header_with};

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
        WorkLimit::Default,
        WorkLimit::Raised(4),
        WorkLimit::AtMost(u64::MAX),
        WorkLimit::Unlimited,
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
        }
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
        assert_eq!(refusal.retry_with(), Some(WorkLimit::Raised(2)));
    }
    let raised = DecodeOptions::default().with_sample_limit(WorkLimit::Raised(2));
    read_header_with(&bytes, &raised).expect("Raised(2) admits the header");
    decode_with(&bytes, &raised).expect("Raised(2) admits the decode");
}
