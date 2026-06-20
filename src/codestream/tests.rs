//! Unit tests for main-header parsing.
//!
//! Each header is assembled byte-for-byte from the Annex A field layout, so the
//! expected `MainHeader` is checked against the spec, not against our own
//! parser. The `opj_dump` cross-check on real seed codestreams lands with the
//! fixture corpus (#4) and the tile-part walk (#6).

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
fn unknown_marker_is_codestream() {
    let mut bytes = be16(marker::SOC).to_vec();
    bytes.extend_from_slice(&seg(marker::SIZ, &one_component()));
    bytes.extend_from_slice(&be16(0xFF01)); // not a marker we know
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}
