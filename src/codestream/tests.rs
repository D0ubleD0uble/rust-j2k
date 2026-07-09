//! Unit tests for main-header parsing.
//!
//! Each header is assembled byte-for-byte from the Annex A field layout, so the
//! expected `MainHeader` is checked against the spec, not against our own
//! parser. On top of that,
//! [`siz_matches_opj_dump_across_the_conformance_corpus`] cross-checks the
//! parsed per-component geometry of all 23 conformance codestreams against the
//! `opj_dump` values recorded in the corpus manifest.

use super::markers::{Cod, Progression, Qcd, QuantStyle, Siz, SizComponent, Transform, marker};
use super::*;
use crate::Error;

fn be16(v: u16) -> [u8; 2] {
    v.to_be_bytes()
}

/// Wrap a body in `marker + Lmarker + body`, with `Lmarker` counting itself.
fn seg(m: u16, body: &[u8]) -> Vec<u8> {
    let mut s = Vec::new();
    s.extend_from_slice(&be16(m));
    s.extend_from_slice(&be16((body.len() + 2) as u16));
    s.extend_from_slice(body);
    s
}

/// SIZ body (everything after `Lsiz`): 512x256 single tile, `csiz` components.
fn siz_body(csiz: u16, comps: &[(u8, u8, u8)]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&be16(0)); // Rsiz
    b.extend_from_slice(&512u32.to_be_bytes()); // Xsiz
    b.extend_from_slice(&256u32.to_be_bytes()); // Ysiz
    b.extend_from_slice(&0u32.to_be_bytes()); // XOsiz
    b.extend_from_slice(&0u32.to_be_bytes()); // YOsiz
    b.extend_from_slice(&512u32.to_be_bytes()); // XTsiz
    b.extend_from_slice(&256u32.to_be_bytes()); // YTsiz
    b.extend_from_slice(&0u32.to_be_bytes()); // XTOsiz
    b.extend_from_slice(&0u32.to_be_bytes()); // YTOsiz
    b.extend_from_slice(&be16(csiz));
    for &(ssiz, xr, yr) in comps {
        b.extend_from_slice(&[ssiz, xr, yr]);
    }
    b
}

/// One unsigned 16-bit component (Ssiz = depth-1 = 15, sign bit clear).
fn one_component() -> Vec<u8> {
    siz_body(1, &[(15, 1, 1)])
}

#[allow(clippy::too_many_arguments)]
fn cod_body(
    scod: u8,
    prog: u8,
    layers: u16,
    mct: u8,
    nl: u8,
    xcb: u8,
    ycb: u8,
    style: u8,
    transform: u8,
) -> Vec<u8> {
    let mut b = vec![scod, prog];
    b.extend_from_slice(&be16(layers));
    b.push(mct);
    b.extend_from_slice(&[nl, xcb, ycb, style, transform]);
    b
}

/// Default valid COD: LRCP, single layer, 5 levels, 64x64 code-blocks.
fn cod_default(transform: u8) -> Vec<u8> {
    cod_body(0, 0, 1, 0, 5, 4, 4, 0, transform)
}

/// QCD body, no quantization (reversible): one exponent byte per subband.
fn qcd_none(guard: u8, exponents: &[u8]) -> Vec<u8> {
    let mut b = vec![guard << 5]; // style 0 (no quantization) in the low 5 bits
    for &e in exponents {
        b.push(e << 3);
    }
    b
}

/// QCD body, scalar expounded: a 16-bit (exponent, mantissa) per subband.
fn qcd_expounded(guard: u8, steps: &[(u8, u16)]) -> Vec<u8> {
    let mut b = vec![(guard << 5) | 2];
    for &(e, m) in steps {
        b.extend_from_slice(&be16((u16::from(e) << 11) | (m & 0x07FF)));
    }
    b
}

/// Assemble SOC + segments + a terminating SOT marker.
fn codestream(segments: &[Vec<u8>]) -> Vec<u8> {
    let mut h = Vec::new();
    h.extend_from_slice(&be16(marker::SOC));
    for s in segments {
        h.extend_from_slice(s);
    }
    h.extend_from_slice(&be16(marker::SOT));
    h
}

#[test]
fn valid_reversible_header_parses() {
    let exps = [8u8; 16]; // 3*5 + 1 subbands for 5 levels
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &exps)),
    ]);

    let (header, sot_offset) = parse_main_header(&bytes).expect("parse");

    assert_eq!(
        header,
        MainHeader {
            siz: Siz {
                x_size: 512,
                y_size: 256,
                x_offset: 0,
                y_offset: 0,
                tile_width: 512,
                tile_height: 256,
                tile_x_offset: 0,
                tile_y_offset: 0,
                components: vec![SizComponent {
                    bit_depth: 16,
                    signed: false,
                    x_sampling: 1,
                    y_sampling: 1,
                }],
            },
            cod: Cod {
                progression: Progression::Lrcp,
                layers: 1,
                decomposition_levels: 5,
                code_block_width: 4,
                code_block_height: 4,
                code_block_style: 0,
                transform: Transform::Reversible53,
                precinct_sizes: vec![],
            },
            qcd: Qcd {
                style: QuantStyle::None,
                guard_bits: 2,
                steps: vec![(8, 0); 16],
            },
        }
    );
    // The offset points at the terminating SOT marker.
    assert_eq!(sot_offset, bytes.len() - 2);
    assert_eq!(&bytes[sot_offset..sot_offset + 2], &be16(marker::SOT));
}

#[test]
fn valid_irreversible_header_parses() {
    let steps = [(10u8, 1234u16); 16];
    let bytes = codestream(&[
        seg(marker::SIZ, &siz_body(1, &[(0x80 | 11, 1, 1)])), // signed 12-bit
        seg(marker::COD, &cod_default(0)),
        seg(marker::QCD, &qcd_expounded(1, &steps)),
    ]);

    let (header, _) = parse_main_header(&bytes).expect("parse");

    assert_eq!(header.siz.components[0].bit_depth, 12);
    assert!(header.siz.components[0].signed);
    assert_eq!(header.cod.transform, Transform::Irreversible97);
    assert_eq!(header.qcd.style, QuantStyle::ScalarExpounded);
    assert_eq!(header.qcd.guard_bits, 1);
    assert_eq!(header.qcd.steps, vec![(10, 1234); 16]);
}

