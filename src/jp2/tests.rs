//! JP2 box-layer tests: what decodes, and the typed error for every malformed
//! or unsupported box shape. Each test goes through the public [`decode`], so
//! the container sniffing and the glue are under test too.
//!
//! The codestream is a committed lossless fixture (24x16, three 8-bit
//! components); the boxes around it are built here byte by byte.

use super::SIGNATURE;
use crate::{
    Association, ChannelDefinition, ChannelType, ColourSpace, DecodeOptions, Error, Image,
    Jp2Metadata, Ratio, Res, Resolution, UuidBox, decode, decode_with, read_header,
};

mod palette;

const CODESTREAM: &[u8] = include_bytes!("../../tests/fixtures/multicomponent_lossless.j2k");
const WIDTH: u32 = 24;
const HEIGHT: u32 = 16;
const COMPONENTS: u16 = 3;

/// A box with a 32-bit `LBox`.
fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let length = u32::try_from(8 + payload.len()).expect("test box fits LBox");
    let mut out = length.to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// A box with `LBox = 1` and a 64-bit `XLBox`.
fn xl_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = 1u32.to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(&(16 + payload.len() as u64).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// A box with `LBox = 0`: it runs to the end of the file.
fn open_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = 0u32.to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// A box header claiming `lbox` bytes, with no contents after it.
fn header_only(lbox: u32, kind: &[u8; 4]) -> Vec<u8> {
    let mut out = lbox.to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out
}

/// A box header with `LBox = 1` and the given `XLBox`, with no contents.
fn xl_header_only(xlbox: u64, kind: &[u8; 4]) -> Vec<u8> {
    let mut out = header_only(1, kind);
    out.extend_from_slice(&xlbox.to_be_bytes());
    out
}

fn ftyp(brand: &[u8; 4], compatible: &[&[u8; 4]]) -> Vec<u8> {
    let mut payload = brand.to_vec();
    payload.extend_from_slice(&0u32.to_be_bytes()); // MinV
    for code in compatible {
        payload.extend_from_slice(*code);
    }
    bx(b"ftyp", &payload)
}

fn jp2_ftyp() -> Vec<u8> {
    ftyp(b"jp2 ", &[b"jp2 "])
}

fn ihdr(height: u32, width: u32, components: u16) -> Vec<u8> {
    let mut payload = height.to_be_bytes().to_vec();
    payload.extend_from_slice(&width.to_be_bytes());
    payload.extend_from_slice(&components.to_be_bytes());
    payload.extend_from_slice(&[7, 7, 0, 0]); // BPC 8-bit unsigned, C, UnkC, IPR
    bx(b"ihdr", &payload)
}

fn good_ihdr() -> Vec<u8> {
    ihdr(HEIGHT, WIDTH, COMPONENTS)
}

/// An enumerated sRGB colour specification.
fn colr() -> Vec<u8> {
    bx(b"colr", &[1, 0, 0, 0, 0, 0, 16])
}

fn jp2h(children: &[Vec<u8>]) -> Vec<u8> {
    bx(b"jp2h", &children.concat())
}

fn good_jp2h() -> Vec<u8> {
    jp2h(&[good_ihdr(), colr()])
}

fn jp2c() -> Vec<u8> {
    bx(b"jp2c", CODESTREAM)
}

/// The signature box followed by `boxes`.
fn file(boxes: &[Vec<u8>]) -> Vec<u8> {
    let mut out = SIGNATURE.to_vec();
    out.extend_from_slice(&boxes.concat());
    out
}

/// The smallest JP2 file: signature, `ftyp`, `jp2h` with `ihdr` + `colr`,
/// `jp2c`.
fn minimal() -> Vec<u8> {
    file(&[jp2_ftyp(), good_jp2h(), jp2c()])
}

/// What `decode` gives for the bare codestream.
fn raw() -> Image {
    decode(CODESTREAM).expect("the fixture codestream decodes")
}

/// Assert `bytes` decode to the bare codestream's components, with the
/// metadata a JP2 file carries today.
#[track_caller]
fn assert_decodes_like_raw(bytes: &[u8]) {
    let image = decode(bytes).unwrap_or_else(|e| panic!("JP2 decode failed: {e}"));
    let raw = raw();
    assert_eq!((image.width, image.height), (raw.width, raw.height));
    assert_eq!(image.components, raw.components);
    assert!(image.jp2.is_some(), "a JP2 file carries metadata");
}

#[track_caller]
fn file_format(bytes: &[u8], fragment: &str) {
    match decode(bytes) {
        Err(Error::FileFormat(message)) => assert!(
            message.contains(fragment),
            "expected a FileFormat error mentioning {fragment:?}, got {message:?}"
        ),
        other => panic!("expected FileFormat mentioning {fragment:?}, got {other:?}"),
    }
}

#[track_caller]
fn unsupported(bytes: &[u8], fragment: &str) {
    match decode(bytes) {
        Err(Error::Unsupported(message)) => assert!(
            message.contains(fragment),
            "expected an Unsupported error mentioning {fragment:?}, got {message:?}"
        ),
        other => panic!("expected Unsupported mentioning {fragment:?}, got {other:?}"),
    }
}

#[test]
fn the_fixture_is_what_the_tests_assume() {
    let raw = raw();
    assert_eq!((raw.width, raw.height), (WIDTH, HEIGHT));
    assert_eq!(raw.components.len(), usize::from(COMPONENTS));
    assert_eq!(raw.jp2, None, "a raw codestream carries no JP2 metadata");
}

#[test]
fn a_minimal_jp2_decodes_to_its_codestream() {
    assert_decodes_like_raw(&minimal());
}

/// The skeleton every JP2 decode carries without `cdef` or `pclr`: the colour
/// space from `colr`, one colour channel per component, no palette.
#[test]
fn a_jp2_decode_carries_the_metadata_skeleton() {
    let image = decode(&minimal()).expect("decodes");
    let colour = |n| ChannelDefinition {
        kind: ChannelType::Colour,
        association: Association::Colour(n),
    };
    assert_eq!(
        image.jp2,
        Some(Jp2Metadata {
            colour: ColourSpace::Srgb,
            channels: vec![colour(1), colour(2), colour(3)],
            palette_applied: false,
            resolution: Resolution::default(),
            xml: Vec::new(),
            uuid: Vec::new(),
        })
    );
}

#[test]
fn a_jp2c_with_a_64_bit_length_decodes() {
    assert_decodes_like_raw(&file(&[
        jp2_ftyp(),
        good_jp2h(),
        xl_box(b"jp2c", CODESTREAM),
    ]));
}

/// `ihdr` is checked against the full-resolution image area, so a reduced
/// decode of a JP2 file works and equals the reduced codestream decode.
#[test]
fn a_reduced_decode_of_a_jp2_matches_its_codestream() {
    let options = DecodeOptions::default().with_resolution_reduction(1);
    let file = decode_with(&minimal(), options).expect("reduced JP2 decode");
    let raw = decode_with(CODESTREAM, options).expect("reduced codestream decode");
    assert_eq!((file.width, file.height), (raw.width, raw.height));
    assert_eq!((file.width, file.height), (WIDTH / 2, HEIGHT / 2));
    assert_eq!(file.components, raw.components);
    assert!(file.jp2.is_some());
}

#[test]
fn a_last_jp2c_with_length_zero_decodes() {
    assert_decodes_like_raw(&file(&[
        jp2_ftyp(),
        good_jp2h(),
        open_box(b"jp2c", CODESTREAM),
    ]));
}

/// Unknown boxes are skipped at the top level and inside `jp2h`, a 64-bit
/// length on another box is read, and a second `ihdr` is ignored.
#[test]
fn unknown_and_repeated_boxes_are_skipped() {
    assert_decodes_like_raw(&file(&[
        jp2_ftyp(),
        bx(b"xml ", b"<a/>"),
        xl_box(b"uuid", &[0; 20]),
        jp2h(&[good_ihdr(), bx(b"zzzz", b"unknown"), colr(), ihdr(1, 1, 1)]),
        bx(b"xml ", b"<b/>"),
        jp2c(),
    ]));
}

/// Only the first `jp2c` is the codestream (I.5.4). OpenJPEG then walks what
/// follows it and accepts anything it cannot follow, so these all decode
/// (each measured with `opj_decompress` 2.5.0, which warns at most).
#[test]
fn bytes_after_the_codestream_that_openjpeg_accepts_decode() {
    let tails: Vec<(&str, Vec<u8>)> = vec![
        ("a second jp2c", bx(b"jp2c", b"not a codestream")),
        (
            "a bad length after a second jp2c, which ends the walk",
            [bx(b"jp2c", b"zz"), header_only(3, b"junk")].concat(),
        ),
        ("a box past the end of the file", header_only(100, b"junk")),
        (
            "a bad length after a box past the end",
            [header_only(100, b"junk"), header_only(3, b"junk")].concat(),
        ),
        ("ASCII text", b"hello, trailing text\n".to_vec()),
        ("three bytes", b"abc".to_vec()),
        ("seven zero bytes", vec![0; 7]),
        ("sixteen zero bytes (LBox 0)", vec![0; 16]),
        ("LBox 0", [header_only(0, b"junk"), b"zz".to_vec()].concat()),
        (
            "a truncated XLBox field",
            [header_only(1, b"junk"), vec![0; 3]].concat(),
        ),
        ("a well-formed box", bx(b"xml ", b"<after/>")),
    ];
    for (what, tail) in tails {
        let mut bytes = minimal();
        bytes.extend_from_slice(&tail);
        let image = decode(&bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(image.components, raw().components, "{what}");
    }
}

/// The two faults OpenJPEG does reject after the codestream: a box whose
/// `LBox` is 2..=7, or whose `XLBox` is below 16, wherever the walk reaches it.
#[test]
fn a_short_box_length_after_the_codestream_is_rejected() {
    for lbox in 2..=7 {
        let mut bytes = minimal();
        bytes.extend_from_slice(&header_only(lbox, b"junk"));
        file_format(&bytes, "after the codestream has length");
    }
    for xlbox in [0, 8, 15] {
        let mut bytes = minimal();
        bytes.extend_from_slice(&xl_header_only(xlbox, b"junk"));
        file_format(&bytes, "after the codestream has XLBox");
    }
    // Reached after well-formed boxes of both length forms.
    let mut bytes = minimal();
    bytes.extend_from_slice(&bx(b"xml ", b"<a/>"));
    bytes.extend_from_slice(&xl_box(b"uuid", &[0; 8]));
    bytes.extend_from_slice(&header_only(3, b"junk"));
    file_format(&bytes, "after the codestream has length 3");
}

/// A JPX-branded file that lists `jp2 ` as compatible is read; so is a `jp2 `
/// brand with an empty compatibility list.
#[test]
fn jp2_compatibility_in_brand_or_list_is_enough() {
    assert_decodes_like_raw(&file(&[
        ftyp(b"jpx ", &[b"jpx ", b"jp2 "]),
        good_jp2h(),
        jp2c(),
    ]));
    assert_decodes_like_raw(&file(&[ftyp(b"jp2 ", &[]), good_jp2h(), jp2c()]));
}

#[test]
fn a_jpx_without_jp2_compatibility_is_unsupported() {
    unsupported(
        &file(&[ftyp(b"jpx ", &[b"jpx ", b"jpxb"]), good_jp2h(), jp2c()]),
        "without `jp2 ` compatibility",
    );
}

/// A palette in `jp2h` is applied: one channel per `cmap` entry.
#[test]
fn a_palette_box_decodes() {
    let image = decode(&file(&[
        jp2_ftyp(),
        jp2h(&[
            good_ihdr(),
            palette::pclr_box(),
            colr(),
            palette::cmap_box(),
        ]),
        jp2c(),
    ]))
    .expect("a palettised file decodes");
    palette::assert_palette_applied(&image);
}

/// A `pclr` without a `cmap` is ignored (I.5.3.4: both or neither), as
/// OpenJPEG does.
#[test]
fn a_palette_box_without_a_component_mapping_box_is_ignored() {
    let bytes = file(&[
        jp2_ftyp(),
        jp2h(&[good_ihdr(), colr(), palette::pclr_box()]),
        jp2c(),
    ]);
    assert_decodes_like_raw(&bytes);
    assert!(!decode(&bytes).unwrap().jp2.unwrap().palette_applied);
}

/// The header box not applied yet is refused by name, so a file is never
/// decoded into channels it would present wrongly.
#[test]
fn a_channel_definition_box_is_unsupported() {
    let with = |child: Vec<u8>| file(&[jp2_ftyp(), jp2h(&[good_ihdr(), colr(), child]), jp2c()]);
    unsupported(&with(bx(b"cdef", &[0, 0])), "`cdef`");
}

/// A box whose length is shorter than its own 8-byte header, for every such
/// length.
#[test]
fn a_box_length_of_1_to_7_is_rejected() {
    for lbox in 2..=7 {
        let bytes = file(&[jp2_ftyp(), header_only(lbox, b"zzzz"), good_jp2h(), jp2c()]);
        file_format(&bytes, "shorter than its 8-byte header");
    }
    // `LBox = 1` announces an `XLBox`; with the file ending first, the field is
    // truncated.
    let mut truncated = file(&[jp2_ftyp(), good_jp2h()]);
    truncated.extend_from_slice(&header_only(1, b"jp2c"));
    truncated.extend_from_slice(&[0, 0, 0]);
    file_format(&truncated, "XLBox field is truncated");
}

#[test]
fn an_xlbox_of_zero_or_past_the_end_is_rejected() {
    for xlbox in [0, 8, 15] {
        let bytes = file(&[
            jp2_ftyp(),
            xl_header_only(xlbox, b"zzzz"),
            good_jp2h(),
            jp2c(),
        ]);
        file_format(&bytes, "shorter than its 16-byte header");
    }
    // One byte longer than the file, and far past it: both are truncation
    // for any box but the codestream, and nothing is allocated from the length.
    for xlbox in [16 + 4 + 1, 1 << 40, u64::MAX] {
        let mut bytes = file(&[jp2_ftyp(), good_jp2h()]);
        bytes.extend_from_slice(&xl_header_only(xlbox, b"xml "));
        bytes.extend_from_slice(b"<a/>");
        file_format(&bytes, "remain in the file");
    }
}

#[test]
fn a_child_box_overrunning_its_superbox_is_rejected() {
    // The `colr` child claims 100 bytes; `jp2h` holds 22 + 15.
    let mut children = good_ihdr();
    children.extend_from_slice(&header_only(100, b"colr"));
    children.extend_from_slice(&colr()[8..]);
    file_format(
        &file(&[jp2_ftyp(), bx(b"jp2h", &children), jp2c()]),
        "remain in the `jp2h` box",
    );
}

/// OpenJPEG reads a codestream to its end whatever its box length says, so a
/// `jp2c` whose `LBox` or `XLBox` runs past the end of the file is clamped to
/// it and decodes.
#[test]
fn a_jp2c_overrunning_the_file_is_clamped_and_decodes() {
    let claimed = u32::try_from(8 + CODESTREAM.len() + 100).unwrap();
    let mut bytes = file(&[jp2_ftyp(), good_jp2h()]);
    bytes.extend_from_slice(&header_only(claimed, b"jp2c"));
    bytes.extend_from_slice(CODESTREAM);
    assert_decodes_like_raw(&bytes);

    for xlbox in [16 + CODESTREAM.len() as u64 + 1, 1 << 40, u64::MAX] {
        let mut bytes = file(&[jp2_ftyp(), good_jp2h()]);
        bytes.extend_from_slice(&xl_header_only(xlbox, b"jp2c"));
        bytes.extend_from_slice(CODESTREAM);
        assert_decodes_like_raw(&bytes);
    }
}

/// Clamping does not rescue a codestream that is actually cut short: the
/// codestream parser still rejects it.
#[test]
fn a_clamped_jp2c_with_a_truncated_codestream_is_still_rejected() {
    let cut = &CODESTREAM[..CODESTREAM.len() - 40];
    let mut bytes = file(&[jp2_ftyp(), good_jp2h()]);
    bytes.extend_from_slice(&header_only(
        u32::try_from(8 + CODESTREAM.len()).unwrap(),
        b"jp2c",
    ));
    bytes.extend_from_slice(cut);
    assert!(
        matches!(decode(&bytes), Err(Error::Codestream(_))),
        "{:?}",
        decode(&bytes)
    );
}

/// `ihdr` may come anywhere in `jp2h`, as OpenJPEG reads it.
#[test]
fn an_ihdr_after_other_header_boxes_decodes() {
    assert_decodes_like_raw(&file(&[
        jp2_ftyp(),
        jp2h(&[colr(), bx(b"zzzz", b"?"), good_ihdr()]),
        jp2c(),
    ]));
}

/// A second `jp2h` before the codestream is merged into the first, as
/// OpenJPEG does: its `ihdr` is ignored (here it disagrees with SIZ) and its
/// other children are added to the first box's.
#[test]
fn a_second_jp2h_is_merged_into_the_first() {
    let bytes = file(&[
        jp2_ftyp(),
        good_jp2h(),
        jp2h(&[ihdr(1, 1, 1), bx(b"colr", &[1, 0, 0, 0, 0, 0, 17])]),
        jp2c(),
    ]);
    assert_decodes_like_raw(&bytes);
    let parsed = super::parse(&bytes).expect("parses");
    assert_eq!(parsed.header.ihdr.width, WIDTH, "the first ihdr wins");
    assert_eq!(parsed.header.colr().len(), 2, "both colr boxes, in order");
    assert_eq!(parsed.header.colr()[0], &colr()[8..]);
}

/// The guarded box meets the guard wherever OpenJPEG would apply it: in a
/// second `jp2h`, or misplaced at the top level after the first `jp2h`.
#[test]
fn a_guarded_box_in_a_second_jp2h_or_after_jp2h_is_unsupported() {
    let cdef = bx(b"cdef", &[0, 0]);
    let in_second_jp2h = file(&[
        jp2_ftyp(),
        good_jp2h(),
        jp2h(&[good_ihdr(), cdef.clone()]),
        jp2c(),
    ]);
    unsupported(&in_second_jp2h, "`cdef`");
    let after_jp2h = file(&[jp2_ftyp(), good_jp2h(), cdef, jp2c()]);
    unsupported(&after_jp2h, "`cdef`");
}

/// The palette boxes are applied wherever OpenJPEG applies them: in a second
/// `jp2h`, or misplaced at the top level after the first `jp2h` (each shape
/// measured with `opj_decompress` 2.5.0).
#[test]
fn palette_boxes_in_a_second_jp2h_or_after_jp2h_are_applied() {
    let (pclr, cmap) = (palette::pclr_box(), palette::cmap_box());
    let shapes = [
        (
            "both in a second jp2h",
            file(&[
                jp2_ftyp(),
                good_jp2h(),
                jp2h(&[good_ihdr(), pclr.clone(), cmap.clone()]),
                jp2c(),
            ]),
        ),
        (
            "pclr in jp2h, cmap in a second jp2h",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), colr(), pclr.clone()]),
                jp2h(&[good_ihdr(), cmap.clone()]),
                jp2c(),
            ]),
        ),
        (
            "both after jp2h",
            file(&[jp2_ftyp(), good_jp2h(), pclr.clone(), cmap.clone(), jp2c()]),
        ),
        (
            "pclr in jp2h, cmap after it",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), colr(), pclr.clone()]),
                cmap.clone(),
                jp2c(),
            ]),
        ),
    ];
    for (what, bytes) in shapes {
        let image = decode(&bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
        palette::assert_palette_applied(&image);
    }
}

