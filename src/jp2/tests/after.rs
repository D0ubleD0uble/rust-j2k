//! The codestream box's length and the boxes after the codestream, through
//! the public [`decode`]. OpenJPEG 2.5.0 does not use the `jp2c` box's
//! length: it decodes from the start of the box, then walks the boxes from
//! where the decode stopped and reads its header boxes again. Every shape here
//! was measured with OpenJPEG 2.5.0: `opj_decompress` for the default decode,
//! and a probe linked to `libopenjp2` 2.5.0 calling `opj_read_header`,
//! `opj_decode` and `opj_end_decompress` with and without
//! `OPJ_DPARAMETERS_IGNORE_PCLR_CMAP_CDEF_FLAG` for the raw channels. The
//! after-`jp2c` header box shapes regenerate the ones from the PR #176 review.

use super::palette::{cmap_box, pclr_box};
use super::*;

/// The single-component codestream the palette tests use (32x24, 8-bit).
pub(super) const ONE_COMPONENT: &[u8] =
    include_bytes!("../../../tests/fixtures/multilayer_lossless.j2k");

/// The fixture codestream with its only tile-part's `Psot` set to 0, so the
/// tile-part runs to the end of the codestream.
pub(super) fn psot_zero_codestream() -> Vec<u8> {
    let sot = CODESTREAM
        .windows(2)
        .position(|w| w == [0xFF, 0x90])
        .expect("the fixture has a tile-part");
    let mut out = CODESTREAM.to_vec();
    out[sot + 6..sot + 10].copy_from_slice(&[0; 4]);
    out
}

/// The minimal file with a `jp2c` header claiming `lbox` bytes in front of
/// the whole codestream, then `tail`.
pub(super) fn with_jp2c_length(lbox: u32, tail: &[u8]) -> Vec<u8> {
    let mut bytes = file(&[jp2_ftyp(), good_jp2h()]);
    bytes.extend_from_slice(&header_only(lbox, b"jp2c"));
    bytes.extend_from_slice(CODESTREAM);
    bytes.extend_from_slice(tail);
    bytes
}

/// `minimal()` with these boxes after the codestream.
pub(super) fn after(tail: &[Vec<u8>]) -> Vec<u8> {
    [minimal(), tail.concat()].concat()
}

/// A file without a `colr`, then these boxes after the codestream.
pub(super) fn no_colour_then(tail: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = file(&[jp2_ftyp(), jp2h(&[good_ihdr()]), jp2c()]);
    bytes.extend_from_slice(&tail.concat());
    bytes
}

/// A file over the single-component codestream with these `jp2h` children
/// after `ihdr` and `colr`, then `tail` after the codestream.
pub(super) fn one_component(children: &[Vec<u8>], tail: &[Vec<u8>]) -> Vec<u8> {
    let mut all = vec![ihdr(24, 32, 1), colr()];
    all.extend_from_slice(children);
    let mut bytes = file(&[jp2_ftyp(), jp2h(&all), bx(b"jp2c", ONE_COMPONENT)]);
    bytes.extend_from_slice(&tail.concat());
    bytes
}

/// A `cdef` labelling the three components as colours 1, 2, 3.
pub(super) fn identity_cdef() -> Vec<u8> {
    super::channels::cdef_of(&[(0, 0, 1), (1, 0, 2), (2, 0, 3)])
}

pub(super) fn decode_raw(bytes: &[u8]) -> crate::Result<Image> {
    let options = DecodeOptions {
        raw_channels: true,
        ..DecodeOptions::default()
    };
    decode_with(bytes, options)
}

/// A `jp2c` whose `LBox` is 2..=7 or whose `XLBox` is below 16 decodes:
/// OpenJPEG does not use the length, and the codestream is read from the end
/// of the box header.
#[test]
fn a_codestream_box_shorter_than_its_header_decodes() {
    for lbox in 2..=7 {
        assert_decodes_like_raw(&with_jp2c_length(lbox, &[]));
    }
    for xlbox in [0, 8, 15] {
        let mut bytes = file(&[jp2_ftyp(), good_jp2h()]);
        bytes.extend_from_slice(&xl_header_only(xlbox, b"jp2c"));
        bytes.extend_from_slice(CODESTREAM);
        assert_decodes_like_raw(&bytes);
    }
    // Any other box that short is still refused.
    file_format(
        &file(&[jp2_ftyp(), good_jp2h(), header_only(5, b"zzzz"), jp2c()]),
        "shorter than its 8-byte header",
    );
}

