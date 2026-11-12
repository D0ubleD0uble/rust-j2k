//! The decode's tier-2 work bound: a codestream whose packets make tier-2
//! walk far more than its bytes is refused with [`Error::Limit`], in about the
//! time a decode of that many bytes takes, instead of running for seconds or
//! minutes. OpenJPEG 2.5.0 has no such bound and walks them all; see
//! `tests/fixtures/work_bound/README.md` and `docs/correctness.md`.
//!
//! Packet headers that name many code-blocks and include none are not such
//! a walk: the decode steps over the code-blocks a header reads no bit for,
//! a row at a time, so those files decode, as encoder output with many
//! layers does.
//!
//! The committed files are written by `scripts/gen-work-bound-fixtures.py`.
//! The larger shapes are built here, with the same layout.

mod support;

use std::path::PathBuf;

use rust_j2k::{Error, decode, read_header};
use support::jp2::sha256_hex;

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

/// Decodes to one flat component of 128, as OpenJPEG 2.5.0 decodes it.
fn assert_flat(name: &str, bytes: &[u8], width: usize, height: usize) {
    let image = decode(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(image.components.len(), 1, "{name}");
    let samples = &image.components[0].samples;
    assert_eq!(samples.len(), width * height, "{name}");
    assert!(samples.iter().all(|&s| s == 128), "{name}: not flat 128");
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
/// Each tile's data is `body`.
#[allow(clippy::too_many_arguments)]
fn codestream(
    x0: u32,
    width: u32,
    height: u32,
    tile: u32,
    comps: &[(u8, u8)],
    cod: &[u8],
    qcd: &[u8],
    body: &[u8],
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
        let psot = 12 + 2 + body.len() as u32;
        cs.extend_from_slice(&[0xFF, 0x90, 0, 10]);
        cs.extend_from_slice(&(index as u16).to_be_bytes());
        cs.extend_from_slice(&psot.to_be_bytes());
        cs.extend_from_slice(&[0, 1, 0xFF, 0x93]);
        cs.extend_from_slice(body);
    }
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

/// One component in `tile` x `tile` tiles, each with no wavelet levels,
/// 4x4 code-blocks in one precinct and `layers` LRCP layers, whose packets
/// are `first` and then 0x80 (present, a bit of 0 for each tag-tree node
/// read). Each code-block is named in every packet's header, and none is
/// included.
fn layers_of_nothing(width: u32, height: u32, tile: u32, layers: u16, first: u8) -> Vec<u8> {
    let mut cod = vec![0, 0];
    cod.extend_from_slice(&layers.to_be_bytes());
    cod.extend_from_slice(&[0, 0, 0, 0, 0, 1]);
    let mut body = vec![first];
    body.resize(usize::from(layers), 0x80);
    codestream(
        0,
        width,
        height,
        tile,
        &[(1, 1)],
        &cod,
        &[0x40, 0x40],
        &body,
    )
}

/// `t2bomb` from the generator: every packet 0x80, so the inclusion tree's
/// root is past each layer, and the decode steps over all of the tile's
/// code-blocks at once.
fn t2bomb(width: u32, height: u32, tile: u32, layers: u16) -> Vec<u8> {
    layers_of_nothing(width, height, tile, layers, 0x80)
}

/// `t2skip` from the generator: the first packet 0xC0, so the root resolves
/// and each of its children is read past each layer instead. On a tile of 512
/// x 512 (128 x 128 code-blocks) every packet then takes 256 steps, two for
/// each row of code-blocks, for one byte; on 2048 x 1024, 512 steps.
fn t2skip(width: u32, height: u32, tile: u32, layers: u16) -> Vec<u8> {
    layers_of_nothing(width, height, tile, layers, 0xC0)
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
    let body = vec![0; usize::from(u16::MAX) * comps];
    codestream(1, 1, 2, 2, &vec![(1, 1); comps], &cod, &qcd, &body)
}

/// The committed shapes past the bound: RPCL and PCRL sweeps that check 64
/// components at each of 512^2 positions for about 2000 empty packets, a
/// one-column CPRL file whose sweep visits 2^23 positions for 512 packets,
/// once and repeated by 31 POC volumes, and `t2skip` on 512 x 256 code-blocks
/// with 40000 layers, about 2 x 10^7 steps for 40 KB of packets. Each
/// decodes to a flat image with an unbounded walk, as OpenJPEG 2.5.0 decodes
/// it.
#[test]
fn the_committed_shapes_are_refused_at_the_work_bound() {
    for name in [
        "sweep_rpcl_1024_64.j2k",
        "sweep_pcrl_1024_64.j2k",
        "cprl_tall.j2k",
        "cprl_tall_poc31.j2k",
        "t2skip_2048x1024_40000.j2k",
    ] {
        let bytes = std::fs::read(dir().join(name)).unwrap();
        assert_limit(name, &bytes);
    }
}

/// Packets that name every code-block and include none, under a root the
/// header puts past each layer: the committed 1 KB file of 2^17 code-blocks
/// and 1000 layers, and 65535 layers over 2^14 code-blocks. Each cost a
/// visit per code-block per layer, 1.3 x 10^8 and 10^9 visits, before the
/// decode stepped over them; now each layer is one step. OpenJPEG 2.5.0
/// decodes both to a flat 128.
#[test]
fn packets_that_include_nothing_decode() {
    let name = "t2bomb_2048x1024_1000.j2k";
    let bytes = std::fs::read(dir().join(name)).unwrap();
    assert_flat(name, &bytes, 2048, 1024);
    assert_flat(
        "t2bomb 512 x 65535",
        &t2bomb(512, 512, 512, 65535),
        512,
        512,
    );
}

/// `opj_compress` output with 4x4 code-blocks and 64 or 100 quality layers on
/// a nearly flat 2048^2 image. OpenJPEG's encoder sets every packet's
/// "present" bit, so each layer's header names every one of its 262144
/// code-blocks; both files were refused before the decode stepped over the
/// ones read past the layer. Each decodes to the samples OpenJPEG 2.5.0
/// decodes (the SHA-256 of its 8-bit output).
#[test]
fn encoder_output_with_many_layers_decodes() {
    for (name, sha256) in [
        (
            "opj_patch2048_b4_q64.j2k",
            "80c5ce50a9d9924b4429898da04cf9d3e6ba7b8d8775521a84bfd030bf76d529",
        ),
        (
            "opj_patch2048_b4_q100.j2k",
            "3f2603a0a9cdd97c064f5f865d96adeea056f434a135756395a9763731a20251",
        ),
    ] {
        let bytes = std::fs::read(dir().join(name)).unwrap();
        let image = decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let component = &image.components[0];
        assert_eq!((component.width, component.height), (2048, 2048), "{name}");
        let samples: Vec<u8> = component.samples.iter().map(|&s| s as u8).collect();
        assert_eq!(sha256_hex(&samples), sha256, "{name}");
    }
}

/// The bound is one budget for the whole decode, not one per tile: 16 tiles
/// of 128 x 128 code-blocks and 5000 layers spend about 2 x 10^7 steps in
/// all, past the bound for their 80 KB of packets, while each tile alone
/// spends 1.3 million.
#[test]
fn the_tiles_share_one_work_bound() {
    assert_limit("16 tiles", &t2skip(2048, 2048, 512, 5000));
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

/// Within the bound the same shape decodes: `t2skip` on 512 x 256
/// code-blocks with 30000 layers spends about 1.5 x 10^7 steps, and the image
/// is the flat 128 OpenJPEG 2.5.0 decodes.
#[test]
fn a_walk_within_the_work_bound_decodes() {
    assert_flat("t2skip 30000", &t2skip(2048, 1024, 2048, 30000), 2048, 1024);
}
