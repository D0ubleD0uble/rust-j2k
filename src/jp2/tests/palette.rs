//! Palette (`pclr` + `cmap`) tests through the public [`decode`]. Every shape
//! here was measured with `opj_decompress` 2.5.0; each test says what
//! OpenJPEG does with it.

use super::*;

/// A single-component 32x24 8-bit codestream, for OpenJPEG's
/// single-component correction.
const ONE_COMPONENT: &[u8] = include_bytes!("../../../tests/fixtures/multilayer_lossless.j2k");

/// Palette entry `j` of column `i`: no column is the identity, so a direct
/// channel and a lookup never agree by accident.
fn entry(j: usize, i: usize) -> u8 {
    ((j * (i + 1) + 40 * i + 17) & 0xFF) as u8
}

/// A `pclr` payload with `entries` rows over columns described by `bi`, each
/// entry `value(j, i)` stored big-endian in `ceil(depth / 8)` bytes.
fn pclr_payload(entries: u16, bi: &[u8], value: impl Fn(usize, usize) -> u64) -> Vec<u8> {
    let mut out = entries.to_be_bytes().to_vec();
    out.push(u8::try_from(bi.len()).unwrap());
    out.extend_from_slice(bi);
    for j in 0..usize::from(entries) {
        for (i, &b) in bi.iter().enumerate() {
            let bytes = (usize::from(b & 0x7F) + 1).div_ceil(8);
            // Up to 16 bytes for a 128-bit column: zero-pad past a u64.
            out.extend(std::iter::repeat_n(0, bytes.saturating_sub(8)));
            out.extend_from_slice(&value(j, i).to_be_bytes()[8 - bytes.min(8)..]);
        }
    }
    out
}

/// The standard test palette: 256 entries, three 8-bit columns.
pub(super) fn pclr_box() -> Vec<u8> {
    pclr_with(256)
}

fn pclr_with(entries: u16) -> Vec<u8> {
    bx(
        b"pclr",
        &pclr_payload(entries, &[7, 7, 7], |j, i| u64::from(entry(j, i))),
    )
}

/// A `cmap` box from `(CMP, MTYP, PCOL)` triples.
fn cmap_of(entries: &[(u16, u8, u8)]) -> Vec<u8> {
    let payload: Vec<u8> = entries
        .iter()
        .flat_map(|&(cmp, mtyp, pcol)| {
            let [c0, c1] = cmp.to_be_bytes();
            [c0, c1, mtyp, pcol]
        })
        .collect();
    bx(b"cmap", &payload)
}

/// Channel `i` is component 0 looked up in column `i`.
pub(super) fn cmap_box() -> Vec<u8> {
    cmap_of(&[(0, 1, 0), (0, 1, 1), (0, 1, 2)])
}

/// A file over the three-component fixture with these `jp2h` children after
/// `ihdr` and `colr`.
fn with_children(children: &[Vec<u8>]) -> Vec<u8> {
    let mut all = vec![good_ihdr(), colr()];
    all.extend_from_slice(children);
    file(&[jp2_ftyp(), jp2h(&all), jp2c()])
}

/// The same over the single-component codestream.
fn one_component_with(children: &[Vec<u8>]) -> Vec<u8> {
    let mut all = vec![ihdr(24, 32, 1), colr()];
    all.extend_from_slice(children);
    file(&[jp2_ftyp(), jp2h(&all), bx(b"jp2c", ONE_COMPONENT)])
}

/// What channel `column` holds when it looks up `indices` in the standard
/// palette of `entries` rows, clamping as OpenJPEG does.
fn looked_up(indices: &[i32], column: usize, entries: usize) -> Vec<i32> {
    indices
        .iter()
        .map(|&k| i32::from(entry((k.max(0) as usize).min(entries - 1), column)))
        .collect()
}

/// `image` is the standard palette over component 0 of the three-component
/// fixture: three 8-bit channels, channel `i` from column `i`.
#[track_caller]
pub(super) fn assert_palette_applied(image: &Image) {
    let index = &raw().components[0];
    assert_eq!(image.components.len(), 3);
    for (i, channel) in image.components.iter().enumerate() {
        assert_eq!((channel.width, channel.height), (WIDTH, HEIGHT));
        assert_eq!((channel.bit_depth, channel.signed), (8, false));
        assert_eq!(
            channel.samples,
            looked_up(&index.samples, i, 256),
            "channel {i}"
        );
    }
    let jp2 = image.jp2.as_ref().expect("JP2 metadata");
    assert!(jp2.palette_applied);
    assert_eq!(jp2.channels.len(), 3);
}

