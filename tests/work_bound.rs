//! The decode's tier-2 work bound: a codestream whose packets make tier-2
//! walk far more than its bytes is refused with [`Error::Limit`], in about the
//! time a decode of that many bytes takes, instead of running for seconds or
//! minutes. OpenJPEG 2.5.0 has no such bound and walks them all; see
//! `tests/fixtures/work_bound/README.md` and `docs/correctness.md`.
//!
//! The committed files are written by `scripts/gen-work-bound-fixtures.py`.
//! The larger shapes are built here, with the same layout.

use std::path::PathBuf;

use rust_j2k::{Error, decode, read_header};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/work_bound")
}

fn assert_limit(name: &str, bytes: &[u8]) {
    match decode(bytes) {
        Err(Error::Limit(m)) => assert!(m.contains("work bound"), "{name}: {m}"),
        Err(e) => panic!("{name}: expected the work bound, got {e}"),
        Ok(_) => panic!("{name}: expected the work bound, got an image"),
    }
    // The header does not walk a raw codestream's packets.
    read_header(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
}

/// A marker segment: the code, the length, the body.
fn seg(code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = code.to_be_bytes().to_vec();
    out.extend_from_slice(&(2 + body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// A codestream of 8-bit components, one per entry of `comps` (each its
/// `(XRsiz, YRsiz)`), on the image area `[x0, x0 + width) x [0, height)` in
/// `tile` x `tile` tiles, under the COD body `cod` and the QCD body `qcd`.
/// Each tile's data is `packets` copies of the byte `packet`.
#[allow(clippy::too_many_arguments)]
fn codestream(
    x0: u32,
    width: u32,
    height: u32,
    tile: u32,
    comps: &[(u8, u8)],
    cod: &[u8],
    qcd: &[u8],
    packet: u8,
    packets: usize,
) -> Vec<u8> {
    let mut siz = 0u16.to_be_bytes().to_vec();
    for v in [x0 + width, height, x0, 0, tile, tile, x0, 0] {
        siz.extend_from_slice(&v.to_be_bytes());
    }
    siz.extend_from_slice(&(comps.len() as u16).to_be_bytes());
    for &(dx, dy) in comps {
        siz.extend_from_slice(&[7, dx, dy]);
    }
    let mut cs = vec![0xFF, 0x4F];
    cs.extend(seg(0xFF51, &siz));
    cs.extend(seg(0xFF52, cod));
    cs.extend(seg(0xFF5C, qcd));
    let tiles = width.div_ceil(tile) * height.div_ceil(tile);
    for index in 0..tiles {
        let psot = 12 + 2 + packets as u32;
        cs.extend_from_slice(&[0xFF, 0x90, 0, 10]);
        cs.extend_from_slice(&(index as u16).to_be_bytes());
        cs.extend_from_slice(&psot.to_be_bytes());
        cs.extend_from_slice(&[0, 1, 0xFF, 0x93]);
        cs.extend(std::iter::repeat_n(packet, packets));
    }
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

/// `t2bomb` from the generator, tiled: a `width` x `height` image in `tile` x
/// `tile` tiles, each with no wavelet levels, 4x4 code-blocks in one precinct
/// and `layers` LRCP layers, and each tile's packets the byte 0x80: present,
/// nothing included, every code-block of the tile visited.
fn t2bomb(width: u32, height: u32, tile: u32, layers: u16) -> Vec<u8> {
    let mut cod = vec![0, 0];
    cod.extend_from_slice(&layers.to_be_bytes());
    cod.extend_from_slice(&[0, 0, 0, 0, 0, 1]);
    let packets = usize::from(layers);
    codestream(
        0,
        width,
        height,
        tile,
        &[(1, 1)],
        &cod,
        &[0x40, 0x40],
        0x80,
        packets,
    )
}

/// One sample column on x in [1, 2), two rows, in `progression` (0 LRCP, 1
/// RLCP) with 65535 layers, 32 wavelet levels and `comps` components. Every
/// resolution but the top one is empty in x, so it holds no packet, yet the
/// layer-resolution loops still check each component at each of them: 65535
/// x 33 x (1 + `comps`) visits for 65535 x `comps` empty packets (0x00).
fn empty_resolutions(progression: u8, comps: usize) -> Vec<u8> {
    let mut cod = vec![0, progression];
    cod.extend_from_slice(&u16::MAX.to_be_bytes());
    cod.extend_from_slice(&[0, 32, 0, 0, 0, 1]);
    let mut qcd = vec![0x40];
    qcd.extend(std::iter::repeat_n(0x40, 3 * 32 + 1));
    let packets = usize::from(u16::MAX) * comps;
    codestream(1, 1, 2, 2, &vec![(1, 1); comps], &cod, &qcd, 0x00, packets)
}

/// The committed shapes: a 1 KB LRCP file of 2^17 code-blocks and 1000
/// layers, RPCL and PCRL sweeps that check 64 components at each of 512^2
/// positions for about 2000 empty packets, and a
/// one-column CPRL file whose sweep visits 2^23 positions for 512 packets,
/// once and repeated by 31 POC volumes. Each decodes to a flat image with an
/// unbounded walk, as OpenJPEG 2.5.0 decodes it.
#[test]
fn the_committed_shapes_are_refused_at_the_work_bound() {
    for name in [
        "t2bomb_2048x1024_1000.j2k",
        "sweep_rpcl_1024_64.j2k",
        "sweep_pcrl_1024_64.j2k",
        "cprl_tall.j2k",
        "cprl_tall_poc31.j2k",
    ] {
        let bytes = std::fs::read(dir().join(name)).unwrap();
        assert_limit(name, &bytes);
    }
}

/// 65535 layers over 2^14 code-blocks: about 10^9 visits for 64 KB of
/// packets. OpenJPEG 2.5.0 fails it ("segment too long"); unbounded, this
/// decoder walked it in about 26 s.
#[test]
fn the_most_layers_over_many_code_blocks_are_refused() {
    assert_limit("t2bomb 512 x 65535", &t2bomb(512, 512, 512, 65535));
}

/// The bound is one budget for the whole decode, not one per tile: 64 tiles
/// of 256 code-blocks and 2000 layers spend about 33 million visits in all,
/// past the bound for their 128 KB of packets, while each tile alone spends
/// half a million.
#[test]
fn the_tiles_share_one_work_bound() {
    assert_limit("64 tiles", &t2bomb(512, 512, 64, 2000));
}

/// The layer-resolution orders check every component at every resolution of
/// every layer, packets or not: over 32 empty resolutions and 8 components
/// that is about 19 million visits for 512 KB of empty packets, refused in
/// LRCP and RLCP alike.
#[test]
fn the_layer_orders_are_bounded_over_empty_resolutions() {
    assert_limit("LRCP", &empty_resolutions(0, 8));
    assert_limit("RLCP", &empty_resolutions(1, 8));
}

/// Within the bound the same shape decodes: 56 code-blocks and 19000 layers
/// spend about 1.1 million visits, and the image is the flat 128 OpenJPEG
/// 2.5.0 decodes.
#[test]
fn a_walk_within_the_work_bound_decodes() {
    let image = decode(&t2bomb(32, 28, 32, 19000)).unwrap();
    assert_eq!(image.components[0].samples, vec![128; 32 * 28]);
}
