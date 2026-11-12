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

/// One sample column on x in [1, 2), `2 * tiles` rows in tiles of two, in
/// `progression` (0 LRCP, 1 RLCP) with 65535 layers, 32 wavelet levels and
/// `comps` components. Every resolution but the top one is empty in x, so it
/// holds no packet; each tile is 65535 x `comps` empty packets (0x00). The
/// layer-resolution loops take a step at each of 65535 x 33 points per tile,
/// and one for each component with a precinct there.
fn empty_resolutions(progression: u8, comps: usize, tiles: u32) -> Vec<u8> {
    let mut cod = vec![0, progression];
    cod.extend_from_slice(&u16::MAX.to_be_bytes());
    cod.extend_from_slice(&[0, 32, 0, 0, 0, 1]);
    let mut qcd = vec![0x40];
    qcd.extend(std::iter::repeat_n(0x40, 3 * 32 + 1));
    let body = vec![0; usize::from(u16::MAX) * comps];
    codestream(1, 1, 2 * tiles, 2, &vec![(1, 1); comps], &cod, &qcd, &body)
}

/// One sample column on x in [1, 2), 2^17 rows, one tile, in `progression`
/// (2 RPCL, 3 PCRL, 4 CPRL), one layer, 32 wavelet levels and 16
/// components. Every resolution but the top one is empty in x. Component 0's
/// top precinct is 2^15 x 2, so it has 2^16 precincts and sets the sweep's
/// step to 2 rows; the other 15 have 2^15 x 2^15, four each. Every packet is
/// empty (0x00).
fn thin_column(progression: u8) -> Vec<u8> {
    const COMPS: u16 = 16;
    const HEIGHT: u32 = 1 << 17;
    let mut siz = 0u16.to_be_bytes().to_vec();
    for v in [2, HEIGHT, 1, 0, 2, HEIGHT, 0, 0] {
        siz.extend_from_slice(&u32::to_be_bytes(v));
    }
    siz.extend_from_slice(&COMPS.to_be_bytes());
    for _ in 0..COMPS {
        siz.extend_from_slice(&[7, 1, 1]);
    }
    // User precincts; one layer; 32 levels, 64x64 code-blocks, 5/3; then
    // (PPx, PPy) = (15, 15) at every resolution, and for component 0 (15, 1)
    // at the top one.
    let mut cod = vec![1, progression, 0, 1, 0, 32, 4, 4, 0, 1];
    cod.extend([0xFF; 33]);
    let mut coc = vec![0, 1, 32, 4, 4, 0, 1];
    coc.extend([0xFF; 32]);
    coc.push(0x1F);
    let mut qcd = vec![0x40];
    qcd.extend(std::iter::repeat_n(0x40, 3 * 32 + 1));
    let packets = HEIGHT / 2 + u32::from(COMPS - 1) * HEIGHT.div_ceil(1 << 15);
    let mut cs = vec![0xFF, 0x4F];
    for (code, body) in [(0xFF51, siz), (0xFF52, cod), (0xFF53, coc), (0xFF5C, qcd)] {
        cs.extend(seg(code, &body));
    }
    cs.extend_from_slice(&[0xFF, 0x90, 0, 10, 0, 0]);
    cs.extend_from_slice(&(12 + 2 + packets).to_be_bytes());
    cs.extend_from_slice(&[0, 1, 0xFF, 0x93]);
    cs.resize(cs.len() + packets as usize, 0);
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
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
/// and 1000 layers, and 65535 layers over 2^14 and 2^19 code-blocks. Each
/// cost a visit per code-block per layer, up to 3.4 x 10^10 visits, before
/// the decode stepped over them; now each layer is one step. A row at a time
/// would be 4.7 x 10^7 steps for the last, past its bound. OpenJPEG 2.5.0
/// decodes all three to a flat 128.
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
    assert_flat(
        "t2bomb 2896 x 65535",
        &t2bomb(2896, 2896, 2896, 65535),
        2896,
        2896,
    );
}

