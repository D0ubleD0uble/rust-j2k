//! A tile whose `SOT`s state two counts (`TNsot`), where the `TPsot ==
//! TNsot` look-ahead reads past the tile-part at fault.
//!
//! With `strict` on, the counts in "tile T declares A tile-parts in one
//! `SOT` and B in another" are raised by one when the look-ahead finds the
//! next part of the first tile to complete has `TPsot == TNsot`. That part
//! may lie past the tile-part at fault, so on a prefix the look-ahead's
//! answer depends on bytes that have not arrived. With `strict` off the
//! message gives the counts the two `SOT`s state, which the input holds
//! from the first prefix that is refused, so every longer prefix gives the
//! same error.
//!
//! OpenJPEG 2.5.0 decodes these files (it does not compare a tile's
//! counts); refusing them is the departure "a `TNsot` that differs between
//! a tile's parts" in `docs/openjpeg-departures.md`.

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{DecodeOptions, Error, IncrementalDecoder, decode_with, read_header_with};
use synth::{cod_body, qcd_none_body, seg, tile_part};

/// From the random tile-part generator of the PR #603 review (seed 23,
/// case 356): two tiles. Tile 1's part 0 says 2 parts, tile 0's part 0
/// says 3, tile 1's part 1 completes it (the look-ahead starts here), tile
/// 0's part 1 says 1 (the fault, from byte 170), and tile 1's part 2 has
/// `TPsot == TNsot` (its `SOT` is whole from byte 185).
const CASE_356: [u8; 238] = [
    0xFF, 0x4F, 0xFF, 0x51, 0x00, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x08, 0x20, 0x00, 0x00, 0x02, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x10, 0x00, 0x00, 0x02, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x07, 0x01, 0x01, 0x07, 0x01, 0x01,
    0xFF, 0x52, 0x00, 0x0E, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x04, 0x04, 0x00, 0x01, 0x00, 0x11,
    0xFF, 0x5C, 0x00, 0x07, 0x40, 0x40, 0x40, 0x40, 0x40, 0xFF, 0x90, 0x00, 0x0A, 0x00, 0x01, 0x00,
    0x00, 0x00, 0x29, 0x00, 0x02, 0xFF, 0x52, 0x00, 0x0C, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x04,
    0x04, 0x00, 0x01, 0xFF, 0x58, 0x00, 0x06, 0x00, 0x05, 0x81, 0x02, 0xFF, 0x93, 0x62, 0x81, 0x8F,
    0x72, 0x6B, 0xFF, 0x90, 0x00, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x12, 0x00, 0x03, 0xFF, 0x93,
    0x69, 0x0D, 0x20, 0x8A, 0xFF, 0x90, 0x00, 0x0A, 0x00, 0x01, 0x00, 0x00, 0x00, 0x1A, 0x01, 0x02,
    0xFF, 0x53, 0x00, 0x09, 0x00, 0x00, 0x01, 0x04, 0x04, 0x00, 0x01, 0xFF, 0x93, 0x60, 0xFF, 0x90,
    0x00, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0F, 0x01, 0x01, 0xFF, 0x93, 0x6B, 0xFF, 0x90, 0x00,
    0x0A, 0x00, 0x01, 0x00, 0x00, 0x00, 0x1A, 0x02, 0x02, 0xFF, 0x5C, 0x00, 0x07, 0x40, 0x40, 0x40,
    0x40, 0x40, 0xFF, 0x93, 0x1E, 0x87, 0x4D, 0xFF, 0x90, 0x00, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x19, 0x02, 0x01, 0xFF, 0x5C, 0x00, 0x07, 0x40, 0x40, 0x40, 0x40, 0x40, 0xFF, 0x93, 0x08, 0x27,
    0xFF, 0x90, 0x00, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0E, 0x03, 0x03, 0xFF, 0x93,
];