/// A `jp2c` shorter than its codestream decodes too, identical to the bare
/// codestream (`LBox` 8, 108, and the codestream's length minus 10), and the
/// walk after it starts at the codestream's `EOC`, not at the box's end.
#[test]
fn a_codestream_box_shorter_than_its_codestream_decodes() {
    let short = u32::try_from(8 + CODESTREAM.len() - 10).unwrap();
    for lbox in [8, 108, short] {
        assert_decodes_like_raw(&with_jp2c_length(lbox, &[]));
    }
    let bytes = with_jp2c_length(108, &bx(b"xml ", b"<a/>"));
    assert_eq!(decode(&bytes).unwrap().jp2.unwrap().xml, [b"<a/>"]);
    file_format(
        &with_jp2c_length(108, &header_only(3, b"junk")),
        "after the codestream has length 3",
    );
    file_format(
        &with_jp2c_length(5, &bx(b"colr", &[1])),
        "after the codestream, a `colr` box holds 1 bytes",
    );
}

/// A `jp2c` longer than its codestream: OpenJPEG walks from the codestream's
/// `EOC`, so the bytes after it inside the box are read as boxes. Zeros read
/// as one box running to the end of the file; a box shorter than its header is
/// a fault.
#[test]
fn bytes_after_eoc_inside_the_codestream_box_are_walked() {
    let padded = |padding: &[u8]| {
        file(&[
            jp2_ftyp(),
            good_jp2h(),
            bx(b"jp2c", &[CODESTREAM, padding].concat()),
        ])
    };
    assert_decodes_like_raw(&padded(&[0; 20]));
    file_format(
        &padded(&header_only(3, b"junk")),
        "after the codestream has length 3",
    );
}

/// The header boxes after the codestream that OpenJPEG rejects when it reads
/// them again after decoding.
#[test]
fn malformed_header_boxes_after_the_codestream_are_rejected() {
    let short_meth1 = || bx(b"colr", &[1, 0, 0, 0]);
    let rows: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "a signature box",
            after(&[SIGNATURE.to_vec()]),
            "a signature box",
        ),
        ("an ftyp box", after(&[jp2_ftyp()]), "an `ftyp` box"),
        (
            "an empty jp2h",
            after(&[jp2h(&[])]),
            "`jp2h` box has no `ihdr`",
        ),
        (
            "a jp2h without ihdr",
            after(&[jp2h(&[colr()])]),
            "`jp2h` box has no `ihdr`",
        ),
        (
            "a jp2h holding a bad colr",
            after(&[jp2h(&[good_ihdr(), bx(b"colr", &[])])]),
            "a `colr` box holds 0 bytes",
        ),
        (
            "a jp2h holding two cdef",
            after(&[jp2h(&[good_ihdr(), identity_cdef(), identity_cdef()])]),
            "a second `cdef`",
        ),
        (
            "a jp2h child shorter than its header",
            after(&[jp2h(&[good_ihdr(), header_only(3, b"colr")])]),
            "shorter than its 8-byte header",
        ),
        (
            "a colr of 0 bytes",
            after(&[bx(b"colr", &[])]),
            "holds 0 bytes",
        ),
        (
            "a colr of 1 byte",
            after(&[bx(b"colr", &[1])]),
            "holds 1 bytes",
        ),
        (
            "a colr of 2 bytes",
            after(&[bx(b"colr", &[1, 0])]),
            "holds 2 bytes",
        ),
        (
            "a colr of 1 byte running to the end of the file",
            after(&[open_box(b"colr", &[1])]),
            "holds 1 bytes",
        ),
        (
            "a colr past the end of the file",
            after(&[header_only(100, b"colr"), vec![1, 0, 0]]),
            "is 100 bytes but only 11 remain",
        ),
        (
            "a bpcc of the wrong length",
            after(&[bx(b"bpcc", &[7, 7])]),
            "a `bpcc` box holds 2 bytes",
        ),
        (
            "a cmap with no pclr",
            after(&[cmap_box()]),
            "a `cmap` box with no `pclr`",
        ),
        (
            "a malformed pclr",
            after(&[bx(b"pclr", &[0, 0, 3])]),
            "declares 0 entries",
        ),
        (
            "a malformed cdef",
            after(&[bx(b"cdef", &[0, 0])]),
            "declares 0 channel definitions",
        ),
        (
            "two cdef",
            after(&[identity_cdef(), identity_cdef()]),
            "a second `cdef`",
        ),
        (
            "a short METH 1 colr with no usable colr before it",
            no_colour_then(&[short_meth1()]),
            "`METH` 1 holds 4 bytes",
        ),
        (
            "an empty METH 2 colr with no usable colr before it",
            no_colour_then(&[bx(b"colr", &[2, 0, 0])]),
            "`METH` 2 holds no ICC profile",
        ),
        (
            "a short METH 1 colr after an ignored METH 3 one",
            no_colour_then(&[bx(b"colr", &[3, 0, 0]), short_meth1()]),
            "`METH` 1 holds 4 bytes",
        ),
        (
            "a short METH 1 colr when the header's colr is METH 3",
            [
                file(&[
                    jp2_ftyp(),
                    jp2h(&[good_ihdr(), bx(b"colr", &[3, 0, 0])]),
                    jp2c(),
                ]),
                short_meth1(),
            ]
            .concat(),
            "`METH` 1 holds 4 bytes",
        ),
        (
            "a pclr then a cmap shorter than 4 * NPC bytes",
            after(&[pclr_box(), bx(b"cmap", &[0; 8])]),
            "the `cmap` box holds 8 bytes; mapping 3 palette columns needs 12",
        ),
        (
            "a pclr after the applied palette",
            one_component(&[pclr_box(), cmap_box()], &[pclr_box()]),
            "a second `pclr`",
        ),
        (
            "a cmap after the applied palette",
            one_component(&[pclr_box(), cmap_box()], &[cmap_box()]),
            "a second `cmap`",
        ),
        (
            "a cmap after a dropped palette (pclr without cmap)",
            one_component(&[pclr_box()], &[cmap_box()]),
            "a `cmap` box with no `pclr`",
        ),
        (
            "a pclr and two cmap",
            one_component(&[], &[pclr_box(), cmap_box(), cmap_box()]),
            "a second `cmap`",
        ),
        (
            "a pclr with its entries cut short",
            one_component(&[], &[bx(b"pclr", &[0, 2, 3, 7, 7, 7, 0, 0, 0, 0, 0])]),
            "entries need 6 bytes",
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
        // `read_header` reads the same boxes and agrees.
        assert_eq!(read_header(&bytes).err(), decode(&bytes).err(), "{what}");
    }
}

