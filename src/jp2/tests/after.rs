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
pub(super) fn no_color_then(tail: &[Vec<u8>]) -> Vec<u8> {
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

/// A `cdef` labeling the three components as colors 1, 2, 3.
pub(super) fn identity_cdef() -> Vec<u8> {
    super::channels::cdef_of(&[(0, 0, 1), (1, 0, 2), (2, 0, 3)])
}

pub(super) fn decode_raw(bytes: &[u8]) -> crate::Result<Image> {
    let options = DecodeOptions {
        raw_channels: true,
        ..DecodeOptions::default()
    };
    decode_with(bytes, &options)
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
            no_color_then(&[short_meth1()]),
            "`METH` 1 holds 4 bytes",
        ),
        (
            "an empty METH 2 colr with no usable colr before it",
            no_color_then(&[bx(b"colr", &[2, 0, 0])]),
            "`METH` 2 holds no ICC profile",
        ),
        (
            "a short METH 1 colr after an ignored METH 3 one",
            no_color_then(&[bx(b"colr", &[3, 0, 0]), short_meth1()]),
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
            no_color_then(&[colr(), bx(b"colr", &[1, 0, 0, 0])]),
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

/// A `colr` read after the codestream does not set the color space: OpenJPEG
/// has set the image's color space before it reads it.
#[test]
fn a_colr_after_the_codestream_does_not_set_the_color_space() {
    let image = decode(&no_color_then(&[colr()])).unwrap();
    assert_eq!(image.jp2.unwrap().color, ColorSpace::Unknown);
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

/// Codestreams whose one-byte packet headers each visit every code-block of the
/// tile, from `t2bomb` in `scripts/gen-psot0-jp2-fixtures.py`: one past the
/// visit bound on the walk that checks where a `Psot = 0` codestream ends, and
/// one past its geometry bound.
const T2BOMB_VISITS: &[u8] = include_bytes!("../../../tests/fixtures/psot0/jp2_t2bomb_visits.jp2");
const T2BOMB_BLOCKS: &[u8] = include_bytes!("../../../tests/fixtures/psot0/jp2_t2bomb_blocks.jp2");
/// The same shape with 56 code-blocks and 19000 layers: about a million
/// visits, within the bound.
const T2_MANY_LAYERS: &[u8] =
    include_bytes!("../../../tests/fixtures/psot0/jp2_t2_many_layers.jp2");
/// RPCL and PCRL sweeps of 256^2 positions that check 64 components at each,
/// for 1148 empty packets, from `sweep` in the same script.
const T2_SWEEP_RPCL: &[u8] = include_bytes!("../../../tests/fixtures/psot0/jp2_t2_sweep_rpcl.jp2");
const T2_SWEEP_PCRL: &[u8] = include_bytes!("../../../tests/fixtures/psot0/jp2_t2_sweep_pcrl.jp2");
/// A CPRL sweep of one 130x130 component under 1x1 precincts, from
/// `cprl_sweep`: 16900 precincts, past the geometry bound.
const T2_SWEEP_CPRL: &[u8] = include_bytes!("../../../tests/fixtures/psot0/jp2_t2_sweep_cprl.jp2");
/// CPRL sweeps whose step comes from different resolutions on each axis, from
/// `cprl_steps`. The tall one has 512 precincts and sweeps 2^23 positions; the
/// POC one repeats a 64^2-position sweep over 33 resolutions in 31 volumes.
/// Both stay within the geometry bound and pass the visit bound.
const T2_SWEEP_CPRL_TALL: &[u8] =
    include_bytes!("../../../tests/fixtures/psot0/jp2_t2_sweep_cprl_tall.jp2");
const T2_SWEEP_CPRL_POC: &[u8] =
    include_bytes!("../../../tests/fixtures/psot0/jp2_t2_sweep_cprl_poc.jp2");

/// The codestream of `file` cut two bytes past its `jp2c` box's end, the slice
/// the walk is run on.
fn box_end_cut(file: &[u8]) -> &[u8] {
    let mut at = 0;
    loop {
        let length = u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        if &file[at + 4..at + 8] == b"jp2c" {
            return &file[at + 8..at + length + 2];
        }
        at += length;
    }
}

/// The walk stops at its bounds and answers that the codestream does not end
/// at the box's end, after at most [`PROBE_VISITS_MIN`] visits for a tile this
/// small, whether the visits are code-block entries or sweep positions, and
/// after none for a tile past the geometry bound. Unbounded, the same walks go
/// past both.
#[test]
fn the_packet_walk_at_the_box_end_is_bounded() {
    use crate::jp2::{PROBE_GEOMETRY, PROBE_VISITS_MIN, probe_packets};
    use crate::tier2::{Budget, MAX_CODE_BLOCKS, decode_packets_within};

    let unbounded = |cut: &[u8]| {
        let parsed = crate::codestream::parse_header(cut).unwrap();
        let cs = crate::codestream::parse_tiles(cut, parsed).unwrap();
        let header = cs.tile_header(0).unwrap();
        let mut budget = Budget::decode(u64::MAX);
        decode_packets_within(&header, &cs.tiles[0], &mut budget).expect("ends at the cut");
        (u64::MAX - budget.visits, MAX_CODE_BLOCKS - budget.blocks)
    };

    for file in [
        T2BOMB_VISITS,
        T2_SWEEP_RPCL,
        T2_SWEEP_PCRL,
        T2_SWEEP_CPRL_TALL,
        T2_SWEEP_CPRL_POC,
    ] {
        let cut = box_end_cut(file);
        assert!(unbounded(cut).0 > PROBE_VISITS_MIN);
        let (ends, visits) = probe_packets(cut, 0);
        assert!(!ends);
        assert!(visits <= PROBE_VISITS_MIN, "{visits} visits");
        // The visit bound is what stopped the walk, not the geometry bound
        // (which spends none): it stopped on a charge larger than what was
        // left, and one charge in these files is at most a precinct's
        // code-blocks (held to `PROBE_GEOMETRY` by the geometry bound) or a
        // sweep step over a few components and resolutions.
        assert!(
            visits > PROBE_VISITS_MIN - PROBE_GEOMETRY as u64,
            "{visits} visits"
        );
    }

    for file in [T2BOMB_BLOCKS, T2_SWEEP_CPRL] {
        let cut = box_end_cut(file);
        assert!(unbounded(cut).1 > PROBE_GEOMETRY);
        assert_eq!(probe_packets(cut, 0), (false, 0));
    }

    let cut = box_end_cut(T2_MANY_LAYERS);
    assert!(probe_packets(cut, 0).0);
}

/// A tile with more data may spend more: 8 visits per byte past 2^19 bytes.
#[test]
fn the_packet_walk_bound_grows_with_the_tile_data() {
    use crate::jp2::probe_visits;
    assert_eq!(probe_visits(0), 1 << 22);
    assert_eq!(probe_visits(1 << 19), 1 << 22);
    assert_eq!(probe_visits((1 << 19) + 1), (1 << 22) + 8);
    assert_eq!(probe_visits(1 << 24), 1 << 27);
    assert_eq!(probe_visits(u64::MAX), u64::MAX);
}

/// The decode's work bound is no smaller than this walk's at any size, so a
/// single-tile codestream whose packets this walk finishes is one the
/// decode's walk finishes too: the box's end is never chosen for a tile the
/// decode then refuses at its work bound.
#[test]
fn the_decode_work_bound_covers_the_packet_walk() {
    use crate::jp2::probe_visits;
    use crate::tier2::decode_visits;
    for bytes in [
        0,
        1 << 19,
        1 << 21,
        (1 << 21) + 1,
        1 << 24,
        1 << 40,
        u64::MAX,
    ] {
        assert!(probe_visits(bytes) <= decode_visits(bytes), "{bytes} bytes");
    }
}

/// The walk resolves only the walked tile's header, not every tile's (which
/// costs `tiles × components`, seconds for a file of 100 KB). A fault in
/// another tile's header (here tile 0's, a COD pairing the 9/7 wavelet with
/// no quantization) fails the codestream once every tile's header is
/// resolved, but not the walk of the last tile; the decode refuses that file
/// either way.
#[test]
fn the_packet_walk_resolves_only_its_own_tile() {
    let mut codestream =
        include_bytes!("../../../tests/fixtures/psot0/tiled_psot0_no_eoc.j2k").to_vec();
    let at = |bytes: &[u8], code: [u8; 2]| bytes.windows(2).position(|w| w == code).unwrap();
    let cod = at(&codestream, [0xFF, 0x52]);
    let length = usize::from(u16::from_be_bytes([
        codestream[cod + 2],
        codestream[cod + 3],
    ]));
    let mut other = codestream[cod..cod + 2 + length].to_vec();
    other[13] ^= 1;
    // Tile 0's tile-part comes first: put the COD before its SOD and grow its
    // `Psot` to match.
    let sot = at(&codestream, [0xFF, 0x90]);
    let psot = u32::from_be_bytes(codestream[sot + 6..sot + 10].try_into().unwrap());
    let grown = psot + u32::try_from(other.len()).unwrap();
    codestream[sot + 6..sot + 10].copy_from_slice(&grown.to_be_bytes());
    codestream.splice(sot + 12..sot + 12, other);
    // Room for an EOC, as in the cut two bytes past the box's end.
    codestream.extend_from_slice(&[0, 0]);

    let parsed = crate::codestream::parse_header(&codestream).unwrap();
    let whole = crate::codestream::parse_tiles(&codestream, parsed).map(|_| ());
    assert!(matches!(whole, Err(Error::Marker(_))), "{whole:?}");
    assert!(crate::jp2::probe_packets(&codestream, 3).0);
}

/// A codestream of `tiles` 1x1 tiles in a row and `components` 8-bit
/// components, no wavelet levels and one layer, each packet empty (one zero
/// byte). Every tile-part has its length but the last, which has `Psot = 0`,
/// and two bytes of room for an `EOC` follow, as in the cut past a box's end.
fn empty_packets(tiles: u32, components: u16) -> Vec<u8> {
    let segment = |code: u16, body: &[u8]| {
        let mut out = code.to_be_bytes().to_vec();
        out.extend_from_slice(&u16::try_from(body.len() + 2).unwrap().to_be_bytes());
        out.extend_from_slice(body);
        out
    };
    let mut siz = vec![0, 0];
    for value in [tiles, 1, 0, 0, 1, 1, 0, 0] {
        siz.extend_from_slice(&value.to_be_bytes());
    }
    siz.extend_from_slice(&components.to_be_bytes());
    for _ in 0..components {
        siz.extend_from_slice(&[7, 1, 1]);
    }
    let mut out = vec![0xFF, 0x4F];
    out.extend(segment(0xFF51, &siz));
    out.extend(segment(0xFF52, &[0, 0, 0, 1, 0, 0, 4, 4, 0, 1]));
    out.extend(segment(0xFF5C, &[0x40, 0x40]));
    let packets = usize::from(components);
    for tile in 0..tiles {
        let psot = if tile + 1 == tiles {
            0
        } else {
            14 + packets as u32
        };
        out.extend_from_slice(&[0xFF, 0x90, 0, 10]);
        out.extend_from_slice(&u16::try_from(tile).unwrap().to_be_bytes());
        out.extend_from_slice(&psot.to_be_bytes());
        out.extend_from_slice(&[0, 1, 0xFF, 0x93]);
        out.extend(std::iter::repeat_n(0, packets));
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// A codestream of more than [`PROBE_TILES`] tiles or [`PROBE_COMPONENTS`]
/// components is not walked, since its tile list and the walked tile's
/// header grow with them; at the bound it is.
#[test]
fn the_packet_walk_skips_a_codestream_of_too_many_tiles_or_components() {
    use crate::jp2::{PROBE_COMPONENTS, PROBE_TILES, probe_packets};

    let tiles = u32::try_from(PROBE_TILES).unwrap();
    assert!(probe_packets(&empty_packets(tiles, 1), (tiles - 1) as u16).0);
    assert_eq!(
        probe_packets(&empty_packets(tiles + 1, 1), tiles as u16),
        (false, 0)
    );

    let components = u16::try_from(PROBE_COMPONENTS).unwrap();
    assert!(probe_packets(&empty_packets(1, components), 0).0);
    assert_eq!(
        probe_packets(&empty_packets(1, components + 1), 0),
        (false, 0)
    );
}
