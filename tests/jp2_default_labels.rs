//! The labels on `Jp2Metadata::channels` for a file without a `cdef`, or when
//! `raw_channels` leaves the `cdef` unapplied: only the color space's own
//! channels are colors (I.5.3.6 gives a channel past them no meaning), and
//! the rest are unspecified.

mod support;

use rust_j2k::{
    Association, ChannelDefinition, ChannelType, ColorOutput, Component, DecodeOptions, decode,
    decode_with, read_header_with,
};
use support::jp2::sha256_hex;

fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

fn srgb() -> DecodeOptions {
    DecodeOptions::default().with_color(ColorOutput::Srgb)
}

/// The hash `scripts/gen-icc-oracles.py` records: samples in raster order,
/// each a 4-byte big-endian signed integer.
fn plane_hash(component: &Component) -> String {
    let bytes: Vec<u8> = component
        .samples
        .iter()
        .flat_map(|s| s.to_be_bytes())
        .collect();
    sha256_hex(&bytes)
}

fn color(n: u16) -> ChannelDefinition {
    ChannelDefinition {
        kind: ChannelType::Color,
        association: Association::Color(n),
    }
}

const UNSPECIFIED: ChannelDefinition = ChannelDefinition {
    kind: ChannelType::Unspecified,
    association: Association::None,
};

fn read(path: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(path);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn raw() -> DecodeOptions {
    DecodeOptions::default().with_raw_channels(true)
}

/// sRGB files with an opacity channel: `raw_channels` skips the `cdef`, so
/// the fourth channel is past sRGB's three colors.
#[test]
fn a_fourth_channel_of_an_srgb_file_is_unspecified_without_the_cdef() {
    for path in ["jp2_real/3_13bit_and_1bit.jp2", "jp2_cdef_rgba.jp2"] {
        let bytes = read(path);
        let want = vec![color(1), color(2), color(3), UNSPECIFIED];
        let image = decode_with(&bytes, &raw()).unwrap();
        assert_eq!(image.jp2.as_ref().unwrap().channels, want, "{path}: decode");
        let header = read_header_with(&bytes, &raw()).unwrap();
        assert_eq!(header.jp2.unwrap().channels, want, "{path}: header");
        // The default decode applies the `cdef`, which labels it as written.
        let applied = decode(&bytes).unwrap();
        assert_eq!(
            applied.jp2.unwrap().channels[3].kind,
            ChannelType::Opacity,
            "{path}: with the cdef"
        );
    }
}

/// An enumerated sRGB file of four components with no `cdef` whose fourth is
/// signed 8-bit, unlike the three unsigned colors. The fourth channel is
/// past sRGB's colors, so the sRGB step leaves it alone and passes the file
/// through, as OpenJPEG does (`opj_decompress -force-rgb` writes the four
/// planes as coded; the hashes are of its PGX output). Same as the file with
/// a `cdef`.
///
/// The file is the codestream of `jp2_cdef_rgba.jp2` (16x12, four 8-bit
/// components) with the fourth component's `Ssiz` set to signed, in a JP2
/// file with `ihdr` BPC 255, a `bpcc` box and an enumerated sRGB `colr`.
#[test]
fn an_srgb_file_with_a_differently_formatted_fourth_channel_passes_through() {
    const PLANES: [&str; 4] = [
        "9440fef292cd0d865bc9d98c84ef8a648e5cc79bc95bf54857f32b3fb207722a",
        "70be7146623f03e79146c2b179778ffa52c2cabfd1674839bd9115c326450c9b",
        "d086834ee4d5cc3b7340601f3ec141c0adae87e3fb4d340ef74955e5feb17946",
        "2f2e0062b7ebc44c2b561ae57c532497543f930e791a9d5dfbd9da60d782fa0b",
    ];
    let source = read("jp2_cdef_rgba.jp2");
    let at = source.windows(4).position(|w| w == b"jp2c").unwrap() + 4;
    let mut codestream = source[at..].to_vec();
    codestream[42 + 3 * 3] = 0x87;
    let mut ihdr = 12u32.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&16u32.to_be_bytes());
    ihdr.extend_from_slice(&[0, 4, 255, 7, 0, 0]);
    let mut jp2h = bx(b"ihdr", &ihdr);
    jp2h.extend(bx(b"bpcc", &[7, 7, 7, 0x87]));
    jp2h.extend(bx(b"colr", &[1, 0, 0, 0, 0, 0, 16]));
    let mut bytes = b"\x00\x00\x00\x0cjP  \r\n\x87\n".to_vec();
    bytes.extend(bx(b"ftyp", b"jp2 \0\0\0\0jp2 "));
    bytes.extend(bx(b"jp2h", &jp2h));
    bytes.extend(bx(b"jp2c", &codestream));

    let coded = decode(&bytes).unwrap();
    let hashes: Vec<String> = coded.components.iter().map(plane_hash).collect();
    assert_eq!(hashes, PLANES, "the planes as coded are OpenJPEG's");
    let image = decode_with(&bytes, &srgb()).expect("sRGB output");
    assert_eq!(image.components, coded.components);
    let header = read_header_with(&bytes, &srgb()).expect("sRGB header");
    assert_eq!(header.components.len(), 4);
    assert_eq!(header.jp2.unwrap(), image.jp2.unwrap());
    let labels = &coded.jp2.unwrap().channels;
    assert_eq!(labels[3], UNSPECIFIED);
}
