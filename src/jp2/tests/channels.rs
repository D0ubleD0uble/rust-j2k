//! Channel definition (`cdef`) tests through the public [`decode`]. Every
//! shape here was measured with `opj_decompress` 2.5.0; each test says what
//! OpenJPEG does with it.

use super::*;
use crate::Component;

/// A `cdef` box from `(Cn, Typ, Asoc)` triples, with `N` the triple count.
pub(super) fn cdef_of(entries: &[(u16, u16, u16)]) -> Vec<u8> {
    let count = u16::try_from(entries.len()).unwrap();
    bx(b"cdef", &cdef_payload(count, entries))
}

fn cdef_payload(count: u16, entries: &[(u16, u16, u16)]) -> Vec<u8> {
    let mut out = count.to_be_bytes().to_vec();
    for &(cn, typ, asoc) in entries {
        out.extend_from_slice(&cn.to_be_bytes());
        out.extend_from_slice(&typ.to_be_bytes());
        out.extend_from_slice(&asoc.to_be_bytes());
    }
    out
}

/// file2's layout: components 0, 1, 2 are colours 3, 2, 1.
pub(super) fn reversed() -> Vec<u8> {
    cdef_of(&[(0, 0, 3), (1, 0, 2), (2, 0, 1)])
}

/// Channels `1..=n` labelled as colours in order.
pub(super) fn colours(n: u16) -> Vec<ChannelDefinition> {
    (1..=n)
        .map(|c| ChannelDefinition {
            kind: ChannelType::Colour,
            association: Association::Colour(c),
        })
        .collect()
}

/// The three-component fixture decoded with its channels reversed and
/// labelled as colours 1, 2, 3.
#[track_caller]
pub(super) fn assert_reversed(image: &Image) {
    let raw = raw();
    let want: Vec<&Component> = raw.components.iter().rev().collect();
    let got: Vec<&Component> = image.components.iter().collect();
    assert_eq!(got, want, "channels reversed");
    assert_eq!(image.jp2.as_ref().expect("metadata").channels, colours(3));
}

/// A file over the three-component fixture with these `jp2h` children after
/// `ihdr` and `colr`.
fn with_children(children: &[Vec<u8>]) -> Vec<u8> {
    let mut all = vec![good_ihdr(), colr()];
    all.extend_from_slice(children);
    file(&[jp2_ftyp(), jp2h(&all), jp2c()])
}

/// The channels of `bytes` as indices into the bare codestream's components.
fn order(bytes: &[u8]) -> Vec<usize> {
    let raw = raw();
    decode(bytes)
        .expect("decodes")
        .components
        .iter()
        .map(|c| {
            raw.components
                .iter()
                .position(|r| r == c)
                .expect("a codestream component")
        })
        .collect()
}

fn definition(kind: ChannelType, association: Association) -> ChannelDefinition {
    ChannelDefinition { kind, association }
}

/// The shapes OpenJPEG accepts, with the order it outputs.
#[test]
fn accepted_layouts_come_out_in_openjpeg_order() {
    let cases: &[(&str, Vec<u8>, [usize; 3])] = &[
        (
            "identity",
            cdef_of(&[(0, 0, 1), (1, 0, 2), (2, 0, 3)]),
            [0, 1, 2],
        ),
        ("reversed", reversed(), [2, 1, 0]),
        (
            "reversed, listed backwards",
            cdef_of(&[(2, 0, 1), (1, 0, 2), (0, 0, 3)]),
            [2, 1, 0],
        ),
        (
            "rotated",
            cdef_of(&[(0, 0, 2), (1, 0, 3), (2, 0, 1)]),
            [2, 0, 1],
        ),
        (
            "two entries claim colour 2",
            cdef_of(&[(0, 0, 2), (1, 0, 2), (2, 0, 3)]),
            [0, 1, 2],
        ),
        (
            "an opacity channel displaced by a colour",
            cdef_of(&[(1, 1, 0), (0, 0, 2), (2, 0, 3)]),
            [1, 0, 2],
        ),
        (
            "an opacity entry with a colour association moves nothing",
            cdef_of(&[(0, 1, 3), (1, 0, 2), (2, 0, 1)]),
            [2, 1, 0],
        ),
        (
            "unspecified entries with colour associations move nothing",
            cdef_of(&[(0, 65535, 3), (1, 65535, 2), (2, 65535, 1)]),
            [0, 1, 2],
        ),
        (
            "Asoc 0 and 65535 on colour channels",
            cdef_of(&[(0, 0, 0), (1, 0, 65535), (2, 0, 3)]),
            [0, 1, 2],
        ),
        (
            "a channel named twice",
            cdef_of(&[(0, 0, 1), (1, 0, 2), (2, 0, 3), (2, 1, 0)]),
            [0, 1, 2],
        ),
        (
            "bytes after the last entry",
            bx(
                b"cdef",
                &[
                    cdef_payload(3, &[(0, 0, 1), (1, 0, 2), (2, 0, 3)]),
                    vec![0xFF; 3],
                ]
                .concat(),
            ),
            [0, 1, 2],
        ),
        (
            "a reserved type",
            cdef_of(&[(0, 7, 1), (1, 0, 2), (2, 0, 3)]),
            [0, 1, 2],
        ),
    ];
    for (what, cdef, want) in cases {
        assert_eq!(
            order(&with_children(std::slice::from_ref(cdef))),
            want,
            "{what}"
        );
    }
}