/// Before `jp2h`, OpenJPEG ignores a misplaced header box ("JPEG2000 Header
/// box not read yet ... will be ignored"), so the file decodes as coded.
#[test]
fn header_boxes_misplaced_before_jp2h_are_ignored() {
    let bytes = file(&[
        jp2_ftyp(),
        bx(b"cdef", &[0, 0]),
        bx(b"pclr", &[0; 8]),
        bx(b"cmap", &[0; 4]),
        bx(b"colr", &[1, 0, 0, 0, 0, 0, 17]),
        good_jp2h(),
        jp2c(),
    ]);
    assert_decodes_like_raw(&bytes);
    let parsed = super::parse(&bytes).expect("parses");
    assert_eq!(parsed.header.colr(), vec![&colr()[8..]]);
    assert_eq!(parsed.other.len(), 4);
}

/// A file that is malformed and also carries a guarded box reports the fault,
/// not the unsupported box.
#[test]
fn a_structural_fault_wins_over_an_unsupported_box() {
    let with_cdef = |ihdr_box: Vec<u8>, stream: &[u8]| {
        file(&[
            jp2_ftyp(),
            jp2h(&[ihdr_box, colr(), bx(b"cdef", &[0, 0])]),
            bx(b"jp2c", stream),
        ])
    };
    file_format(
        &with_cdef(ihdr(1, 1, 1), CODESTREAM),
        "`ihdr` declares a 1x1 image",
    );
    assert!(matches!(
        decode(&with_cdef(good_ihdr(), b"not a codestream")),
        Err(Error::Codestream(_))
    ));
}

