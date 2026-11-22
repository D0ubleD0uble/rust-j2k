//! `ReportEntry::TruncatedTilePart` on every prefix of a file that ends in or
//! after a tile-part's `SOT` segment, with `strict` off.
//!
//! While the input ends inside the 12 bytes of the `SOT` segment, the
//! tile-part is not read as begun and no `TruncatedTilePart` is given: the
//! report is the one for the input that ends where the tile-part starts. From
//! byte 12 on, the whole `SOT` is there and the entry is given, a header cut
//! until the `SOD` marker is whole, and a data cut after it.

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{DecodeOptions, IncrementalDecoder, Refined, ReportEntry, decode_with};
use synth::{cod_body, qcd_none_body, seg, tile_part};

/// One 8x8 tile in two tile-parts, then `EOC`, and the offset of the second
/// tile-part. With `psot_zero`, that tile-part's `Psot` is 0 and `EOC` is left
/// off, as the codestream's last tile-part may be.
fn file(psot_zero: bool) -> (Vec<u8>, usize) {
    let mut out = [
        vec![0xFF, 0x4F],
        seg(
            0xFF51,
            &synth::siz_body([8, 8, 0, 0, 8, 8, 0, 0], &[[7, 1, 1]]),
        ),
        seg(0xFF52, &cod_body(1, 0, 4, 0)),
        seg(0xFF5C, &qcd_none_body(2, &[8])),
    ]
    .concat();
    out.extend(tile_part(0, 0, 2, &[], &[0; 2]));
    let at = out.len();
    out.extend(tile_part(0, 1, 2, &[], &[0; 2]));
    if psot_zero {
        out[at + 6..at + 10].fill(0);
    } else {
        out.extend([0xFF, 0xD9]);
    }
    (out, at)
}

/// The whole report, as text, so that one entry's absence or change shows.
fn shown(entries: &[ReportEntry]) -> Vec<String> {
    entries.iter().map(ToString::to_string).collect()
}

/// The `TruncatedTilePart` entries of a report: tile-part, `declared`,
/// `present`, `header_cut`, and the display text.
fn cuts(entries: &[ReportEntry]) -> Vec<(u8, Option<u64>, u64, bool, String)> {
    entries
        .iter()
        .filter_map(|entry| match *entry {
            ReportEntry::TruncatedTilePart {
                tile,
                tile_part,
                declared,
                present,
                header_cut,
                ..
            } => {
                assert_eq!(tile, 0);
                Some((tile_part, declared, present, header_cut, entry.to_string()))
            }
            _ => None,
        })
        .collect()
}

/// Every prefix from the start of tile-part 1 to the end of the file decodes
/// with `strict` off, and `refine` gives its report on a decoder fed the
/// prefix at once and on one fed a byte at a time. Before byte 12 of the
/// tile-part there is no `TruncatedTilePart`, and the report is the one at
/// byte 0. From byte 12 there is one, a header cut up to the `SOD`, and none
/// when the tile-part is whole.
#[test]
fn a_prefix_ending_inside_the_sot_gives_no_truncated_tile_part() {
    for psot_zero in [false, true] {
        let (bytes, at) = file(psot_zero);
        let options = DecodeOptions::default().with_strict(false);
        let before = decode_with(&bytes[..at], &options).unwrap();
        assert!(cuts(&before.report.entries).is_empty());
        let mut stepped = IncrementalDecoder::new(options.clone()).unwrap();
        stepped.feed(&bytes[..at]);
        let mut refined = 0;
        for length in at..=bytes.len() {
            let prefix = &bytes[..length];
            let into = length - at;
            let image = decode_with(prefix, &options).unwrap();
            let got = cuts(&image.report.entries);
            let psot = if psot_zero { None } else { Some(16) };
            // The tile-part is 16 bytes with its `Psot`, or runs to the end
            // when that is 0, and then it is cut whenever its header is.
            let expected: Vec<_> = if into < 12 || (into >= 14 && psot_zero) {
                vec![]
            } else if into < 14 {
                let text = match psot {
                    Some(_) => format!(
                        "tile 0 tile-part 1 is cut short in its header: {into} of its 16 bytes \
                         are there"
                    ),
                    None => format!(
                        "tile 0 tile-part 1 is cut short in its header: {into} bytes of it are \
                         there, and its `Psot` is 0"
                    ),
                };
                vec![(1, psot, into as u64, true, text)]
            } else if into < 16 {
                let text =
                    format!("tile 0 tile-part 1 is cut short: {into} of its 16 bytes are there");
                vec![(1, psot, into as u64, false, text)]
            } else {
                vec![]
            };
            assert_eq!(got, expected, "psot_zero {psot_zero}, +{into}");
            if into < 12 {
                // The report at the start of the tile-part: no entry is added
                // or changed by the cut `SOT`.
                assert_eq!(
                    shown(&image.report.entries),
                    shown(&before.report.entries),
                    "psot_zero {psot_zero}, +{into}"
                );
            }
            let mut fresh = IncrementalDecoder::new(options.clone()).unwrap();
            fresh.feed(prefix);
            match fresh.refine() {
                Ok(Refined::Image(again)) => {
                    assert_eq!(shown(&again.report.entries), shown(&image.report.entries));
                }
                other => panic!("psot_zero {psot_zero}, +{into}: {other:?}"),
            }
            if length > at {
                stepped.feed(&bytes[length - 1..length]);
            }
            match stepped.refine() {
                Ok(Refined::Image(again)) => {
                    assert_eq!(
                        shown(&again.report.entries),
                        shown(&image.report.entries),
                        "psot_zero {psot_zero}, +{into}"
                    );
                    refined += 1;
                }
                Ok(_) => {}
                Err(error) => panic!("psot_zero {psot_zero}, +{into}: {error}"),
            }
        }
        assert!(refined > 0, "the byte-fed decoder never refined");
    }
}