/// Each channel carries its own entry; the label moves with the samples.
#[test]
fn every_channel_is_labelled_with_its_entry() {
    use Association as A;
    use ChannelType as T;
    let cases: &[(&str, Vec<u8>, [ChannelDefinition; 3])] = &[
        (
            "an opacity channel displaced by a colour",
            cdef_of(&[(1, 1, 0), (0, 0, 2), (2, 0, 3)]),
            [
                definition(T::Opacity, A::WholeImage),
                definition(T::Colour, A::Colour(2)),
                definition(T::Colour, A::Colour(3)),
            ],
        ),
        (
            "every type unspecified",
            cdef_of(&[(0, 65535, 65535), (1, 65535, 65535), (2, 65535, 65535)]),
            [definition(T::Unspecified, A::None); 3],
        ),
        (
            "premultiplied, reserved and colour",
            cdef_of(&[(0, 2, 0), (1, 9, 65535), (2, 0, 3)]),
            [
                definition(T::PremultipliedOpacity, A::WholeImage),
                definition(T::Other(9), A::None),
                definition(T::Colour, A::Colour(3)),
            ],
        ),
        (
            "a channel named twice keeps its last entry",
            cdef_of(&[(0, 0, 1), (1, 0, 2), (2, 0, 3), (2, 1, 0)]),
            [
                definition(T::Colour, A::Colour(1)),
                definition(T::Colour, A::Colour(2)),
                definition(T::Opacity, A::WholeImage),
            ],
        ),
    ];
    for (what, cdef, want) in cases {
        let image = decode(&with_children(std::slice::from_ref(cdef))).expect(what);
        assert_eq!(image.jp2.expect("metadata").channels, want, "{what}");
    }
}

/// Every malformed `cdef` OpenJPEG refuses is `FileFormat`.
#[test]
fn malformed_channel_definitions_are_file_format_errors() {
    let good = || cdef_of(&[(0, 0, 1), (1, 0, 2), (2, 0, 3)]);
    let rows: Vec<(&str, Vec<u8>, &str)> = vec![
        // "Insufficient data for CDEF box."
        (
            "an empty box",
            with_children(&[bx(b"cdef", &[])]),
            "fewer than its 2-byte count",
        ),
        (
            "one byte",
            with_children(&[bx(b"cdef", &[0])]),
            "fewer than its 2-byte count",
        ),
        // "Number of channel description is equal to zero in CDEF box."
        (
            "N = 0",
            with_children(&[bx(b"cdef", &[0, 0])]),
            "declares 0 channel definitions",
        ),
        (
            "N = 3 with two entries",
            with_children(&[bx(b"cdef", &cdef_payload(3, &[(0, 0, 1), (1, 0, 2)]))]),
            "declares 3 channel definitions (18 bytes) but holds 12",
        ),
        (
            "N = 65535 with one entry",
            with_children(&[bx(b"cdef", &cdef_payload(65535, &[(0, 0, 1)]))]),
            "declares 65535 channel definitions",
        ),
        // "Invalid component index 3 (>= 3)."
        (
            "Cn past the last channel",
            with_children(&[cdef_of(&[(0, 0, 1), (1, 0, 2), (2, 0, 3), (3, 0, 1)])]),
            "entry 3 describes channel 3, but the file has 3",
        ),
        (
            "Asoc - 1 past the last channel",
            with_children(&[cdef_of(&[(0, 0, 1), (1, 0, 2), (2, 0, 4)])]),
            "with colour 4, but the file has 3 channels",
        ),
        (
            "Asoc - 1 past the last channel, type unspecified",
            with_children(&[cdef_of(&[(0, 0, 1), (1, 0, 2), (2, 65535, 4)])]),
            "with colour 4, but the file has 3 channels",
        ),
        // "Incomplete channel definitions."
        (
            "a channel with no definition",
            with_children(&[cdef_of(&[(0, 0, 1), (1, 0, 2)])]),
            "does not define channel 2",
        ),
        (
            "every entry names channel 0",
            with_children(&[cdef_of(&[(0, 0, 1), (0, 0, 2), (0, 0, 3)])]),
            "does not define channel 1",
        ),
        // "failed to read the header" (the second `cdef` handler fails).
        (
            "two cdef in jp2h",
            with_children(&[good(), good()]),
            "more than one `cdef`",
        ),
        (
            "a second cdef in a second jp2h",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), colr(), good()]),
                jp2h(&[good_ihdr(), reversed()]),
                jp2c(),
            ]),
            "more than one `cdef`",
        ),
        (
            "a second cdef after jp2h",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), colr(), good()]),
                reversed(),
                jp2c(),
            ]),
            "more than one `cdef`",
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