/// The JP2 box layer's reject matrix: every malformed box shape, mapped to
/// its typed error and the check that raised it.
#[test]
fn reject_matrix_maps_every_malformed_box_shape_to_its_typed_error() {
    let ihdr_payload = good_ihdr()[8..].to_vec();
    let rows: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "signature box with the wrong contents",
            {
                let mut bytes = minimal();
                bytes[11] = 0x0B;
                bytes
            },
            "signature box",
        ),
        (
            "signature box one byte too long",
            [
                bx(b"jP  ", &[0x0D, 0x0A, 0x87, 0x0A, 0x00]),
                jp2_ftyp(),
                good_jp2h(),
                jp2c(),
            ]
            .concat(),
            "signature box",
        ),
        (
            "signature box alone",
            SIGNATURE.to_vec(),
            "ends after the signature",
        ),
        (
            "ftyp not second",
            file(&[good_jp2h(), jp2_ftyp(), jp2c()]),
            "`ftyp` box must follow the signature, not `jp2h`",
        ),
        (
            "ftyp too short",
            file(&[bx(b"ftyp", b"jp2 "), good_jp2h(), jp2c()]),
            "`ftyp` box holds 4 bytes",
        ),
        (
            "ftyp with a partial compatibility code",
            file(&[bx(b"ftyp", b"jp2 \0\0\0\0jp2 xx"), good_jp2h(), jp2c()]),
            "`ftyp` box holds 14 bytes",
        ),
        (
            "no jp2h (signature, ftyp, jp2c)",
            file(&[jp2_ftyp(), jp2c()]),
            "no `jp2h` header box before its `jp2c`",
        ),
        (
            "jp2c before jp2h",
            file(&[jp2_ftyp(), jp2c(), good_jp2h()]),
            "no `jp2h` header box before its `jp2c`",
        ),
        ("no jp2c", file(&[jp2_ftyp(), good_jp2h()]), "no `jp2c`"),
        (
            "neither jp2h nor jp2c",
            file(&[jp2_ftyp()]),
            "no `jp2h` header box and no `jp2c`",
        ),
        (
            "empty jp2h",
            file(&[jp2_ftyp(), jp2h(&[]), jp2c()]),
            "`jp2h` box has no `ihdr`",
        ),
        (
            "jp2h without ihdr",
            file(&[jp2_ftyp(), jp2h(&[colr()]), jp2c()]),
            "`jp2h` box has no `ihdr`",
        ),
        (
            "a second jp2h without ihdr",
            file(&[jp2_ftyp(), good_jp2h(), jp2h(&[colr()]), jp2c()]),
            "`jp2h` box has no `ihdr`",
        ),
        (
            "ihdr of 13 bytes",
            file(&[
                jp2_ftyp(),
                jp2h(&[bx(b"ihdr", &ihdr_payload[..13]), colr()]),
                jp2c(),
            ]),
            "`ihdr` box holds 13 bytes",
        ),
        (
            "ihdr of 15 bytes",
            file(&[
                jp2_ftyp(),
                jp2h(&[
                    bx(b"ihdr", &[ihdr_payload.as_slice(), &[0]].concat()),
                    colr(),
                ]),
                jp2c(),
            ]),
            "`ihdr` box holds 15 bytes",
        ),
        (
            "ihdr width differs from SIZ",
            file(&[
                jp2_ftyp(),
                jp2h(&[ihdr(HEIGHT, WIDTH + 1, COMPONENTS), colr()]),
                jp2c(),
            ]),
            "`ihdr` declares a 25x16 image",
        ),
        (
            "ihdr height differs from SIZ",
            file(&[
                jp2_ftyp(),
                jp2h(&[ihdr(HEIGHT - 1, WIDTH, COMPONENTS), colr()]),
                jp2c(),
            ]),
            "`ihdr` declares a 24x15 image",
        ),
        (
            "ihdr NC of 0",
            file(&[jp2_ftyp(), jp2h(&[ihdr(HEIGHT, WIDTH, 0), colr()]), jp2c()]),
            "none may be 0",
        ),
        (
            "ihdr width of 0",
            file(&[
                jp2_ftyp(),
                jp2h(&[ihdr(HEIGHT, 0, COMPONENTS), colr()]),
                jp2c(),
            ]),
            "none may be 0",
        ),
        (
            "ihdr height of 0",
            file(&[
                jp2_ftyp(),
                jp2h(&[ihdr(0, WIDTH, COMPONENTS), colr()]),
                jp2c(),
            ]),
            "none may be 0",
        ),
        (
            "ihdr NC over 16384",
            file(&[
                jp2_ftyp(),
                jp2h(&[ihdr(HEIGHT, WIDTH, 16385), colr()]),
                jp2c(),
            ]),
            "declares 16385 components",
        ),
        (
            "LBox 0 inside jp2h",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), open_box(b"colr", &colr()[8..])]),
                jp2c(),
            ]),
            "has length 0",
        ),
        (
            "LBox 0 on a box that is not last",
            file(&[jp2_ftyp(), open_box(b"xml ", b""), good_jp2h(), jp2c()]),
            "a box of length 0 (to the end of the file) swallowed the rest",
        ),
        (
            "LBox 2..7 after the codestream",
            [minimal(), header_only(5, b"junk")].concat(),
            "after the codestream has length 5",
        ),
        (
            "XLBox below 16 after the codestream",
            [minimal(), xl_header_only(15, b"junk")].concat(),
            "after the codestream has XLBox 15",
        ),
        (
            "a box header cut short before jp2c",
            [file(&[jp2_ftyp(), good_jp2h()]), vec![0, 0, 0]].concat(),
            "a box header needs 8 bytes but 3 remain in the file",
        ),
        (
            "a jp2h child header cut short",
            file(&[jp2_ftyp(), jp2h(&[good_ihdr(), vec![0, 0, 0, 9]]), jp2c()]),
            "a box header needs 8 bytes but 4 remain in the `jp2h` box",
        ),
    ];
    for (what, bytes, fragment) in rows {
        match decode(&bytes) {
            Err(Error::FileFormat(message)) => assert!(
                message.contains(fragment),
                "{what}: expected a message mentioning {fragment:?}, got {message:?}"
            ),
            other => panic!("{what}: expected FileFormat, got {other:?}"),
        }
    }
}