/// Two 8x8 tiles of one component, with the case 356 layout: tile 1 part 0
/// of 2, tile 0 part 0 of 3, tile 1 part 1 of 2, tile 0 part 1 of 1 with
/// `data` bytes, then tile 1 part 2 of `last`, and `EOC`. With `last` 2
/// the look-ahead raises the counts; with 3 it does not.
fn built(data: usize, last: u8) -> Vec<u8> {
    [
        vec![0xFF, 0x4F],
        seg(
            0xFF51,
            &synth::siz_body([16, 8, 0, 0, 8, 8, 0, 0], &[[7, 1, 1]]),
        ),
        seg(0xFF52, &cod_body(1, 0, 4, 0)),
        seg(0xFF5C, &qcd_none_body(2, &[8])),
        tile_part(1, 0, 2, &[], &[0; 4]),
        tile_part(0, 0, 3, &[], &[0; 4]),
        tile_part(1, 1, 2, &[], &[0; 4]),
        tile_part(0, 1, 1, &[], &vec![0; data]),
        tile_part(1, 2, last, &[], &[0; 4]),
        vec![0xFF, 0xD9],
    ]
    .concat()
}

fn options(strict: bool) -> DecodeOptions {
    DecodeOptions::default().with_strict(strict)
}

/// The files, each with the strict-off error of every prefix from the
/// first refused on, and the strict-on error of the whole file.
fn cases() -> Vec<(&'static str, Vec<u8>, &'static str, &'static str)> {
    let stated = "Codestream(\"tile 0 declares 1 tile-parts in one `SOT` and 3 in another\")";
    let raised = "Codestream(\"tile 0 declares 2 tile-parts in one `SOT` and 4 in another\")";
    vec![
        ("case 356", CASE_356.to_vec(), stated, raised),
        ("raised", built(4, 2), stated, raised),
        ("raised, a long part between", built(300, 2), stated, raised),
        (
            "not raised",
            built(4, 3),
            stated,
            "Codestream(\"tile 0 tile-part 1 of a declared 1 tile-parts\")",
        ),
    ]
}

/// With `strict` off, every prefix decodes until one fails, and every
/// longer one fails with the same error. A refusal under the sample limit
/// is passed over: an `IncrementalDecoder` waits on it, as the limit grows
/// with the bytes fed. `read_header_with`'s error is the decode's,
/// `IncrementalDecoder::header` fed a byte at a time agrees with
/// `read_header_with`, and the two tile-part walks agree on each prefix.
#[test]
fn every_prefix_from_the_first_refusal_gives_one_message() {
    for (name, bytes, want, _) in cases() {
        let mut decoder = IncrementalDecoder::new(options(false)).unwrap();
        let mut failed: Option<String> = None;
        let mut ok = false;
        for length in 1..=bytes.len() {
            let prefix = &bytes[..length];
            decoder.feed(&bytes[length - 1..length]);
            rust_j2k::fuzz::tile_parts_walks_agree(prefix);
            let decoded = decode_with(prefix, &options(false)).map(|_| ());
            let header = read_header_with(prefix, &options(false));
            if let Err(error) = &header
                && !matches!(error, Error::Incomplete(_))
            {
                let decoded = decoded.as_ref().err().map(|e| format!("{e:?}"));
                assert_eq!(decoded, Some(format!("{error:?}")), "{name}, {length}");
            }
            match (decoded, &failed) {
                (Err(Error::Incomplete(_)), None) if !ok => {}
                (Err(Error::Limit(_)), None) => {}
                (Ok(()), None) => ok = true,
                (Ok(()), Some(error)) => panic!("{name}, {length}: Ok after {error}"),
                (Err(error), None) => failed = Some(format!("{error:?}")),
                (Err(error), Some(first)) => {
                    assert_eq!(&format!("{error:?}"), first, "{name}, {length}")
                }
            }
            let header = match header {
                Ok(header) => Ok(Some(header)),
                Err(Error::Incomplete(_)) => Ok(None),
                Err(error) => Err(error),
            };
            assert_eq!(
                format!("{:?}", decoder.header()),
                format!("{header:?}"),
                "{name}, {length}"
            );
        }
        assert!(ok, "{name}: no prefix decoded");
        assert_eq!(failed.as_deref(), Some(want), "{name}");
    }
}

/// With `strict` on, the whole file's error keeps the raised counts where
/// the look-ahead raises them. Where it does not, the part past its count
/// is refused first.
#[test]
fn strict_keeps_the_raised_counts() {
    for (name, bytes, _, want) in cases() {
        let error = decode_with(&bytes, &options(true)).unwrap_err();
        assert_eq!(format!("{error:?}"), want, "{name}");
    }
}