/// A `cdef` before `jp2h` is ignored, even a malformed one ("JPEG2000 Header
/// box not read yet, 'cdef' box will be ignored").
#[test]
fn a_cdef_before_jp2h_is_ignored() {
    for cdef in [reversed(), bx(b"cdef", &[0, 0])] {
        let bytes = file(&[jp2_ftyp(), cdef, good_jp2h(), jp2c()]);
        assert_eq!(order(&bytes), [0, 1, 2]);
    }
}

/// With a palette, the entries are checked against the palette's channels;
/// with a `pclr` but no `cmap` (the palette is dropped), against the
/// codestream's components.
#[test]
fn entries_are_checked_against_the_channels_after_the_palette() {
    use super::palette::{cmap_box, pclr_box};
    const ONE: &[u8] = include_bytes!("../../../tests/fixtures/multilayer_lossless.j2k");
    let one = |children: &[Vec<u8>]| {
        let mut all = vec![ihdr(24, 32, 1), colr()];
        all.extend_from_slice(children);
        file(&[jp2_ftyp(), jp2h(&all), bx(b"jp2c", ONE)])
    };
    let four = cdef_of(&[(0, 0, 1), (1, 0, 2), (2, 0, 3), (3, 0, 1)]);
    assert!(decode(&one(&[pclr_box(), cmap_box(), reversed()])).is_ok());
    file_format(
        &one(&[pclr_box(), cmap_box(), four]),
        "describes channel 3, but the file has 3",
    );
    file_format(
        &one(&[pclr_box(), reversed()]),
        "with colour 3, but the file has 1 channels",
    );
    let image = decode(&one(&[pclr_box(), cdef_of(&[(0, 1, 0)])])).expect("decodes");
    assert_eq!(
        image.jp2.unwrap().channels,
        [definition(ChannelType::Opacity, Association::WholeImage)]
    );
}

/// `raw_channels` skips the `cdef`: the components come out as coded, with
/// the default labels, and the entries are not checked (OpenJPEG skips
/// `opj_jp2_check_color` under its ignore flag). The box layout still is.
#[test]
fn raw_channels_skips_the_channel_definitions() {
    let options = DecodeOptions {
        raw_channels: true,
        ..DecodeOptions::default()
    };
    let raw = raw();
    for cdef in [
        reversed(),
        cdef_of(&[(0, 0, 1), (1, 0, 2)]),
        cdef_of(&[(5, 0, 9)]),
    ] {
        let image = decode_with(&with_children(&[cdef]), &options).expect("decodes");
        assert_eq!(image.components, raw.components);
        assert_eq!(image.jp2.unwrap().channels, colours(3));
    }
    for (bytes, fragment) in [
        (
            with_children(&[bx(b"cdef", &[0, 0])]),
            "0 channel definitions",
        ),
        (
            with_children(&[reversed(), reversed()]),
            "more than one `cdef`",
        ),
    ] {
        match decode_with(&bytes, &options) {
            Err(Error::FileFormat(message)) => assert!(message.contains(fragment), "{message}"),
            other => panic!("expected FileFormat, got {other:?}"),
        }
    }
}

/// A palette's index planes stay labelled as index planes under
/// `raw_channels`, with a `cdef` present too.
#[test]
fn raw_channels_keeps_the_index_plane_label_with_a_cdef() {
    use super::palette::{cmap_box, pclr_box};
    let options = DecodeOptions {
        raw_channels: true,
        ..DecodeOptions::default()
    };
    let image = decode_with(
        &with_children(&[pclr_box(), cmap_box(), reversed()]),
        &options,
    )
    .expect("decodes");
    let channels = image.jp2.unwrap().channels;
    assert_eq!(
        channels[0],
        definition(ChannelType::Unspecified, Association::None)
    );
    assert_eq!(channels[1..], colours(3)[1..]);
}

/// A reduced decode reorders the reduced channels the same way.
#[test]
fn a_reduced_decode_applies_the_channel_definitions() {
    let options = DecodeOptions::default().with_resolution_reduction(1);
    let image = decode_with(&with_children(&[reversed()]), &options).expect("decodes");
    let raw = decode_with(CODESTREAM, &options).unwrap();
    let want: Vec<_> = raw.components.iter().rev().cloned().collect();
    assert_eq!(image.components, want);
}

/// Truncating a file with a `cdef` anywhere, or corrupting any byte of the
/// box, never panics.
#[test]
fn a_damaged_cdef_never_panics() {
    let bytes = with_children(&[reversed()]);
    let at = bytes.windows(4).position(|w| w == b"cdef").unwrap() - 4;
    for end in at..at + 30 {
        let _ = decode(&bytes[..end]);
    }
    for i in at..at + 28 {
        for value in [0x00, 0x01, 0x7F, 0xFF] {
            let mut damaged = bytes.clone();
            damaged[i] = value;
            let _ = decode(&damaged);
        }
    }
}
