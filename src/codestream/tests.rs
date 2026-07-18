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

    let (header, sot_offset, _) = parse_main_header(&bytes).expect("parse");

    assert_eq!(
        header,
        MainHeader::new(
            Siz {
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
            Cod {
                progression: Progression::Lrcp,
                layers: 1,
                decomposition_levels: 5,
                code_block_width: 4,
                code_block_height: 4,
                code_block_style: 0,
                use_sop: false,
                use_eph: false,
                multiple_component_transform: false,
                transform: Transform::Reversible53,
                precinct_sizes: vec![],
            },
            Qcd {
                style: QuantStyle::None,
                guard_bits: 2,
                steps: vec![(8, 0); 16],
            },
        )
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

    let (header, _, _) = parse_main_header(&bytes).expect("parse");

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

    let (header, _, _) = parse_main_header(&bytes).expect("parse");
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
fn multiple_components_parse() {
    let bytes = codestream(&[
        seg(
            marker::SIZ,
            &siz_body(3, &[(15, 1, 1), (7, 2, 2), (7, 2, 2)]),
        ),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    let (header, _, _) = parse_main_header(&bytes).expect("multi-component header parses");
    assert_eq!(header.siz.components.len(), 3);
    // Each component keeps its own depth and sub-sampling.
    assert_eq!(header.siz.component_extent(0), Some((512, 256)));
    assert_eq!(header.siz.component_extent(1), Some((256, 128)));
    assert_eq!(header.siz.components[1].bit_depth, 8);
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
fn a_nonzero_image_and_tile_offset_is_accepted() {
    // A non-zero image origin with a tile origin at or before it (Table A-9) is
    // valid JPEG 2000 — the reference-grid geometry the conformance entries
    // p1_01 and p1_07 carry. `validate_geometry` must pass it.
    let mut siz = siz_of(1, &[(8, 1, 1)]);
    siz.x_size = 128;
    siz.y_size = 128;
    siz.x_offset = 5;
    siz.y_offset = 3;
    siz.tile_x_offset = 1;
    siz.tile_y_offset = 2;
    assert!(validate_geometry(&siz).is_ok());
}

#[test]
fn an_image_offset_at_or_past_the_far_edge_is_marker() {
    // Table A-9: `XOsiz < Xsiz`. An origin at the far edge encloses no image.
    let mut siz = siz_of(1, &[(8, 1, 1)]);
    siz.x_size = 64;
    siz.y_size = 64;
    siz.x_offset = 64;
    assert!(matches!(validate_geometry(&siz), Err(Error::Marker(_))));
}

#[test]
fn a_tile_offset_past_the_image_offset_is_marker() {
    // Table A-9: `XTOsiz <= XOsiz`. A tile grid starting right of the image
    // origin is malformed, not an undecoded feature.
    let mut siz = siz_of(1, &[(8, 1, 1)]);
    siz.x_size = 64;
    siz.y_size = 64;
    siz.x_offset = 2;
    siz.tile_x_offset = 3;
    assert!(matches!(validate_geometry(&siz), Err(Error::Marker(_))));
}

#[test]
fn a_first_tile_that_never_reaches_the_image_is_marker() {
    // Table A-9: `XTOsiz + XTsiz > XOsiz`. Otherwise the first tile column clips
    // to nothing and `tile_rect` hands `resolution_geoms` an empty leading tile.
    let mut siz = siz_of(1, &[(8, 1, 1)]);
    siz.x_size = 64;
    siz.y_size = 64;
    siz.x_offset = 10;
    siz.tile_x_offset = 0;
    siz.tile_width = 8; // 0 + 8 = 8 <= 10
    assert!(matches!(validate_geometry(&siz), Err(Error::Marker(_))));
}

#[test]
fn zero_size_tile_is_marker() {
    let bytes = codestream(&[seg(marker::SIZ, &siz_geom(512, 256, 0, 0, 0, 256, 0, 0))]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

/// A tile grid finer than `Isot` can name has tiles no tile-part can reach, so
/// SIZ is malformed rather than merely unsupported. `Isot` runs 0..=65534
/// (Table A-10), which caps the grid at 65535 tiles.
#[test]
fn a_tile_grid_past_the_isot_range_is_marker() {
    // 65536 one-pixel tiles across a 65536x1 image: one tile past the limit.
    let bytes = codestream(&[seg(marker::SIZ, &siz_geom(65536, 1, 0, 0, 1, 1, 0, 0))]);
    assert!(matches!(err(&bytes), Error::Marker(_)));

    // 65535 tiles is the largest grid that fits, and it parses.
    let bytes = codestream(&[seg(marker::SIZ, &siz_geom(65535, 1, 0, 0, 1, 1, 0, 0))]);
    assert!(
        matches!(err(&bytes), Error::Codestream(_)),
        "reaches the missing-COD check"
    );
}

/// The tile grid SIZ describes, and each tile's rectangle on the reference grid.
/// The right and bottom tiles of a grid that does not divide the image evenly are
/// clipped, which is what makes an edge tile partial.
#[test]
fn the_tile_grid_covers_the_image_with_clipped_edge_tiles() {
    let header = parsed(&[
        // 40x40 image in 16x16 tiles: a 3x3 grid whose last row and column are
        // 8 wide, not 16.
        seg(marker::SIZ, &siz_geom(40, 40, 0, 0, 16, 16, 0, 0)),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    let siz = &header.siz;
    assert_eq!(siz.tile_grid(), (3, 3));
    assert_eq!(siz.num_tiles(), 9);

    let rect = |i| siz.tile_rect(i).expect("tile in the grid");
    assert_eq!(
        (rect(0).x0, rect(0).y0, rect(0).x1, rect(0).y1),
        (0, 0, 16, 16)
    );
    assert_eq!(
        (rect(1).x0, rect(1).x1),
        (16, 32),
        "tiles are numbered in raster order"
    );
    assert_eq!(
        (rect(2).x0, rect(2).x1),
        (32, 40),
        "the last column is clipped to the image"
    );
    assert_eq!(
        (rect(8).x0, rect(8).y0, rect(8).x1, rect(8).y1),
        (32, 32, 40, 40)
    );
    assert!(siz.tile_rect(9).is_none(), "past the grid");

    // Every tile of the grid, laid side by side, is the image and nothing else.
    let area: u64 = (0..siz.num_tiles()).map(|i| rect(i).area()).sum();
    assert_eq!(area, 40 * 40);
}

#[test]
fn oversize_image_area_is_a_limit() {
    // 16384×16384 = 2^28 samples, past the 2^26 decode guard.
    let n = 16384;
    let bytes = codestream(&[seg(marker::SIZ, &siz_geom(n, n, 0, 0, n, n, 0, 0))]);
    assert!(matches!(err(&bytes), Error::Limit(_)));
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
fn every_progression_order_parses() {
    for (code, want) in [
        (0, Progression::Lrcp),
        (1, Progression::Rlcp),
        (2, Progression::Rpcl),
        (3, Progression::Pcrl),
        (4, Progression::Cprl),
    ] {
        let bytes = codestream(&[
            seg(marker::SIZ, &one_component()),
            seg(marker::COD, &cod_body(0, code, 1, 0, 5, 4, 4, 0, 1)),
            seg(marker::QCD, &qcd_none(2, &[8; 16])),
        ]);
        let (header, _, _) = parse_main_header(&bytes).unwrap_or_else(|e| panic!("{code}: {e:?}"));
        assert_eq!(header.cod.progression, want, "progression code {code}");
    }
}

#[test]
fn reserved_progression_order_is_marker() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_body(0, 5, 1, 0, 5, 4, 4, 0, 1)),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
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
fn multiple_layers_parse() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_body(0, 0, 5, 0, 5, 4, 4, 0, 1)), // 5 layers
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    let (header, _, _) = parse_main_header(&bytes).expect("multi-layer header parses");
    assert_eq!(header.cod.layers, 5);
}

#[test]
fn zero_layers_is_marker() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_body(0, 0, 0, 0, 5, 4, 4, 0, 1)),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

/// A main header whose COD signals an explicit precinct partition (`Scod` bit 0)
/// with `sizes` as its `SPcod` tail, wrapped in a complete codestream. Two
/// decomposition levels, so a partition is three bytes.
fn with_precincts(sizes: &[u8]) -> Vec<u8> {
    let mut cod = cod_body(0x01, 0, 1, 0, 2, 4, 4, 0, 1);
    cod.extend_from_slice(sizes);
    let header = vec![
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod),
        seg(marker::QCD, &qcd_none(2, &[8; 7])),
    ];
    let data = [0xDE, 0xAD];
    assemble(&header, &sot_seg(0, psot_for(&data), 0, 1), &data, true)
}

/// `Scod` bit 0 adds one `SPcod` byte per resolution — `NL + 1` of them, coarsest
/// first — each packing `PPx` in the low nibble and `PPy` in the high one
/// (Table A-21).
#[test]
fn explicit_precinct_sizes_parse_from_spcod() {
    // Three resolutions: 2^0 × 2^1, 2^6 × 2^5, 2^7 × 2^7.
    let bytes = with_precincts(&[0x10, 0x56, 0x77]);
    let cs = parse(&bytes).expect("explicit precincts parse");
    assert_eq!(
        cs.header.cod.precinct_sizes,
        vec![(0, 1), (6, 5), (7, 7)],
        "PPx is the low nibble, PPy the high one"
    );
    let coding = &cs.header.components[0].coding;
    assert_eq!(coding.precinct(0), (0, 1));
    assert_eq!(coding.precinct(2), (7, 7));
}

/// A maximal partition — `Scod` bit 0 clear — carries no `SPcod` bytes and reads
/// back as `(15, 15)` everywhere, which is the single precinct that spans any
/// tile-component the sample budget admits.
#[test]
fn an_implicit_precinct_partition_is_maximal() {
    let data = [0xDE, 0xAD];
    let bytes = assemble(
        &default_header(),
        &sot_seg(0, psot_for(&data), 0, 1),
        &data,
        true,
    );
    let cs = parse(&bytes).expect("implicit precincts parse");
    assert!(cs.header.cod.precinct_sizes.is_empty());
    assert_eq!(cs.header.components[0].coding.precinct(0), (15, 15));
    assert_eq!(cs.header.components[0].coding.precinct(5), (15, 15));
}

/// A 2^0 precinct is legal only at the coarsest resolution (Table A-21): above
/// it the precinct halves onto the subband grid, so a zero exponent would ask
/// for a partition that does not exist. That is an illegal field encoding, not a
/// missing feature.
#[test]
fn a_zero_precinct_exponent_above_resolution_zero_is_a_marker_error() {
    assert!(
        parse(&with_precincts(&[0x00, 0x55, 0x55])).is_ok(),
        "2^0 is legal at resolution 0"
    );
    for sizes in [[0x55, 0x50, 0x55], [0x55, 0x05, 0x55]] {
        assert!(
            matches!(perr(&with_precincts(&sizes)), Error::Marker(_)),
            "{sizes:02X?}"
        );
    }
}

#[test]
fn sop_and_eph_flags_parse_independently() {
    for (scod, sop, eph) in [
        (0x00, false, false),
        (0x02, true, false), // p0_12 signals SOP only
        (0x04, false, true), // p0_11 signals EPH only
        (0x06, true, true),
    ] {
        let bytes = codestream(&[
            seg(marker::SIZ, &one_component()),
            seg(marker::COD, &cod_body(scod, 0, 1, 0, 5, 4, 4, 0, 1)),
            seg(marker::QCD, &qcd_none(2, &[8; 16])),
        ]);
        let (header, _, _) =
            parse_main_header(&bytes).unwrap_or_else(|e| panic!("{scod:#04X}: {e:?}"));
        assert_eq!(
            (header.cod.use_sop, header.cod.use_eph),
            (sop, eph),
            "Scod {scod:#04X}"
        );
    }
}

/// `Scod` bits 3-7 are reserved; setting one is an illegal field, not a feature
/// we have yet to decode.
#[test]
fn reserved_scod_bits_are_marker() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_body(0x08, 0, 1, 0, 5, 4, 4, 0, 1)),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

#[test]
fn reversible_colour_transform_parses() {
    // Three matching components, 5/3 wavelet, MCT signalled: this is RCT.
    let bytes = codestream(&[
        seg(marker::SIZ, &siz_body(3, &[(7, 1, 1); 3])),
        seg(marker::COD, &cod_body(0, 0, 1, 1, 5, 4, 4, 0, 1)), // mct = 1, 5/3
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    let (header, _, _) = parse_main_header(&bytes).expect("RCT header parses");
    assert!(header.cod.multiple_component_transform);
    assert_eq!(header.cod.transform, Transform::Reversible53);
}

/// The wavelet chooses the transform, so MCT on the 9/7 path is ICT (issue #76).
/// A well-formed three-component 9/7 codestream with the transform signalled
/// parses; the geometry check accepts three matching components on either arm.
#[test]
fn irreversible_colour_transform_parses() {
    let header = parsed(&[
        seg(marker::SIZ, &siz_body(3, &[(7, 1, 1); 3])),
        seg(marker::COD, &cod_body(0, 0, 1, 1, 5, 4, 4, 0, 0)), // mct = 1, 9/7
        seg(marker::QCD, &qcd_expounded(2, &[(8, 0); 16])),
    ]);
    assert!(header.cod.multiple_component_transform);
    assert_eq!(header.cod.transform, Transform::Irreversible97);
}

/// The three colour components must share a wavelet: a COC that moves one of the
/// first three to the other arm makes the transform undefined (integers on one,
/// floats on another), and is rejected.
#[test]
fn a_colour_transform_over_mixed_wavelets_is_unsupported() {
    // All-9/7 COD with MCT; a COC drops component 1 to 5/3.
    let bytes = codestream(&[
        seg(marker::SIZ, &siz_body(3, &[(7, 1, 1); 3])),
        seg(marker::COD, &cod_body(0, 0, 1, 1, 5, 4, 4, 0, 0)), // mct = 1, 9/7
        seg(marker::COC, &coc_body(&[1], 0, 5, 4, 4, 0, 1)),    // component 1 → 5/3
        seg(marker::QCD, &qcd_expounded(2, &[(8, 0); 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

/// `Smct = 2` selects Part 2's array MCT, a different feature.
#[test]
fn array_multiple_component_transform_is_unsupported() {
    let bytes = codestream(&[
        seg(marker::SIZ, &siz_body(3, &[(7, 1, 1); 3])),
        seg(marker::COD, &cod_body(0, 0, 1, 2, 5, 4, 4, 0, 1)), // mct = 2
    ]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

/// The colour transform is defined over the first three components, so a
/// codestream that signals it with fewer is describing something the transform
/// cannot express. Skipping it (as OpenJPEG does) would decode the wrong image.
#[test]
fn colour_transform_needs_three_components() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_body(0, 0, 1, 1, 5, 4, 4, 0, 1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    let e = err(&bytes);
    assert!(
        matches!(&e, Error::Marker(m) if m.contains("first three")),
        "got {e:?}"
    );
}

/// The three transformed components must share a depth, sign, and sub-sampling.
#[test]
fn colour_transform_needs_matching_components() {
    let bytes = codestream(&[
        seg(
            marker::SIZ,
            &siz_body(3, &[(7, 1, 1), (11, 1, 1), (7, 1, 1)]),
        ),
        seg(marker::COD, &cod_body(0, 0, 1, 1, 5, 4, 4, 0, 1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
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
        seg(marker::CAP, &[0, 0]), // extended capabilities (HTJ2K et al.)
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

/// A POC marker is decoded now: it parses into progression volumes rather than
/// being rejected. A single volume covering the whole codestream in LRCP is a
/// no-op relative to the COD order, but it must round-trip through the parser.
#[test]
fn a_poc_marker_parses_into_volumes() {
    // One volume: resolutions [0, 6), components [0, 1), layers [0, 1), LRCP.
    let poc = [0u8, 0, 0, 1, 6, 1, 0];
    let header = vec![
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::POC, &poc),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ];
    let data = [0xDE, 0xAD];
    let bytes = assemble(&header, &sot_seg(0, psot_for(&data), 0, 1), &data, true);
    let cs = parse(&bytes).expect("POC parses");
    assert_eq!(cs.header.poc.len(), 1);
    let v = cs.header.poc[0];
    assert_eq!((v.res_start, v.res_end), (0, 6));
    assert_eq!((v.comp_start, v.comp_end), (0, 1));
    assert_eq!(v.layer_end, 1);
    assert_eq!(v.progression, markers::Progression::Lrcp);
}

/// A POC body of `n` copies of the whole-codestream LRCP volume.
fn poc_volumes(n: usize) -> Vec<u8> {
    [0u8, 0, 0, 1, 6, 1, 0]
        .iter()
        .copied()
        .cycle()
        .take(7 * n)
        .collect()
}

/// `LYEpoc` ranges over 1–65535 (Table A-33): a zero-layer volume would enclose
/// no packets, so it is a malformed field like an empty resolution or component
/// range, not a no-op.
#[test]
fn a_poc_volume_with_zero_layers_is_a_marker_error() {
    let poc = [0u8, 0, 0, 0, 6, 1, 0]; // LYEpoc == 0
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::POC, &poc),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

/// The volume list is capped at OpenJPEG's 32 (`opj_tcp_t::pocs`): every volume
/// re-enumerates the packet space and a duplicate emission consumes no bitstream
/// bytes, so an uncapped list buys quadratic work with linear input. 32 volumes
/// parse; 33 reject as the guard, whether they arrive in one marker or split
/// across two.
#[test]
fn a_poc_past_the_volume_guard_is_a_limit() {
    let header = |poc_segs: &[Vec<u8>]| {
        let mut h = vec![
            seg(marker::SIZ, &one_component()),
            seg(marker::COD, &cod_default(1)),
        ];
        h.extend(poc_segs.iter().cloned());
        h.push(seg(marker::QCD, &qcd_none(2, &[8; 16])));
        h
    };
    let at_cap = codestream(&header(&[seg(marker::POC, &poc_volumes(32))]));
    assert!(
        parse_main_header(&at_cap).is_ok(),
        "32 volumes are within the guard"
    );

    let one_marker = codestream(&header(&[seg(marker::POC, &poc_volumes(33))]));
    assert!(matches!(err(&one_marker), Error::Limit(_)));

    let split = codestream(&header(&[
        seg(marker::POC, &poc_volumes(20)),
        seg(marker::POC, &poc_volumes(13)),
    ]));
    assert!(matches!(err(&split), Error::Limit(_)));
}

/// A tile's walk runs the *combined* main-plus-tile volume list, and OpenJPEG's
/// 32-entry array holds that combination, so the guard binds the sum: a main
/// header at the cap leaves a tile-part POC no room.
#[test]
fn main_and_tile_poc_volumes_share_the_guard() {
    let header = vec![
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::POC, &poc_volumes(32)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ];
    let tile_poc = seg(marker::POC, &poc_volumes(1));
    let bytes = assemble_with_tile_markers(&header, &[tile_poc], &[1, 2]);
    let error = parse(&bytes).expect_err("33 combined volumes should reject");
    assert!(matches!(error, Error::Limit(_)));
}

/// `Xcrg`/`Ycrg` big-endian per component — the `CRG` body (A.9.1).
fn crg_body(offsets: &[(u16, u16)]) -> Vec<u8> {
    let mut b = Vec::with_capacity(offsets.len() * 4);
    for &(x, y) in offsets {
        b.extend_from_slice(&be16(x));
        b.extend_from_slice(&be16(y));
    }
    b
}

/// A CRG records one `(Xcrg, Ycrg)` sub-pixel offset per component; the decode is
/// unaffected (the offsets are for display registration, A.9.1). The values match
/// the ones `p0_03` carries.
#[test]
fn a_crg_marker_records_the_registration_offsets() {
    let header = vec![
        seg(marker::SIZ, &siz_body(3, &[(15, 1, 1); 3])),
        seg(marker::COD, &cod_default(1)),
        seg(
            marker::CRG,
            &crg_body(&[(65424, 32558), (0, 0), (1, 0xFFFF)]),
        ),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ];
    let data = [0xDE, 0xAD];
    let bytes = assemble(&header, &sot_seg(0, psot_for(&data), 0, 1), &data, true);
    let cs = parse(&bytes).expect("CRG parses");
    assert_eq!(cs.header.crg, vec![(65424, 32558), (0, 0), (1, 0xFFFF)]);
}

/// The CRG body is exactly `4 · Csiz` bytes; a body of any other length is a
/// malformed field, not a missing feature — like OpenJPEG's own length check.
#[test]
fn a_crg_of_the_wrong_length_is_a_marker_error() {
    // Two components need 8 bytes; give 4.
    let bytes = codestream(&[
        seg(marker::SIZ, &siz_body(2, &[(15, 1, 1); 2])),
        seg(marker::COD, &cod_default(1)),
        seg(marker::CRG, &crg_body(&[(1, 2)])),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

/// One CRG per codestream (A.9.1); a second is a malformed header.
#[test]
fn a_duplicate_crg_is_a_codestream_error() {
    let crg = seg(marker::CRG, &crg_body(&[(1, 2)]));
    let mut segs = vec![
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
    ];
    segs.push(crg.clone());
    segs.push(crg);
    segs.push(seg(marker::QCD, &qcd_none(2, &[8; 16])));
    assert!(matches!(err(&codestream(&segs)), Error::Codestream(_)));
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
fn reserved_decomposition_levels_is_marker() {
    // Table A-15 allows 0–32 levels; 33 is a reserved encoding, rejected at
    // parse rather than deep in tier-2.
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_body(0, 0, 1, 0, 33, 4, 4, 0, 1)),
        seg(marker::QCD, &qcd_none(2, &[8; 97])),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

#[test]
fn too_few_qcd_steps_for_the_decomposition_is_marker() {
    // NL = 5 needs 3·5 + 1 = 16 entries; 10 leaves subbands without a step.
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 10])),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

#[test]
fn too_few_expounded_qcd_steps_is_marker() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(0)),
        seg(marker::QCD, &qcd_expounded(1, &[(10, 1234); 3])),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

#[test]
fn irreversible_wavelet_with_no_quantization_is_marker() {
    // 9/7 coefficients need a scalar step to mean anything; the pairing is
    // rejected at parse rather than misreported during dequantization.
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(0)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
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
fn oversized_qcd_step_table_is_capped() {
    // 120 one-byte entries, more than the 3·32 + 1 subbands a 32-level
    // decomposition can carry. The excess is dropped — OpenJPEG caps at
    // J2K_MAXBANDS and decodes — so a 65535-byte Lqcd cannot be cloned into
    // every component's parameters (the memory amplifier).
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 120])),
    ]);
    let (header, _, _) = parse_main_header(&bytes).expect("padding parses");
    assert_eq!(header.qcd.steps.len(), 97);
    assert_eq!(header.qcd.steps[0], (8, 0));
}

#[test]
fn oversized_expounded_qcd_step_table_is_capped() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(0)),
        seg(marker::QCD, &qcd_expounded(1, &[(10, 1234); 120])),
    ]);
    let (header, _, _) = parse_main_header(&bytes).expect("padding parses");
    assert_eq!(header.qcd.steps.len(), 97);
    assert_eq!(header.qcd.steps[0], (10, 1234));
}

#[test]
fn full_depth_qcd_step_table_parses() {
    // Exactly 97 entries is legal even though this COD only needs 16.
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 97])),
    ]);
    assert!(parse_main_header(&bytes).is_ok());
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
fn non_marker_ff00_and_ffff_are_lost_sync() {
    // 0xFF00 and 0xFFFF are assigned to no marker; walking them as segments
    // would read the following bytes as a length. They are lost sync, a
    // malformed codestream rather than an unsupported feature.
    for code in [0xFF00u16, 0xFFFF] {
        let mut bytes = be16(marker::SOC).to_vec();
        bytes.extend_from_slice(&seg(marker::SIZ, &one_component()));
        bytes.extend_from_slice(&be16(code));
        bytes.extend_from_slice(&[0x00, 0x04, 0xAA, 0xBB]); // a plausible fake segment
        assert!(matches!(err(&bytes), Error::Codestream(_)), "{code:#06X}");
    }
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
    assert_eq!(cs.tiles.len(), 1);
    assert_eq!(cs.tiles[0].index, 0);
    assert_eq!(cs.tiles[0].data.as_ref(), &data[..]);
    assert_eq!(cs.header.cod.transform, Transform::Reversible53);
}

#[test]
fn psot_zero_runs_to_eoc() {
    let data = [1, 2, 3, 4, 5];
    let sot = sot_seg(0, 0, 0, 1); // last tile-part: extends to EOC
    let bytes = assemble(&default_header(), &sot, &data, true);

    let cs = parse(&bytes).expect("parse");
    assert_eq!(cs.tiles[0].data.as_ref(), &data[..]);
}

/// Bytes after the closing EOC are ignored when `Psot` declares the length,
/// as OpenJPEG ignores them. A `Psot = 0` tile-part has no declared length —
/// everything to the buffer-end EOC is the tile (also OpenJPEG's reading, and
/// the only sound one: SOP's raw `Nsop` bytes can spell `FF D9`, so scanning
/// for an earlier EOC could truncate a valid tile).
#[test]
fn trailing_bytes_after_eoc_are_ignored_when_psot_declares_the_length() {
    let data = [1, 2, 3, 4, 5];
    let sot = sot_seg(0, psot_for(&data), 0, 1);
    let mut bytes = assemble(&default_header(), &sot, &data, true);
    bytes.extend_from_slice(&[0xAB, 0xCD, 0xFF, 0xD9]); // garbage ending in FF D9
    let cs = parse(&bytes).expect("trailing bytes after the declared extent");
    assert_eq!(cs.tiles[0].data.as_ref(), &data[..]);

    // With Psot = 0 the same tail is absorbed into the tile up to the final
    // EOC; the packet self-check downstream is what rejects it. Anchoring is
    // pinned here: the embedded FF D9 must NOT terminate the data early.
    let sot = sot_seg(0, 0, 0, 1);
    let mut bytes = assemble(&default_header(), &sot, &data, true);
    bytes.extend_from_slice(&[0xAB, 0xCD, 0xFF, 0xD9]);
    let cs = parse(&bytes).expect("EOC anchored at the buffer end");
    assert_eq!(cs.tiles[0].data.len(), data.len() + 4);
}

#[test]
fn empty_tile_part_data_parses() {
    let sot = sot_seg(0, psot_for(&[]), 0, 1);
    let bytes = assemble(&default_header(), &sot, &[], true);

    let cs = parse(&bytes).expect("parse");
    assert!(cs.tiles[0].data.is_empty());
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
    assert_eq!(cs.tiles[0].data.as_ref(), &data[..]);
}

/// `Isot` must name a tile the grid holds. The default header is a single tile,
/// so tile 1 does not exist and the codestream is malformed.
#[test]
fn a_tile_index_past_the_grid_is_codestream() {
    let sot = sot_seg(1, 0, 0, 1); // Isot = 1, but the grid holds one tile
    let bytes = assemble(&default_header(), &sot, &[1, 2], true);
    assert!(matches!(perr(&bytes), Error::Codestream(_)));
}

/// `TNsot` states how many tile-parts the tile has. A tile that declares two and
/// carries one is truncated, whatever the rest of the codestream says.
#[test]
fn a_tnsot_the_tile_parts_do_not_add_up_to_is_codestream() {
    let data = [1, 2];
    let sot = sot_seg(0, psot_for(&data), 0, 2); // TNsot = 2, but only one part follows
    let bytes = assemble(&default_header(), &sot, &data, true);
    assert!(matches!(perr(&bytes), Error::Codestream(_)));
}

/// A tile's parts are numbered from zero and arrive in order (A.4.2), so the
/// first one to appear must be `TPsot = 0`. Starting at 1 means part 0 is missing.
#[test]
fn a_tile_part_index_out_of_order_is_codestream() {
    let sot = sot_seg(0, 0, 1, 1); // TPsot = 1 with no part 0 before it
    let bytes = assemble(&default_header(), &sot, &[1, 2], true);
    assert!(matches!(perr(&bytes), Error::Codestream(_)));
}

/// A tile split across several tile-parts is one tile: its parts' packet bytes
/// are joined, in `TPsot` order, into the single stream the packet walk reads.
/// A tile-part holds a whole number of packets (B.9), so the join is exact.
#[test]
fn several_tile_parts_of_one_tile_are_joined_in_order() {
    let first_data = [7, 7, 7];
    // TNsot = 0 in the first part: "not stated yet", which A.4.2 allows.
    let first = sot_seg(0, psot_for(&first_data), 0, 0);

    let mut bytes = be16(marker::SOC).to_vec();
    for part in default_header() {
        bytes.extend_from_slice(&part);
    }
    bytes.extend_from_slice(&first);
    bytes.extend_from_slice(&be16(marker::SOD));
    bytes.extend_from_slice(&first_data);
    // The last part carries Psot = 0, so it runs to the closing EOC.
    bytes.extend_from_slice(&sot_seg(0, 0, 1, 2));
    bytes.extend_from_slice(&be16(marker::SOD));
    bytes.extend_from_slice(&[8, 8]);
    bytes.extend_from_slice(&be16(marker::EOC));

    let cs = parse(&bytes).expect("two tile-parts of one tile");
    assert_eq!(cs.tiles.len(), 1);
    assert_eq!(cs.tiles[0].data.as_ref(), &[7, 7, 7, 8, 8]);
}

/// Every tile of the grid must be carried. A codestream that names a 2x1 grid and
/// ships one tile describes an image it does not contain — a truncation, not a
/// feature to decode around.
#[test]
fn a_tile_with_no_tile_part_is_codestream() {
    let header = vec![
        // 512x256 in 256-wide tiles: a 2x1 grid.
        seg(marker::SIZ, &siz_geom(512, 256, 0, 0, 256, 256, 0, 0)),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ];
    // Only tile 0 is carried; tile 1 never appears.
    let bytes = assemble(&header, &sot_seg(0, 0, 0, 1), &[1, 2], true);
    assert!(matches!(perr(&bytes), Error::Codestream(_)));
}

/// PPM's payload frames one packet-header chunk per tile-part with a 4-byte
/// `Nppm` length (A.7.4). `decode_ppm_chunks` strips the lengths and returns the
/// per-tile-part chunks, in `Zppm` order, joining markers so an `Nppm` can
/// straddle a marker boundary — the same bytes an inline codestream would carry
/// as each tile-part's packet headers, which is why a PPM stream decodes to the
/// same packets as its inline equivalent (proven end-to-end by `p1_03`/`p1_05`).
#[test]
fn ppm_chunks_split_by_nppm_across_markers() {
    // Two tile-parts in one marker: Nppm 1 then Nppm 2.
    let one = decode_ppm_chunks(vec![(0, vec![0, 0, 0, 1, 0xAA, 0, 0, 0, 2, 0xBB, 0xCC])]).unwrap();
    assert_eq!(one, vec![vec![0xAA], vec![0xBB, 0xCC]]);

    // Nppm=3 straddles two markers; joined in Zppm order (given out of order).
    let split =
        decode_ppm_chunks(vec![(1, vec![0xBB, 0xCC]), (0, vec![0, 0, 0, 3, 0xAA])]).unwrap();
    assert_eq!(split, vec![vec![0xAA, 0xBB, 0xCC]]);

    // A zero-length chunk is a tile-part that carries no packet headers.
    assert_eq!(
        decode_ppm_chunks(vec![(0, vec![0, 0, 0, 0])]).unwrap(),
        vec![Vec::<u8>::new()]
    );

    // No PPM markers: no chunks.
    assert_eq!(decode_ppm_chunks(vec![]).unwrap(), Vec::<Vec<u8>>::new());
}

/// A malformed PPM payload — a duplicate `Zppm`, an `Nppm` the payload cannot
/// satisfy, or a trailing fragment too short to be a length — is a codestream
/// error, not a wrong decode.
#[test]
fn ppm_chunks_reject_malformed_framing() {
    // Nppm=5 but only one byte follows.
    assert!(matches!(
        decode_ppm_chunks(vec![(0, vec![0, 0, 0, 5, 0xAA])]),
        Err(Error::Codestream(_))
    ));
    // Three trailing bytes: too short for a 4-byte Nppm.
    assert!(matches!(
        decode_ppm_chunks(vec![(0, vec![0, 0, 0])]),
        Err(Error::Codestream(_))
    ));
    // Two markers share Zppm 0.
    assert!(matches!(
        decode_ppm_chunks(vec![(0, vec![0, 0, 0, 0]), (0, vec![0, 0, 0, 0])]),
        Err(Error::Codestream(_))
    ));
}

/// PPT moves the tile's packet headers into the tile-part header (A.7.5): it
/// parses into the tile's packed-header buffer, ordered by `Zppt`, and the
/// tile-part data is only the packet bodies. Two PPT markers, out of Zppt order,
/// stitch back in order.
#[test]
fn tile_part_ppt_collects_the_packed_headers() {
    let data = [0xBB, 0xBB];
    // Zppt 1 carries `[0xCD]`, Zppt 0 carries `[0xAB]`; the buffer is `AB CD`.
    let ppt1 = seg(marker::PPT, &[1, 0xCD]);
    let ppt0 = seg(marker::PPT, &[0, 0xAB]);
    let psot = (12 + ppt1.len() + ppt0.len() + 2 + data.len()) as u32;

    let mut bytes = be16(marker::SOC).to_vec();
    for part in default_header() {
        bytes.extend_from_slice(&part);
    }
    bytes.extend_from_slice(&sot_seg(0, psot, 0, 1));
    bytes.extend_from_slice(&ppt1);
    bytes.extend_from_slice(&ppt0);
    bytes.extend_from_slice(&be16(marker::SOD));
    bytes.extend_from_slice(&data);
    bytes.extend_from_slice(&be16(marker::EOC));

    let cs = parse(&bytes).expect("PPT parses");
    assert_eq!(cs.tiles[0].packed_headers, vec![0xAB, 0xCD]);
    assert_eq!(cs.tiles[0].data.as_ref(), &data[..]);
}

/// A `Zppt` repeated across a tile-part's PPT markers is a malformed header.
#[test]
fn tile_part_duplicate_zppt_is_a_codestream_error() {
    let data = [0xBB];
    let a = seg(marker::PPT, &[0, 0x11]);
    let b = seg(marker::PPT, &[0, 0x22]); // Zppt 0 again
    let psot = (12 + a.len() + b.len() + 2 + data.len()) as u32;

    let mut bytes = be16(marker::SOC).to_vec();
    for part in default_header() {
        bytes.extend_from_slice(&part);
    }
    bytes.extend_from_slice(&sot_seg(0, psot, 0, 1));
    bytes.extend_from_slice(&a);
    bytes.extend_from_slice(&b);
    bytes.extend_from_slice(&be16(marker::SOD));
    bytes.extend_from_slice(&data);
    bytes.extend_from_slice(&be16(marker::EOC));

    assert!(matches!(perr(&bytes), Error::Codestream(_)));
}

/// TLM is main-header-only (A.7.1) and SOP/EPH belong after SOD, so meeting
/// one in a tile-part header is a malformed codestream — unlike the QCC above,
/// which is legal there and merely outside the subset.
#[test]
fn structurally_illegal_tile_header_marker_is_codestream() {
    let data = [1, 2];
    for illegal in [seg(marker::TLM, &[0, 0x60]), seg(marker::SOP, &[0, 0])] {
        let psot = (12 + illegal.len() + 2 + data.len()) as u32;
        let mut bytes = be16(marker::SOC).to_vec();
        for part in default_header() {
            bytes.extend_from_slice(&part);
        }
        bytes.extend_from_slice(&sot_seg(0, psot, 0, 1));
        bytes.extend_from_slice(&illegal);
        bytes.extend_from_slice(&be16(marker::SOD));
        bytes.extend_from_slice(&data);
        bytes.extend_from_slice(&be16(marker::EOC));

        assert!(matches!(perr(&bytes), Error::Codestream(_)));
    }
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
    assert_eq!(siz.image_extent_at(0), (512, 256));
    for i in 0..3 {
        assert_eq!(siz.component_extent(i), Some((512, 256)));
    }
    assert_eq!(siz.component_extent(3), None);
}

#[test]
fn subsampled_siz_derives_each_component_extent() {
    // Image is 512x256 at the origin; the four classic sub-sampling factors.
    let siz = siz_of(4, &[(7, 1, 1), (7, 2, 1), (7, 1, 2), (7, 2, 2)]);
    assert_eq!(siz.image_extent_at(0), (512, 256));
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
    assert_eq!(siz.image_extent_at(0), (7, 7));
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

/// A 4096x4096 SIZ carrying `count` unit-sampled 8-bit components. One such
/// component sits inside the decode guard; enough of them do not.
fn many_component_siz(count: u16) -> Vec<u8> {
    let mut body = siz_geom(4096, 4096, 0, 0, 4096, 4096, 0, 0);
    body.truncate(body.len() - 5); // drop the Csiz and its one component record
    body.extend_from_slice(&be16(count));
    for _ in 0..count {
        body.extend_from_slice(&[7, 1, 1]);
    }
    body
}

#[test]
fn many_components_exceed_the_sample_budget() {
    // Each component is reconstructed into its own buffer, so the guard bounds
    // the *sum* of the component areas. A 4096x4096 image is well inside the
    // single-component guard; a hundred of its components are not.
    let bytes = codestream(&[
        seg(marker::SIZ, &many_component_siz(100)),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    let e = err(&bytes);
    assert!(
        matches!(&e, Error::Limit(m) if m.contains("decode guard")),
        "got {e:?}"
    );

    // One component of the same image stays inside the guard.
    let bytes = codestream(&[
        seg(marker::SIZ, &siz_geom(4096, 4096, 0, 0, 4096, 4096, 0, 0)),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(parse_main_header(&bytes).is_ok());
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
    // Codes this decoder names — the Part 2 block (0xFF74..=0xFF78) among them —
    // reject under their own message, checked in the reject matrix. These are the
    // ones it genuinely does not recognise, which the walk must still step over.
    for code in [
        0xFF01u16, // no part of the standard we implement
        0xFF2F,    // just below the reserved segment-less range
        0xFF40,    // just above it
        0xFF73,    // just below the Part 2 block
        0xFF79,    // just above the Part 2 block
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
    // codestream is interpreted, so each is named and rejected rather than
    // passed over. PPM has left this list — it is decoded now (issue #71); the
    // length markers TLM/PLM/PLT are absent too, being informational (#72).
    for code in [marker::CAP, marker::PPT, marker::SOP] {
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

        // The class-1 references are one `.pgx` per graded component, decoded
        // at the entry's recorded `reduction` (OpenJPEG's `C1P0_ResFactor_list`;
        // 0 everywhere but p0_08) — so their dimensions are the oracle for
        // `component_extent_at` at that reduction, for all 23 entries. A corpus
        // refresh that changes a factor without re-recording it fails here.
        let reduction = entry["reduction"]
            .as_u64()
            .unwrap_or_else(|| panic!("{name}: manifest entry records no reduction"))
            as u8;
        for (i, reference) in entry["references"]["class1"]
            .as_array()
            .expect("class1 refs")
            .iter()
            .enumerate()
        {
            let (rw, rh) = pgx_extent(&dir.join(reference.as_str().unwrap()));
            let (cw, ch) = siz
                .component_extent_at(i, reduction)
                .expect("graded component exists");
            assert_eq!(
                (cw, ch),
                (rw, rh),
                "{name}: component {i} extent at reduction {reduction} disagrees with its \
                 reference",
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
/// codestream exists to catch, so assert the *kind* of failure: a feature
/// rejection means the walk got there, a structural one means it did not.
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

    // The walk steps over `0xFF30` and the whole header parses: p0_02's COD, its
    // COC, and its `restart | pterm | segsym` style are all decoded. A naive
    // walker reports `Codestream("truncated marker segment")` here instead,
    // which is the regression this pins.
    let cs = parse(&bytes).expect("p0_02's main header is inside the decoded subset");

    // The COC overrides component 0, so the resolved parameters are its, not
    // COD's. p0_02 has one component and `opj_dump` reports `cblksty=0x34`.
    assert_eq!(cs.header.components.len(), 1);
    assert_eq!(cs.header.components[0].coding.code_block_style, 0x34);
}

// --- the reject matrix (issue #78) ----------------------------------------

/// Which typed error a caller sees, per the mapping the crate commits to:
/// `Codestream` for structural damage (truncation, lost sync, a missing
/// required marker), `Marker` for a field encoded illegally, `Unsupported`
/// for valid JPEG 2000 that falls outside the decoded subset, and `Limit` for
/// an input past one of the decoder's resource guards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variant {
    Codestream,
    Marker,
    Unsupported,
    Limit,
}

fn variant_of(e: &Error) -> Variant {
    match e {
        Error::Codestream(_) => Variant::Codestream,
        Error::Marker(_) => Variant::Marker,
        Error::Unsupported(_) => Variant::Unsupported,
        Error::Limit(_) => Variant::Limit,
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
/// input past a resource guard is `Limit`, an illegal field is `Marker`, and
/// structural damage is `Codestream`. Nothing
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
        // Multiple components are decoded, so they left this table. What
        // replaces them is the allocation guard: every component reconstructs
        // into its own buffer, so a large image with many components must be
        // refused rather than attempted.
        (
            "components over the sample budget",
            err(&codestream(&[
                seg(marker::SIZ, &many_component_siz(100)),
                seg(marker::COD, &cod_default(1)),
                seg(marker::QCD, &qcd_none(2, &[8; 16])),
            ])),
            Variant::Limit,
        ),
        // A non-zero image origin is decoded now; one at or past the far edge
        // (XOsiz >= Xsiz, Table A-9) encloses no image and is an illegal field.
        (
            "image origin at the far edge",
            err(&codestream(&[seg(
                marker::SIZ,
                &siz_geom(512, 256, 512, 0, 512, 256, 0, 0),
            )])),
            Variant::Marker,
        ),
        // A tile grid is decoded now; one finer than `Isot` can name is not a
        // missing feature but an illegal field, since no tile-part could ever
        // name its tiles.
        (
            "tile grid past the Isot range",
            err(&codestream(&[seg(
                marker::SIZ,
                &siz_geom(65536, 1, 0, 0, 1, 1, 0, 0),
            )])),
            Variant::Marker,
        ),
        // The precinct partition is decoded, so it left this table; a zero
        // exponent above resolution 0 is still an illegal field.
        (
            "zero precinct exponent above resolution 0",
            err(&header_with_cod({
                let mut body = cod_body(0x01, 0, 1, 0, 1, 4, 4, 0, 1);
                body.extend_from_slice(&[0x55, 0x50]);
                body
            })),
            Variant::Marker,
        ),
        // The SOP/EPH delimiters are decoded, so they left this table; a
        // reserved Scod bit is still an illegal field.
        (
            "reserved Scod bit",
            err(&header_with_cod(cod_body(0x08, 0, 1, 0, 5, 4, 4, 0, 1))),
            Variant::Marker,
        ),
        // Every progression order is decoded, so they left this table; a
        // reserved code is still an illegal field.
        // Multiple quality layers are decoded, so they left this table; a
        // declared zero is still an illegal field.
        (
            "zero quality layers",
            err(&header_with_cod(cod_body(0, 0, 0, 0, 5, 4, 4, 0, 1))),
            Variant::Marker,
        ),
        // Both colour transforms are decoded now (RCT and ICT), so they left
        // this table. What remains out of subset: Part 2's array MCT, a
        // codestream that signals the transform without the three components it
        // is defined over, and one that mixes wavelets across those components.
        (
            "colour transform over mixed wavelets",
            err(&codestream(&[
                seg(marker::SIZ, &siz_body(3, &[(7, 1, 1); 3])),
                seg(marker::COD, &cod_body(0, 0, 1, 1, 5, 4, 4, 0, 0)),
                seg(marker::COC, &coc_body(&[1], 0, 5, 4, 4, 0, 1)),
                seg(marker::QCD, &qcd_expounded(2, &[(8, 0); 16])),
            ])),
            Variant::Unsupported,
        ),
        (
            "Part 2 array multiple-component transform",
            err(&header_with_cod(cod_body(0, 0, 1, 2, 5, 4, 4, 0, 1))),
            Variant::Unsupported,
        ),
        (
            "colour transform without three components",
            err(&header_with_cod(cod_body(0, 0, 1, 1, 5, 4, 4, 0, 1))),
            Variant::Marker,
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

    // Every main-header marker the subset does not decode. TLM and PLM have
    // left this table (informational, decoded — issue #72); PPM too (its packed
    // packet headers are decoded — issue #71); PLT is decoded in the tile-part
    // header and structurally illegal in the main one.
    for (name, code) in [("PPT", marker::PPT), ("SOP", marker::SOP)] {
        rows.push((
            name,
            err(&header_with(&seg(code, &[0, 0]))),
            Variant::Unsupported,
        ));
    }

    // Every JPEG 2000 Part 2 codestream marker (0xFF74..=0xFF78): out of the
    // Part 1 subset, so each is an `Unsupported` feature the moment it appears,
    // never half-parsed as Part 1 (issue #78). CAP (Part 15/HTJ2K) is above.
    for (name, code) in [
        ("MCT", marker::MCT),
        ("MCC", marker::MCC),
        ("NLT", marker::NLT),
        ("MCO", marker::MCO),
        ("CBD", marker::CBD),
    ] {
        rows.push((
            name,
            err(&header_with(&seg(code, &[0, 0]))),
            Variant::Unsupported,
        ));
    }
    rows.push((
        "PLT in the main header",
        err(&header_with(&seg(marker::PLT, &[0, 0]))),
        Variant::Codestream,
    ));
    rows.push((
        "EPH",
        err(&header_with(&be16(marker::EPH))),
        Variant::Unsupported,
    ));

    // Every code-block style flag that is not yet decoded, individually. Each
    // changes how Tier-1 reads a code-block, so none may be ignored. `bypass`,
    // `restart`, `predictable termination`, `segmentation symbols`, `vertically
    // causal context` and `reset context probabilities` have left this table,
    // leaving only the two HTJ2K bits.
    use markers::code_block_style::{LAZY, PTERM, RESET, SEGSYM, TERMALL, VCAUSAL};
    for (bit, name) in markers::code_block_style::FLAGS {
        if bit & (LAZY | TERMALL | PTERM | SEGSYM | VCAUSAL | RESET) != 0 {
            continue;
        }
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

/// The decoded style bits — bypass, restart, predictable termination,
/// segmentation symbols, vertically causal context, and reset context
/// probabilities — parse; only the two HTJ2K bits do not, and a style byte
/// mixing them still rejects, naming only the parts that block it.
#[test]
fn the_decoded_styles_parse_and_the_rest_still_reject() {
    use markers::code_block_style::{LAZY, PTERM, RESET, SEGSYM, TERMALL, VCAUSAL};

    // Every combination of the six decoded flags parses.
    for style in [
        0,
        LAZY,
        TERMALL,
        PTERM,
        SEGSYM,
        VCAUSAL,
        RESET,
        LAZY | TERMALL | PTERM | SEGSYM | VCAUSAL | RESET,
    ] {
        let bytes = codestream(&[
            seg(marker::SIZ, &one_component()),
            seg(marker::COD, &cod_body(0, 0, 1, 0, 5, 4, 4, style, 1)),
            seg(marker::QCD, &qcd_none(2, &[8; 16])),
        ]);
        let (header, _, _) = parse_main_header(&bytes).expect("a decoded style parses");
        assert_eq!(header.cod.code_block_style, style);
    }

    // A decoded flag beside an undecoded one: the message names only the
    // undecoded half, so it says what is actually missing.
    let e = err(&header_with_cod(cod_body(
        0,
        0,
        1,
        0,
        5,
        4,
        4,
        VCAUSAL | RESET | 0x40, // + HTJ2K high-throughput block coding
        1,
    )));
    let Error::Unsupported(message) = &e else {
        panic!("got {e:?}")
    };
    assert!(message.contains("high-throughput"), "{message}");
    for decoded in [
        "selective arithmetic coding bypass",
        "vertically causal context",
        "reset context probabilities",
        "termination on each coding pass",
        "segmentation symbols",
        "predictable termination",
    ] {
        assert!(
            !message.contains(decoded),
            "{decoded} is decoded and must not be named as a blocker: {message}"
        );
    }
}

/// A code-block style names the flags it carries, so a rejection tells the
/// caller which feature to look up rather than printing a bare bit pattern.
#[test]
fn code_block_style_rejection_names_the_flags() {
    // Two undecoded flags: the HTJ2K high-throughput and mixed-mode bits.
    let bytes = header_with_cod(cod_body(0, 0, 1, 0, 5, 4, 4, 0x40 | 0x80, 1));
    let e = err(&bytes);
    let Error::Unsupported(message) = &e else {
        panic!("got {e:?}")
    };
    assert!(message.contains("high-throughput"), "{message}");
    assert!(message.contains("mixed-mode"), "{message}");

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

// --- COC / QCC per-component overrides (issue #58) -------------------------

/// `SPcoc` body: the tail COD and COC share, minus the component index and
/// `Scoc`.
fn coc_body(
    index_bytes: &[u8],
    scoc: u8,
    nl: u8,
    xcb: u8,
    ycb: u8,
    style: u8,
    transform: u8,
) -> Vec<u8> {
    let mut b = index_bytes.to_vec();
    b.push(scoc);
    b.extend_from_slice(&[nl, xcb, ycb, style, transform]);
    b
}

fn parsed(segments: &[Vec<u8>]) -> crate::codestream::MainHeader {
    let (header, _, _) = parse_main_header(&codestream(segments)).expect("parses");
    header
}

/// The whole of A.6.2 and A.6.5: a component with no COC takes COD's coding
/// parameters, one with a COC takes the COC's, and the same for QCC over QCD.
#[test]
fn overrides_resolve_else_default_per_component() {
    let siz = siz_body(3, &[(15, 1, 1); 3]);
    let header = parsed(&[
        seg(marker::SIZ, &siz),
        seg(marker::COD, &cod_body(0, 0, 1, 0, 5, 4, 4, 0, 1)),
        // Component 1 alone overrides the coding style: 3 levels, restart.
        seg(marker::COC, &coc_body(&[1], 0, 3, 2, 2, 0x04, 1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
        // Component 2 alone overrides the quantization: 4 guard bits. Its
        // levels stay COD's 5, so the table needs the full 16 entries.
        seg(marker::QCC, &{
            let mut b = vec![2u8];
            b.extend_from_slice(&qcd_none(4, &[9; 16]));
            b
        }),
    ]);

    // Component 0: both defaults.
    assert_eq!(header.components[0].coding, header.cod.coding());
    assert_eq!(header.components[0].quant, header.qcd);

    // Component 1: COC's coding, QCD's quantization.
    assert_eq!(header.components[1].coding.decomposition_levels, 3);
    assert_eq!(header.components[1].coding.code_block_style, 0x04);
    assert_eq!(header.components[1].coding.code_block_width, 2);
    assert_eq!(header.components[1].quant, header.qcd);

    // Component 2: COD's coding, QCC's quantization.
    assert_eq!(header.components[2].coding, header.cod.coding());
    assert_eq!(header.components[2].quant.guard_bits, 4);
    assert_ne!(header.components[2].quant, header.qcd);
}

/// `Ccoc`/`Cqcc` is one byte while `Csiz < 257` and two bytes at 257 and above
/// (A.6.2). Read at the wrong width, every field after it shifts by a byte and
/// the segment either overruns or decodes garbage. `p0_13` has 257 components,
/// so this boundary is live in the corpus.
#[test]
fn component_index_is_two_bytes_from_257_components() {
    // 256 components: one-byte index.
    let narrow = vec![(15u8, 1u8, 1u8); 256];
    let header = parsed(&[
        seg(marker::SIZ, &siz_body(256, &narrow)),
        seg(marker::COD, &cod_body(0, 0, 1, 0, 5, 4, 4, 0, 1)),
        seg(marker::COC, &coc_body(&[255], 0, 3, 4, 4, 0, 1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert_eq!(header.components[255].coding.decomposition_levels, 3);
    assert_eq!(header.components[0].coding.decomposition_levels, 5);

    // 257 components: two-byte index. The same one-byte body must now fail.
    let wide = vec![(15u8, 1u8, 1u8); 257];
    let header = parsed(&[
        seg(marker::SIZ, &siz_body(257, &wide)),
        seg(marker::COD, &cod_body(0, 0, 1, 0, 5, 4, 4, 0, 1)),
        seg(marker::COC, &coc_body(&[0x01, 0x00], 0, 3, 4, 4, 0, 1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert_eq!(header.components[256].coding.decomposition_levels, 3);
    assert_eq!(header.components[0].coding.decomposition_levels, 5);
}

#[test]
fn an_override_past_the_component_count_is_a_marker_error() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::COC, &coc_body(&[3], 0, 3, 4, 4, 0, 1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

/// A.6.2 and A.6.5 allow one COC and one QCC per component per header. A second
/// is malformed, not a later-wins override — guessing would decode an image the
/// encoder never described.
#[test]
fn a_second_override_for_one_component_is_a_codestream_error() {
    let coc = seg(marker::COC, &coc_body(&[0], 0, 3, 4, 4, 0, 1));
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        coc.clone(),
        coc,
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Codestream(_)));

    let qcc = seg(marker::QCC, &{
        let mut b = vec![0u8];
        b.extend_from_slice(&qcd_none(2, &[8; 16]));
        b
    });
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
        qcc.clone(),
        qcc,
    ]);
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

/// `Scoc` bit 0 signals an explicit precinct partition for one component, which
/// overrides COD's for that component alone; its other seven bits are reserved
/// and must be zero.
#[test]
fn a_coc_carries_its_own_precinct_partition() {
    let with = |scoc, tail: &[u8]| {
        let mut coc = coc_body(&[0], scoc, 3, 4, 4, 0, 1);
        coc.extend_from_slice(tail);
        let header = vec![
            seg(marker::SIZ, &one_component()),
            seg(marker::COD, &cod_default(1)),
            seg(marker::COC, &coc),
            seg(marker::QCD, &qcd_none(2, &[8; 16])),
        ];
        let data = [0xDE, 0xAD];
        assemble(&header, &sot_seg(0, psot_for(&data), 0, 1), &data, true)
    };
    // Four resolutions (NL = 3), so four precinct bytes.
    let bytes = with(0x01, &[0x22, 0x33, 0x44, 0x55]);
    let cs = parse(&bytes).expect("COC precincts parse");
    assert_eq!(
        cs.header.components[0].coding.precinct_sizes,
        vec![(2, 2), (3, 3), (4, 4), (5, 5)],
    );
    // COD's own partition stays maximal — the COC replaced it for the component,
    // not for the codestream.
    assert!(cs.header.cod.precinct_sizes.is_empty());

    assert!(matches!(perr(&with(0x02, &[])), Error::Marker(_)));
    assert!(matches!(perr(&with(0x80, &[])), Error::Marker(_)));
}

/// A COC's style byte goes through the same gate COD's does: an undecoded flag
/// on any component is still undecoded.
#[test]
fn a_coc_cannot_smuggle_in_an_undecoded_code_block_style() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::COC, &coc_body(&[0], 0, 3, 4, 4, 0x40, 1)), // HTJ2K (undecoded)
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    let e = err(&bytes);
    let Error::Unsupported(message) = &e else {
        panic!("got {e:?}")
    };
    assert!(message.contains("high-throughput"), "{message}");
    assert!(
        message.contains("COC"),
        "the message must name the marker: {message}"
    );
}

/// Which colour transform applies follows from the wavelet, and COC makes the
/// wavelet per component. RCT is defined over three integer components, so a COC
/// that moves one of the first three to 9/7 describes a transform that does not
/// exist.
#[test]
fn a_coc_moving_a_colour_component_to_the_97_wavelet_is_unsupported() {
    // Each 9/7 component gets a scalar QCC so the wavelet/quant pairing is
    // sound and the failure under test is the colour transform alone.
    let scalar_qcc = |component: u8| {
        seg(marker::QCC, &{
            let mut b = vec![component];
            b.extend_from_slice(&qcd_expounded(2, &[(8, 0); 16]));
            b
        })
    };

    let siz = siz_body(3, &[(15, 1, 1); 3]);
    let bytes = codestream(&[
        seg(marker::SIZ, &siz),
        // mct = 1, reversible 5/3: a valid RCT codestream.
        seg(marker::COD, &cod_body(0, 0, 1, 1, 5, 4, 4, 0, 1)),
        // ... except component 1 is 9/7.
        seg(marker::COC, &coc_body(&[1], 0, 5, 4, 4, 0, 0)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
        scalar_qcc(1),
    ]);
    let e = err(&bytes);
    let Error::Unsupported(message) = &e else {
        panic!("got {e:?}")
    };
    assert!(message.contains("component 1"), "{message}");

    // A fourth component on 9/7 is outside the transform, so it is allowed.
    let siz = siz_body(4, &[(15, 1, 1); 4]);
    let bytes = codestream(&[
        seg(marker::SIZ, &siz),
        seg(marker::COD, &cod_body(0, 0, 1, 1, 5, 4, 4, 0, 1)),
        seg(marker::COC, &coc_body(&[3], 0, 5, 4, 4, 0, 0)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
        scalar_qcc(3),
    ]);
    assert!(parse_main_header(&bytes).is_ok());
}

/// A malformed QCC must report as a QCC fault, not as a fault in QCD. The two
/// share a body parser, and a per-component override that blames the
/// codestream-wide default sends a reader to the wrong marker.
#[test]
fn a_malformed_qcc_names_qcc_and_not_qcd() {
    // Scalar expounded (style 2) with an odd-length step table: truncated.
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
        seg(marker::QCC, &[0, (2 << 5) | 2, 0x00]),
    ]);
    let e = err(&bytes);
    let Error::Codestream(message) = &e else {
        panic!("got {e:?}")
    };
    assert!(message.contains("QCC"), "{message}");
    assert!(!message.contains("QCD"), "{message}");
}

// --- RGN: region of interest, maxshift (issue #77) --------------------------

fn rgn_body(index_bytes: &[u8], srgn: u8, shift: u8) -> Vec<u8> {
    let mut b = index_bytes.to_vec();
    b.extend_from_slice(&[srgn, shift]);
    b
}

/// An RGN names one component and lifts its region above the background by
/// `SPrgn` bit-planes. Components it does not name keep a shift of zero.
#[test]
fn rgn_sets_the_maxshift_of_one_component() {
    let header = parsed(&[
        seg(marker::SIZ, &siz_body(3, &[(15, 1, 1); 3])),
        seg(marker::COD, &cod_default(1)),
        seg(marker::RGN, &rgn_body(&[1], 0, 9)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert_eq!(header.components[0].roi_shift, 0);
    assert_eq!(header.components[1].roi_shift, 9);
    assert_eq!(header.components[2].roi_shift, 0);
}

/// `Crgn` has the same width rule as `Ccoc` and `Cqcc`: one byte below 257
/// components, two from 257 up.
#[test]
fn rgn_component_index_is_two_bytes_from_257_components() {
    let header = parsed(&[
        seg(marker::SIZ, &siz_body(257, &vec![(15u8, 1u8, 1u8); 257])),
        seg(marker::COD, &cod_default(1)),
        seg(marker::RGN, &rgn_body(&[0x01, 0x00], 0, 4)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert_eq!(header.components[256].roi_shift, 4);
    assert_eq!(header.components[0].roi_shift, 0);
}

/// `Srgn = 0` (implicit — the maxshift of Annex H) is the only style Part 1
/// defines. OpenJPEG reads the byte and never looks at it, so a Part 2 style
/// decodes there as though it were maxshift; reject it instead.
#[test]
fn a_non_maxshift_roi_style_is_unsupported() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::RGN, &rgn_body(&[0], 1, 9)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Unsupported(_)));
}

#[test]
fn an_rgn_past_the_component_count_is_a_marker_error() {
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::RGN, &rgn_body(&[3], 0, 9)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

#[test]
fn a_second_rgn_for_one_component_is_a_codestream_error() {
    let rgn = seg(marker::RGN, &rgn_body(&[0], 0, 9));
    let bytes = codestream(&[
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        rgn.clone(),
        rgn,
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ]);
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

/// A short or long RGN segment is malformed: `Lrgn` is exactly `2 + Crgn`.
#[test]
fn a_wrong_length_rgn_is_rejected() {
    for body in [vec![0u8, 0], vec![0u8, 0, 9, 9]] {
        let bytes = codestream(&[
            seg(marker::SIZ, &one_component()),
            seg(marker::COD, &cod_default(1)),
            seg(marker::RGN, &body),
            seg(marker::QCD, &qcd_none(2, &[8; 16])),
        ]);
        assert!(
            matches!(err(&bytes), Error::Codestream(_)),
            "{body:?} should reject"
        );
    }
}

/// SOC + `header` + a tile-part whose header carries `markers` before SOD.
///
/// The markers ride in the `sot` slot of [`assemble`], which splices its
/// argument verbatim between the main header and SOD — one byte layout for
/// every tile-part test. `Psot` counts them as part of the tile-part header.
fn assemble_with_tile_markers(header: &[Vec<u8>], markers: &[Vec<u8>], data: &[u8]) -> Vec<u8> {
    let markers_len: usize = markers.iter().map(Vec::len).sum();
    let mut sot = sot_seg(0, psot_for(data) + markers_len as u32, 0, 1);
    for m in markers {
        sot.extend_from_slice(m);
    }
    assemble(header, &sot, data, true)
}

/// A tile-part RGN *replaces* the main header's for that tile (A.6.3), so
/// honouring only the main-header one would decode a different image rather
/// than a slightly worse one. `p0_06` is exactly this codestream: maxshift 11
/// in the main header, 9 in the tile-part header — 9 is what the tile was
/// coded with.
///
/// The override belongs to the *tile*, so it lands on the tile's header and the
/// main header keeps its own value. Another tile of the same image, carrying no
/// RGN, would still take 11.
#[test]
fn a_tile_part_rgn_overrides_the_main_header_shift() {
    let mut header = default_header();
    header.push(seg(marker::RGN, &rgn_body(&[0], 0, 11)));
    let rgn = seg(marker::RGN, &rgn_body(&[0], 0, 9));
    let bytes = assemble_with_tile_markers(&header, &[rgn], &[1, 2]);

    let cs = parse(&bytes).expect("parse");
    assert_eq!(cs.tile_header(0).unwrap().components[0].roi_shift, 9);
    assert_eq!(
        cs.header.components[0].roi_shift, 11,
        "the tile's override must not be written back over the main header"
    );
}

/// A tile-part RGN with no main-header counterpart sets the shift from zero,
/// and it applies only to the component it names.
#[test]
fn a_tile_part_rgn_names_one_component() {
    let header = [
        seg(marker::SIZ, &siz_body(3, &[(15, 1, 1); 3])),
        seg(marker::COD, &cod_default(1)),
        seg(marker::RGN, &rgn_body(&[2], 0, 11)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ];
    let rgn = seg(marker::RGN, &rgn_body(&[1], 0, 9));
    let bytes = assemble_with_tile_markers(&header, &[rgn], &[1, 2]);

    let cs = parse(&bytes).expect("parse");
    let tile = cs.tile_header(0).expect("resolve tile 0");
    assert_eq!(tile.components[0].roi_shift, 0);
    assert_eq!(tile.components[1].roi_shift, 9, "the tile-part's RGN");
    assert_eq!(
        tile.components[2].roi_shift, 11,
        "the main header's, unopposed"
    );
}

/// A tile-part POC **appends** to the main header's, it does not replace it: a
/// tile inherits the main volumes and adds its own after them (A.6.6, matching
/// OpenJPEG's seed-then-append). Order is load-bearing — the first volume to
/// reach a packet emits it — so the resolved sequence must be `[main, tile]`.
#[test]
fn a_tile_part_poc_appends_to_the_main_header_poc() {
    // Main POC: resolutions [0, 2), LRCP. Tile POC: resolutions [0, 3), RLCP.
    let main_poc = [0u8, 0, 0, 1, 2, 1, 0];
    let header = [
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_default(1)),
        seg(marker::POC, &main_poc),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ];
    let tile_poc = seg(marker::POC, &[0u8, 0, 0, 1, 3, 1, 1]);
    let bytes = assemble_with_tile_markers(&header, &[tile_poc], &[1, 2]);

    let cs = parse(&bytes).expect("parse");
    let tile = cs.tile_header(0).expect("resolve tile 0");
    assert_eq!(tile.poc.len(), 2, "main volume then tile volume");
    assert_eq!(
        (tile.poc[0].res_end, tile.poc[0].progression),
        (2, markers::Progression::Lrcp),
        "the main header's volume comes first",
    );
    assert_eq!(
        (tile.poc[1].res_end, tile.poc[1].progression),
        (3, markers::Progression::Rlcp),
        "the tile-part's volume is appended",
    );
}

/// **A tile COD outranks a main-header COC.** This is the whole reason the main
/// header keeps its raw overrides rather than only its resolved components.
///
/// A.6.1's precedence for a component is `tile COC > tile COD > main COC > main
/// COD`. Component 0 here has a main-header COC (3 levels) and the tile carries a
/// COD (2 levels). Resolving the tile by laying its markers over the main
/// header's *resolved* components would leave the main COC's 3 levels on top —
/// the value that lost — and every subband bound, packet count and code-block in
/// the tile would be computed for a pyramid the encoder never used.
///
/// Component 1, with no COC anywhere, takes the tile COD too; a component whose
/// COC is in the *tile* header would outrank it, which the next test covers.
#[test]
fn a_tile_cod_outranks_a_main_header_coc() {
    let header = [
        seg(marker::SIZ, &siz_body(2, &[(15, 1, 1); 2])),
        // Main COD: 5 levels for everything.
        seg(marker::COD, &cod_body(0, 0, 1, 0, 5, 4, 4, 0, 1)),
        // Main COC: component 0 alone drops to 3 levels.
        seg(marker::COC, &coc_body(&[0], 0, 3, 4, 4, 0, 1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ];
    // Tile COD: 2 levels, for every component of this tile.
    let tile_cod = seg(marker::COD, &cod_body(0, 0, 1, 0, 2, 4, 4, 0, 1));
    let bytes = assemble_with_tile_markers(&header, &[tile_cod], &[1, 2]);

    let cs = parse(&bytes).expect("parse");
    let tile = cs.tile_header(0).expect("resolve tile 0");
    assert_eq!(
        tile.components[0].coding.decomposition_levels, 2,
        "the tile COD must beat the main-header COC, not lose to it"
    );
    assert_eq!(tile.components[1].coding.decomposition_levels, 2);

    // The main header keeps its own resolution: another tile carrying no COD
    // would still see the COC's 3 levels.
    assert_eq!(cs.header.components[0].coding.decomposition_levels, 3);
    assert_eq!(cs.header.components[1].coding.decomposition_levels, 5);
}

/// The rest of A.6.1's ladder: a tile COC beats the tile COD, and a component
/// named by neither tile marker falls back through the main COC to the main COD.
/// The same order governs QCC over QCD.
#[test]
fn tile_overrides_resolve_in_the_standard_s_precedence_order() {
    let header = [
        seg(marker::SIZ, &siz_body(3, &[(15, 1, 1); 3])),
        seg(marker::COD, &cod_body(0, 0, 1, 0, 5, 4, 4, 0, 1)),
        // Component 2's only override lives in the main header.
        seg(marker::COC, &coc_body(&[2], 0, 4, 4, 4, 0, 1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ];
    let tile_markers = [
        // Tile COD: 2 levels, the tile's new default.
        seg(marker::COD, &cod_body(0, 0, 1, 0, 2, 4, 4, 0, 1)),
        // Tile COC: component 0 alone goes to 1 level, outranking the tile COD.
        seg(marker::COC, &coc_body(&[0], 0, 1, 4, 4, 0, 1)),
        // Tile QCD: 5 guard bits for the tile.
        seg(marker::QCD, &qcd_none(5, &[8; 16])),
        // Tile QCC: component 1 alone goes to 6, outranking the tile QCD.
        seg(marker::QCC, &{
            let mut b = vec![1u8];
            b.extend_from_slice(&qcd_none(6, &[8; 16]));
            b
        }),
    ];
    let bytes = assemble_with_tile_markers(&header, &tile_markers, &[1, 2]);

    let cs = parse(&bytes).expect("parse");
    let tile = cs.tile_header(0).expect("resolve tile 0");
    let levels = |c: usize| tile.components[c].coding.decomposition_levels;
    assert_eq!(levels(0), 1, "tile COC beats tile COD");
    assert_eq!(levels(1), 2, "tile COD, with no COC anywhere");
    assert_eq!(levels(2), 2, "tile COD beats the main COC's 4");

    let guard = |c: usize| tile.components[c].quant.guard_bits;
    assert_eq!(guard(0), 5, "tile QCD");
    assert_eq!(guard(1), 6, "tile QCC beats tile QCD");
    assert_eq!(guard(2), 5, "tile QCD");
}

/// A component named by no tile marker at all still resolves through the main
/// header's own ladder, unchanged.
#[test]
fn a_tile_with_no_overrides_takes_the_main_header_s_resolution() {
    let header = [
        seg(marker::SIZ, &siz_body(2, &[(15, 1, 1); 2])),
        seg(marker::COD, &cod_body(0, 0, 1, 0, 5, 4, 4, 0, 1)),
        seg(marker::COC, &coc_body(&[0], 0, 3, 4, 4, 0, 1)),
        seg(marker::QCD, &qcd_none(2, &[8; 16])),
    ];
    let bytes = assemble(&header, &sot_seg(0, 0, 0, 1), &[1, 2], true);

    let cs = parse(&bytes).expect("parse");
    assert_eq!(cs.tile_header(0).unwrap().components, cs.header.components);
}

/// The coding-parameter overrides belong to the *first* tile-part of a tile
/// (A.4.2): they say how the tile's packets are read, and a later part cannot
/// restate what the earlier parts were already decoded under. A conformant
/// encoder never emits one, so a codestream that does is malformed.
#[test]
fn a_coding_override_in_a_later_tile_part_is_codestream() {
    let first_data = [7, 7, 7];
    let tile_cod = seg(marker::COD, &cod_body(0, 0, 1, 0, 2, 4, 4, 0, 1));

    let mut bytes = be16(marker::SOC).to_vec();
    for part in default_header() {
        bytes.extend_from_slice(&part);
    }
    bytes.extend_from_slice(&sot_seg(0, psot_for(&first_data), 0, 2));
    bytes.extend_from_slice(&be16(marker::SOD));
    bytes.extend_from_slice(&first_data);
    // The second tile-part carries a COD, which only the first one may.
    bytes.extend_from_slice(&sot_seg(0, 0, 1, 2));
    bytes.extend_from_slice(&tile_cod);
    bytes.extend_from_slice(&be16(marker::SOD));
    bytes.extend_from_slice(&[8, 8]);
    bytes.extend_from_slice(&be16(marker::EOC));

    assert!(matches!(perr(&bytes), Error::Codestream(_)));
}

/// A tile COD/QCD pair must hold every invariant the main header's does: the
/// quantization table has to cover the levels the coding style declares. A tile
/// COD that deepens the pyramid past what the main QCD's table describes is a
/// malformed marker, caught when the tile resolves rather than at decode time.
#[test]
fn a_tile_cod_outrunning_the_quantization_table_is_marker() {
    let header = [
        seg(marker::SIZ, &one_component()),
        seg(marker::COD, &cod_body(0, 0, 1, 0, 2, 4, 4, 0, 1)),
        // Seven entries: enough for 2 levels (3*2 + 1), not for 5.
        seg(marker::QCD, &qcd_none(2, &[8; 7])),
    ];
    let tile_cod = seg(marker::COD, &cod_body(0, 0, 1, 0, 5, 4, 4, 0, 1));
    let bytes = assemble_with_tile_markers(&header, &[tile_cod], &[1, 2]);
    assert!(matches!(perr(&bytes), Error::Marker(_)));
}

/// A.6.3 allows one RGN per component per header. Two in the same tile-part
/// header for one component is a malformed codestream, exactly as it is in the
/// main header; guessing which wins would decode an image the encoder never
/// described.
#[test]
fn a_second_tile_part_rgn_for_one_component_is_a_codestream_error() {
    let rgn = seg(marker::RGN, &rgn_body(&[0], 0, 9));
    let bytes = assemble_with_tile_markers(&default_header(), &[rgn.clone(), rgn], &[1, 2]);
    assert!(matches!(perr(&bytes), Error::Codestream(_)));
}

/// The tile-part form goes through the same `decode_rgn` as the main header's,
/// so a non-maxshift style rejects as unsupported there too.
#[test]
fn a_tile_part_rgn_with_a_part_2_style_is_unsupported() {
    let rgn = seg(marker::RGN, &rgn_body(&[0], 1, 9));
    let bytes = assemble_with_tile_markers(&default_header(), &[rgn], &[1, 2]);
    assert!(matches!(perr(&bytes), Error::Unsupported(_)));
}

// --- TLM / PLM / PLT: length markers (issue #72) -----------------------------

use super::markers::TlmEntry;

/// TLM body: `Ztlm`, `Stlm`, then the entry bytes verbatim.
fn tlm_body(stlm: u8, entries: &[u8]) -> Vec<u8> {
    let mut b = vec![0, stlm];
    b.extend_from_slice(entries);
    b
}

/// Length markers are informational, so the parse records TLM's entries and
/// nothing more. `Stlm = 0x60` is `ST = 2` (two-byte tile indices) and
/// `SP = 1` (four-byte lengths) — the shape `p0_05` carries.
#[test]
fn tlm_records_the_tile_part_lengths() {
    let mut segments = default_header();
    segments.push(seg(
        marker::TLM,
        &tlm_body(0x60, &[0, 0, 0x00, 0x13, 0xFF, 0x4C]),
    ));
    let header = parsed(&segments);
    assert_eq!(
        header.tlm,
        vec![TlmEntry {
            tile_index: 0,
            length: 0x0013_FF4C,
        }]
    );
}

/// `ST = 0` omits the tile index — entry order implies it — and `SP = 0`
/// carries two-byte lengths. Entries accumulate across TLM markers in order.
#[test]
fn tlm_indices_can_be_implied_and_lengths_two_bytes() {
    let mut segments = default_header();
    segments.push(seg(marker::TLM, &tlm_body(0x00, &[0x12, 0x34])));
    let header = parsed(&segments);
    assert_eq!(
        header.tlm,
        vec![TlmEntry {
            tile_index: 0,
            length: 0x1234,
        }]
    );
}

/// Two TLM markers concatenate: one list of tile-parts, in codestream order.
#[test]
fn tlm_entries_accumulate_across_markers() {
    let mut segments = default_header();
    // ST = 1 (one-byte explicit index), SP = 0: entries `index, len16`.
    segments.push(seg(marker::TLM, &tlm_body(0x10, &[0, 0x00, 0x14])));
    segments.push(seg(marker::TLM, &tlm_body(0x10, &[0, 0x00, 0x2C])));
    let header = parsed(&segments);
    assert_eq!(header.tlm.len(), 2);
    assert_eq!(header.tlm[0].length, 0x14);
    assert_eq!(header.tlm[1].length, 0x2C);
}

/// `ST = 3` is undefined (Table A-33). OpenJPEG warns and ignores the TLM
/// chain; as with `Scoc`, rejecting the illegal encoding is stricter.
#[test]
fn tlm_st_3_is_a_marker_error() {
    let mut segments = default_header();
    segments.push(seg(marker::TLM, &tlm_body(0x30, &[0, 0, 0, 0])));
    let bytes = codestream(&segments);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

/// `Stlm` defines only bits 4–6; a reserved bit set is an illegal encoding.
#[test]
fn tlm_reserved_stlm_bits_are_a_marker_error() {
    for stlm in [0x01u8, 0x08, 0x80] {
        let mut segments = default_header();
        segments.push(seg(marker::TLM, &tlm_body(stlm, &[0, 0x10])));
        let bytes = codestream(&segments);
        assert!(matches!(err(&bytes), Error::Marker(_)), "Stlm {stlm:#04X}");
    }
}

/// A body that does not divide into whole entries is malformed.
#[test]
fn tlm_misaligned_body_is_a_codestream_error() {
    let mut segments = default_header();
    segments.push(seg(marker::TLM, &tlm_body(0x60, &[0, 0, 0, 0x13])));
    let bytes = codestream(&segments);
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

/// The subset's grid is a single tile — enforced at SIZ — so an entry naming
/// any other tile points at nothing. OpenJPEG makes the same range check
/// against its tile grid.
#[test]
fn tlm_naming_a_missing_tile_is_a_marker_error() {
    let mut segments = default_header();
    segments.push(seg(marker::TLM, &tlm_body(0x10, &[1, 0x00, 0x14])));
    let bytes = codestream(&segments);
    assert!(matches!(err(&bytes), Error::Marker(_)));
}

/// PLM is validated for its `Zplm` byte and otherwise skipped, exactly as
/// OpenJPEG skips it: its packet-length chains may split across PLM markers
/// mid-entry, so nothing else can be checked per marker. Synthetic fixture —
/// no conformance entry carries PLM (`tests/conformance_corpus.rs` pins that
/// gap): `Zplm = 0`, then one `Nplm = 2` chain of a two-byte packet length.
#[test]
fn plm_parses_and_is_skipped() {
    let mut segments = default_header();
    segments.push(seg(marker::PLM, &[0, 2, 0x85, 0x02]));
    let header = parsed(&segments);
    assert_eq!(header.tlm, vec![]);
}

/// A PLM with no `Zplm` byte at all is malformed.
#[test]
fn plm_without_zplm_is_a_codestream_error() {
    let mut segments = default_header();
    segments.push(seg(marker::PLM, &[]));
    let bytes = codestream(&segments);
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

/// PLT parses in the tile-part header: `Zplt`, then packet lengths as
/// base-128 byte chains (high bit = another byte follows). The lengths are
/// discarded; the parse only requires whole chains.
#[test]
fn plt_parses_in_the_tile_part_header() {
    // Two packet lengths: 0x282 (two bytes, 0x85 then 0x02) and 0x10 (one).
    let plt = seg(marker::PLT, &[0, 0x85, 0x02, 0x10]);
    let bytes = assemble_with_tile_markers(&default_header(), &[plt], &[1, 2]);
    parse(&bytes).expect("PLT is informational and parses");
}

/// A PLT whose last byte still has the continuation bit set ends mid-entry —
/// malformed, and OpenJPEG's `opj_j2k_read_plt` rejects it the same way.
#[test]
fn plt_ending_mid_entry_is_a_codestream_error() {
    let plt = seg(marker::PLT, &[0, 0x85]);
    let bytes = assemble_with_tile_markers(&default_header(), &[plt], &[1, 2]);
    assert!(matches!(perr(&bytes), Error::Codestream(_)));
}

/// PLT belongs to the tile-part header alone (A.7.3); in the main header it is
/// a malformed codestream, not a missing feature.
#[test]
fn plt_in_the_main_header_is_a_codestream_error() {
    let mut segments = default_header();
    segments.push(seg(marker::PLT, &[0, 0x10]));
    let bytes = codestream(&segments);
    assert!(matches!(err(&bytes), Error::Codestream(_)));
}

/// PLM belongs to the main header alone (A.7.2); in a tile-part header it is
/// a malformed codestream.
#[test]
fn plm_in_the_tile_part_header_is_a_codestream_error() {
    let plm = seg(marker::PLM, &[0]);
    let bytes = assemble_with_tile_markers(&default_header(), &[plm], &[1, 2]);
    assert!(matches!(perr(&bytes), Error::Codestream(_)));
}

/// `p0_05`'s TLM against the codestream itself: the single entry must name
/// tile 0 with the exact `Psot` its SOT declares (`opj_dump` does not print
/// TLM, so the codestream is the reference here — Ptlm and Psot describe the
/// same span, A.4.2 and A.7.1). Returns early if the corpus is absent: it is
/// `exclude`d from the packaged crate.
#[test]
fn tlm_of_p0_05_matches_its_tile_part_length() {
    let path = corpus_dir().join("codestreams/p0_05.j2k");
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };

    let cs = parse(&bytes).expect("p0_05 parses");
    assert_eq!(cs.header.tlm.len(), 1, "one tile-part, one entry");
    assert_eq!(cs.header.tlm[0].tile_index, 0);

    // Psot: bytes 6..10 of the SOT segment (marker, Lsot, Isot precede it).
    let sot = bytes
        .windows(2)
        .position(|w| w == be16(marker::SOT))
        .expect("SOT");
    let psot = u32::from_be_bytes(bytes[sot + 6..sot + 10].try_into().unwrap());
    assert_eq!(cs.header.tlm[0].length, psot);
}
