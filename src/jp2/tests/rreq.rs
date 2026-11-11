//! The Reader Requirements box (`rreq`, ISO/IEC 15444-2 M.11.1). OpenJPEG
//! skips it, so the samples never change. A malformed one is left out, and
//! `opj_decompress` decodes such a file to the same samples
//! (`docs/correctness.md`).

use super::*;
use crate::{ReaderRequirements, StandardFeature, VendorFeature};

/// An `rreq` payload with masks of `ml` bytes: `FUAM` all `0xA0`, `DCM` all
/// `0xC0`, standard features 5 and 61, and one vendor feature.
fn payload(ml: u8) -> Vec<u8> {
    let mask = |byte: u8| vec![byte; usize::from(ml)];
    let mut out = vec![ml];
    out.extend(mask(0xA0));
    out.extend(mask(0xC0));
    out.extend_from_slice(&2u16.to_be_bytes());
    for (feature, byte) in [(5u16, 0x80), (61, 0x60)] {
        out.extend_from_slice(&feature.to_be_bytes());
        out.extend(mask(byte));
    }
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&[0x11; 16]);
    out.extend(mask(0x40));
    out
}

/// What [`payload`] holds.
fn expected(ml: u8) -> ReaderRequirements {
    let mask = |byte: u8| vec![byte; usize::from(ml)];
    ReaderRequirements {
        mask_length: ml,
        fully_understand: mask(0xA0),
        decode_completely: mask(0xC0),
        standard_features: vec![
            StandardFeature {
                feature: 5,
                mask: mask(0x80),
            },
            StandardFeature {
                feature: 61,
                mask: mask(0x60),
            },
        ],
        vendor_features: vec![VendorFeature {
            id: [0x11; 16],
            mask: mask(0x40),
        }],
    }
}

/// A minimal file with `rreq` boxes holding `payloads` right after `ftyp`.
fn with_rreq(payloads: &[&[u8]]) -> Vec<u8> {
    let mut boxes = vec![jp2_ftyp()];
    boxes.extend(payloads.iter().map(|p| bx(b"rreq", p)));
    boxes.extend([good_jp2h(), jp2c()]);
    file(&boxes)
}

/// The `rreq` that `decode` and `read_header` both report, after checking
/// the samples are the bare codestream's.
#[track_caller]
fn read(bytes: &[u8]) -> Option<ReaderRequirements> {
    assert_decodes_like_raw(bytes);
    let decoded = decode(bytes).unwrap().jp2.unwrap();
    assert_eq!(read_header(bytes).unwrap().jp2.unwrap(), decoded);
    decoded.reader_requirements
}

#[test]
fn a_well_formed_rreq_is_read_at_every_mask_length() {
    for ml in [1, 2, 4, 8, 3, 0] {
        assert_eq!(
            read(&with_rreq(&[&payload(ml)])),
            Some(expected(ml)),
            "ML {ml}"
        );
    }
}

#[test]
fn an_rreq_after_the_codestream_is_read() {
    let bytes = file(&[jp2_ftyp(), good_jp2h(), jp2c(), bx(b"rreq", &payload(2))]);
    assert_eq!(read(&bytes), Some(expected(2)));
}

/// The first `rreq` counts, even when it is malformed.
#[test]
fn only_the_first_rreq_counts() {
    assert_eq!(
        read(&with_rreq(&[&payload(1), &payload(2)])),
        Some(expected(1))
    );
    assert_eq!(read(&with_rreq(&[&[], &payload(2)])), None);
}

#[test]
fn an_empty_rreq_is_left_out() {
    assert_eq!(read(&with_rreq(&[&[]])), None);
}

/// Cut at every byte short of its end, the box is left out.
#[test]
fn a_truncated_rreq_is_left_out() {
    let whole = payload(2);
    for len in 0..whole.len() {
        assert_eq!(read(&with_rreq(&[&whole[..len]])), None, "{len} bytes");
    }
}

#[test]
fn an_rreq_with_bytes_left_over_is_left_out() {
    let mut long = payload(1);
    long.push(0);
    assert_eq!(read(&with_rreq(&[&long])), None);
}

/// `ML` 255 over a body sized for `ML` 1: nothing is allocated from `ML`.
#[test]
fn an_rreq_whose_mask_length_overruns_it_is_left_out() {
    let mut bad = payload(1);
    bad[0] = 255;
    assert_eq!(read(&with_rreq(&[&bad])), None);
}

/// `NSF` or `NVF` 65535 over a body holding two and one entries: the counts
/// size nothing.
#[test]
fn an_rreq_whose_feature_count_overruns_it_is_left_out() {
    // ML 1: ML, FUAM, DCM, then NSF at 3; two 3-byte standard features, then
    // NVF at 11.
    for at in [3, 11] {
        let mut bad = payload(1);
        assert_eq!(&bad[at..at + 2], &[0, if at == 3 { 2 } else { 1 }]);
        bad[at..at + 2].copy_from_slice(&u16::MAX.to_be_bytes());
        assert_eq!(read(&with_rreq(&[&bad])), None, "count at {at}");
    }
}