/// The header boxes after the codestream that OpenJPEG reads and accepts.
/// None of them is applied.
#[test]
fn header_boxes_after_the_codestream_that_openjpeg_accepts_decode() {
    let rows: Vec<(&str, Vec<u8>)> = vec![
        ("a jp2h with ihdr and colr", after(&[good_jp2h()])),
        (
            "a jp2h whose ihdr is malformed (ignored)",
            after(&[jp2h(&[bx(b"ihdr", &[0])])]),
        ),
        ("a malformed ihdr", after(&[bx(b"ihdr", &[0])])),
        ("a valid colr", after(&[colr()])),
        (
            "a short METH 1 colr after a usable one",
            after(&[bx(b"colr", &[1, 0, 0, 0])]),
        ),
        (
            "an empty METH 2 colr after a usable one",
            after(&[bx(b"colr", &[2, 0, 0])]),
        ),
        ("a METH 3 colr", after(&[bx(b"colr", &[3, 0, 0])])),
        (
            "a bpcc of NC bytes",
            after(&[bx(b"bpcc", &[200, 200, 200])]),
        ),
        ("a pclr", after(&[pclr_box()])),
        ("a cdef", after(&[identity_cdef()])),
        (
            "a cdef with entries out of range",
            after(&[super::channels::cdef_of(&[(99, 0, 99)])]),
        ),
        ("a malformed res ", after(&[bx(b"res ", &[1, 2, 3])])),
        (
            "an unknown box past the end",
            after(&[header_only(100, b"junk")]),
        ),
        (
            "a colr with an XLBox of 2^33 (OpenJPEG cannot read it and stops)",
            after(&[xl_header_only(1 << 33, b"colr"), vec![1]]),
        ),
        ("a jp2c of length 3", after(&[header_only(3, b"jp2c")])),
        (
            "a colr running to the end of the file",
            after(&[open_box(b"colr", &colr()[8..])]),
        ),
        (
            "a short METH 1 colr after a usable one read after jp2c",
            no_colour_then(&[colr(), bx(b"colr", &[1, 0, 0, 0])]),
        ),
        (
            "a cdef after the applied cdef",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), colr(), identity_cdef()]),
                jp2c(),
                identity_cdef(),
            ]),
        ),
        (
            "a pclr after a dropped palette",
            one_component(&[pclr_box()], &[pclr_box()]),
        ),
        (
            "a pclr then a cmap",
            one_component(&[], &[pclr_box(), cmap_box()]),
        ),
        (
            "a pclr then a cmap mapping a missing component (never checked)",
            one_component(&[], &[pclr_box(), bx(b"cmap", &[0, 9, 7, 7].repeat(3))]),
        ),
    ];
    for (what, bytes) in rows {
        let image = decode(&bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
        let bare = decode(
            if bytes
                .windows(ONE_COMPONENT.len())
                .any(|w| w == ONE_COMPONENT)
            {
                ONE_COMPONENT
            } else {
                CODESTREAM
            },
        )
        .unwrap();
        assert_eq!(image.components, bare.components, "{what}: nothing applied");
    }
}

