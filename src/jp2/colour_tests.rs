//! `colr` and `bpcc` tests: the colour space the file declares and the box
//! shapes that are refused. Each goes through the public [`decode`].
//!
//! The accept/reject answers were measured with `opj_decompress` 2.5.0 on
//! the same box shapes (see the module docs of `colour`). The codestream is
//! the committed three-component 24x16 fixture; the boxes are built here.

use super::SIGNATURE;
use crate::{ColourSpace, Error, decode};

const CODESTREAM: &[u8] = include_bytes!("../../tests/fixtures/multicomponent_lossless.j2k");

fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = (8 + payload.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// `ihdr` for the fixture with the given `BPC` and `UnkC`.
fn ihdr(bpc: u8, unk_c: u8) -> Vec<u8> {
    let mut payload = 16u32.to_be_bytes().to_vec();
    payload.extend_from_slice(&24u32.to_be_bytes());
    payload.extend_from_slice(&3u16.to_be_bytes());
    payload.extend_from_slice(&[bpc, 7, unk_c, 0]);
    bx(b"ihdr", &payload)
}

/// A `colr` with an enumerated colour space.
fn enum_colr(enum_cs: u32) -> Vec<u8> {
    let mut payload = vec![1, 0, 0];
    payload.extend_from_slice(&enum_cs.to_be_bytes());
    bx(b"colr", &payload)
}

/// A `colr` with the given `METH` and bytes after `METH`, `PREC`, `APPROX`.
fn colr(method: u8, rest: &[u8]) -> Vec<u8> {
    let mut payload = vec![method, 0, 0];
    payload.extend_from_slice(rest);
    bx(b"colr", &payload)
}

/// A file with `ihdr` (`BPC` 8-bit, `UnkC` 0) and `children` in `jp2h`, then
/// `between` at the top level, then the codestream.
fn file_with(ihdr_box: Vec<u8>, children: &[Vec<u8>], between: &[Vec<u8>]) -> Vec<u8> {
    let mut out = SIGNATURE.to_vec();
    out.extend_from_slice(&bx(b"ftyp", b"jp2 \0\0\0\0jp2 "));
    let mut inner = ihdr_box;
    inner.extend_from_slice(&children.concat());
    out.extend_from_slice(&bx(b"jp2h", &inner));
    out.extend_from_slice(&between.concat());
    out.extend_from_slice(&bx(b"jp2c", CODESTREAM));
    out
}

fn with(children: &[Vec<u8>]) -> Vec<u8> {
    file_with(ihdr(7, 0), children, &[])
}

#[track_caller]
fn colour_of(bytes: &[u8]) -> ColourSpace {
    decode(bytes)
        .unwrap_or_else(|e| panic!("decode failed: {e}"))
        .jp2
        .expect("a JP2 file carries metadata")
        .colour
}

#[track_caller]
fn refused(bytes: &[u8], fragment: &str) {
    match decode(bytes) {
        Err(Error::FileFormat(message)) => assert!(
            message.contains(fragment),
            "expected a FileFormat error mentioning {fragment:?}, got {message:?}"
        ),
        other => panic!("expected FileFormat mentioning {fragment:?}, got {other:?}"),
    }
}

#[test]
fn enumerated_colour_spaces_map_to_their_variants() {
    assert_eq!(colour_of(&with(&[enum_colr(16)])), ColourSpace::Srgb);
    assert_eq!(colour_of(&with(&[enum_colr(17)])), ColourSpace::Greyscale);
    assert_eq!(colour_of(&with(&[enum_colr(18)])), ColourSpace::Sycc);
    for other in [0, 12, 14, 15, 19, 20, 21, 24, u32::MAX] {
        assert_eq!(
            colour_of(&with(&[enum_colr(other)])),
            ColourSpace::Enumerated(other)
        );
    }
}

#[test]
fn a_file_without_colr_has_an_unknown_colour_space() {
    assert_eq!(colour_of(&with(&[])), ColourSpace::Unknown);
}

#[test]
fn prec_approx_and_extra_bytes_do_not_change_the_enumerated_value() {
    let long = bx(b"colr", &[1, 5, 3, 0, 0, 0, 17, 9, 9, 9, 9]);
    assert_eq!(colour_of(&with(&[long])), ColourSpace::Greyscale);
}

#[test]
fn the_icc_profile_is_the_bytes_after_the_three_header_bytes() {
    let profile: Vec<u8> = (0..=255).collect();
    assert_eq!(
        colour_of(&with(&[colr(2, &profile)])),
        ColourSpace::RestrictedIcc(profile)
    );
    // A profile too short to be one is passed through, as OpenJPEG does.
    assert_eq!(
        colour_of(&with(&[colr(2, &[1, 2, 3, 4, 5])])),
        ColourSpace::RestrictedIcc(vec![1, 2, 3, 4, 5])
    );
}

#[test]
fn the_first_usable_colr_wins() {
    assert_eq!(
        colour_of(&with(&[enum_colr(17), enum_colr(16)])),
        ColourSpace::Greyscale
    );
    assert_eq!(
        colour_of(&with(&[colr(2, &[7, 7, 7]), enum_colr(16)])),
        ColourSpace::RestrictedIcc(vec![7, 7, 7])
    );
}

#[test]
fn a_method_other_than_1_or_2_is_ignored_and_does_not_count_as_the_first() {
    assert_eq!(colour_of(&with(&[colr(3, b"abcd")])), ColourSpace::Unknown);
    for method in [0, 3, 4, 255] {
        assert_eq!(
            colour_of(&with(&[colr(method, &[0, 0, 0, 16]), enum_colr(18)])),
            ColourSpace::Sycc,
            "METH {method} then a usable box"
        );
    }
}

#[test]
fn unknown_colourspace_in_ihdr_overrides_colr() {
    let bytes = file_with(ihdr(7, 1), &[enum_colr(16)], &[]);
    assert_eq!(colour_of(&bytes), ColourSpace::Unknown);
    let bytes = file_with(ihdr(7, 1), &[colr(2, &[1, 2, 3, 4])], &[]);
    assert_eq!(colour_of(&bytes), ColourSpace::Unknown);
    // Only 1 means unknown; OpenJPEG reads other values like 0.
    let bytes = file_with(ihdr(7, 2), &[enum_colr(16)], &[]);
    assert_eq!(colour_of(&bytes), ColourSpace::Srgb);
    // A malformed box is still refused.
    refused(&file_with(ihdr(7, 1), &[bx(b"colr", &[])], &[]), "`colr`");
}

#[test]
fn every_colr_needs_three_bytes_even_one_that_is_ignored() {
    for short in [vec![], vec![1], vec![1, 0]] {
        let bad = bx(b"colr", &short);
        refused(&with(std::slice::from_ref(&bad)), "at least 3");
        refused(&with(&[enum_colr(16), bad.clone()]), "at least 3");
        // Misplaced at the top level after `jp2h`, which OpenJPEG applies.
        refused(
            &file_with(ihdr(7, 0), &[enum_colr(16)], std::slice::from_ref(&bad)),
            "at least 3",
        );
    }
    // Three bytes pass the size rule: a later `METH 1` box with no `EnumCS`
    // is ignored whole.
    assert_eq!(
        colour_of(&with(&[enum_colr(16), bx(b"colr", &[1, 0, 0])])),
        ColourSpace::Srgb
    );
}

#[test]
fn the_first_usable_colr_can_be_a_misplaced_one() {
    let bytes = file_with(ihdr(7, 0), &[], &[enum_colr(17)]);
    assert_eq!(colour_of(&bytes), ColourSpace::Greyscale);
    // Inside `jp2h` first, then the misplaced one is a later box.
    let bytes = file_with(ihdr(7, 0), &[enum_colr(16)], &[enum_colr(17)]);
    assert_eq!(colour_of(&bytes), ColourSpace::Srgb);
    // A skipped `METH 3` inside, so the misplaced one is the first usable.
    let bytes = file_with(ihdr(7, 0), &[colr(3, &[0])], &[enum_colr(17)]);
    assert_eq!(colour_of(&bytes), ColourSpace::Greyscale);
}

#[test]
fn a_first_colr_too_short_for_its_method_is_refused() {
    refused(&with(&[bx(b"colr", &[1, 0, 0])]), "`METH` 1");
    refused(&with(&[bx(b"colr", &[1, 0, 0, 0, 0, 0])]), "`METH` 1");
    refused(&with(&[colr(2, &[])]), "no ICC profile");
    // A skipped box does not shield a later empty profile.
    refused(&with(&[colr(3, &[]), colr(2, &[])]), "no ICC profile");
    // A box that is ignored because an earlier one counted is never parsed
    // past its size.
    assert_eq!(
        colour_of(&with(&[enum_colr(17), colr(2, &[])])),
        ColourSpace::Greyscale
    );
    assert_eq!(
        colour_of(&with(&[enum_colr(17), bx(b"colr", &[1, 0, 0, 0, 0])])),
        ColourSpace::Greyscale
    );
}

#[test]
fn a_bpcc_must_hold_one_byte_per_component() {
    let bpcc = |bytes: &[u8]| bx(b"bpcc", bytes);
    // BPC 255: the depths are in `bpcc`.
    refused(&file_with(ihdr(255, 0), &[bpcc(&[7, 7])], &[]), "`bpcc`");
    refused(&file_with(ihdr(255, 0), &[bpcc(&[])], &[]), "`bpcc`");
    refused(
        &file_with(ihdr(255, 0), &[bpcc(&[7, 7, 7, 7])], &[]),
        "`bpcc`",
    );
    // OpenJPEG checks the length whatever BPC says.
    refused(&file_with(ihdr(7, 0), &[bpcc(&[7, 7])], &[]), "`bpcc`");
    // Every `bpcc`, not just the first, and a misplaced one too.
    refused(
        &file_with(ihdr(255, 0), &[bpcc(&[7, 7, 7]), bpcc(&[7])], &[]),
        "`bpcc`",
    );
    refused(
        &file_with(ihdr(255, 0), &[bpcc(&[7, 7, 7])], &[bpcc(&[7, 7])]),
        "`bpcc`",
    );
}

#[test]
fn bpcc_depths_do_not_change_what_siz_says() {
    let plain = decode(&with(&[enum_colr(16)])).expect("decodes");
    for (bpc, bpcc) in [(255, [11, 11, 11]), (255, [0, 12, 3]), (7, [0, 0, 0])] {
        let bytes = file_with(ihdr(bpc, 0), &[bx(b"bpcc", &bpcc), enum_colr(16)], &[]);
        let image = decode(&bytes).expect("decodes");
        assert_eq!(image.components, plain.components);
    }
    // BPC 255 with no `bpcc` at all decodes too.
    assert_eq!(
        decode(&file_with(ihdr(255, 0), &[enum_colr(16)], &[]))
            .expect("decodes")
            .components,
        plain.components
    );
}

#[test]
fn a_malformed_colr_is_refused_before_an_unsupported_box() {
    let bytes = with(&[bx(b"pclr", &[0, 1]), bx(b"colr", &[])]);
    refused(&bytes, "`colr`");
    let bytes = with(&[bx(b"cdef", &[0, 0]), bx(b"bpcc", &[7])]);
    refused(&bytes, "`bpcc`");
}

/// The nine ISO/IEC 15444-4 JP2 reader files, read through the same box walk
/// `decode` uses. Files 2 and 9 carry `cdef` / `pclr`, which `decode` still
/// refuses as unsupported before it returns metadata, so this checks the
/// parsed colour space for all nine and `tests/jp2_colour.rs` checks `decode`
/// for the rest.
#[test]
fn the_conformance_files_declare_the_expected_colour_spaces() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/conformance/jp2/files");
    let expected = [
        ColourSpace::Srgb,
        ColourSpace::Sycc,
        ColourSpace::Sycc,
        ColourSpace::Greyscale,
        ColourSpace::Unknown, // file5: ICC, checked below
        ColourSpace::Greyscale,
        ColourSpace::Unknown, // file7
        ColourSpace::Unknown, // file8
        ColourSpace::Srgb,
    ];
    let icc_lengths = [(5, 546), (7, 13332), (8, 414)];
    for (i, want) in expected.iter().enumerate() {
        let n = i + 1;
        let bytes = std::fs::read(dir.join(format!("file{n}.jp2")))
            .unwrap_or_else(|e| panic!("file{n}: {e}"));
        let parsed = super::parse(&bytes).unwrap_or_else(|e| panic!("file{n}: {e}"));
        match icc_lengths.iter().find(|(file, _)| *file == n) {
            Some((_, len)) => match &parsed.colour {
                ColourSpace::RestrictedIcc(profile) => {
                    assert_eq!(profile.len(), *len, "file{n}");
                    // The profile is the first `colr` box's payload less its
                    // three header bytes.
                    assert_eq!(profile.as_slice(), &parsed.header.colr[0][3..], "file{n}");
                }
                other => panic!("file{n}: expected RestrictedIcc, got {other:?}"),
            },
            None => assert_eq!(&parsed.colour, want, "file{n}"),
        }
    }
}

/// `bpcc` 12,12,12,0 in a real Kakadu-style file: the box is accepted and the
/// header parses. `decode` of it stops at the `cdef` guard (see
/// `tests/jp2_real.rs`), so this reads the box walk directly.
#[test]
fn a_real_file_with_a_bpcc_box_parses() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/jp2_real/3_13bit_and_1bit.jp2");
    let bytes = std::fs::read(path).expect("fixture");
    let parsed = super::parse(&bytes).expect("parses");
    assert_eq!(parsed.header.bpcc, vec![&[12, 12, 12, 0][..]]);
    assert_eq!(parsed.header.ihdr.bits_per_component, 255);
}