/// Input with neither magic is still the codestream parser's to reject.
#[test]
fn input_with_neither_magic_is_a_codestream_error() {
    for bytes in [&b""[..], b"not a codestream", b"\x00\x00\x00\x0cjP"] {
        assert!(
            matches!(decode(bytes), Err(Error::Codestream(_))),
            "{bytes:?}"
        );
    }
}

/// Every prefix of a valid file, and every single-byte corruption of its box
/// headers, is a typed result: never a panic.
#[test]
fn truncations_and_header_corruptions_never_panic() {
    let bytes = minimal();
    let codestream_at = bytes.len() - CODESTREAM.len();
    for end in 0..bytes.len() {
        let _ = decode(&bytes[..end]);
    }
    for at in 0..codestream_at {
        for value in [0x00, 0x01, 0x07, 0x0F, 0x80, 0xFF] {
            let mut corrupt = bytes.clone();
            corrupt[at] = value;
            let _ = decode(&corrupt);
        }
    }
}

/// The walk keeps what the later header code reads: the `jp2h` children as raw
/// slices in file order, the other top-level boxes before the codestream, and
/// the unread bytes after it.
#[test]
fn the_walk_records_header_boxes_and_the_bytes_around_the_codestream() {
    let trailing = bx(b"xml ", b"<after/>");
    let bytes = file(&[
        jp2_ftyp(),
        bx(b"xml ", b"<before/>"),
        jp2h(&[
            good_ihdr(),
            colr(),
            bx(b"colr", &[3, 0, 0]),
            bx(b"bpcc", &[7, 7, 7]),
            bx(b"res ", &[1, 2]),
            bx(b"zzzz", b"?"),
        ]),
        jp2c(),
        trailing.clone(),
    ]);
    let parsed = super::parse(&bytes).expect("parses");
    let header = &parsed.header;
    assert_eq!(
        (
            header.ihdr.height,
            header.ihdr.width,
            header.ihdr.components
        ),
        (HEIGHT, WIDTH, COMPONENTS)
    );
    let first_colr = colr();
    assert_eq!(header.colr(), vec![&first_colr[8..], &[3, 0, 0][..]]);
    assert_eq!(header.bpcc(), vec![&[7, 7, 7][..]]);
    assert_eq!(header.res(), vec![&[1, 2][..]]);
    assert!(header.pclr().is_empty() && header.cmap().is_empty() && header.cdef().is_empty());
    assert_eq!(header.unknown().len(), 1);
    assert_eq!(&header.unknown()[0].kind, b"zzzz");
    assert_eq!(parsed.codestream, CODESTREAM);
    assert_eq!(parsed.other.len(), 1);
    assert_eq!(parsed.other[0].payload, b"<before/>");
    assert_eq!(parsed.after_codestream.len(), 1);
    assert_eq!(&parsed.after_codestream[0].kind, b"xml ");
    assert_eq!(parsed.after_codestream[0].payload, b"<after/>");
}

