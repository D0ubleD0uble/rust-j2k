//! The sample limit, [`DecodeOptions::sample_limit`]: how many samples a
//! decoded image may hold. The default is 2^28 samples, counted at the
//! decode's resolution reduction; `WorkLimit::Raised` and
//! `WorkLimit::Unlimited` raise or lift it.
//!
//! The fixtures in `tests/fixtures/sample_limit/` are written by
//! `scripts/gen-sample-limit-fixtures.py` (see the README there).

use std::path::PathBuf;

use rust_j2k::{
    DecodeOptions, Error, WorkLimit, decode, decode_with, read_header, read_header_with,
};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sample_limit")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The flat 16385x16385 file: 268468225 samples, just over 2^28.
fn flat() -> Vec<u8> {
    fixture("flat_16385_tiled_lossless.j2k")
}

/// The flat file's main header and the first `SOT` marker, without its tile
/// data. A decode that gets past the sample limit fails on the missing data,
/// before it allocates any plane.
fn flat_header_only() -> Vec<u8> {
    let mut bytes = flat();
    let sot = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    bytes.truncate(sot + 2);
    bytes
}

/// `codestream` in a minimal JP2 file whose `ihdr` matches the flat image.
fn in_jp2(codestream: &[u8]) -> Vec<u8> {
    let bx = |kind: &[u8; 4], payload: &[u8]| {
        let mut out = (8 + payload.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    };
    let mut ihdr = 16385u32.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&16385u32.to_be_bytes());
    ihdr.extend_from_slice(&[0, 1, 7, 7, 0, 0]);
    let jp2h = [bx(b"ihdr", &ihdr), bx(b"colr", &[1, 0, 0, 0, 0, 0, 17])].concat();
    [
        vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A],
        bx(b"ftyp", b"jp2 \0\0\0\0jp2 "),
        bx(b"jp2h", &jp2h),
        bx(b"jp2c", codestream),
    ]
    .concat()
}

fn with_limit(limit: WorkLimit) -> DecodeOptions {
    DecodeOptions::default().with_sample_limit(limit)
}

/// Sample `(x, y)` of component `c` of the 6000x4000 picture, as
/// `scripts/gen-sample-limit-fixtures.py` draws it. The script checks that
/// `opj_decompress` decodes the file to exactly this picture.
fn picture(c: usize, x: i32, y: i32) -> i32 {
    [(x >> 8) * 10, (y >> 8) * 15, ((x >> 9) ^ (y >> 9)) * 30][c] & 255
}

/// A 6000x4000 RGB image is 72 M samples: refused by the old 2^26 limit,
/// decoded at the default, and bit-exact against `opj_decompress`.
#[test]
fn a_6000x4000_rgb_image_decodes_at_defaults_and_matches_openjpeg() {
    let image = decode(&fixture("rgb_6000x4000.jp2")).expect("decodes at defaults");
    assert_eq!((image.width, image.height), (6000, 4000));
    assert_eq!(image.components.len(), 3);
    for (c, component) in image.components.iter().enumerate() {
        assert_eq!((component.width, component.height), (6000, 4000));
        assert_eq!((component.bit_depth, component.signed), (8, false));
        let wrong = (0..4000)
            .flat_map(|y| (0..6000).map(move |x| (x, y)))
            .zip(&component.samples)
            .filter(|&((x, y), &s)| s != picture(c, x, y))
            .count();
        assert_eq!(wrong, 0, "plane {c}: {wrong} samples differ from OpenJPEG");
    }
}

/// Over the default limit, the decode and the header read are refused with
/// [`Error::Limit`], and the message names the option to raise and the
/// factor that would admit the file.
#[test]
fn over_the_default_limit_is_refused_and_the_message_names_the_option() {
    for (what, bytes) in [
        ("header only", flat_header_only()),
        ("JP2 header only", in_jp2(&flat_header_only())),
        ("file", flat()),
    ] {
        for result in [decode(&bytes).map(|_| ()), read_header(&bytes).map(|_| ())] {
            match result {
                Err(Error::Limit(m)) => {
                    assert!(
                        m.starts_with("image area 16385×16385 needs 268468225 samples, over the sample limit of 268435456"),
                        "{what}: {m}"
                    );
                    assert!(m.contains("`DecodeOptions::sample_limit`"), "{what}: {m}");
                    assert!(m.contains("`WorkLimit::Raised(2)`"), "{what}: {m}");
                    assert!(m.contains("`WorkLimit::Unlimited`"), "{what}: {m}");
                }
                other => panic!("{what}: {other:?}"),
            }
        }
    }
}