/// 31 identical POC volumes over 4 precincts and 1024 layers of empty
/// packets: each packet is read once and met 30 times more, skipped without
/// a visit each time. OpenJPEG 2.5.0 decodes it to a flat 128.
#[test]
fn repeated_poc_volumes_decode() {
    let name = "pocdup_4x4_1024_31.j2k";
    let bytes = std::fs::read(dir().join(name)).unwrap();
    assert_flat(name, &bytes, 4, 4);
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

/// `setskip` from the generator: one 38608 x 1 tile in RPCL, no wavelet
/// levels, 1 x 1 precincts, `layers` layers and `volumes` identical POC
/// volumes. One component at `XRsiz` 254 sets the sweep's step; 64 more at
/// 255 have 152 precincts each, but the sweep meets only their first, so
/// the volumes reach 9880 packets a layer and read 216. Every packet read is
/// empty (0x00).
fn setskip(layers: u16, volumes: usize) -> Vec<u8> {
    let mut comps = vec![(254, 1)];
    comps.extend([(255, 1); 64]);
    let mut cod = vec![1, 2];
    cod.extend_from_slice(&layers.to_be_bytes());
    cod.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0x00]);
    let body = vec![0; 216 * usize::from(layers)];
    let mut cs = codestream(
        0,
        152 * 254,
        1,
        152 * 254,
        &comps,
        &cod,
        &[0x40, 0x40],
        &body,
    );
    // RSpoc, CSpoc, LYEpoc, REpoc, CEpoc, Ppoc, before the first SOT.
    let mut volume = vec![0, 0];
    volume.extend_from_slice(&layers.to_be_bytes());
    volume.extend_from_slice(&[1, 65, 2]);
    let sot = cs.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    cs.splice(sot..sot, seg(0xFF5F, &volume.repeat(volumes)));
    cs
}

/// More packets reached than the bitmap may hold, in a positional order, so
/// the packets read are kept in a set, and each one skipped from it is
/// charged 12 steps: 2048 layers in 31 volumes skip 1.3 x 10^7 packets,
/// past the bound for their 442 KB. With one volume, nothing is skipped and
/// it decodes. OpenJPEG 2.5.0 decodes both to a flat 128.
#[test]
fn skips_from_the_poc_set_are_bounded() {
    assert_limit("setskip 31 volumes", &setskip(2048, 31));
    let image = decode(&setskip(2048, 1)).unwrap();
    assert_eq!(image.components.len(), 65);
    assert!(image.components[0].samples.iter().all(|&s| s == 128));
}

