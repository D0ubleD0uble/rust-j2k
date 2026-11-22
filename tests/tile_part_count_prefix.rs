//! `ReportEntry::TilePartCountMismatch` on every prefix of a file, with
//! `strict` off.
//!
//! `declared` is the count the tile's `SOT`s state, and `corrected` says
//! whether OpenJPEG's `TPsot == TNsot` correction applies, which holds every
//! tile to one part more. The correction rests on a `SOT` that can lie past
//! the tile the entry is for, so the entry is what the prefix holds: the
//! counts its `SOT`s state, the parts whose `SOD` it holds, and the
//! correction once it holds the whole `SOT` that sets it. A longer prefix
//! never changes a tile's `declared`, and never takes the correction back.
//!
//! OpenJPEG 2.5.0 makes the correction for the whole codestream: it raises
//! every tile's count (`opj_j2k_read_tile_header`, `j2k.c:9714`), so a tile
//! whose own `SOT`s are right is held to one part more as well. It does not
//! compare a tile's count with its parts, and decodes both files here with
//! exit 0, with and without `-allow-partial`, warning "Non conformant
//! codestream TPsot==TNsot" on `corrected_file()`.

#[path = "support/synth.rs"]
mod synth;

use std::collections::HashMap;

use rust_j2k::{DecodeOptions, Error, IncrementalDecoder, Refined, ReportEntry, decode_with};
use synth::{cod_body, qcd_none_body, seg, tile_part};

/// Two 8x8 tiles of one component and their tile-parts, then `EOC`.
fn file(parts: &[(u16, u8, u8)]) -> Vec<u8> {
    let mut out = [
        vec![0xFF, 0x4F],
        seg(
            0xFF51,
            &synth::siz_body([16, 8, 0, 0, 8, 8, 0, 0], &[[7, 1, 1]]),
        ),
        seg(0xFF52, &cod_body(1, 0, 4, 0)),
        seg(0xFF5C, &qcd_none_body(2, &[8])),
    ]
    .concat();
    for &(tile, part, count) in parts {
        out.extend(tile_part(tile, part, count, &[], &[0; 4]));
    }
    out.extend([0xFF, 0xD9]);
    out
}

/// The issue's file: every `SOT` states 2 parts, and tile 0 has a third,
/// with `TPsot == TNsot`. Tile 0 is the first to have its two parts, so the
/// look-ahead from there finds that third part and makes the correction:
/// tile 0 then has all its parts, and tile 1 has one fewer than it is held
/// to.
fn corrected_file() -> Vec<u8> {
    file(&[(0, 0, 2), (0, 1, 2), (1, 0, 2), (1, 1, 2), (0, 2, 2)])
}

/// Tile 1 states 3 parts and carries 2; no part has `TPsot == TNsot`.
fn uncorrected_file() -> Vec<u8> {
    file(&[(0, 0, 2), (0, 1, 2), (1, 0, 3), (1, 1, 3)])
}

fn options(strict: bool) -> DecodeOptions {
    DecodeOptions::default().with_strict(strict)
}

/// A count entry's fields: tile, declared, found, corrected.
type Count = (u32, u16, u16, bool);

/// The count entries of a report, checking each one's text.
fn counts(entries: &[ReportEntry]) -> Vec<Count> {
    entries
        .iter()
        .filter_map(|entry| match *entry {
            ReportEntry::TilePartCountMismatch {
                tile,
                declared,
                found,
                corrected,
                ..
            } => {
                let shown = if corrected {
                    format!(
                        "tile {tile} declares {declared} tile-part(s), {} with the `TPsot == \
                         TNsot` correction, and {found} were found",
                        declared + 1
                    )
                } else {
                    format!("tile {tile} declares {declared} tile-part(s) and {found} were found")
                };
                assert_eq!(entry.to_string(), shown);
                Some((tile, declared, found, corrected))
            }
            _ => None,
        })
        .collect()
}

/// The count entries the prefix should give, worked out from its bytes: by
/// tile, the count its whole `SOT`s state and the parts whose `SOD` it
/// holds, and the correction once it holds the whole `SOT` at `sets` (the
/// part with `TPsot == TNsot`, if the file has one).
fn expected(prefix: &[u8], sets: Option<usize>) -> Vec<Count> {
    let corrected = sets.is_some_and(|at| at + 12 <= prefix.len());
    let mut stated: HashMap<u32, u16> = HashMap::new();
    let mut found: HashMap<u32, u16> = HashMap::new();
    let mut at = prefix.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    while at + 12 <= prefix.len() && prefix[at..at + 2] == [0xFF, 0x90] {
        let tile = u32::from(u16::from_be_bytes([prefix[at + 4], prefix[at + 5]]));
        let psot = u32::from_be_bytes(prefix[at + 6..at + 10].try_into().unwrap());
        stated.insert(tile, u16::from(prefix[at + 11]));
        if at + 14 <= prefix.len() {
            *found.entry(tile).or_default() += 1;
        }
        at += psot as usize;
    }
    let mut out: Vec<Count> = found
        .into_iter()
        .filter_map(|(tile, found)| {
            let declared = stated[&tile];
            (found != declared + u16::from(corrected)).then_some((tile, declared, found, corrected))
        })
        .collect();
    out.sort_unstable();
    out
}