/// `Raised` and `Unlimited` admit the file. The header reads; the decode
/// passes the limit and fails only on the missing tile data, before any
/// plane is allocated, so the test stays cheap.
#[test]
fn raised_and_unlimited_admit_an_image_over_the_default() {
    let codestream = flat_header_only();
    let file = in_jp2(&codestream);
    for (bytes, limit) in [
        (&codestream, WorkLimit::Raised(2)),
        (&codestream, WorkLimit::Unlimited),
        (&file, WorkLimit::Raised(2)),
        (&file, WorkLimit::Unlimited),
    ] {
        let options = with_limit(limit.clone());
        let header = read_header_with(bytes, &options).unwrap_or_else(|e| panic!("{limit:?}: {e}"));
        assert_eq!((header.width, header.height), (16385, 16385), "{limit:?}");
        match decode_with(bytes, &options) {
            Err(Error::Codestream(_)) => {}
            other => panic!("{limit:?}: expected the missing tile data, got {other:?}"),
        }
    }
    // `Raised(1)` is the default limit.
    assert!(matches!(
        read_header_with(&file, &with_limit(WorkLimit::Raised(1))),
        Err(Error::Limit(_))
    ));
}

/// The limit counts the samples at the decode's reduction: three levels down
/// the flat image is 2049x2049, which fits, and it decodes as
/// `opj_decompress -r 3` does (all zero). Two levels down still fits; none
/// does not.
#[test]
fn a_reduced_decode_of_an_image_over_the_limit_is_admitted() {
    let bytes = flat();
    let reduced = DecodeOptions::default().with_resolution_reduction(3);
    let image = decode_with(&bytes, &reduced).expect("fits once reduced");
    assert_eq!((image.width, image.height), (2049, 2049));
    let [component] = &image.components[..] else {
        panic!("{} components", image.components.len());
    };
    assert_eq!(component.samples.len(), 2049 * 2049);
    assert!(component.samples.iter().all(|&s| s == 0));

    let header = read_header_with(&bytes, &reduced).expect("header fits once reduced");
    assert_eq!((header.width, header.height), (2049, 2049));
    let header_only = flat_header_only();
    let one = DecodeOptions::default().with_resolution_reduction(1);
    assert!(read_header_with(&header_only, &one).is_ok());
    assert!(matches!(
        decode_with(&header_only, &one),
        Err(Error::Codestream(_))
    ));
}

/// The reduction's own levels appear in the message when the reduced image is
/// still over the limit.
#[test]
fn a_reduced_image_still_over_the_limit_names_the_reduction() {
    // 40000 x 40000 at one level down is 20000 x 20000: 4e8 samples, which a
    // 32-bit target can still address.
    let mut bytes = flat_header_only();
    // SOC, then SIZ: marker, Lsiz, Rsiz, then Xsiz at 8, Ysiz, XOsiz, YOsiz,
    // XTsiz, YTsiz.
    assert_eq!(&bytes[2..4], &[0xFF, 0x51]);
    for at in [8, 12, 24, 28] {
        bytes[at..at + 4].copy_from_slice(&40_000u32.to_be_bytes());
    }
    let options = DecodeOptions::default().with_resolution_reduction(1);
    match read_header_with(&bytes, &options) {
        Err(Error::Limit(m)) => assert!(
            m.starts_with(
                "image area 20000×20000 at resolution reduction 1 needs 400000000 samples"
            ) && m.contains("`WorkLimit::Raised(2)`"),
            "{m}"
        ),
        other => panic!("{other:?}"),
    }
}

/// `Raised(0)` raises nothing: it is a contradiction in the options, refused
/// whatever the input.
#[test]
fn a_sample_limit_raised_by_zero_is_invalid_options() {
    let options = with_limit(WorkLimit::Raised(0));
    for bytes in [flat(), b"not a codestream".to_vec()] {
        for result in [
            decode_with(&bytes, &options).map(|_| ()),
            read_header_with(&bytes, &options).map(|_| ()),
        ] {
            match result {
                Err(Error::InvalidOptions(m)) => {
                    assert_eq!(
                        m,
                        "a sample limit raised by a factor of 0; the default is \
                         `WorkLimit::Default`"
                    );
                }
                other => panic!("{other:?}"),
            }
        }
    }
}

