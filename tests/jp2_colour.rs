//! The colour space `decode` reports for the ISO/IEC 15444-4 JP2 reader files
//! (`tests/fixtures/conformance/jp2/files/file1.jp2` .. `file9.jp2`).
//!
//! The oracle is each file's own `colr` box: a separate, minimal box walk here
//! pulls out the first `colr` payload, and the ICC bytes `decode` returns must
//! equal it past the three header bytes (and hash to the recorded SHA-256).

mod support;

use std::path::PathBuf;

use rust_j2k::{ColourSpace, decode};
use support::jp2::sha256_hex;

enum Want {
    Colour(ColourSpace),
    /// The first `colr` is an ICC profile of this length and SHA-256.
    Icc(usize, &'static str),
}

const EXPECT: &[(u32, Want)] = &[
    (1, Want::Colour(ColourSpace::Srgb)),
    (2, Want::Colour(ColourSpace::Sycc)),
    (3, Want::Colour(ColourSpace::Sycc)),
    (4, Want::Colour(ColourSpace::Greyscale)),
    (
        5,
        Want::Icc(
            546,
            "d9f63489c08414e9a86acf88cac9b022144841ec1ea0e77d19c68f9d563d5581",
        ),
    ),
    (6, Want::Colour(ColourSpace::Greyscale)),
    (
        7,
        Want::Icc(
            13332,
            "356ce369ce2d641b1de3b484d360a9e6da63fc8cf658283e863a87b3343f2cff",
        ),
    ),
    (
        8,
        Want::Icc(
            414,
            "64ed586e213e2dd285927f0eb9cc994076b2777b9815589a532889d7703e05da",
        ),
    ),
    (9, Want::Colour(ColourSpace::Srgb)),
];

fn path(n: u32) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/conformance/jp2/files/file{n}.jp2"))
}

/// The payload of the first `colr` box inside the first `jp2h`. Test-side and
/// minimal: these files are known good, so any surprise is a panic.
fn first_colr(bytes: &[u8]) -> &[u8] {
    fn boxes(bytes: &[u8]) -> Vec<(&[u8], &[u8])> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let lbox = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind = &bytes[at + 4..at + 8];
            let (header, length) = match lbox {
                0 => (8, bytes.len() - at),
                1 => (
                    16,
                    u64::from_be_bytes(bytes[at + 8..at + 16].try_into().unwrap()) as usize,
                ),
                n => (8, n),
            };
            out.push((kind, &bytes[at + header..at + length]));
            at += length;
        }
        out
    }
    let top = boxes(bytes);
    let (_, jp2h) = top.iter().find(|(kind, _)| *kind == b"jp2h").unwrap();
    let (_, colr) = boxes(jp2h)
        .into_iter()
        .find(|(kind, _)| *kind == b"colr")
        .unwrap();
    colr
}

/// Kakadu writes `UnkC` 1 beside a valid `colr`; the `colr` value is still
/// reported. (Each file's `ihdr` is checked for `UnkC` 1 so this stays true to
/// its purpose.)
#[test]
fn kakadu_files_with_unkc_1_report_their_colr_value() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jp2_real");
    let files = [
        ("byte.jp2", ColourSpace::Greyscale),
        ("byte_tlm_plt.jp2", ColourSpace::Greyscale),
        ("int16.jp2", ColourSpace::Greyscale),
        ("rgbwcmyk01_YeGeo_kakadu.jp2", ColourSpace::Srgb),
        ("small_200ppcm.jp2", ColourSpace::Srgb),
    ];
    for (name, want) in files {
        let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let ihdr = bytes.windows(4).position(|w| w == b"ihdr").unwrap() + 4;
        assert_eq!(bytes[ihdr + 12], 1, "{name}: UnkC");
        let image = decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(image.jp2.expect("metadata").colour, want, "{name}");
    }
}

#[test]
fn decode_reports_the_colour_space_of_each_conformance_file() {
    for (n, want) in EXPECT {
        let bytes = std::fs::read(path(*n)).unwrap_or_else(|e| panic!("file{n}: {e}"));
        match want {
            Want::Colour(colour) => {
                let image = decode(&bytes).unwrap_or_else(|e| panic!("file{n}: {e}"));
                assert_eq!(&image.jp2.expect("metadata").colour, colour, "file{n}");
            }
            Want::Icc(len, sha256) => {
                let image = decode(&bytes).unwrap_or_else(|e| panic!("file{n}: {e}"));
                let ColourSpace::RestrictedIcc(profile) = image.jp2.expect("metadata").colour
                else {
                    panic!("file{n}: expected a restricted ICC profile");
                };
                let payload = first_colr(&bytes);
                assert_eq!(payload[0], 2, "file{n}: the first colr is METH 2");
                assert_eq!(profile.len(), *len, "file{n}: profile length");
                assert_eq!(profile.as_slice(), &payload[3..], "file{n}: box payload");
                assert_eq!(sha256_hex(&profile), *sha256, "file{n}: profile hash");
            }
        }
    }
}

/// File 2's `cdef` (components 0, 1, 2 are colours 3, 2, 1) puts its channels
/// in colour order and labels them so. Its sYCC samples are not converted, so
/// it still grades as *needs sRGB output* in `tests/conformance_jp2.rs`, after
/// its as-coded check.
#[test]
fn file2_comes_out_in_colour_order() {
    use rust_j2k::{Association, ChannelDefinition, ChannelType};
    let bytes = std::fs::read(path(2)).expect("file2");
    let at = bytes.windows(4).position(|w| w == b"cdef").unwrap() + 4;
    assert_eq!(
        &bytes[at..at + 20],
        &[0, 3, 0, 0, 0, 0, 0, 3, 0, 1, 0, 0, 0, 2, 0, 2, 0, 0, 0, 1],
        "the cdef box"
    );
    let image = decode(&bytes).expect("decodes");
    let jp2 = image.jp2.expect("metadata");
    assert_eq!(jp2.colour, ColourSpace::Sycc);
    let colours: Vec<ChannelDefinition> = (1..=3)
        .map(|n| ChannelDefinition {
            kind: ChannelType::Colour,
            association: Association::Colour(n),
        })
        .collect();
    assert_eq!(jp2.channels, colours);
    let jp2c = bytes.windows(4).position(|w| w == b"jp2c").unwrap() + 4;
    let raw = decode(&bytes[jp2c..]).expect("bare codestream");
    let reversed: Vec<_> = raw.components.into_iter().rev().collect();
    assert_eq!(image.components, reversed);
}
