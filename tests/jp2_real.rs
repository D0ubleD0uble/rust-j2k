//! Real-world JP2 files from GDAL's test data (`tests/fixtures/jp2_real/`,
//! provenance in its README): Kakadu, ECW SDK and OpenJPEG output with the box
//! layouts those encoders write.
//!
//! For a file without channel definitions the oracle is its own codestream:
//! `decode` of the `.jp2` must equal `decode` of its first `jp2c` box's
//! contents, extracted here by a separate, deliberately simple box walk. A file
//! with a `cdef` box is checked against `opj_decompress` 2.5.0's decode of the
//! `.jp2`, committed as one PGX per channel under `references/`, and its
//! channel labels against its `cdef` box. A file whose codestream is itself
//! broken is rejected for that first, as the raw codestream is.

mod support;

use std::path::PathBuf;

use rust_j2k::{Association, ChannelDefinition, ChannelType, Error, decode};
use support::jp2::sha256_hex;
use support::pgx::parse_pgx;

/// What a fixture must do.
enum Outcome {
    /// Decode equal to its extracted codestream.
    Decodes,
    /// Decode to the committed OpenJPEG references
    /// `references/<stem>_<k>.pgx`, every sample within `max_abs` of them,
    /// with these channel labels (the file's `cdef` entries, in output order).
    Reference {
        channels: &'static [ChannelDefinition],
        max_abs: i32,
    },
    /// Be rejected with the same `Codestream` error as its extracted
    /// codestream, whose message holds this text.
    BrokenCodestream(&'static str),
}

/// Every vendored file, its SHA-256 (the README records the same), and its
/// outcome.
const FIXTURES: &[(&str, &str, Outcome)] = &[
    (
        "3_13bit_and_1bit.jp2",
        "f67d0b8fa68634659a74e0c7e1768348ed64b588e835bb8efbf6f8308acc72f7",
        // cdef (0,0,1) (1,0,2) (2,0,3) (3,1,0): 13-bit colour, 1-bit opacity.
        Outcome::Reference {
            channels: RGBA,
            max_abs: 0,
        },
    ),
    (
        "byte.jp2",
        "ef330c025133f062b4cca8c8171f8aef4f245be975c5e534dc9ce463689c3aa0",
        Outcome::Decodes,
    ),
    (
        "byte_tlm_plt.jp2",
        "c2bccd9839cd4cb24ac6bc84fd888d707bbf1d873af07616fa020e05ab491a49",
        Outcome::Decodes,
    ),
    (
        "byte_with_xmp.jp2",
        "cda36211b536322d28b96f17123ee2154e14826b69533e44ade7242906cc0b7d",
        // cdef (0,0,1): one grey channel.
        Outcome::Reference {
            channels: &[COLOUR_1],
            max_abs: 0,
        },
    ),
    (
        "int16.jp2",
        "62fef13b5f49de6302a86fe1f18663626c0b7e5f2470893d32136bc16af690e3",
        Outcome::Decodes,
    ),
    (
        "rgb16_ecwsdk.jp2",
        "86cf3c0a005b640e1cc9bd8dcf9f5955897b6576551383fc89f9d18adf32a507",
        // The codestream has no EOC. OpenJPEG 2.5.0 reads its header but fails
        // to decode the tile ("segment too long"), so neither decoder decodes
        // it. Its `cdef` (every type unspecified) is checked by
        // `the_ecw_unspecified_cdef_is_read` below.
        Outcome::BrokenCodestream("not terminated by EOC"),
    ),
    (
        "rgbwcmyk01_YeGeo_kakadu.jp2",
        "85a6382b972a5f5b39edf70282235663c3be596c430e4d2b208b5570611cbdfe",
        Outcome::Decodes,
    ),
    (
        "small_200ppcm.jp2",
        "27a1cc037485e8e575ee57cf0f85e66146d9bc88420d49afcd874c5db6839838",
        Outcome::Decodes,
    ),
    (
        "stefan_full_rgba.jp2",
        "53ecd23500a0f7921f8af6ef39b101a776503a797462ed10dcde77790b0916d5",
        // cdef (0,0,1) (1,0,2) (2,0,3) (3,1,0), written by opj_compress. The
        // codestream is 9/7: 10 of its 97200 samples differ from OpenJPEG's by
        // 1, from OpenJPEG's 9/7 scaling constant (docs/correctness.md).
        Outcome::Reference {
            channels: RGBA,
            max_abs: 1,
        },
    ),
];

const COLOUR_1: ChannelDefinition = colour(1);
const RGBA: &[ChannelDefinition] = &[
    colour(1),
    colour(2),
    colour(3),
    ChannelDefinition {
        kind: ChannelType::Opacity,
        association: Association::WholeImage,
    },
];

const fn colour(n: u16) -> ChannelDefinition {
    ChannelDefinition {
        kind: ChannelType::Colour,
        association: Association::Colour(n),
    }
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jp2_real")
}

/// The contents of the first top-level `jp2c` box. Test-side and minimal on
/// purpose: these files are known-good, so any surprise is a panic here.
fn first_jp2c(bytes: &[u8]) -> &[u8] {
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
        if kind == b"jp2c" {
            return &bytes[at + header..at + length];
        }
        at += length;
    }
    panic!("no jp2c box");
}

