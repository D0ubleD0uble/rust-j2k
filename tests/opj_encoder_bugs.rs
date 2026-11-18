//! Two `opj_compress` 2.5.0 encodes that `opj_decompress` 2.5.0 decodes, with
//! exit 0, to a wrong image. This decoder refuses both with
//! [`Error::Codestream`]. The oracle files (`*.opj.raw`) are OpenJPEG's wrong
//! output, kept so that a change to either behavior shows. See
//! `tests/fixtures/opj_encoder_bugs/README.md`.

#[path = "support/synth.rs"]
mod synth;
use std::path::PathBuf;

use rust_j2k::{DecodeOptions, Error, ReportEntry, decode, decode_with};

fn dir() -> PathBuf {
    synth::fixtures("opj_encoder_bugs")
}

fn read(name: &str) -> Vec<u8> {
    synth::read_file(dir().join(name))
}

/// The components' samples one after another, as 8-bit values.
fn flat(image: &rust_j2k::Image) -> Vec<u8> {
    image
        .components
        .iter()
        .flat_map(|c| c.samples.iter().map(|&s| u8::try_from(s).unwrap()))
        .collect()
}

fn count_differing(a: &[u8], b: &[u8]) -> usize {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).filter(|(x, y)| x != y).count()
}

/// `-TP R` with sub-sampling `1x2` and `2x1` and `-c [16,16]` writes four
/// tile-parts, the last two empty, and no packet of component 1.
/// `opj_decompress` returns component 1 as a constant plane: 509 of the 1024
/// samples are wrong. Here strict refuses; strict off returns OpenJPEG's
/// image and reports the missing packets.
#[test]
fn tp_with_mixed_sub_sampling_is_refused_and_partial_decode_matches_openjpeg() {
    let bytes = read("tp_mixed_subsampling.j2k");
    let oracle = read("tp_mixed_subsampling.opj.raw");
    let source = read("tp_mixed_subsampling.source.raw");
    assert_eq!(
        count_differing(&oracle, &source),
        509,
        "the oracle is wrong"
    );

    assert!(matches!(decode(&bytes), Err(Error::Codestream(_))));

    let image = decode_with(&bytes, &DecodeOptions::default().with_strict(false)).unwrap();
    assert!(flat(&image) == oracle, "strict off differs from OpenJPEG");
    assert!(!image.report.is_complete());
    assert!(matches!(
        image.report.entries.as_slice(),
        [ReportEntry::MissingPackets { tile: 0, .. }]
    ));
}

/// 272 tile-parts: `TPsot` and `TNsot` are 8 bits and wrap, so `TNsot` is 16.
/// `opj_decompress` warns "Non conformant codestream TPsot==TNsot" and returns
/// 4062 of the 4352 samples wrong. Here strict refuses at the 17th part; strict
/// off reads on to the 256th, where `TPsot` wraps to 0, and refuses there. It
/// does not return OpenJPEG's image.
#[test]
fn more_than_255_tile_parts_are_refused_in_both_modes() {
    let bytes = read("tp_over255.j2k");
    let oracle = read("tp_over255.opj.raw");
    let source = read("tp_over255.source.raw");
    assert_eq!(
        count_differing(&oracle, &source),
        4062,
        "the oracle is wrong"
    );

    assert!(matches!(decode(&bytes), Err(Error::Codestream(_))));
    assert!(matches!(
        decode_with(&bytes, &DecodeOptions::default().with_strict(false)),
        Err(Error::Codestream(_))
    ));
}