/// `ihdr.NC` is not held to `Csiz`: the codestream decides the image, as in
/// OpenJPEG 2.5.0 (owner decision, 2026-11-01). Measured: `opj_decompress`
/// decodes a one-component codestream under an `ihdr` with `NC` 2 or 9. A
/// `bpcc` is still held to `NC`, so it must hold `NC` bytes, not `Csiz`.
#[test]
fn an_ihdr_component_count_unlike_csiz_decodes() {
    for nc in [1, 2, 4, 9, 16384] {
        let bytes = file(&[jp2_ftyp(), jp2h(&[ihdr(HEIGHT, WIDTH, nc), colr()]), jp2c()]);
        assert_decodes_like_raw(&bytes);
        let header = read_header(&bytes).unwrap_or_else(|e| panic!("NC {nc}: {e}"));
        assert_eq!(header.components.len(), usize::from(COMPONENTS), "NC {nc}");
    }
    let with_bpcc = |nc: u16, bytes: usize| {
        file(&[
            jp2_ftyp(),
            jp2h(&[
                ihdr(HEIGHT, WIDTH, nc),
                bx(b"bpcc", &vec![7; bytes]),
                colr(),
            ]),
            jp2c(),
        ])
    };
    assert_decodes_like_raw(&with_bpcc(4, 4));
    file_format(&with_bpcc(4, 3), "a `bpcc` box holds 3 bytes");
}

