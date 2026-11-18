//! The sample limit, [`DecodeOptions::sample_limit`]: how many samples a
//! decoded image may hold. The default is 2^29 samples, counted at the
//! decode's resolution reduction; `WorkLimit::Raised` and
//! `WorkLimit::Unlimited` raise or lift it.
//!
//! The fixtures in `tests/fixtures/sample_limit/` are written by
//! `scripts/gen-sample-limit-fixtures.py` (see the README there).

#[path = "support/synth.rs"]
mod synth;
use std::path::PathBuf;
use synth::{bx, seg, tile_part};

use rust_j2k::{
    DecodeOptions, Error, LimitDetail, LimitKind, Region, Window, WorkLimit, decode, decode_with,
    read_header, read_header_with,
};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sample_limit")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The flat 32769x16385 file: 536920065 samples, just over 2^29.
fn flat() -> Vec<u8> {
    fixture("flat_32769x16385_tiled_lossless.j2k")
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
    let ihdr = synth::ihdr_payload(16385, 32769, 1, [7, 7, 0, 0]);
    synth::jp2_file(
        &[bx(b"ihdr", &ihdr), bx(b"colr", &synth::colr_enum(17))],
        &[],
        codestream,
        &[],
    )
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
#[cfg(target_pointer_width = "64")]
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
                        m.message().starts_with("image area 32769x16385 needs 536920065 samples, over the sample limit of 536870912"),
                        "{what}: {m}"
                    );
                    assert!(
                        m.message().contains("`DecodeOptions::sample_limit`"),
                        "{what}: {m}"
                    );
                    assert!(
                        m.message().contains("`WorkLimit::Raised(2)`"),
                        "{what}: {m}"
                    );
                    assert!(
                        m.message().contains("`WorkLimit::Unlimited`"),
                        "{what}: {m}"
                    );
                    assert_eq!(m.kind(), LimitKind::Samples, "{what}");
                    assert_eq!(m.retry_with(), Some(WorkLimit::Raised(2)), "{what}");
                }
                other => panic!("{what}: {other:?}"),
            }
        }
    }
}

/// The typed hint from a sample-limit refusal sets the option: `with_retry`
/// gives `Raised(2)` for the flat file, and the retried decode gets past the
/// limit. It runs on the header-only file, so it fails only on the missing
/// tile data, as `raised_and_unlimited_admit...` does.
#[cfg(target_pointer_width = "64")]
#[test]
fn a_retry_with_the_typed_hint_admits_the_flat_file() {
    let bytes = flat_header_only();
    let Err(Error::Limit(detail)) = decode(&bytes) else {
        panic!("expected a limit refusal at the default");
    };
    assert_eq!(detail.kind(), LimitKind::Samples);
    let options = DecodeOptions::default()
        .with_retry(&detail)
        .expect("a retry for the sample limit");
    assert_eq!(options.sample_limit, WorkLimit::Raised(2));
    let header = read_header_with(&bytes, &options).expect("the header reads under the retry");
    assert_eq!((header.width, header.height), (32769, 16385));
    assert!(matches!(
        decode_with(&bytes, &options),
        Err(Error::Codestream(_))
    ));
}

/// A Sentinel-2 true-color image (10980x10980, three 8-bit components, 361.7 M
/// samples) is under the default limit: the header reads and the decode gets
/// past the limit, failing only on the missing tile data.
#[test]
fn a_sentinel_2_true_color_image_is_under_the_default_limit() {
    let mut bytes = main_header(10980, 1024, 3, 1, &[]);
    bytes.extend_from_slice(&[0xFF, 0x90]);
    let header = read_header(&bytes).expect("10980x10980x3 is under the default");
    assert_eq!((header.width, header.height), (10980, 10980));
    assert_eq!(header.components.len(), 3);
    assert!(matches!(decode(&bytes), Err(Error::Codestream(_))));
}

/// On a 32-bit target every image over the default is also past the address
/// cap, so it is refused as `AddressSpace`, with nothing to retry with.
#[cfg(not(target_pointer_width = "64"))]
#[test]
fn over_the_default_limit_is_refused_as_address_space() {
    for (what, bytes) in [
        ("header only", flat_header_only()),
        ("JP2 header only", in_jp2(&flat_header_only())),
        ("file", flat()),
    ] {
        for result in [decode(&bytes).map(|_| ()), read_header(&bytes).map(|_| ())] {
            match result {
                Err(Error::Limit(m)) => {
                    assert_eq!(m.kind(), LimitKind::AddressSpace, "{what}: {m}");
                    assert_eq!(m.retry_with(), None, "{what}");
                }
                other => panic!("{what}: {other:?}"),
            }
        }
    }
}