#[test]
fn derived_quant_keeps_single_step() {
    let mut body = vec![(2u8 << 5) | 1]; // guard 2, derived
    body.extend_from_slice(&be16((9 << 11) | 42)); // one (exp, mantissa)
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(0)),
        seg(marker::QCD, &body),
    ]);

    let (header, _) = parse_main_header(&bytes).expect("parse");
    assert_eq!(header.qcd.style, QuantStyle::ScalarDerived);
    assert_eq!(header.qcd.steps, vec![(9, 42)]);
}

#[test]
fn comment_segment_is_skipped() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COM, &[0, 1, b'h', b'i']),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(parse_main_header(&bytes).is_ok());
}

// --- reject matrix -------------------------------------------------------

fn err(bytes: &[u8]) -> Error {
    parse_main_header(bytes).expect_err("should reject")
}

#[test]
fn missing_soc_is_codestream() {
    // Starts with SIZ instead of SOC.
    let bytes = seg(marker::SIZ, &one_component());
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

#[test]
fn siz_not_first_is_codestream() {
    let mut bytes = be16(marker::SOC).to_vec();
    bytes.extend_from_slice(&seg(marker::COD, &cod_default(1)));
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

#[test]
fn truncated_segment_is_codestream() {
    let mut bytes = be16(marker::SOC).to_vec();
    // SIZ marker with a length field promising more than the buffer holds.
    bytes.extend_from_slice(&be16(marker::SIZ));
    bytes.extend_from_slice(&be16(100));
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

#[test]
fn trailing_bytes_in_siz_is_codestream() {
    let mut body = one_component();
    body.push(0xAB); // one byte the layout does not account for
    let bytes = codestream(&[
        seg(marker::SIZ, &body),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

#[test]
fn multiple_components_is_unsupported() {
    let body = siz_body(3, &[(15, 1, 1), (15, 1, 1), (15, 1, 1)]);
    let bytes = codestream(&[seg(marker::SIZ, &body)]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

#[test]
fn zero_components_is_marker() {
    let bytes = codestream(&[seg(marker::SIZ, &siz_body(0, &[]))]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

/// SIZ body with explicit geometry and one valid unsigned 15-bit component, so
/// the geometry-subset rejects (validated before the component list) can be
/// exercised one field at a time.
#[allow(clippy::too_many_arguments)]
fn siz_geom(x: u32, y: u32, xo: u32, yo: u32, xt: u32, yt: u32, xto: u32, yto: u32) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&be16(0)); // Rsiz
    for v in [x, y, xo, yo, xt, yt, xto, yto] {
        b.extend_from_slice(&v.to_be_bytes());
    }
    b.extend_from_slice(&be16(1)); // Csiz
    b.extend_from_slice(&[15, 1, 1]); // one unsigned 16-bit component
    b
}

#[test]
fn zero_size_image_is_marker() {
    let bytes = codestream(&[seg(marker::SIZ, &siz_geom(0, 256, 0, 0, 512, 256, 0, 0))]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

#[test]
fn nonzero_image_offset_is_unsupported() {
    let bytes = codestream(&[seg(marker::SIZ, &siz_geom(512, 256, 1, 0, 512, 256, 0, 0))]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

#[test]
fn nonzero_tile_offset_is_unsupported() {
    let bytes = codestream(&[seg(marker::SIZ, &siz_geom(512, 256, 0, 0, 512, 256, 1, 0))]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

#[test]
fn zero_size_tile_is_marker() {
    let bytes = codestream(&[seg(marker::SIZ, &siz_geom(512, 256, 0, 0, 0, 256, 0, 0))]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

#[test]
fn tile_smaller_than_image_is_unsupported() {
    // A 512×256 image carved into 256-wide tiles is a multi-tile grid (not yet decoded).
    let bytes = codestream(&[seg(marker::SIZ, &siz_geom(512, 256, 0, 0, 256, 256, 0, 0))]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

#[test]
fn oversize_image_area_is_unsupported() {
    // 16384×16384 = 2^28 samples, past the 2^26 decode guard.
    let n = 16384;
    let bytes = codestream(&[seg(marker::SIZ, &siz_geom(n, n, 0, 0, n, n, 0, 0))]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

#[test]
fn tile_larger_than_image_is_accepted() {
    // A tile larger than the image (XTsiz > Xsiz) is legal — the tile clips to
    // the image and the grid is still single-tile — so it must parse, pinning
    // the `<` boundary in the multi-tile check against a future `!=` tightening.
    let exps = [8u8; 16];
    let bytes = codestream(&[
        seg(marker::SIZ, &siz_geom(512, 256, 0, 0, 1024, 512, 0, 0)),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &exps)),
    ]);
    assert!(parse_main_header(&bytes).is_ok());
}

#[test]
fn non_lrcp_progression_is_unsupported() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_body(0, 2, 1, 0, 5, 4, 4, 0, 1)), // RPCL
    ]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

#[test]
fn reserved_progression_is_marker() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_body(0, 7, 1, 0, 5, 4, 4, 0, 1)),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

#[test]
fn multiple_layers_is_unsupported() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_body(0, 0, 2, 0, 5, 4, 4, 0, 1)),
    ]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

#[test]
fn explicit_precincts_is_unsupported() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_body(0x01, 0, 1, 0, 5, 4, 4, 0, 1)),
    ]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

#[test]
fn sop_eph_flag_is_unsupported() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_body(0x02, 0, 1, 0, 5, 4, 4, 0, 1)),
    ]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

#[test]
fn multi_component_transform_is_unsupported() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_body(0, 0, 1, 1, 5, 4, 4, 0, 1)), // mct = 1
    ]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

#[test]
fn reserved_transform_is_marker() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_body(0, 0, 1, 0, 5, 4, 4, 0, 5)),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

#[test]
fn out_of_subset_marker_is_unsupported() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::COC, &[0, 0, 0]), // component coding override
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

#[test]
fn missing_cod_is_codestream() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

#[test]
fn missing_qcd_is_codestream() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
    ]);
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

#[test]
fn duplicate_cod_is_codestream() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

#[test]
fn reserved_quant_style_is_marker() {
    let mut body = vec![(2u8 << 5) | 3]; // style 3 is reserved
    body.extend_from_slice(&be16(0));
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(0)),
        seg(marker::QCD, &body),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

#[test]
fn eoc_before_tile_part_is_codestream() {
    let mut bytes = be16(marker::SOC).to_vec();
    bytes.extend_from_slice(&seg(marker::SIZ, &one_component()));
    bytes.extend_from_slice(&seg(marker::COD, &cod_default(1)));
    bytes.extend_from_slice(&seg(marker::QCD, &qcd_none(2, &[8; 16])));
    bytes.extend_from_slice(&be16(marker::EOC));
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

#[test]
fn unknown_marker_without_a_length_is_codestream() {
    // Stepping over an unknown marker needs its length to be there. At the very
    // end of the header there is nothing to read, which is truncation — a
    // malformed codestream, not merely an unsupported one.
    let mut bytes = be16(marker::SOC).to_vec();
    bytes.extend_from_slice(&seg(marker::SIZ, &one_component()));
    bytes.extend_from_slice(&be16(0xFF01)); // not a marker we know
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

// --- tile-part walk (issue #6) -------------------------------------------
//
// As with the main header, each stream is assembled from the Annex A field
// layout so the expectation is checked against the spec, not the parser. The
// `opj_dump` cross-check on real seed codestreams lands with the fixture
// corpus (#4); these synthetic cases pin the offset/length and reject logic.

/// SOT segment: `marker + Lsot(=10) + Isot + Psot + TPsot + TNsot`.
fn sot_seg(isot: u16, psot: u32, tpsot: u8, tnsot: u8) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&be16(isot));
    body.extend_from_slice(&psot.to_be_bytes());
    body.push(tpsot);
    body.push(tnsot);
    seg(marker::SOT, &body)
}

/// A complete, in-subset main header: SIZ + reversible COD + no-quant QCD.
fn default_header() -> Vec<Vec<u8>> {
    vec![
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]
}

/// SOC + `header` + `sot` + SOD + `data`, optionally closed by EOC.
fn assemble(header: &[Vec<u8>], sot: &[u8], data: &[u8], eoc: bool) -> Vec<u8> {
    let mut s = be16(marker::SOC).to_vec();
    for part in header {
        s.extend_from_slice(part);
    }
    s.extend_from_slice(sot);
    s.extend_from_slice(&be16(marker::SOD));
    s.extend_from_slice(data);
    if eoc {
        s.extend_from_slice(&be16(marker::EOC));
    }
    s
}

/// `Psot` spanning one tile-part: the 12-byte SOT segment, the 2-byte SOD
/// marker, plus the packet data.
fn psot_for(data: &[u8]) -> u32 {
    (12 + 2 + data.len()) as u32
}

fn perr(bytes: &[u8]) -> Error {
    parse(bytes).expect_err("should reject")
}

#[test]
fn valid_single_tile_part_parses() {
    let data = [0xDE, 0xAD, 0xBE, 0xEF];
    let sot = sot_seg(0, psot_for(&data), 0, 1);
    let bytes = assemble(&default_header(), &sot, &data, true);

    let cs = parse(&bytes).expect("parse");
    assert_eq!(cs.tile_parts.len(), 1);
    assert_eq!(cs.tile_parts[0].tile_index, 0);
    assert_eq!(cs.tile_parts[0].data, &data);
    assert_eq!(cs.header.cod.transform, Transform::Reversible53);
}

#[test]
fn psot_zero_runs_to_eoc() {
    let data = [1, 2, 3, 4, 5];
    let sot = sot_seg(0, 0, 0, 1); // last tile-part: extends to EOC
    let bytes = assemble(&default_header(), &sot, &data, true);

    let cs = parse(&bytes).expect("parse");
    assert_eq!(cs.tile_parts[0].data, &data);
}

#[test]
fn empty_tile_part_data_parses() {
    let sot = sot_seg(0, psot_for(&[]), 0, 1);
    let bytes = assemble(&default_header(), &sot, &[], true);

    let cs = parse(&bytes).expect("parse");
    assert!(cs.tile_parts[0].data.is_empty());
}

#[test]
fn tile_part_header_comment_is_skipped() {
    let data = [9, 9];
    let com = seg(marker::COM, &[0, 1, b'x']);
    let psot = (12 + com.len() + 2 + data.len()) as u32;

    let mut bytes = be16(marker::SOC).to_vec();
    for part in default_header() {
        bytes.extend_from_slice(&part);
    }
    bytes.extend_from_slice(&sot_seg(0, psot, 0, 1));
    bytes.extend_from_slice(&com);
    bytes.extend_from_slice(&be16(marker::SOD));
    bytes.extend_from_slice(&data);
    bytes.extend_from_slice(&be16(marker::EOC));

    let cs = parse(&bytes).expect("parse");
    assert_eq!(cs.tile_parts[0].data, &data);
}

#[test]
fn nonzero_tile_index_is_unsupported() {
    let sot = sot_seg(1, 0, 0, 1); // Isot != 0
    let bytes = assemble(&default_header(), &sot, &[1, 2], true);
    assert!(matches!(perr(&bytes), Error::Unsupported(_)));
}

#[test]
fn multiple_tile_parts_count_is_unsupported() {
    let data = [1, 2];
    let sot = sot_seg(0, psot_for(&data), 0, 2); // TNsot = 2
    let bytes = assemble(&default_header(), &sot, &data, true);
    assert!(matches!(perr(&bytes), Error::Unsupported(_)));
}

#[test]
fn nonzero_tile_part_index_is_unsupported() {
    let sot = sot_seg(0, 0, 1, 1); // TPsot = 1
    let bytes = assemble(&default_header(), &sot, &[1, 2], true);
    assert!(matches!(perr(&bytes), Error::Unsupported(_)));
}

#[test]
fn second_tile_part_is_unsupported() {
    // First part states TNsot=0 ("not stated") so it passes the field checks;
    // a second SOT where EOC was expected is what trips the reject.
    let data = [7, 7, 7];
    let first = sot_seg(0, psot_for(&data), 0, 0);

    let mut bytes = be16(marker::SOC).to_vec();
    for part in default_header() {
        bytes.extend_from_slice(&part);
    }
    bytes.extend_from_slice(&first);
    bytes.extend_from_slice(&be16(marker::SOD));
    bytes.extend_from_slice(&data);
    bytes.extend_from_slice(&sot_seg(0, 0, 1, 2)); // a second tile-part
    bytes.extend_from_slice(&be16(marker::SOD));
    bytes.extend_from_slice(&[8, 8]);
    bytes.extend_from_slice(&be16(marker::EOC));

    assert!(matches!(perr(&bytes), Error::Unsupported(_)));
}

#[test]
fn out_of_subset_tile_header_marker_is_unsupported() {
    // A QCC override in the tile-part header is a later phase.
    let data = [1, 2];
    let qcc = seg(marker::QCC, &[0, 0, 0]);
    let psot = (12 + qcc.len() + 2 + data.len()) as u32;

    let mut bytes = be16(marker::SOC).to_vec();
    for part in default_header() {
        bytes.extend_from_slice(&part);
    }
    bytes.extend_from_slice(&sot_seg(0, psot, 0, 1));
    bytes.extend_from_slice(&qcc);
    bytes.extend_from_slice(&be16(marker::SOD));
    bytes.extend_from_slice(&data);
    bytes.extend_from_slice(&be16(marker::EOC));

    assert!(matches!(perr(&bytes), Error::Unsupported(_)));
}

#[test]
fn unexpected_tile_header_marker_is_codestream() {
    let mut bytes = be16(marker::SOC).to_vec();
    for part in default_header() {
        bytes.extend_from_slice(&part);
    }
    bytes.extend_from_slice(&sot_seg(0, 0, 0, 1));
    bytes.extend_from_slice(&be16(0xFF30)); // reserved, not valid in a header
    assert!(matches!(perr(&bytes), Error::Codestream(_)));
}

#[test]
fn psot_overrun_is_codestream() {
    let data = [1, 2];
    let sot = sot_seg(0, 9999, 0, 1); // Psot far past the buffer
    let bytes = assemble(&default_header(), &sot, &data, true);
    assert!(matches!(perr(&bytes), Error::Codestream(_)));
}

#[test]
fn missing_eoc_is_codestream() {
    let data = [1, 2];
    let sot = sot_seg(0, psot_for(&data), 0, 1);
    let bytes = assemble(&default_header(), &sot, &data, false); // no EOC
    assert!(matches!(perr(&bytes), Error::Codestream(_)));
}

#[test]
fn psot_zero_without_eoc_is_codestream() {
    let data = [1, 2, 3];
    let sot = sot_seg(0, 0, 0, 1);
    let bytes = assemble(&default_header(), &sot, &data, false);
    assert!(matches!(perr(&bytes), Error::Codestream(_)));
}

#[test]
fn truncated_sot_is_codestream() {
    let mut bytes = be16(marker::SOC).to_vec();
    for part in default_header() {
        bytes.extend_from_slice(&part);
    }
    bytes.extend_from_slice(&be16(marker::SOT));
    bytes.extend_from_slice(&be16(10)); // Lsot promises 8 body bytes
    bytes.extend_from_slice(&[0, 0]); // only 2 are present
    assert!(matches!(perr(&bytes), Error::Codestream(_)));
}

// --- per-component SIZ geometry (issue #56) -------------------------------

/// A valid header carrying `comps` components, for exercising `decode_siz`
/// directly: `parse_main_header` rejects anything but one component.
fn siz_of(csiz: u16, comps: &[(u8, u8, u8)]) -> Siz {
    let body = siz_body(csiz, comps);
    decode_siz(Cursor::new(&body)).expect("SIZ parses")
}

#[test]
fn multi_component_siz_parses_every_component() {
    // 3 components: 8-bit unsigned, 12-bit signed, 1-bit unsigned.
    let siz = siz_of(3, &[(7, 1, 1), (0x80 | 11, 1, 1), (0, 1, 1)]);
    assert_eq!(
        siz.components,
        vec![
            SizComponent {
                bit_depth: 8,
                signed: false,
                x_sampling: 1,
                y_sampling: 1
            },
            SizComponent {
                bit_depth: 12,
                signed: true,
                x_sampling: 1,
                y_sampling: 1
            },
            SizComponent {
                bit_depth: 1,
                signed: false,
                x_sampling: 1,
                y_sampling: 1
            },
        ]
    );
    // Unit sub-sampling: every component covers the whole image area.
    assert_eq!(siz.image_extent(), (512, 256));
    for i in 0..3 {
        assert_eq!(siz.component_extent(i), Some((512, 256)));
    }
    assert_eq!(siz.component_extent(3), None);
}

#[test]
fn subsampled_siz_derives_each_component_extent() {
    // Image is 512x256 at the origin; the four classic sub-sampling factors.
    let siz = siz_of(4, &[(7, 1, 1), (7, 2, 1), (7, 1, 2), (7, 2, 2)]);
    assert_eq!(siz.image_extent(), (512, 256));
    assert_eq!(siz.component_extent(0), Some((512, 256)));
    assert_eq!(siz.component_extent(1), Some((256, 256)));
    assert_eq!(siz.component_extent(2), Some((512, 128)));
    assert_eq!(siz.component_extent(3), Some((256, 128)));
}

#[test]
fn component_extent_ceils_each_edge_separately() {
    // The spec subtracts two ceilings; it does not ceil the difference. With
    // Xsiz=9, XOsiz=1 and XRsiz=2 the two forms disagree:
    //   ceil(9/2) - ceil(1/2) = 5 - 1 = 4   (the standard)
    //   ceil((9 - 1)/2)       = 4           (agrees here)
    // but at XOsiz=3 they diverge:
    //   ceil(9/2) - ceil(3/2) = 5 - 2 = 3   (the standard)
    //   ceil((9 - 3)/2)       = 3           (agrees)
    // and at Xsiz=8, XOsiz=1:
    //   ceil(8/2) - ceil(1/2) = 4 - 1 = 3   (the standard)
    //   ceil((8 - 1)/2)       = 4           (wrong)
    let mut siz = siz_of(1, &[(7, 2, 2)]);
    siz.x_size = 8;
    siz.y_size = 8;
    siz.x_offset = 1;
    siz.y_offset = 1;
    assert_eq!(siz.component_extent(0), Some((3, 3)));
    assert_eq!(siz.image_extent(), (7, 7));
}

#[test]
fn too_many_components_is_marker() {
    // Csiz above the Table A-9 limit of 16384, without the body to match: the
    // count is rejected before any allocation is sized from it.
    let body = siz_body(markers::MAX_COMPONENTS + 1, &[]);
    assert!(matches!(
        decode_siz(Cursor::new(&body)),
        Err(Error::Marker(_))
    ));
}

#[test]
fn component_record_count_must_match_csiz() {
    // Csiz claims 2 components but only one 3-byte record follows.
    let body = siz_body(2, &[(7, 1, 1)]);
    assert!(matches!(
        decode_siz(Cursor::new(&body)),
        Err(Error::Codestream(_))
    ));
}

#[test]
fn zero_sub_sampling_factor_is_marker() {
    for comp in [(7, 0, 1), (7, 1, 0)] {
        let body = siz_body(1, &[comp]);
        assert!(
            matches!(decode_siz(Cursor::new(&body)), Err(Error::Marker(_))),
            "{comp:?} should reject"
        );
    }
}

#[test]
fn bit_depth_above_38_is_marker() {
    // Ssiz low 7 bits are depth-1: 37 -> depth 38 (the Table A-11 limit), 38 -> 39.
    assert_eq!(siz_of(1, &[(37, 1, 1)]).components[0].bit_depth, 38);
    let body = siz_body(1, &[(38, 1, 1)]);
    assert!(matches!(
        decode_siz(Cursor::new(&body)),
        Err(Error::Marker(_))
    ));
    // The sign bit must not be mistaken for depth.
    assert_eq!(siz_of(1, &[(0x80 | 37, 1, 1)]).components[0].bit_depth, 38);
    assert!(siz_of(1, &[(0x80 | 37, 1, 1)]).components[0].signed);
}

#[test]
fn multi_component_codestream_reports_the_component_count() {
    // SIZ itself accepts it; the subset check rejects it afterwards, naming the
    // component count rather than whatever COD trips on first.
    let bytes = codestream(&[
        seg(marker::SIZ, &siz_body(3, &[(7, 1, 1); 3])),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    let e = err(&bytes);
    assert!(
        matches!(&e, Error::Unsupported(m) if m.contains("3 components")),
        "got {e:?}"
    );
}

// --- main-header marker walk (issue #56) ----------------------------------

/// A valid single-component header with `extra` spliced in before the SOT.
fn header_with(extra: &[u8]) -> Vec<u8> {
    let mut bytes = be16(marker::SOC).to_vec();
    bytes.extend_from_slice(&seg(marker::SIZ, &one_component()));
    bytes.extend_from_slice(&seg(marker::COD, &cod_default(1)));
    bytes.extend_from_slice(&seg(marker::QCD, &qcd_none(2, &[8; 16])));
    bytes.extend_from_slice(extra);
    bytes.extend_from_slice(&be16(marker::SOT));
    bytes
}

#[test]
fn reserved_markers_carry_no_segment() {
    // 0xFF30..=0xFF3F stand alone. A walker that reads a length after one of
    // them swallows the SOT that follows. p0_02 has 0xFF30 in its main header.
    for code in [0xFF30u16, 0xFF37, 0xFF3F] {
        let bytes = header_with(&be16(code));
        parse_main_header(&bytes).unwrap_or_else(|e| panic!("{code:#06X} should walk: {e:?}"));
    }
    // Several in a row, and adjacent to other segments.
    let mut extra = be16(0xFF30).to_vec();
    extra.extend_from_slice(&be16(0xFF3F));
    extra.extend_from_slice(&seg(marker::COM, b"note"));
    extra.extend_from_slice(&be16(0xFF31));
    assert!(parse_main_header(&header_with(&extra)).is_ok());
}

/// An unknown marker is *walked* by its length — that is what lets the header be
/// traversed — but the decoder will not decode past it. Every marker code is
/// allocated by some part of the standard, and an unknown one may change what
/// the packet data means, so silently ignoring it would risk a wrong image.
#[test]
fn unknown_marker_segments_are_walked_then_reported_unsupported() {
    for code in [
        0xFF01u16, // no part of the standard we implement
        0xFF2F,    // just below the reserved segment-less range
        0xFF40,    // just above it
        0xFF74,    // Part 2 MCT
        0xFF78,    // Part 2 CBD
    ] {
        let bytes = header_with(&seg(code, &[1, 2, 3, 4]));

        // The walk locates it and keeps going: the SOT after it is still found.
        let (segments, _) =
            walk_main_header(&bytes).unwrap_or_else(|e| panic!("{code:#06X}: {e:?}"));
        assert!(
            segments.iter().any(|s| s.code == code),
            "{code:#06X} located"
        );

        // Interpreting the header then refuses to guess what it meant.
        let e = err(&bytes);
        assert!(
            matches!(&e, Error::Unsupported(m) if m.contains("unrecognized")),
            "{code:#06X}: got {e:?}",
        );
    }
}

#[test]
fn a_non_marker_where_a_marker_belongs_is_codestream() {
    // Every marker's high byte is 0xFF; anything else means we have lost sync.
    let bytes = header_with(&[0x12, 0x34]);
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

#[test]
fn header_altering_markers_are_rejected_not_skipped() {
    // These are known markers outside the subset. Each changes how the
    // codestream is interpreted (PPM/PPT relocate the packet headers), so each
    // is named and rejected rather than passed over.
    for code in [
        marker::CAP,
        marker::COC,
        marker::QCC,
        marker::RGN,
        marker::POC,
        marker::TLM,
        marker::PLM,
        marker::PLT,
        marker::PPM,
        marker::PPT,
        marker::CRG,
        marker::SOP,
    ] {
        let bytes = header_with(&seg(code, &[0, 0]));
        assert!(
            matches!(err(&bytes), Error::Unsupported(_)),
            "{code:#06X} should be Unsupported"
        );
    }
    // EPH carries no segment, so it is spliced in bare.
    let bytes = header_with(&be16(marker::EPH));
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

#[test]
fn duplicate_siz_is_codestream() {
    let bytes = header_with(&seg(marker::SIZ, &one_component()));
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

#[test]
fn a_second_soc_in_the_main_header_is_codestream() {
    let bytes = header_with(&be16(marker::SOC));
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

/// A segment-less marker costs two input bytes but one located-segment record,
/// so a run of them must not grow the list without bound.
#[test]
fn too_many_marker_segments_is_codestream() {
    let mut extra = Vec::new();
    for _ in 0..=MAX_MAIN_HEADER_SEGMENTS {
        extra.extend_from_slice(&be16(0xFF30));
    }
    let bytes = header_with(&extra);
    assert!(matches!(err(&bytes), Error::Codestream(_)));

    // One under the cap still walks (the three real segments plus the run).
    let mut extra = Vec::new();
    for _ in 0..MAX_MAIN_HEADER_SEGMENTS - 4 {
        extra.extend_from_slice(&be16(0xFF30));
    }
    assert!(parse_main_header(&header_with(&extra)).is_ok());
}

// --- corpus cross-check against the opj_dump oracle (issue #56) ------------

fn corpus_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance")
}

/// Read a `.pgx` reference image's declared dimensions from its header line.
///
/// The full reader lives in `tests/conformance_part4.rs`; here we only need the
/// trailing `<width> <height>`, which is the last two whitespace-separated
/// tokens of the first line whatever the sign is spelled like.
fn pgx_extent(path: &std::path::Path) -> (u32, u32) {
    let bytes = std::fs::read(path).expect("read reference");
    let end = bytes.iter().position(|&b| b == b'\n').expect("header line");
    let header = std::str::from_utf8(&bytes[..end]).expect("ASCII header");
    let tokens: Vec<&str> = header.split_ascii_whitespace().collect();
    let n = tokens.len();
    (
        tokens[n - 2].parse().expect("width"),
        tokens[n - 1].parse().expect("height"),
    )
}

/// Every conformance main header walks, and the per-component geometry SIZ
/// yields agrees with `opj_dump` — which is what `manifest.json`'s `features`
/// block records (see its `provenance.features`).
///
/// This is the oracle for issue #56: depth, sign, and sub-sampling for all
/// `Csiz` components of 23 real codestreams, including `p0_13`'s 257 components
/// and the four sub-sampling shapes in `p0_06`. The walk covers markers the
/// decoder rejects (COC, PPM, POC, …) and `p0_02`'s reserved `0xFF30`, because
/// walking is subset-agnostic.
///
/// Returns early if the corpus is absent: it is `exclude`d from the packaged
/// crate, so a `cargo test` unpacked from crates.io has no fixtures.
#[test]
fn siz_matches_opj_dump_across_the_conformance_corpus() {
    let dir = corpus_dir();
    let Ok(text) = std::fs::read_to_string(dir.join("manifest.json")) else {
        return;
    };
    let manifest: serde_json::Value = serde_json::from_str(&text).expect("manifest parses");

    for entry in manifest["entries"].as_array().expect("entries") {
        let name = entry["codestream"].as_str().expect("codestream path");
        let bytes = std::fs::read(dir.join(name)).expect("read codestream");

        let (segments, _) = walk_main_header(&bytes).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(segments[0].code, marker::SIZ, "{name}: SIZ must be first");
        let siz = decode_siz(Cursor::new(segments[0].body))
            .unwrap_or_else(|e| panic!("{name}: decode_siz: {e:?}"));

        let features = &entry["features"];
        // `features.width`/`height` are `Xsiz`/`Ysiz`, the reference grid, not
        // the image area — p1_01 declares 127x227 with a (5, 128) origin.
        assert_eq!(
            siz.x_size as u64,
            features["width"].as_u64().unwrap(),
            "{name}: Xsiz"
        );
        assert_eq!(
            siz.y_size as u64,
            features["height"].as_u64().unwrap(),
            "{name}: Ysiz"
        );
        assert_eq!(
            siz.components.len() as u64,
            features["components"].as_u64().unwrap(),
            "{name}: Csiz",
        );

        for (i, comp) in siz.components.iter().enumerate() {
            assert_eq!(
                comp.bit_depth as u64,
                features["precision"][i].as_u64().unwrap(),
                "{name}: component {i} depth",
            );
            assert_eq!(
                comp.signed,
                features["signed"][i].as_bool().unwrap(),
                "{name}: component {i} sign",
            );
            assert_eq!(
                [comp.x_sampling as u64, comp.y_sampling as u64],
                [
                    features["subsampling"][i][0].as_u64().unwrap(),
                    features["subsampling"][i][1].as_u64().unwrap(),
                ],
                "{name}: component {i} sub-sampling",
            );
        }

        // The class-1 references are one `.pgx` per graded component, decoded at
        // full resolution — so their dimensions are the oracle for
        // `component_extent`. The sole exception is p0_08, which OpenJPEG's
        // conformance suite decodes at resolution reduction 1
        // (`C1P0_ResFactor_list` in its tests/conformance/CMakeLists.txt), so
        // its references are half-size in each axis. Tracked separately; here
        // we assert the reduction rather than skip it, so a corpus refresh that
        // changes it fails loudly.
        let reduced = name.contains("p0_08");
        for (i, reference) in entry["references"]["class1"]
            .as_array()
            .expect("class1 refs")
            .iter()
            .enumerate()
        {
            let (rw, rh) = pgx_extent(&dir.join(reference.as_str().unwrap()));
            let (cw, ch) = siz.component_extent(i).expect("graded component exists");
            let (want_w, want_h) = if reduced {
                (cw.div_ceil(2), ch.div_ceil(2))
            } else {
                (cw, ch)
            };
            assert_eq!(
                (want_w, want_h),
                (rw, rh),
                "{name}: component {i} extent disagrees with its reference",
            );
        }
    }
}

/// `p0_02` carries the reserved segment-less marker `0xFF30` in its main header,
/// after `COM` and before `SOT`. The walk runs to completion before any segment
/// is interpreted, so the whole header — `0xFF30` included — is traversed on the
/// way to the `Unsupported` verdict its `COD` earns.
///
/// A walker that read a length after `0xFF30` would consume the `SOT` and report
/// `Codestream("truncated marker segment")` instead. That is the bug this
/// codestream exists to catch, so assert the *reason*, not merely that it failed.
#[test]
fn p0_02_reserved_marker_walks_cleanly() {
    let path = corpus_dir().join("codestreams/p0_02.j2k");
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };

    let (segments, _) = walk_main_header(&bytes).expect("main header walks");
    assert!(
        segments.iter().any(|s| s.code == 0xFF30),
        "p0_02 should carry the reserved 0xFF30 marker",
    );
    assert!(
        segments
            .iter()
            .find(|s| s.code == 0xFF30)
            .unwrap()
            .body
            .is_empty(),
        "0xFF30 carries no segment",
    );

    let e = parse(&bytes).expect_err("p0_02 is outside the decoded subset");
    assert!(
        matches!(&e, Error::Unsupported(m) if m.contains("SOP/EPH")),
        "expected the COD subset rejection, got {e:?}",
    );
}

// --- the reject matrix (issue #78) ----------------------------------------

/// Which typed error a caller sees, per the mapping the crate commits to:
/// `Codestream` for structural damage (truncation, lost sync, a missing
/// required marker), `Marker` for a field encoded illegally, and `Unsupported`
/// for valid JPEG 2000 that falls outside the decoded subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variant {
    Codestream,
    Marker,
    Unsupported,
}

fn variant_of(e: &Error) -> Variant {
    match e {
        Error::Codestream(_) => Variant::Codestream,
        Error::Marker(_) => Variant::Marker,
        Error::Unsupported(_) => Variant::Unsupported,
        other => panic!("main-header parsing should not raise {other:?}"),
    }
}

/// A valid single-component header with a COD whose fields are overridden.
fn header_with_cod(cod: Vec<u8>) -> Vec<u8> {
    codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ])
}

/// Every input outside the decoded subset, mapped to the typed error a caller
/// sees. This is the contract: valid-but-not-yet-decoded is `Unsupported`, an
/// illegal field is `Marker`, and structural damage is `Codestream`. Nothing
/// here may be silently accepted — the alternative to a clean rejection is not
/// a slightly wrong image, it is an arbitrary one.
///
/// As each Phase 2 milestone lands, its row moves out of this table and into
/// the decoded set. A row that stops rejecting fails here first.
#[test]
fn reject_matrix_maps_every_out_of_subset_input_to_its_typed_error() {
    let jp2 = {
        let mut bytes = JP2_SIGNATURE.to_vec();
        bytes.extend_from_slice(b"....ftypjp2 ");
        bytes
    };

    // (what the input carries, the error it produced, the variant it must be)
    let mut rows: Vec<(&str, Error, Variant)> = vec![
        (
            "JP2 file format wrapper",
            parse(&jp2).expect_err("JP2 is not a bare codestream"),
            Variant::Unsupported,
        ),
        (
            "HTJ2K capabilities (CAP)",
            err(&header_with(&seg(marker::CAP, &[0, 0]))),
            Variant::Unsupported,
        ),
        (
            "Part 2 multiple-component transform (MCT)",
            err(&header_with(&seg(0xFF74, &[0, 0]))),
            Variant::Unsupported,
        ),
        (
            "Part 2 component bit depth (CBD)",
            err(&header_with(&seg(0xFF78, &[0, 0]))),
            Variant::Unsupported,
        ),
        (
            "multiple components",
            err(&codestream(&[seg(
                marker::SIZ,
                &siz_body(3, &[(7, 1, 1); 3]),
            )])),
            Variant::Unsupported,
        ),
        (
            "image origin offset",
            err(&codestream(&[seg(
                marker::SIZ,
                &siz_geom(512, 256, 1, 0, 512, 256, 0, 0),
            )])),
            Variant::Unsupported,
        ),
        (
            "multi-tile grid",
            err(&codestream(&[seg(
                marker::SIZ,
                &siz_geom(512, 256, 0, 0, 256, 256, 0, 0),
            )])),
            Variant::Unsupported,
        ),
        (
            "explicit precinct partition",
            err(&header_with_cod(cod_body(0x01, 0, 1, 0, 5, 4, 4, 0, 1))),
            Variant::Unsupported,
        ),
        (
            "SOP/EPH signalled in COD",
            err(&header_with_cod(cod_body(0x02, 0, 1, 0, 5, 4, 4, 0, 1))),
            Variant::Unsupported,
        ),
        (
            "progression order other than LRCP",
            err(&header_with_cod(cod_body(0, 1, 1, 0, 5, 4, 4, 0, 1))),
            Variant::Unsupported,
        ),
        (
            "multiple quality layers",
            err(&header_with_cod(cod_body(0, 0, 2, 0, 5, 4, 4, 0, 1))),
            Variant::Unsupported,
        ),
        (
            "multiple-component transform (COD)",
            err(&header_with_cod(cod_body(0, 0, 1, 1, 5, 4, 4, 0, 1))),
            Variant::Unsupported,
        ),
        (
            "reserved progression order",
            err(&header_with_cod(cod_body(0, 5, 1, 0, 5, 4, 4, 0, 1))),
            Variant::Marker,
        ),
        (
            "reserved wavelet transform",
            err(&header_with_cod(cod_body(0, 0, 1, 0, 5, 4, 4, 0, 2))),
            Variant::Marker,
        ),
        (
            "zero components",
            err(&codestream(&[seg(marker::SIZ, &siz_body(0, &[]))])),
            Variant::Marker,
        ),
        (
            "component count above the 16384 limit",
            err(&codestream(&[seg(
                marker::SIZ,
                &siz_body(markers::MAX_COMPONENTS + 1, &[]),
            )])),
            Variant::Marker,
        ),
        (
            "zero sub-sampling factor",
            err(&codestream(&[seg(marker::SIZ, &siz_body(1, &[(7, 0, 1)]))])),
            Variant::Marker,
        ),
        (
            "bit depth above 38",
            err(&codestream(&[seg(
                marker::SIZ,
                &siz_body(1, &[(38, 1, 1)]),
            )])),
            Variant::Marker,
        ),
        (
            "zero-size image",
            err(&codestream(&[seg(
                marker::SIZ,
                &siz_geom(0, 256, 0, 0, 512, 256, 0, 0),
            )])),
            Variant::Marker,
        ),
        (
            "missing SOC",
            err(&seg(marker::SIZ, &one_component())),
            Variant::Codestream,
        ),
        (
            "SIZ not the first marker",
            err(&{
                let mut b = be16(marker::SOC).to_vec();
                b.extend_from_slice(&seg(marker::COD, &cod_default(1)));
                b
            }),
            Variant::Codestream,
        ),
        (
            "unknown marker without a length",
            err(&{
                let mut b = be16(marker::SOC).to_vec();
                b.extend_from_slice(&seg(marker::SIZ, &one_component()));
                b.extend_from_slice(&be16(0xFF01));
                b
            }),
            Variant::Codestream,
        ),
        (
            "duplicate COD",
            err(&codestream(&[
                seg(marker::SIZ, &one_component()),
                seg(marker::COD, &cod_default(1)),
                seg(marker::COD, &cod_default(1)),
                seg(marker::QCD, &qcd_none(2, &[8; 16])),
            ])),
            Variant::Codestream,
        ),
    ];

    // Every main-header marker the subset does not decode.
    for (name, code) in [
        ("COC", marker::COC),
        ("QCC", marker::QCC),
        ("RGN", marker::RGN),
        ("POC", marker::POC),
        ("TLM", marker::TLM),
        ("PLM", marker::PLM),
        ("PLT", marker::PLT),
        ("PPM", marker::PPM),
        ("PPT", marker::PPT),
        ("CRG", marker::CRG),
        ("SOP", marker::SOP),
    ] {
        rows.push((
            name,
            err(&header_with(&seg(code, &[0, 0]))),
            Variant::Unsupported,
        ));
    }
    rows.push((
        "EPH",
        err(&header_with(&be16(marker::EPH))),
        Variant::Unsupported,
    ));

    // Every code-block style flag, individually. Each changes how Tier-1 reads a
    // code-block, so none may be ignored.
    for (bit, name) in markers::code_block_style::FLAGS {
        rows.push((
            name,
            err(&header_with_cod(cod_body(0, 0, 1, 0, 5, 4, 4, bit, 1))),
            Variant::Unsupported,
        ));
    }

    let wrong: Vec<String> = rows
        .iter()
        .filter(|(_, error, want)| variant_of(error) != *want)
        .map(|(feature, error, want)| format!("  {feature}: expected {want:?}, got {error:?}"))
        .collect();
    assert!(
        wrong.is_empty(),
        "{} inputs mapped to the wrong error variant:\n{}",
        wrong.len(),
        wrong.join("\n"),
    );
}

/// A code-block style names the flags it carries, so a rejection tells the
/// caller which feature to look up rather than printing a bare bit pattern.
#[test]
fn code_block_style_rejection_names_the_flags() {
    let bytes = header_with_cod(cod_body(0, 0, 1, 0, 5, 4, 4, 0x01 | 0x20, 1));
    let e = err(&bytes);
    let Error::Unsupported(message) = &e else {
        panic!("got {e:?}")
    };
    assert!(
        message.contains("selective arithmetic coding bypass"),
        "{message}"
    );
    assert!(message.contains("segmentation symbols"), "{message}");

    // The high bits select the HTJ2K block coder rather than being reserved, so
    // they are named too. Such a codestream also carries CAP, which rejects on
    // its own, but the style byte alone must not read as the default style.
    for (bit, name) in [(0x40u8, "high-throughput"), (0x80, "mixed-mode")] {
        let e = err(&header_with_cod(cod_body(0, 0, 1, 0, 5, 4, 4, bit, 1)));
        let Error::Unsupported(message) = &e else {
            panic!("{bit:#04X}: got {e:?}")
        };
        assert!(message.contains(name), "{bit:#04X}: {message}");
    }

    // Every bit of the style byte is allocated, so nothing goes unnamed.
    let all: u8 = markers::code_block_style::FLAGS
        .iter()
        .fold(0, |acc, (bit, _)| acc | bit);
    assert_eq!(all, u8::MAX, "every style bit must be named");
}