/// A file with a wrong `ihdr` width and a truncated codestream reports the
/// box fault: `ihdr` is held to SIZ before any tile-part is read, as
/// OpenJPEG does ("Error with SIZ marker: IHDR w(769) h(512) vs. SIZ w(768)
/// h(512)" on file1 cut to 20000 bytes). Header faults found by the channel
/// plans come before tile faults too.
#[test]
fn a_header_fault_wins_over_a_truncated_codestream() {
    let truncated = &CODESTREAM[..CODESTREAM.len() - 40];
    assert!(matches!(decode(truncated), Err(Error::Codestream(_))));
    let with = |children: &[Vec<u8>], wide: bool| {
        let width = if wide { WIDTH + 1 } else { WIDTH };
        let mut all = vec![ihdr(HEIGHT, width, COMPONENTS), colr()];
        all.extend_from_slice(children);
        file(&[jp2_ftyp(), jp2h(&all), bx(b"jp2c", truncated)])
    };
    file_format(&with(&[], true), "`ihdr` declares a 25x16 image");
    file_format(
        &with(&[bx(b"cmap", &[0, 0, 1, 0])], false),
        "a `cmap` box but no `pclr` box",
    );
    unsupported(&with(&[bx(b"cdef", &[0, 0])], false), "`cdef`");
    // With no header fault, the truncation is what is reported.
    assert!(matches!(
        decode(&with(&[], false)),
        Err(Error::Codestream(_))
    ));
}

fn res_box(children: &[Vec<u8>]) -> Vec<u8> {
    bx(b"res ", &children.concat())
}