/// `Raised` and `Unlimited` admit the file. The header reads; the decode
/// passes the limit and fails only on the missing tile data, before any
/// plane is allocated, so the test stays cheap.
#[cfg(target_pointer_width = "64")]
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
        assert_eq!((header.width, header.height), (32769, 16385), "{limit:?}");
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
/// the flat image is 4097x2049, which fits, and it decodes as
/// `opj_decompress -r 3` does (all zero). Two levels down still fits; none
/// does not.
#[test]
fn a_reduced_decode_of_an_image_over_the_limit_is_admitted() {
    let bytes = flat();
    let reduced = DecodeOptions::default().with_resolution_reduction(3);
    let image = decode_with(&bytes, &reduced).expect("fits once reduced");
    assert_eq!((image.width, image.height), (4097, 2049));
    let [component] = &image.components[..] else {
        panic!("{} components", image.components.len());
    };
    assert_eq!(component.samples.len(), 4097 * 2049);
    assert!(component.samples.iter().all(|&s| s == 0));

    let header = read_header_with(&bytes, &reduced).expect("header fits once reduced");
    assert_eq!((header.width, header.height), (4097, 2049));
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
#[cfg(target_pointer_width = "64")]
#[test]
fn a_reduced_image_still_over_the_limit_names_the_reduction() {
    // 50000 x 50000 at one level down is 25000 x 25000: 6.25e8 samples.
    let mut bytes = flat_header_only();
    // SOC, then SIZ: marker, Lsiz, Rsiz, then Xsiz at 8, Ysiz, XOsiz, YOsiz,
    // XTsiz, YTsiz.
    assert_eq!(&bytes[2..4], &[0xFF, 0x51]);
    for at in [8, 12, 24, 28] {
        bytes[at..at + 4].copy_from_slice(&50_000u32.to_be_bytes());
    }
    let options = DecodeOptions::default().with_resolution_reduction(1);
    match read_header_with(&bytes, &options) {
        Err(Error::Limit(m)) => assert!(
            m.message().starts_with(
                "image area 25000x25000 at resolution reduction 1 needs 625000000 samples"
            ) && m.message().contains("`WorkLimit::Raised(2)`"),
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
                        m.message(),
                        "`DecodeOptions::sample_limit` is `WorkLimit::Raised(0)`, but the \
                         factor must be at least 1"
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

/// The main header of a `size` x `size` image in tiles of `tile`, of
/// `components` unsigned 8-bit components, six wavelet levels, 4x4
/// code-blocks, 5/3, no quantization, LRCP and `layers` layers, followed by
/// `extra` (more main-header segments).
fn main_header(size: u32, tile: u32, components: u16, layers: u16, extra: &[u8]) -> Vec<u8> {
    let siz = synth::siz_body(
        [size, size, 0, 0, tile, tile, 0, 0],
        &vec![[7, 1, 1]; usize::from(components)],
    );
    let cod = synth::cod_body(layers, 6, 0, 0);
    // A guard of 2 and an exponent of 8 for every subband.
    let qcd = synth::qcd_none_body(2, &[8; 19]);
    [
        vec![0xFF, 0x4F],
        seg(0xFF51, &siz),
        seg(0xFF52, &cod),
        seg(0xFF5C, &qcd),
        extra.to_vec(),
    ]
    .concat()
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
        out.extend_from_slice(&tile_part(index as u16, 0, 1, &[], &packets));
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
            assert!(
                m.message().contains("`DecodeOptions::packet_work_limit`"),
                "{m}"
            );
            assert!(
                m.message().contains("retry with `WorkLimit::Raised(2)`"),
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

/// The factor `Raised(n)` a packet work refusal gives, read from its data.
/// The message names the same one.
fn named_factor(limit: &LimitDetail) -> u32 {
    assert_eq!(limit.kind(), LimitKind::PacketWork, "{limit}");
    let Some(WorkLimit::Raised(n)) = limit.retry_with() else {
        panic!("no factor given: {limit:?}");
    };
    assert!(
        limit
            .message()
            .contains(&format!("`WorkLimit::Raised({n})`")),
        "{limit}"
    );
    n
}

/// The refusal of a decode refused for work under `limit` at `-r 6`.
fn refused_at_r6(bytes: &[u8], limit: WorkLimit) -> LimitDetail {
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
        message
            .message()
            .contains("code-blocks in one component of tile"),
        "{message}"
    );
    assert!(
        message
            .message()
            .contains("steps left of the packet work bound"),
        "{message}"
    );
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
    let poc = seg(0xFF5F, &[&entry[..], &entry[..]].concat());
    let mut out = main_header(64, 64, components, layers, &poc);
    out.extend_from_slice(&tile_part(0, 0, 1, &[], &[0; 16]));
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
        message
            .message()
            .contains("volumes of tile 0 reach 16775430"),
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

// Under a window or a tile, the limit counts every component of the tiles
// the decode reads. The flat image is 32769x16385 in a 9x5 grid of 4096x4096
// tiles, so its last column and row of tiles are one sample wide or tall.

fn window(x0: u32, y0: u32, x1: u32, y1: u32) -> DecodeOptions {
    DecodeOptions::default().with_region(Region::Window(Window::new(x0, y0, x1, y1)))
}

/// The message of the `Limit` that `result` must be.
fn limit<T: std::fmt::Debug>(result: Result<T, Error>) -> String {
    match result {
        Err(Error::Limit(m)) => m.message().to_owned(),
        other => panic!("expected Error::Limit, got {other:?}"),
    }
}

/// A 256x256 window of the flat image, which is over the default limit,
/// decodes at the default: inside one tile, across four, and at the far
/// corner, where it reaches the one-sample tiles. The flat image is all zero,
/// and so is `opj_decompress -d` of each window.
#[test]
fn a_small_window_of_an_image_over_the_limit_decodes_at_the_default() {
    let bytes = flat();
    for (x0, y0, x1, y1) in [
        (1000, 1000, 1256, 1256),
        (3968, 3968, 4224, 4224),
        (32513, 16129, 32769, 16385),
    ] {
        let options = window(x0, y0, x1, y1);
        let header = read_header_with(&bytes, &options).expect("the tiles read fit");
        assert_eq!((header.width, header.height), (256, 256));
        let image = decode_with(&bytes, &options).expect("the tiles read fit");
        assert_eq!((image.width, image.height), (256, 256));
        let [component] = &image.components[..] else {
            panic!("{} components", image.components.len());
        };
        assert_eq!(component.samples.len(), 256 * 256);
        assert!(component.samples.iter().all(|&s| s == 0));
    }
}

/// The count is the tiles read, not the window: 32 whole tiles are exactly
/// 2^29 samples and fit; a window of the same size one sample further in
/// reaches all 45 tiles and is refused. A window whose own size is over the
/// limit is refused too, and the whole image is counted as it is without a
/// window. The header-only file fails on the missing tile data once it is
/// past the limit, before any plane is allocated.
#[cfg(target_pointer_width = "64")]
#[test]
fn a_window_is_held_to_the_samples_of_the_tiles_it_reads() {
    let bytes = flat_header_only();
    let thirty_two = window(0, 0, 32768, 16384);
    let header = read_header_with(&bytes, &thirty_two).expect("32 tiles fit");
    assert_eq!((header.width, header.height), (32768, 16384));
    assert!(matches!(
        decode_with(&bytes, &thirty_two),
        Err(Error::Codestream(_))
    ));

    let all = window(1, 1, 32769, 16385);
    for m in [
        limit(read_header_with(&bytes, &all)),
        limit(decode_with(&bytes, &all)),
    ] {
        assert!(
            m.starts_with(
                "the area of the tiles read (32769x16385) needs 536920065 samples, over the \
                 sample limit of 536870912"
            ),
            "{m}"
        );
        assert!(m.contains("`WorkLimit::Raised(2)`"), "{m}");
    }
    // Past the limit on its own: 36 tiles, one column of them a sample wide.
    let m = limit(read_header_with(&bytes, &window(0, 0, 32769, 16384)));
    assert!(
        m.starts_with("the area of the tiles read (32769x16384) needs 536887296 samples"),
        "{m}"
    );
    let m = limit(decode_with(&bytes, &window(0, 0, 40000, 20000)));
    assert!(
        m.starts_with("image area 32769x16385 needs 536920065 samples"),
        "{m}"
    );

    // The tiles are counted at the reduction.
    let reduced = all.clone().with_resolution_reduction(1);
    let header = read_header_with(&bytes, &reduced).expect("fits at reduction 1");
    assert_eq!((header.width, header.height), (16384, 8192));
    // `Raised(2)` admits the window as it admits the image.
    let raised = all.with_sample_limit(WorkLimit::Raised(2));
    assert!(read_header_with(&bytes, &raised).is_ok());
}

/// A tile counts its own samples: one tile of the flat image fits, and one
/// tile that is itself over the limit is refused.
#[test]
fn a_tile_is_held_to_its_own_samples() {
    let bytes = flat();
    for (tile, width, height) in [(8, 1, 4096), (44, 1, 1)] {
        let options = DecodeOptions::default().with_region(Region::Tile(tile));
        let image = decode_with(&bytes, &options).expect("one tile fits");
        assert_eq!((image.width, image.height), (width, height), "tile {tile}");
        assert!(image.components[0].samples.iter().all(|&s| s == 0));
    }
    let header_only = flat_header_only();
    let first = DecodeOptions::default().with_region(Region::Tile(0));
    let header = read_header_with(&header_only, &first).expect("one tile fits");
    assert_eq!((header.width, header.height), (4096, 4096));
    assert!(matches!(
        decode_with(&header_only, &first),
        Err(Error::Codestream(_))
    ));

    let mut one_tile = main_header(23171, 23171, 1, 1, &[]);
    one_tile.extend_from_slice(&[0xFF, 0x90]);
    for m in [
        limit(read_header_with(&one_tile, &first)),
        limit(decode_with(&one_tile, &first)),
    ] {
        assert!(
            m.starts_with("image area 23171x23171 needs 536895241 samples"),
            "{m}"
        );
    }
}

/// `bytes`, a codestream from [`main_header`], with the color transform on.
fn with_mct(mut bytes: Vec<u8>) -> Vec<u8> {
    let siz_end = 4 + usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
    assert_eq!(bytes[siz_end..siz_end + 2], [0xFF, 0x52]);
    // Lcod, Scod, the progression order and the layer count, then the MCT.
    bytes[siz_end + 8] = 1;
    bytes
}

/// A component subset does not lower the count: the packets of every
/// component of each tile read are walked, and the color transform needs all
/// three. A 16384x16384 RGB image is over the limit in three components,
/// though one of them fits. Of 256 tiles of 1024x1024, a window with one
/// component reads one tile and decodes; the same subset of the whole image,
/// and a window of the one large tile of a single-tile image, are refused.
#[test]
fn a_component_subset_counts_every_component_of_the_tiles_read() {
    let tiled = with_mct(many_tiles_of_empty_packets(16, 1024, 3));
    for subset in [vec![0], vec![2, 1]] {
        let options = window(100, 200, 356, 456).with_components(Some(subset.clone()));
        let image = decode_with(&tiled, &options).expect("one tile of three components fits");
        assert_eq!(image.components.len(), subset.len());
        for component in &image.components {
            assert_eq!(component.samples.len(), 256 * 256);
            // No coded data: the level shift of an unsigned 8-bit component.
            assert!(component.samples.iter().all(|&s| s == 128));
        }
    }
    let subset = DecodeOptions::default().with_components(Some(vec![0]));
    let m = limit(decode_with(&tiled, &subset));
    assert!(
        m.starts_with("a decode of 3 components needs 805306368 samples"),
        "{m}"
    );

    let mut single = with_mct(main_header(16384, 16384, 3, 1, &[]));
    single.extend_from_slice(&[0xFF, 0x90]);
    let options = window(0, 0, 16384, 16000).with_components(Some(vec![0]));
    for m in [
        limit(read_header_with(&single, &options)),
        limit(decode_with(&single, &options)),
    ] {
        assert!(
            m.starts_with("a decode of the tiles read in 3 components needs 805306368 samples"),
            "{m}"
        );
    }
}

/// A gray JP2 file of `size` x `size`, holding `codestream`, whose palette
/// expands its one component to three 8-bit channels.
fn paletted_jp2(size: u32, codestream: &[u8]) -> Vec<u8> {
    let bx = |kind: &[u8; 4], payload: &[u8]| {
        let mut out = (8 + payload.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    };
    let mut ihdr = size.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&[0, 1, 7, 7, 0, 0]);
    // One entry of three 8-bit columns; channel i is column i of component 0.
    let pclr = [0, 1, 3, 7, 7, 7, 10, 20, 30];
    let cmap = [0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 2];
    let jp2h = [
        bx(b"ihdr", &ihdr),
        bx(b"colr", &[1, 0, 0, 0, 0, 0, 16]),
        bx(b"pclr", &pclr),
        bx(b"cmap", &cmap),
    ]
    .concat();
    [
        vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A],
        bx(b"ftyp", b"jp2 \0\0\0\0jp2 "),
        bx(b"jp2h", &jp2h),
        bx(b"jp2c", codestream),
    ]
    .concat()
}

/// A palette's expanded channels count only the area the decode returns: a
/// 16384x16384 image expanded to three channels is over the limit, and a
/// window of it is not.
#[test]
fn a_palette_under_a_window_counts_the_window() {
    let mut codestream = main_header(16384, 1024, 1, 1, &[]);
    codestream.extend_from_slice(&[0xFF, 0x90]);
    let file = paletted_jp2(16384, &codestream);
    let m = limit(read_header_with(&file, &DecodeOptions::default()));
    assert!(
        m.starts_with("palette expansion to 3 channels needs 805306368 samples"),
        "{m}"
    );
    let header = read_header_with(&file, &window(0, 0, 4096, 4096)).expect("the window fits");
    assert_eq!((header.width, header.height), (4096, 4096));
    assert_eq!(header.components.len(), 3);
    let m = limit(read_header_with(&file, &window(0, 0, 16384, 15000)));
    assert!(
        m.starts_with("palette expansion to 3 channels needs 737280000 samples"),
        "{m}"
    );
}