/// A JP2 box: the length, the type, the payload.
fn jp2_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(8 + payload.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// `cs` in a JP2 file as `jp2_psot0` in the generator writes it: its one
/// tile-part's `Psot` set to 0 and its EOC dropped, in a `jp2c` box that ends
/// where the packets end, followed here by an `xml ` box of `xml`.
fn jp2_psot0(cs: &[u8], xml: &[u8]) -> Vec<u8> {
    let word = |at: usize| u32::from_be_bytes(cs[at..at + 4].try_into().unwrap());
    let mut ihdr = (word(12) - word(20)).to_be_bytes().to_vec();
    ihdr.extend_from_slice(&(word(8) - word(16)).to_be_bytes());
    ihdr.extend_from_slice(&cs[40..42]);
    ihdr.extend_from_slice(&[7, 7, 0, 0]);
    let mut colr = vec![1, 0, 0];
    colr.extend_from_slice(&17u32.to_be_bytes());
    let mut jp2h = jp2_box(b"ihdr", &ihdr);
    jp2h.extend(jp2_box(b"colr", &colr));
    let sot = cs.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    let mut body = cs[..sot + 6].to_vec();
    body.extend_from_slice(&[0; 4]);
    body.extend_from_slice(&cs[sot + 10..cs.len() - 2]);
    let mut out = jp2_box(b"jP  ", b"\x0d\x0a\x87\x0a");
    out.extend(jp2_box(b"ftyp", b"jp2 \0\0\0\0jp2 "));
    out.extend(jp2_box(b"jp2h", &jp2h));
    out.extend(jp2_box(b"jp2c", &body));
    out.extend(jp2_box(b"xml ", xml));
    out
}

/// `read_header` walks a `Psot = 0` tile's packets to see whether its
/// codestream ends at its box's end, holding a POC sequence's packets in a
/// bitmap wherever the decode does. `setskip(1024, 31)` reaches 1.0e7
/// packets, under the decode's 2^24 visits: the codestream ends at its box,
/// and the `xml ` box after it is reported by both calls. OpenJPEG 2.5.0
/// decodes it to a flat 128. `setskip(2048, 2)` reaches 2.0e7, so the decode
/// would keep a set, and the walk gives up: the codestream runs to the end
/// of the file, the `xml ` box is not reported, and the decode refuses its
/// bytes as left after the last packet, as for the files of
/// `a_psot_zero_tile_too_costly_to_walk_reads_to_the_end` in
/// `tests/psot_zero.rs`.
#[test]
fn a_psot_zero_poc_sequence_ends_at_its_box_within_the_decodes_bitmap() {
    let file = jp2_psot0(&setskip(1024, 31), b"<a/>");
    let image = decode(&file).unwrap();
    assert_eq!(image.components.len(), 65);
    assert!(
        image
            .components
            .iter()
            .all(|c| c.samples.iter().all(|&s| s == 128))
    );
    let xml = |metadata: &rust_j2k::Jp2Metadata| {
        let payloads: Vec<Vec<u8>> = metadata.xml.iter().map(|x| x.payload.clone()).collect();
        payloads
    };
    assert_eq!(xml(image.jp2.as_ref().unwrap()), [b"<a/>".to_vec()]);
    let header = read_header(&file).unwrap();
    assert_eq!(xml(header.jp2.as_ref().unwrap()), [b"<a/>".to_vec()]);

    let file = jp2_psot0(&setskip(2048, 2), b"<a/>");
    assert!(xml(read_header(&file).unwrap().jp2.as_ref().unwrap()).is_empty());
    match decode(&file) {
        Err(Error::Codestream(m)) => assert!(m.contains("left after the last packet"), "{m}"),
        other => panic!("expected a codestream error, got {other:?}"),
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

/// The layer-resolution orders check only the components that hold a
/// precinct at a resolution, so 32 empty resolutions of 8 components cost
/// one step per (layer, resolution), about 3 million in all for 512 KB of
/// empty packets: it decodes, to the flat 128 OpenJPEG 2.5.0 decodes, in
/// LRCP and RLCP alike. Those points are still steps: one component in 8
/// such tiles spends 1.8 x 10^7 for 512 KB, past the bound.
#[test]
fn the_layer_orders_are_bounded_over_empty_resolutions() {
    for (name, progression) in [("LRCP", 0), ("RLCP", 1)] {
        let image =
            decode(&empty_resolutions(progression, 8, 1)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(image.components.len(), 8, "{name}");
        for component in &image.components {
            assert_eq!(component.samples, [128, 128], "{name}");
        }
        assert_limit(name, &empty_resolutions(progression, 1, 8));
    }
}

/// The positional orders check only the (component, resolution) pairs that
/// hold a precinct, and RPCL sweeps no resolution where none does. On
/// `thin_column` that is 17 steps at each of 2^16 positions, 1.1 million for
/// 64 KB of empty packets, where checking all 33 resolutions of each
/// component took about 3.5 x 10^7 in RPCL and PCRL, past the bound. Each
/// order decodes to the
/// flat 128 OpenJPEG 2.5.0 decodes, 16 components of 1 x 2^17.
#[test]
fn the_positional_orders_pass_over_resolutions_with_no_precinct() {
    for (name, progression) in [("RPCL", 2), ("PCRL", 3), ("CPRL", 4)] {
        let image = decode(&thin_column(progression)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(image.components.len(), 16, "{name}");
        for component in &image.components {
            assert_eq!((component.width, component.height), (1, 1 << 17), "{name}");
            assert!(component.samples.iter().all(|&s| s == 128), "{name}");
        }
    }
}

/// Within the bound the same shape decodes: `t2skip` on 512 x 256
/// code-blocks with 30000 layers spends about 1.5 x 10^7 steps, and the image
/// is the flat 128 OpenJPEG 2.5.0 decodes.
#[test]
fn a_walk_within_the_work_bound_decodes() {
    assert_flat("t2skip 30000", &t2skip(2048, 1024, 2048, 30000), 2048, 1024);
}