fn res_payload(v: (u16, u16, i8), h: (u16, u16, i8)) -> Vec<u8> {
    let mut out = Vec::new();
    for value in [v.0, v.1, h.0, h.1] {
        out.extend_from_slice(&value.to_be_bytes());
    }
    out.extend_from_slice(&[v.2 as u8, h.2 as u8]);
    out
}

fn ratio(numerator: u16, denominator: u16, exponent: i8) -> Ratio {
    Ratio {
        numerator,
        denominator,
        exponent,
    }
}

/// `resc` and `resd` are read from `res ` as four `u16` and two `i8`: the
/// vertical ratio first, the exponents last.
#[test]
fn the_resolution_box_is_read() {
    let resc = res_payload((10000, 254, 0), (3, 4, -2));
    let resd = res_payload((12901, 32768, 4), (1, 2, 127));
    let bytes = file(&[
        jp2_ftyp(),
        jp2h(&[
            good_ihdr(),
            colr(),
            res_box(&[bx(b"resc", &resc), bx(b"resd", &resd)]),
        ]),
        jp2c(),
    ]);
    assert_decodes_like_raw(&bytes);
    let want = Resolution {
        capture: Some(Res {
            vertical: ratio(10000, 254, 0),
            horizontal: ratio(3, 4, -2),
        }),
        display: Some(Res {
            vertical: ratio(12901, 32768, 4),
            horizontal: ratio(1, 2, 127),
        }),
    };
    assert_eq!(decode(&bytes).unwrap().jp2.unwrap().resolution, want);
    assert_eq!(read_header(&bytes).unwrap().jp2.unwrap().resolution, want);
}

#[test]
fn a_ratio_converts_to_grid_points_per_metre() {
    assert_eq!(ratio(3, 4, 2).grid_points_per_metre(), Some(75.0));
    assert_eq!(ratio(5, 1, -1).grid_points_per_metre(), Some(0.5));
    assert_eq!(ratio(1, 0, 0).grid_points_per_metre(), None);
}