#[test]
fn every_validation_row_is_a_file_format_error() {
    let pclr = pclr_box();
    let cmap = cmap_box();
    let p = &pclr[8..];
    let rows: Vec<(&str, Vec<u8>, &str)> = vec![
        // OpenJPEG: "Need to read a PCLR box before the CMAP box."
        (
            "cmap without pclr",
            with_children(std::slice::from_ref(&cmap)),
            "`cmap` box but no `pclr`",
        ),
        (
            "cmap before pclr",
            with_children(&[cmap.clone(), pclr.clone()]),
            "comes before the `pclr`",
        ),
        (
            "cmap in jp2h, pclr misplaced after it",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), colr(), cmap.clone()]),
                pclr.clone(),
                jp2c(),
            ]),
            "comes before the `pclr`",
        ),
        (
            "cmap in jp2h, pclr in a second jp2h",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), colr(), cmap.clone()]),
                jp2h(&[good_ihdr(), pclr.clone()]),
                jp2c(),
            ]),
            "comes before the `pclr`",
        ),
        (
            "pclr misplaced before jp2h (ignored), cmap in jp2h",
            file(&[
                jp2_ftyp(),
                pclr.clone(),
                jp2h(&[good_ihdr(), colr(), cmap.clone()]),
                jp2c(),
            ]),
            "`cmap` box but no `pclr`",
        ),
        // OpenJPEG: "Invalid component index 3 (>= 3)."
        (
            "CMP >= Csiz",
            with_children(&[pclr.clone(), cmap_of(&[(0, 1, 0), (0, 1, 1), (3, 1, 2)])]),
            "maps component 3, but the codestream has 3",
        ),
        // OpenJPEG: "Invalid value for cmap[0].mtyp = 2."
        (
            "MTYP > 1",
            with_children(&[pclr.clone(), cmap_of(&[(0, 2, 0), (0, 1, 1), (0, 1, 2)])]),
            "mapping type 2",
        ),
        // OpenJPEG: "Invalid component/palette index for direct mapping 3."
        (
            "PCOL >= NPC",
            with_children(&[pclr.clone(), cmap_of(&[(0, 1, 0), (0, 1, 1), (0, 1, 3)])]),
            "names palette column 3, but the palette has 3",
        ),
        // OpenJPEG: "Direct use at #1 however pcol=1."
        (
            "MTYP 0 with PCOL != 0",
            with_children(&[pclr.clone(), cmap_of(&[(0, 1, 0), (1, 0, 1), (0, 1, 2)])]),
            "a direct channel's PCOL must be 0",
        ),
        // OpenJPEG: "Component 0 is mapped twice."
        (
            "a palette column mapped twice",
            with_children(&[pclr.clone(), cmap_of(&[(0, 1, 0), (0, 1, 0), (0, 1, 2)])]),
            "maps palette column 0 twice",
        ),
        // OpenJPEG: "Component 1 doesn't have a mapping." (and its pcol limit).
        (
            "a palette channel whose own column is unmapped",
            with_children(&[pclr.clone(), cmap_of(&[(0, 1, 2), (0, 1, 0), (1, 0, 0)])]),
            "palette column 1 is not mapped",
        ),
        // OpenJPEG fails the header read for each of these.
        (
            "two pclr boxes",
            with_children(&[pclr.clone(), pclr.clone(), cmap.clone()]),
            "more than one `pclr`",
        ),
        // OpenJPEG: "Only one CMAP box is allowed."
        (
            "two cmap boxes",
            with_children(&[pclr.clone(), cmap.clone(), cmap.clone()]),
            "more than one `cmap`",
        ),
        (
            "truncated pclr entries",
            with_children(&[bx(b"pclr", &p[..p.len() - 1]), cmap.clone()]),
            "need 768 bytes but it holds 767",
        ),
        // OpenJPEG: "Invalid PCLR box. Reports 0 entries"
        (
            "NE = 0",
            with_children(&[
                bx(b"pclr", &pclr_payload(0, &[7, 7, 7], |_, _| 0)),
                cmap.clone(),
            ]),
            "declares 0 entries",
        ),
        (
            "NE = 1025",
            with_children(&[
                bx(b"pclr", &pclr_payload(1025, &[7, 7, 7], |_, _| 0)),
                cmap.clone(),
            ]),
            "declares 1025 entries",
        ),
        // OpenJPEG: "Invalid PCLR box. Reports 0 palette columns"
        (
            "NPC = 0",
            with_children(&[bx(b"pclr", &[0, 4, 0, 0, 0, 0, 0]), cmap.clone()]),
            "0 palette columns",
        ),
        (
            "pclr shorter than its header",
            with_children(&[bx(b"pclr", &[0, 4]), cmap.clone()]),
            "fewer than its 3-byte header",
        ),
        (
            "pclr ending inside its Bi bytes",
            with_children(&[bx(b"pclr", &[0, 4, 3, 7]), cmap.clone()]),
            "ends inside their `Bi` bytes",
        ),
        // OpenJPEG: "Insufficient data for CMAP box."
        (
            "cmap with fewer than NPC entries",
            with_children(&[pclr.clone(), bx(b"cmap", &cmap[8..cmap.len() - 2])]),
            "needs 12",
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

/// OpenJPEG reads `NPC` entries from `cmap` and ignores the rest of the box,
/// whatever its length; extra `pclr` bytes are ignored too.
#[test]
fn bytes_past_the_last_cmap_or_pclr_entry_are_ignored() {
    let extra_channel = cmap_of(&[(0, 1, 0), (0, 1, 1), (0, 1, 2), (1, 0, 0)]);
    let mut odd = cmap_box();
    odd.extend_from_slice(&[0, 0]);
    odd[3] += 2;
    let mut long_pclr = pclr_box();
    long_pclr.extend_from_slice(&[0x99, 0x99]);
    long_pclr[3] += 2;
    for (what, children) in [
        ("a fourth cmap entry", vec![pclr_box(), extra_channel]),
        ("two stray cmap bytes", vec![pclr_box(), odd]),
        ("two stray pclr bytes", vec![long_pclr, cmap_box()]),
    ] {
        let image = decode(&with_children(&children)).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_palette_applied(&image);
    }
}

/// Indices past the palette clamp to its last entry (OpenJPEG clamps to
/// `0..NE`).
#[test]
fn indices_past_the_palette_clamp() {
    let image = decode(&with_children(&[pclr_with(100), cmap_box()])).expect("decodes");
    let index = &raw().components[0].samples;
    assert!(
        index.iter().any(|&k| k >= 100),
        "the fixture reaches past 99"
    );
    for (i, channel) in image.components.iter().enumerate() {
        assert_eq!(channel.samples, looked_up(index, i, 100), "channel {i}");
    }
}

/// A direct channel copies its component; OpenJPEG labels it with column
/// `i`'s `Bi`, here 4 bits, even though the samples are 8-bit.
#[test]
fn a_direct_channel_copies_its_component_with_column_i_depth() {
    let pclr = bx(
        b"pclr",
        &pclr_payload(256, &[3, 7, 0x87], |j, i| u64::from(entry(j, i))),
    );
    let image = decode(&with_children(&[
        pclr,
        cmap_of(&[(2, 0, 0), (0, 1, 1), (0, 1, 2)]),
    ]))
    .expect("decodes");
    let raw = raw();
    let direct = &image.components[0];
    assert_eq!(direct.samples, raw.components[2].samples);
    assert_eq!((direct.bit_depth, direct.signed), (4, false));
    assert_eq!(
        image.components[1].samples,
        looked_up(&raw.components[0].samples, 1, 256)
    );
    assert_eq!(
        (image.components[2].bit_depth, image.components[2].signed),
        (8, true)
    );
}

/// OpenJPEG has no general mapping ("Implementation limitation: for palette
/// mapping, pcol[0] should be equal to 0"): a palette channel `i` that looks
/// up another column is `Unsupported`, with or without a direct channel, and
/// before the single-component correction could rewrite it.
#[test]
fn a_palette_channel_reading_another_column_is_unsupported() {
    let message = "looks up palette column";
    unsupported(
        &with_children(&[pclr_box(), cmap_of(&[(0, 1, 2), (0, 1, 0), (0, 1, 1)])]),
        message,
    );
    // The review's `perm_direct_3comp` and `perm_direct_onecomp` shapes; the
    // second used to be rewritten by the correction and decode wrongly.
    unsupported(
        &with_children(&[pclr_box(), cmap_of(&[(0, 1, 1), (0, 1, 0), (2, 0, 0)])]),
        message,
    );
    unsupported(
        &one_component_with(&[pclr_box(), cmap_of(&[(0, 1, 1), (0, 1, 0), (0, 0, 0)])]),
        message,
    );
}

/// A palette channel takes its depth and sign from its column.
#[test]
fn a_palette_channel_takes_its_columns_format() {
    let pclr = bx(
        b"pclr",
        &pclr_payload(256, &[7, 11, 0x84], |j, i| u64::from(entry(j, i))),
    );
    let image = decode(&with_children(&[pclr, cmap_box()])).expect("decodes");
    let formats: Vec<(u8, bool)> = image
        .components
        .iter()
        .map(|c| (c.bit_depth, c.signed))
        .collect();
    assert_eq!(formats, [(8, false), (12, false), (5, true)]);
}

/// OpenJPEG's single-component correction ("Component mapping seems wrong.
/// Trying to correct."): a mapping that leaves a column unused is read as the
/// identity mapping. A direct channel 0 counts as using column 0, so it is
/// not corrected.
#[test]
fn a_single_component_mapping_that_leaves_a_column_unused_is_corrected() {
    let index = decode(ONE_COMPONENT).unwrap().components[0].samples.clone();
    let identity: Vec<Vec<i32>> = (0..3).map(|i| looked_up(&index, i, 256)).collect();
    let decode_samples = |cmap: &[(u16, u8, u8)]| -> Vec<Vec<i32>> {
        decode(&one_component_with(&[pclr_box(), cmap_of(cmap)]))
            .expect("decodes")
            .components
            .into_iter()
            .map(|c| c.samples)
            .collect()
    };
    assert_eq!(decode_samples(&[(0, 1, 0), (0, 1, 1), (0, 1, 2)]), identity);
    assert_eq!(decode_samples(&[(0, 0, 0), (0, 0, 0), (0, 0, 0)]), identity);
    assert_eq!(decode_samples(&[(0, 1, 0), (0, 0, 0), (0, 1, 2)]), identity);
    let direct_first = decode_samples(&[(0, 0, 0), (0, 1, 1), (0, 1, 2)]);
    assert_eq!(direct_first[0], index);
    assert_eq!(direct_first[1..], identity[1..]);
}

/// The correction is for single-component files only: the same direct
/// channel 1 on the three-component fixture is applied as written.
#[test]
fn a_multi_component_mapping_is_not_corrected() {
    let image = decode(&with_children(&[
        pclr_box(),
        cmap_of(&[(0, 1, 0), (2, 0, 0), (0, 1, 2)]),
    ]))
    .expect("decodes");
    assert_eq!(image.components[1].samples, raw().components[2].samples);
}

/// A column the `i32` container cannot hold is refused, as a codestream
/// component of that depth is. A signed 32-bit column fits.
#[test]
fn a_column_deeper_than_the_sample_container_is_unsupported() {
    for bi in [31, 32, 0x7F, 0xA0] {
        let pclr = bx(
            b"pclr",
            &pclr_payload(256, &[bi, 7, 7], |j, i| u64::from(entry(j, i))),
        );
        unsupported(&with_children(&[pclr, cmap_box()]), "exceeds the i32");
    }
    let pclr = bx(
        b"pclr",
        &pclr_payload(256, &[0x9F, 7, 7], |j, i| u64::from(entry(j, i))),
    );
    let image = decode(&with_children(&[pclr, cmap_box()])).expect("signed 32-bit decodes");
    assert_eq!(
        (image.components[0].bit_depth, image.components[0].signed),
        (32, true)
    );
}

/// `raw_channels` returns the components as coded, equal to the bare
/// codestream's decode, and labels the index plane as such.
#[test]
fn raw_channels_returns_the_index_plane() {
    let options = DecodeOptions {
        raw_channels: true,
        ..DecodeOptions::default()
    };
    let image = decode_with(&with_children(&[pclr_box(), cmap_box()]), options).expect("decodes");
    assert_eq!(image.components, raw().components);
    let jp2 = image.jp2.expect("JP2 metadata");
    assert!(!jp2.palette_applied);
    assert_eq!(
        jp2.channels,
        [
            ChannelDefinition {
                kind: ChannelType::Unspecified,
                association: Association::None,
            },
            ChannelDefinition {
                kind: ChannelType::Colour,
                association: Association::Colour(2),
            },
            ChannelDefinition {
                kind: ChannelType::Colour,
                association: Association::Colour(3),
            },
        ]
    );
}

/// Under `raw_channels` the box layout is still checked (OpenJPEG fails these
/// while reading the header), but the mapping is not checked against the
/// codestream (OpenJPEG skips `opj_jp2_check_color` when ignoring the palette).
#[test]
fn raw_channels_checks_the_boxes_but_not_the_mapping() {
    let options = DecodeOptions {
        raw_channels: true,
        ..DecodeOptions::default()
    };
    let bad_mapping = with_children(&[pclr_box(), cmap_of(&[(9, 3, 7), (0, 1, 1), (0, 1, 2)])]);
    let image = decode_with(&bad_mapping, options.clone()).expect("decodes raw");
    assert_eq!(image.components, raw().components);
    assert!(matches!(decode(&bad_mapping), Err(Error::FileFormat(_))));

    let bad_box = with_children(&[bx(b"pclr", &[0, 0, 3, 7, 7, 7]), cmap_box()]);
    assert!(matches!(
        decode_with(&bad_box, options.clone()),
        Err(Error::FileFormat(_))
    ));

    // The review's `w38_opj`: a 38-bit column stored at OpenJPEG's 4-byte
    // stride. Applied it is too deep; raw, it does not block the index plane,
    // as OpenJPEG decodes it under its ignore flag.
    let mut deep = 256u16.to_be_bytes().to_vec();
    deep.extend_from_slice(&[3, 37, 7, 7]);
    for j in 0..256u32 {
        deep.extend_from_slice(&j.to_be_bytes());
        deep.extend_from_slice(&[entry(j as usize, 1), entry(j as usize, 2)]);
    }
    let deep = with_children(&[bx(b"pclr", &deep), cmap_box()]);
    unsupported(&deep, "exceeds the i32");
    let image = decode_with(&deep, options.clone()).expect("decodes raw");
    assert_eq!(image.components, raw().components);
}

/// A reduced decode applies the palette to the reduced index plane.
#[test]
fn a_reduced_decode_applies_the_palette() {
    let options = DecodeOptions::default().with_resolution_reduction(1);
    let image =
        decode_with(&with_children(&[pclr_box(), cmap_box()]), options.clone()).expect("decodes");
    let index = decode_with(CODESTREAM, options.clone()).unwrap().components[0].clone();
    for (i, channel) in image.components.iter().enumerate() {
        assert_eq!((channel.width, channel.height), (index.width, index.height));
        assert_eq!(channel.samples, looked_up(&index.samples, i, 256));
    }
}

/// A `cdef` after a palette describes the palette's channels: reversing
/// them reverses the looked-up columns (measured with `opj_decompress` 2.5.0,
/// also with the `cdef` placed before the `pclr`).
#[test]
fn channel_definitions_apply_to_the_palette_channels() {
    let reversed = super::channels::reversed();
    for children in [
        [pclr_box(), cmap_box(), reversed.clone()],
        [reversed.clone(), pclr_box(), cmap_box()],
    ] {
        let image = decode(&one_component_with(&children)).expect("decodes");
        let index = decode(ONE_COMPONENT).unwrap().components[0].clone();
        for (k, channel) in image.components.iter().enumerate() {
            assert_eq!(
                channel.samples,
                looked_up(&index.samples, 2 - k, 256),
                "channel {k}"
            );
        }
        let jp2 = image.jp2.expect("metadata");
        assert!(jp2.palette_applied);
        assert_eq!(jp2.channels, super::channels::colours(3));
    }
}

/// `read_header` reports the channels in the order `decode` returns them,
/// after the palette and the `cdef` placement. The palette's columns differ
/// in depth and sign, so a header that skipped the placement would list them
/// in the wrong order.
#[test]
fn read_header_places_palette_channels_by_their_definitions() {
    let pclr = bx(
        b"pclr",
        &pclr_payload(256, &[7, 11, 0x84], |j, i| u64::from(entry(j, i))),
    );
    let bytes = with_children(&[pclr, cmap_box(), super::channels::reversed()]);
    let image = decode(&bytes).expect("decodes");
    let header = read_header(&bytes).expect("header");
    let decoded: Vec<(u8, bool)> = image
        .components
        .iter()
        .map(|c| (c.bit_depth, c.signed))
        .collect();
    let read: Vec<(u8, bool)> = header
        .components
        .iter()
        .map(|c| (c.bit_depth, c.signed))
        .collect();
    assert_eq!(decoded, [(5, true), (12, false), (8, false)]);
    assert_eq!(read, decoded);
    assert_eq!(header.jp2, image.jp2);
}