/// A `colr` read after the codestream does not set the colour space: OpenJPEG
/// has set the image's colour space before it reads it.
#[test]
fn a_colr_after_the_codestream_does_not_set_the_colour_space() {
    let image = decode(&no_colour_then(&[colr()])).unwrap();
    assert_eq!(image.jp2.unwrap().colour, ColourSpace::Unknown);
}

/// Under the raw channels OpenJPEG skips the palette and the channel
/// definitions and so still holds them when it reads the boxes after the
/// codestream: a second `cdef` or `pclr` there is an error, where the default
/// decode has dropped them and accepts it. Measured with the probe.
#[test]
fn the_raw_channels_keep_the_header_palette_and_definitions() {
    let cdef_twice = file(&[
        jp2_ftyp(),
        jp2h(&[good_ihdr(), colr(), identity_cdef()]),
        jp2c(),
        identity_cdef(),
    ]);
    assert!(decode(&cdef_twice).is_ok());
    assert!(
        matches!(decode_raw(&cdef_twice), Err(Error::FileFormat(m)) if m.contains("a second `cdef`"))
    );

    let pclr_twice = one_component(&[pclr_box()], &[pclr_box()]);
    assert!(decode(&pclr_twice).is_ok());
    assert!(
        matches!(decode_raw(&pclr_twice), Err(Error::FileFormat(m)) if m.contains("a second `pclr`"))
    );

    // A header palette without `cmap` is kept under the raw channels, so a
    // `cmap` after the codestream finds it.
    let late_cmap = one_component(&[pclr_box()], &[cmap_box()]);
    assert!(decode(&late_cmap).is_err());
    assert!(decode_raw(&late_cmap).is_ok());
}

/// When the last tile-part has `Psot = 0` it runs to the end of the file, so
/// OpenJPEG's walk after the decode starts there and reads nothing: boxes it
/// would reject after a codestream that ends in `EOC` decode.
#[test]
fn nothing_after_a_codestream_whose_last_tile_part_runs_to_the_end_is_read() {
    let codestream = psot_zero_codestream();
    let base = file(&[jp2_ftyp(), good_jp2h(), bx(b"jp2c", &codestream)]);
    let raw = decode(&codestream).expect("the Psot = 0 codestream decodes");
    for tail in [
        bx(b"colr", &[1]),
        SIGNATURE.to_vec(),
        jp2h(&[]),
        cmap_box(),
        header_only(3, b"junk"),
    ] {
        let image = decode(&[base.clone(), tail].concat()).expect("decodes");
        assert_eq!(image.components, raw.components);
    }
    // A short box length on that codestream: read to the end of the file.
    let mut bytes = file(&[jp2_ftyp(), good_jp2h()]);
    bytes.extend_from_slice(&header_only(108, b"jp2c"));
    bytes.extend_from_slice(&codestream);
    assert_eq!(decode(&bytes).expect("decodes").components, raw.components);
}

/// Padding inside a `jp2c` box longer than its codestream is walked for
/// OpenJPEG's reads, but the `xml ` and `uuid` boxes after the box are still
/// found by the box's length. Both files decode in OpenJPEG 2.5.0.
#[test]
fn padding_inside_the_codestream_box_does_not_hide_later_metadata() {
    let xml = bx(b"xml ", b"<a>hello</a>");
    for (what, padding) in [
        ("zeros", vec![0; 8]),
        ("junk", [0x12, 0x34, 0x56, 0x78].repeat(4)),
    ] {
        let bytes = [
            file(&[
                jp2_ftyp(),
                good_jp2h(),
                bx(b"jp2c", &[CODESTREAM, &padding].concat()),
            ]),
            xml.clone(),
        ]
        .concat();
        let image = decode(&bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
        let metadata = image.jp2.expect("a JP2 file");
        assert_eq!(metadata.xml, vec![b"<a>hello</a>".to_vec()], "{what}");
        assert_eq!(read_header(&bytes).unwrap().jp2, Some(metadata), "{what}");
    }
    // A `jp2c` shorter than its codestream: the boxes after `EOC` are found.
    let short = with_jp2c_length(108, &xml);
    let metadata = decode(&short).expect("decodes").jp2.expect("a JP2 file");
    assert_eq!(metadata.xml, vec![b"<a>hello</a>".to_vec()]);
}