#[test]
fn every_fixture_decodes_like_its_oracle() {
    let mut decoded = 0;
    for (name, sha256, outcome) in FIXTURES {
        let bytes = std::fs::read(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(sha256_hex(&bytes), *sha256, "{name}: sha256");
        match outcome {
            Outcome::Decodes => {
                let file = decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
                let raw = decode(first_jp2c(&bytes))
                    .unwrap_or_else(|e| panic!("{name}: raw codestream: {e}"));
                assert_eq!((file.width, file.height), (raw.width, raw.height), "{name}");
                assert_eq!(file.components, raw.components, "{name}");
                assert!(file.jp2.is_some(), "{name}: JP2 metadata");
                assert!(raw.jp2.is_none(), "{name}: raw codestream metadata");
                decoded += 1;
            }
            Outcome::Reference { channels, max_abs } => {
                let file = decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
                let stem = name.trim_end_matches(".jp2");
                assert_eq!(file.components.len(), channels.len(), "{name}");
                for (k, got) in file.components.iter().enumerate() {
                    let path = dir().join(format!("references/{stem}_{k}.pgx"));
                    let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
                    let want = parse_pgx(&raw).unwrap_or_else(|e| panic!("{name}: {e}"));
                    assert_eq!(
                        (got.width, got.height, got.bit_depth, got.signed),
                        (want.width, want.height, want.bit_depth, want.signed),
                        "{name} channel {k}"
                    );
                    assert_eq!(got.samples.len(), want.samples.len(), "{name} channel {k}");
                    let pae = got
                        .samples
                        .iter()
                        .zip(&want.samples)
                        .map(|(a, b)| (a - b).abs())
                        .max()
                        .unwrap_or(0);
                    assert!(pae <= *max_abs, "{name} channel {k}: PAE {pae} > {max_abs}");
                }
                let jp2 = file.jp2.expect("JP2 metadata");
                assert_eq!(jp2.channels, *channels, "{name}");
                decoded += 1;
            }
            Outcome::BrokenCodestream(what) => {
                let file = decode(&bytes);
                let raw = decode(first_jp2c(&bytes));
                match &file {
                    Err(Error::Codestream(message)) => {
                        assert!(message.contains(what), "{name}: {message:?}")
                    }
                    other => panic!("{name}: expected Codestream({what}), got {other:?}"),
                }
                assert_eq!(file, raw, "{name}: the same error as the raw codestream");
            }
        }
    }
    assert_eq!(
        decoded, 8,
        "every fixture but the broken codestream decodes"
    );
}

/// The table above covers exactly the files in the directory.
#[test]
fn the_table_lists_every_vendored_file() {
    let mut on_disk: Vec<String> = std::fs::read_dir(dir())
        .expect("read jp2_real")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".jp2"))
        .collect();
    on_disk.sort();
    let listed: Vec<&str> = FIXTURES.iter().map(|(name, ..)| *name).collect();
    assert_eq!(on_disk, listed);
}

/// `rgb16_ecwsdk.jp2` does not decode (see above), but its `cdef` box is the
/// ECW SDK's all-unspecified layout: N = 3, each entry `Typ 65535`,
/// `Asoc 65535`. Read here so the fixture's claim stays true; the same layout
/// decodes in `jp2_cdef_unspecified` (`tests/jp2_synthetic.rs`).
#[test]
fn the_ecw_unspecified_cdef_is_read() {
    let bytes = std::fs::read(dir().join("rgb16_ecwsdk.jp2")).unwrap();
    let at = bytes.windows(4).position(|w| w == b"cdef").unwrap() + 4;
    let mut want = vec![0, 3];
    for cn in 0..3u8 {
        want.extend_from_slice(&[0, cn, 0xFF, 0xFF, 0xFF, 0xFF]);
    }
    assert_eq!(&bytes[at..at + 20], want.as_slice());
}

/// Every committed reference belongs to a `Reference` fixture.
#[test]
fn every_reference_is_used() {
    let mut on_disk: Vec<String> = std::fs::read_dir(dir().join("references"))
        .expect("read references")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    on_disk.sort();
    let mut listed: Vec<String> = FIXTURES
        .iter()
        .filter_map(|(name, _, outcome)| match outcome {
            Outcome::Reference { channels, .. } => {
                Some((name.trim_end_matches(".jp2"), channels.len()))
            }
            _ => None,
        })
        .flat_map(|(stem, n)| (0..n).map(move |k| format!("{stem}_{k}.pgx")))
        .collect();
    listed.sort();
    assert_eq!(on_disk, listed);
}
