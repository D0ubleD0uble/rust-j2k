//! Real-world JP2 files from GDAL's test data (`tests/fixtures/jp2_real/`,
//! provenance in its README): Kakadu, ECW SDK and OpenJPEG output with the box
//! layouts those encoders write.
//!
//! The oracle is the file's own codestream: `decode` of the `.jp2` must equal
//! `decode` of its first `jp2c` box's contents, extracted here by a separate,
//! deliberately simple box walk. A file whose header carries a box the decoder
//! does not apply yet (`pclr`, `cmap`, `cdef`) must instead be `Unsupported`
//! naming that box. A file whose codestream is itself broken is rejected for
//! that first, as the raw codestream is.

mod support;

use std::path::PathBuf;

use rust_j2k::{Error, decode};
use support::jp2::sha256_hex;

/// What a fixture must do.
enum Outcome {
    /// Decode equal to its extracted codestream.
    Decodes,
    /// Be refused as `Unsupported`, naming this box.
    Unsupported(&'static str),
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
        Outcome::Unsupported("cdef"),
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
        Outcome::Unsupported("cdef"),
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
        // it; the structural fault is reported before the `cdef` guard.
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
        Outcome::Unsupported("cdef"),
    ),
];

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
fn every_fixture_decodes_like_its_codestream_or_is_refused_by_the_guard() {
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
            Outcome::Unsupported(what) => match decode(&bytes) {
                Err(Error::Unsupported(message)) => assert!(
                    message.contains(&format!("`{what}`")),
                    "{name}: expected the `{what}` guard, got {message:?}"
                ),
                other => panic!("{name}: expected Unsupported({what}), got {other:?}"),
            },
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
    assert_eq!(decoded, 5, "five fixtures carry no guarded box");
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
