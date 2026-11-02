//! `ColourOutput::Srgb` through the public API: the sYCC fixtures against
//! `opj_decompress`'s converted output, files that are already sRGB or grey
//! coming out as coded, and the default staying as coded.
//!
//! The conversions themselves (the ICC files, the measured error against the
//! ISO references) are graded in `tests/conformance_jp2.rs`; the hand vectors
//! and the reject matrix are unit tests in `src/jp2/srgb_tests.rs`.

mod support;

use std::path::PathBuf;

use rust_j2k::{ColourOutput, ColourSpace, DecodeOptions, Error, decode, decode_with};
use support::{Expected, Tolerance, compare};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn conformance(n: u32) -> Vec<u8> {
    let path = fixtures().join(format!("conformance/jp2/files/file{n}.jp2"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn srgb() -> DecodeOptions {
    DecodeOptions::default().with_colour(ColourOutput::Srgb)
}

/// Each sYCC fixture converts to within 1 of `opj_decompress` (OpenJPEG's
/// formula truncates and uses 0.344 and 0.714; ours rounds with the IEC
/// constants), at full resolution, with the chroma replicated as OpenJPEG
/// does at an odd origin. The metadata then says sRGB.
#[test]
fn sycc_fixtures_match_openjpeg_within_one() {
    for name in ["jp2_sycc_420_odd", "jp2_sycc_422_odd"] {
        let bytes = std::fs::read(fixtures().join(format!("{name}.jp2"))).unwrap();
        let text = std::fs::read_to_string(fixtures().join(format!("{name}.srgb.json"))).unwrap();
        let want = Expected::from_json(&text).unwrap();
        assert_eq!(
            want.tolerance,
            Tolerance::Absolute { max_abs_error: 1.0 },
            "{name}"
        );
        let image = decode_with(&bytes, srgb()).unwrap_or_else(|e| panic!("{name}: {e}"));
        if let Err(mismatch) = compare(&image, &want) {
            panic!("{name}: {mismatch}");
        }
        // The conversion is not a no-op: as coded, the chroma is sub-sampled.
        let coded = decode(&bytes).unwrap();
        assert!(
            coded.components[1].width < image.components[1].width,
            "{name}"
        );
        let jp2 = image.jp2.as_ref().expect("JP2 metadata");
        assert_eq!(jp2.colour, ColourSpace::Srgb, "{name}: converted to sRGB");
        let declared = coded.jp2.as_ref().expect("JP2 metadata");
        assert_eq!(declared.colour, ColourSpace::Sycc, "{name}: as declared");
    }
}

/// sRGB and greyscale files, and an ICC-free default decode, come out as
/// coded: the conformance files 1, 4, 6 and 9 under `Srgb` equal their
/// default decode sample for sample.
#[test]
fn srgb_and_grey_files_pass_through() {
    for n in [1, 4, 6, 9] {
        let bytes = conformance(n);
        let coded = decode(&bytes).unwrap_or_else(|e| panic!("file{n}: {e}"));
        let image = decode_with(&bytes, srgb()).unwrap_or_else(|e| panic!("file{n}: {e}"));
        assert_eq!(image.components, coded.components, "file{n}");
    }
}

/// The default is as coded: the files that need conversion come out with
/// their sub-sampled chroma or profile-space samples unchanged, so the
/// default decode differs from the `Srgb` one.
#[test]
fn the_default_is_as_coded() {
    assert_eq!(DecodeOptions::default().colour, ColourOutput::AsCoded);
    for n in [2, 3, 5, 7, 8] {
        let bytes = conformance(n);
        let coded = decode(&bytes).unwrap_or_else(|e| panic!("file{n}: {e}"));
        let image = decode_with(&bytes, srgb()).unwrap_or_else(|e| panic!("file{n}: {e}"));
        assert_ne!(image.components, coded.components, "file{n}");
    }
}

/// `raw_channels` means no colour conversion, so combining it with sRGB
/// output is refused.
#[test]
fn raw_channels_with_srgb_is_invalid_options() {
    let options = srgb().with_raw_channels(true);
    assert!(matches!(
        decode_with(&conformance(1), options),
        Err(Error::InvalidOptions(_))
    ));
}

/// The builders set their own field and leave the others alone, and they
/// chain.
#[test]
fn the_builders_set_one_field_each() {
    let base = DecodeOptions::default();
    let raw = base.clone().with_raw_channels(true);
    assert!(raw.raw_channels);
    assert_eq!(raw.colour, ColourOutput::AsCoded);
    assert_eq!(raw.resolution_reduction, 0);
    assert!(!raw.clone().with_raw_channels(false).raw_channels);

    let both = base
        .with_colour(ColourOutput::Srgb)
        .with_resolution_reduction(2);
    assert_eq!(both.colour, ColourOutput::Srgb);
    assert_eq!(both.resolution_reduction, 2);
    assert!(!both.raw_channels);
}
