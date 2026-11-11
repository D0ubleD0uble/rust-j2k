//! The `ftyp`, `ihdr`, `colr` and `rreq` fields on `Jp2Metadata`, through
//! `read_header` and `decode`.
//!
//! The file is the Part 4 `file5.jp2` (a `jpx `-branded file with `jp2 ` in
//! its compatibility list, an `rreq` box and two `colr` boxes), with its
//! `ihdr` flags patched here so they differ from the defaults. The expected
//! values are read off a box dump of the committed file. None of these fields
//! changes the samples.

use std::path::PathBuf;

use rust_j2k::{ColourSpace, Jp2Metadata, decode, read_header};

/// `file5.jp2` with `ihdr`'s `UnkC` and `IPR` set to these values.
fn file5_with(unknown_colourspace: u8, ipr: u8) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/conformance/jp2/files/file5.jp2");
    let mut bytes = std::fs::read(path).unwrap();
    // jP (12), ftyp (32), rreq (24), then jp2h's header and ihdr's: the
    // `ihdr` payload starts at 84, and `BPC`, `C`, `UnkC`, `IPR` end it.
    assert_eq!(&bytes[80..84], b"ihdr");
    assert_eq!(&bytes[94..98], &[7, 7, 0, 0]);
    bytes[96] = unknown_colourspace;
    bytes[97] = ipr;
    bytes
}

/// Check every field the box dump gives, and the `ihdr` flags set by
/// [`file5_with`].
#[track_caller]
fn check(jp2: &Jp2Metadata, unknown_colourspace: u8, ipr: u8) {
    assert_eq!(jp2.file_type.brand, *b"jpx ");
    assert_eq!(jp2.file_type.minor_version, 0);
    // The encoder wrote `00 00 00 03` after `MinV`; by I.5.2's layout it is
    // the first compatibility code, and it is reported as stored.
    assert_eq!(
        jp2.file_type.compatibility,
        [[0, 0, 0, 3], *b"jp2 ", *b"jpx ", *b"jpxb"]
    );

    let header = jp2.image_header;
    assert_eq!(
        (header.compression, header.unknown_colourspace, header.ipr),
        (7, unknown_colourspace, ipr)
    );

    // `rreq`: ML 1, FUAM A0, DCM C0, standard features 5, 61 and 43 with
    // masks 80, 60 and 40, no vendor features.
    let rreq = jp2.reader_requirements.as_ref().expect("an rreq");
    assert_eq!(rreq.mask_length, 1);
    assert_eq!(rreq.fully_understand, [0xA0]);
    assert_eq!(rreq.display_completely, [0xC0]);
    let features: Vec<_> = rreq
        .standard_features
        .iter()
        .map(|f| (f.feature, f.mask.clone()))
        .collect();
    assert_eq!(
        features,
        [(5, vec![0x80]), (61, vec![0x60]), (43, vec![0x40])]
    );
    assert!(rreq.vendor_features.is_empty());

    // The first `colr` is a 546-byte restricted ICC profile with `APPROX` 1,
    // and it is the one that sets the colour space. The second, enumerated
    // ROMM-RGB (21) with `PREC` 1 and `APPROX` 1, is reported and ignored.
    let [first, second] = &jp2.colour_specifications[..] else {
        panic!("{} colr boxes", jp2.colour_specifications.len());
    };
    assert_eq!(
        (first.method, first.precedence, first.approximation),
        (2, 0, 1)
    );
    assert_eq!(first.data.len(), 546);
    assert_eq!(jp2.colour, ColourSpace::RestrictedIcc(first.data.clone()));
    assert_eq!(
        (second.method, second.precedence, second.approximation),
        (1, 1, 1)
    );
    assert_eq!(second.data, [0, 0, 0, 21]);
}

#[test]
fn read_header_reports_the_ftyp_ihdr_colr_and_rreq_fields() {
    for (unknown_colourspace, ipr) in [(1, 0), (0, 1)] {
        let bytes = file5_with(unknown_colourspace, ipr);
        let jp2 = read_header(&bytes).unwrap().jp2.unwrap();
        check(&jp2, unknown_colourspace, ipr);
    }
}

#[test]
fn decode_reports_the_ftyp_ihdr_colr_and_rreq_fields() {
    for (unknown_colourspace, ipr) in [(1, 0), (0, 1)] {
        let bytes = file5_with(unknown_colourspace, ipr);
        let jp2 = decode(&bytes).unwrap().jp2.unwrap();
        check(&jp2, unknown_colourspace, ipr);
    }
}