/// The default is `WorkLimit::Default`, and the setter sets the field.
#[test]
fn the_sample_limit_defaults_to_the_default_bound() {
    assert_eq!(DecodeOptions::default().sample_limit, WorkLimit::Default);
    assert_eq!(
        with_limit(WorkLimit::Unlimited).sample_limit,
        WorkLimit::Unlimited
    );
}

fn segment(marker: u16, body: &[u8]) -> Vec<u8> {
    let mut out = marker.to_be_bytes().to_vec();
    out.extend_from_slice(&(body.len() as u16 + 2).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// The main header of a `size` x `size` image in tiles of `tile`, of
/// `components` unsigned 8-bit components, six wavelet levels, 4x4
/// code-blocks, 5/3, no quantization, LRCP and `layers` layers, followed by
/// `extra` (more main-header segments).
fn main_header(size: u32, tile: u32, components: u16, layers: u16, extra: &[u8]) -> Vec<u8> {
    let mut siz = vec![0, 0];
    for field in [size, size, 0, 0, tile, tile, 0, 0] {
        siz.extend_from_slice(&field.to_be_bytes());
    }
    siz.extend_from_slice(&components.to_be_bytes());
    for _ in 0..components {
        siz.extend_from_slice(&[7, 1, 1]);
    }
    let mut cod = vec![0, 0];
    cod.extend_from_slice(&layers.to_be_bytes());
    cod.extend_from_slice(&[0, 6, 0, 0, 0, 1]);
    // A guard of 2 and an exponent of 8 for every subband.
    let qcd = [&[0x40][..], &[8 << 3; 19]].concat();
    [
        vec![0xFF, 0x4F],
        segment(0xFF51, &siz),
        segment(0xFF52, &cod),
        segment(0xFF5C, &qcd),
        extra.to_vec(),
    ]
    .concat()
}

/// One tile-part of tile `index` holding `body`.
fn tile_part(index: u32, body: &[u8]) -> Vec<u8> {
    let mut sot = (index as u16).to_be_bytes().to_vec();
    sot.extend_from_slice(&(14 + body.len() as u32).to_be_bytes());
    sot.extend_from_slice(&[0, 1]);
    [segment(0xFF90, &sot), vec![0xFF, 0x93], body.to_vec()].concat()
}

/// A codestream of `side` x `side` tiles of `tile` x `tile`, `components`
/// components, six wavelet levels and 4x4 code-blocks, where every tile
/// carries an empty packet for each of its seven resolutions in each
/// component. It is a few hundred bytes, but a tile of 2800 has about 490000
/// code-blocks a component.
fn many_tiles_of_empty_packets(side: u32, tile: u32, components: u16) -> Vec<u8> {
    let mut out = main_header(side * tile, tile, components, 1, &[]);
    let packets = vec![0u8; 7 * usize::from(components)];
    for index in 0..side * side {
        out.extend_from_slice(&tile_part(index, &packets));
    }
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

/// The sample limit is counted at the reduction, so an image far over it is
/// admitted when its reduced area fits. Every tile still builds its geometry
/// at full resolution, and that work is charged to the packet work bound, so
/// many tiles of next to no data are refused quickly at the defaults.
/// `packet_work_limit` is the one setting to raise, and the message names
/// the factor that admits the file.
#[test]
fn many_tiles_of_geometry_are_held_to_the_packet_work_bound() {
    // 7 x 7 tiles, about 24 M code-blocks, past the 2^24 visits the bound
    // allows a file this small.
    let bytes = many_tiles_of_empty_packets(7, 2800, 1);
    assert!(bytes.len() < 2048, "{} bytes", bytes.len());
    let reduced = DecodeOptions::default().with_resolution_reduction(6);
    let header = read_header_with(&bytes, &reduced).expect("the reduced area fits");
    assert_eq!((header.width, header.height), (307, 307));
    match decode_with(&bytes, &reduced) {
        Err(Error::Limit(m)) => {
            assert!(m.contains("`DecodeOptions::packet_work_limit`"), "{m}");
            assert!(
                m.contains("the file needs at least `WorkLimit::Raised(2)`"),
                "{m}"
            );
        }
        other => panic!("{:?}", other.map(|i| (i.width, i.height))),
    }
    // Twice the bound admits it. With no coded data, every sample is the
    // level shift of an unsigned 8-bit component.
    let raised = reduced.with_packet_work_limit(WorkLimit::Raised(2));
    let image = decode_with(&bytes, &raised).expect("admitted under Raised(2)");
    assert_eq!((image.width, image.height), (307, 307));
    assert!(image.components[0].samples.iter().all(|&s| s == 128));
}

/// The factor `Raised(n)` a `Limit` message names.
fn named_factor(message: &str) -> u32 {
    let (_, rest) = message
        .split_once("`WorkLimit::Raised(")
        .unwrap_or_else(|| panic!("no factor named: {message}"));
    rest.split_once(')').unwrap().0.parse().unwrap()
}

/// The message of a decode refused for work under `limit` at `-r 6`.
fn refused_at_r6(bytes: &[u8], limit: WorkLimit) -> String {
    let options = DecodeOptions::default()
        .with_resolution_reduction(6)
        .with_packet_work_limit(limit);
    match decode_with(bytes, &options) {
        Err(Error::Limit(m)) => m,
        other => panic!("{:?}", other.map(|i| (i.width, i.height))),
    }
}

/// The factor for the dropped resolutions' code-blocks counts the charges of
/// the tile's earlier components too. 6 x 6 tiles of 2000 in two components
/// hold about 1.8e7 code-blocks, past the 2^24 visits, and the refusal comes
/// at a tile's second component: the message names `Raised(2)`, which
/// admits the file.
#[test]
fn the_dropped_blocks_factor_counts_the_tiles_earlier_components() {
    let bytes = many_tiles_of_empty_packets(6, 2000, 2);
    let message = refused_at_r6(&bytes, WorkLimit::Default);
    assert!(
        message.contains("code-blocks in this component"),
        "{message}"
    );
    assert!(message.contains("visits already spent"), "{message}");
    assert_eq!(named_factor(&message), 2, "{message}");
    let options = DecodeOptions::default()
        .with_resolution_reduction(6)
        .with_packet_work_limit(WorkLimit::Raised(2));
    let image = decode_with(&bytes, &options).expect("admitted under Raised(2)");
    assert_eq!(image.components.len(), 2);
}

/// One 64 x 64 tile of 37 components and 64770 layers, with two identical
/// POC volumes over all of them (the reach is checked only for a sequence).
/// They reach 64770 x 7 x 37 = 16775430 packets, 1786 under the 2^24
/// visits. The six resolutions `-r 6` drops hold 261 code-blocks a component,
/// 9657 in all, so their charge leaves too few visits for the reach. The
/// tile holds 16 empty packets.
fn poc_reach_past_the_dropped_blocks() -> Vec<u8> {
    let (components, layers) = (37u16, 64770u16);
    // RSpoc 0, CSpoc 0, LYEpoc, REpoc 7, CEpoc, Ppoc LRCP.
    let mut entry = vec![0, 0];
    entry.extend_from_slice(&layers.to_be_bytes());
    entry.extend_from_slice(&[7, components as u8, 0]);
    let poc = segment(0xFF5F, &[&entry[..], &entry[..]].concat());
    let mut out = main_header(64, 64, components, layers, &poc);
    out.extend_from_slice(&tile_part(0, &[0; 16]));
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

/// A POC reach refused after the dropped resolutions' code-blocks are
/// charged names the factor for both: 9657 + 16775430 visits is past 2^24,
/// so `Raised(2)`, though the reach alone fits under the default.
#[test]
fn the_poc_factor_counts_the_dropped_blocks_charged_before_it() {
    let bytes = poc_reach_past_the_dropped_blocks();
    let message = refused_at_r6(&bytes, WorkLimit::Default);
    assert!(
        message.contains("its POC volumes reach 16775430"),
        "{message}"
    );
    assert_eq!(named_factor(&message), 2, "{message}");
    // Under `Raised(2)` the walk gets past the bound; the tile holds only 16
    // packets, so it fails some other way.
    let options = DecodeOptions::default()
        .with_resolution_reduction(6)
        .with_packet_work_limit(WorkLimit::Raised(2));
    let result = decode_with(&bytes, &options);
    assert!(!matches!(result, Err(Error::Limit(_))), "{result:?}");
}