/// The `SOT` offsets of a file, by the `Psot` chain.
fn sots(bytes: &[u8]) -> Vec<usize> {
    let mut at = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    let mut out = Vec::new();
    while bytes[at..at + 2] == [0xFF, 0x90] {
        out.push(at);
        at += u32::from_be_bytes(bytes[at + 6..at + 10].try_into().unwrap()) as usize;
    }
    out
}

/// The files, each with the offset of the `SOT` that sets the correction.
fn cases() -> [(&'static str, Vec<u8>, Option<usize>); 2] {
    let corrected = corrected_file();
    let sets = sots(&corrected)[4];
    [
        ("corrected", corrected, Some(sets)),
        ("not corrected", uncorrected_file(), None),
    ]
}

/// Every prefix that holds the first `SOT` marker decodes with `strict`
/// off, and its count entries are what the prefix holds. `refine` gives the
/// same entries, on a decoder fed the prefix at once and on one fed a byte
/// at a time (when it decodes rather than waits), and `finish` gives the
/// whole file's. A tile's `declared` does not change as the prefix grows,
/// and `corrected` once set stays set.
#[test]
fn every_tile_part_count_mismatch_is_what_the_prefix_holds() {
    for (name, bytes, sets) in cases() {
        let first = sots(&bytes)[0];
        let mut decoder = IncrementalDecoder::new(options(false)).unwrap();
        decoder.feed(&bytes[..first + 1]);
        let mut declared: HashMap<u32, u16> = HashMap::new();
        let mut corrected = false;
        let mut kinds = [false; 2];
        let mut refined = 0;
        for length in first + 2..=bytes.len() {
            let prefix = &bytes[..length];
            decoder.feed(&bytes[length - 1..length]);
            let image = decode_with(prefix, &options(false))
                .unwrap_or_else(|e| panic!("{name}, {length}: {e}"));
            let got = counts(&image.report.entries);
            assert_eq!(got, expected(prefix, sets), "{name}, {length}");
            let mut fresh = IncrementalDecoder::new(options(false)).unwrap();
            fresh.feed(prefix);
            match fresh.refine() {
                Ok(Refined::Image(image)) => {
                    assert_eq!(counts(&image.report.entries), got, "{name}, {length}")
                }
                other => panic!("{name}, {length}: {other:?}"),
            }
            match decoder.refine() {
                Ok(Refined::Image(image)) => {
                    assert_eq!(counts(&image.report.entries), got, "{name}, {length}");
                    refined += 1;
                }
                Ok(_) => {}
                Err(error) => panic!("{name}, {length}: {error}"),
            }
            for &(tile, stated, _, raised) in &got {
                assert_eq!(*declared.entry(tile).or_insert(stated), stated, "{name}");
                assert!(
                    raised || !corrected,
                    "{name}, {length}: correction taken back"
                );
                corrected |= raised;
                kinds[usize::from(raised)] = true;
            }
        }
        let whole = counts(&decoder.finish().unwrap().report.entries);
        assert_eq!(whole, expected(&bytes, sets), "{name}");
        assert_eq!(kinds, [true, sets.is_some()], "{name}");
        assert!(refined > 0, "{name}: the byte-fed decoder never refined");
    }
}

/// The issue's file reports tile 1 with the count its `SOT`s state, and
/// says the correction holds it to one more.
#[test]
fn a_tile_held_to_the_corrected_count_reports_the_stated_one() {
    let image = decode_with(&corrected_file(), &options(false)).unwrap();
    assert_eq!(counts(&image.report.entries), [(1, 2, 2, true)]);
    let image = decode_with(&uncorrected_file(), &options(false)).unwrap();
    assert_eq!(counts(&image.report.entries), [(1, 3, 2, false)]);
}

/// With `strict` on, both files are refused, with the count the `SOT`s state
/// and, for the first, the count the correction holds the tile to.
#[test]
fn strict_refuses_with_the_stated_count() {
    for (bytes, text) in [
        (
            corrected_file(),
            "tile 1 declares 2 tile-parts, 3 with the `TPsot == TNsot` correction, but carries 2",
        ),
        (
            uncorrected_file(),
            "tile 1 declares 3 tile-parts but carries 2",
        ),
    ] {
        let error = decode_with(&bytes, &options(true)).unwrap_err();
        assert!(matches!(error, Error::Codestream(_)), "{error:?}");
        assert!(error.to_string().ends_with(text), "{error}");
    }
}