/// OpenJPEG 2.5.0 does not read `res `, so no shape of it fails there: every
/// one below was built and decoded with `opj_decompress`. Here they decode
/// too, and a malformed `resc` or `resd` is reported as absent. The first
/// well-formed box of each kind counts.
#[test]
fn a_malformed_resolution_box_never_fails_a_decode() {
    let good = res_payload((10000, 254, 0), (10000, 254, 0));
    let capture = |v| {
        Some(Res {
            vertical: v,
            horizontal: v,
        })
    };
    let dpi = capture(ratio(10000, 254, 0));
    let cases: Vec<(&str, Vec<u8>, Resolution)> = vec![
        ("empty res", res_box(&[]), Resolution::default()),
        (
            "garbage in res",
            bx(b"res ", &[1, 2, 3]),
            Resolution::default(),
        ),
        (
            "resc of 9 bytes",
            res_box(&[bx(b"resc", &good[..9])]),
            Resolution::default(),
        ),
        (
            "resc of 11 bytes",
            res_box(&[bx(b"resc", &[good.as_slice(), &[0]].concat())]),
            Resolution::default(),
        ),
        (
            "empty resd",
            res_box(&[bx(b"resd", &[])]),
            Resolution::default(),
        ),
        (
            "a child with LBox 3",
            bx(b"res ", &[header_only(3, b"resc"), good.clone()].concat()),
            Resolution::default(),
        ),
        (
            "a child past the end of res",
            bx(b"res ", &[header_only(200, b"resc"), good.clone()].concat()),
            Resolution::default(),
        ),
        (
            "a zero denominator",
            res_box(&[bx(b"resc", &res_payload((1, 0, 0), (1, 0, 0)))]),
            Resolution {
                capture: capture(ratio(1, 0, 0)),
                display: None,
            },
        ),
        (
            "a good resc before a broken child",
            res_box(&[bx(b"resc", &good), header_only(3, b"resd")]),
            Resolution {
                capture: dpi,
                display: None,
            },
        ),
        (
            "a short resc, then a good one",
            res_box(&[bx(b"resc", &good[..4]), bx(b"resc", &good)]),
            Resolution {
                capture: dpi,
                display: None,
            },
        ),
        (
            "two good resc: the first counts",
            res_box(&[
                bx(b"resc", &good),
                bx(b"resc", &res_payload((1, 1, 1), (1, 1, 1))),
            ]),
            Resolution {
                capture: dpi,
                display: None,
            },
        ),
        (
            "an unknown child",
            res_box(&[bx(b"zzzz", b"?"), bx(b"resd", &good)]),
            Resolution {
                capture: None,
                display: dpi,
            },
        ),
    ];
    for (what, res, want) in cases {
        let bytes = file(&[jp2_ftyp(), jp2h(&[good_ihdr(), colr(), res]), jp2c()]);
        let image = decode(&bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(image.jp2.unwrap().resolution, want, "{what}");
    }
}

/// A `res ` box outside `jp2h` is not read (I.5.3.7 puts it inside).
#[test]
fn a_resolution_box_at_the_top_level_is_not_read() {
    let res = res_box(&[bx(b"resc", &res_payload((10000, 254, 0), (10000, 254, 0)))]);
    let bytes = file(&[jp2_ftyp(), good_jp2h(), res, jp2c()]);
    assert_eq!(
        decode(&bytes).unwrap().jp2.unwrap().resolution,
        Resolution::default()
    );
}

/// Every top-level `xml ` and `uuid` box comes back verbatim in file order,
/// before and after the codestream. Ones nested in a skipped superbox
/// (`asoc`), or inside `jp2h`, are not top-level and are not collected. A
/// `uuid` shorter than its identifier is skipped (OpenJPEG decodes such a
/// file; measured with `opj_decompress`).
#[test]
fn xml_and_uuid_boxes_are_passed_through_in_file_order() {
    let id: [u8; 16] = core::array::from_fn(|i| i as u8);
    let other_id = [0xAB; 16];
    let bytes = file(&[
        jp2_ftyp(),
        bx(b"xml ", b"<one/>"),
        bx(b"uuid", &[id.as_slice(), b"first"].concat()),
        bx(b"uuid", b"too short"),
        bx(b"rreq", &[1, 0x80, 0, 0]),
        bx(b"asoc", &bx(b"xml ", b"<nested/>")),
        jp2h(&[good_ihdr(), colr(), bx(b"xml ", b"<in jp2h/>")]),
        xl_box(b"xml ", b"<two/>"),
        jp2c(),
        bx(b"uuid", &other_id),
        bx(b"xml ", b"<three/>"),
    ]);
    assert_decodes_like_raw(&bytes);
    let metadata = decode(&bytes).unwrap().jp2.unwrap();
    assert_eq!(
        metadata.xml,
        vec![b"<one/>".to_vec(), b"<two/>".to_vec(), b"<three/>".to_vec()]
    );
    assert_eq!(
        metadata.uuid,
        vec![
            UuidBox {
                id,
                payload: b"first".to_vec(),
            },
            UuidBox {
                id: other_id,
                payload: Vec::new(),
            },
        ]
    );
    assert_eq!(read_header(&bytes).unwrap().jp2, Some(metadata));
}

/// After the codestream, the boxes are collected as far as OpenJPEG's walk
/// there goes, and collecting them never fails a decode it accepts: a box
/// past the end of the file, `LBox` 0, or a second `jp2c` ends the walk.
#[test]
fn boxes_after_the_codestream_are_collected_as_far_as_the_walk_goes() {
    let xml = |text: &[u8]| bx(b"xml ", text);
    let tails: Vec<(&str, Vec<u8>, usize)> = vec![
        ("two xml", [xml(b"<a/>"), xml(b"<b/>")].concat(), 2),
        (
            "an xml past the end of the file",
            [xml(b"<a/>"), header_only(100, b"xml ")].concat(),
            1,
        ),
        (
            "an open-ended xml",
            [xml(b"<a/>"), open_box(b"xml ", b"<b/>")].concat(),
            1,
        ),
        (
            "after a second jp2c",
            [xml(b"<a/>"), bx(b"jp2c", b"zz"), xml(b"<b/>")].concat(),
            1,
        ),
        (
            "three stray bytes",
            [xml(b"<a/>"), vec![0, 0, 0]].concat(),
            1,
        ),
    ];
    for (what, tail, count) in tails {
        let bytes = [minimal(), tail].concat();
        let image = decode(&bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(image.jp2.unwrap().xml.len(), count, "{what}");
    }
}

/// `read_header` stops at the codestream's first `SOT` marker: a file cut
/// right after it has the same header as the whole file. Cut before the
/// marker, the main header is unterminated, which OpenJPEG also refuses
/// ("Stream too short").
#[test]
fn read_header_reads_no_tile_data() {
    let sot = CODESTREAM
        .windows(2)
        .position(|w| w == [0xFF, 0x90])
        .expect("the fixture has a tile-part");
    let whole = read_header(&minimal()).expect("header");
    let jp2c_at = minimal().len() - CODESTREAM.len();
    for cut in [sot + 2, sot + 6, sot + 20, CODESTREAM.len() - 1] {
        let bytes = file(&[jp2_ftyp(), good_jp2h(), bx(b"jp2c", &CODESTREAM[..cut])]);
        assert_eq!(read_header(&bytes).as_ref(), Ok(&whole), "cut at {cut}");
        assert!(decode(&bytes).is_err(), "cut at {cut}");
        // Clamped `jp2c` and raw codestream alike.
        let clamped = &minimal()[..jp2c_at + cut];
        assert_eq!(
            read_header(clamped).as_ref(),
            Ok(&whole),
            "clamped at {cut}"
        );
        let raw = read_header(&CODESTREAM[..cut]).expect("raw header");
        assert_eq!(
            (raw.width, raw.height, &raw.components),
            (whole.width, whole.height, &whole.components)
        );
    }
    assert!(matches!(
        read_header(&CODESTREAM[..sot]),
        Err(Error::Codestream(_))
    ));
}

/// GDAL's `byte_with_xmp.jp2` carries two `uuid` boxes at the top level and
/// its GML `xml ` inside an `asoc`, which is skipped. Its `cdef` is still
/// refused, so this reads the metadata past the public guard.
#[test]
fn byte_with_xmp_has_two_uuid_boxes_and_no_top_level_xml() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/jp2_real/byte_with_xmp.jp2"
    );
    let bytes = std::fs::read(path).expect("read the fixture");
    let file = super::parse(&bytes).expect("box walk");
    let (xml, uuid) = super::passthrough(&file);
    assert!(xml.is_empty());
    let ids: Vec<[u8; 16]> = uuid.iter().map(|u| u.id).collect();
    assert_eq!(
        ids,
        [
            *b"\xb1\x4b\xf8\xbd\x08\x3d\x4b\x43\xa5\xae\x8c\xd7\xd5\xa6\xce\x03",
            *b"\xbe\x7a\xcf\xcb\x97\xa9\x42\xe8\x9c\x71\x99\x94\x91\xe3\xaf\xac",
        ]
    );
    assert_eq!(
        uuid.iter().map(|u| u.payload.len()).collect::<Vec<_>>(),
        [348, 3578]
    );
    assert!(matches!(read_header(&bytes), Err(Error::Unsupported(_))));
}
