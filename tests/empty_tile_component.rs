//! Tiles that hold no sample of a sub-sampled component, which ISO B.3
//! (Eq. B-12) allows: a tile narrower than `XRsiz` or shorter than `YRsiz`
//! has `tcx0 == tcx1` or `tcy0 == tcy1` on that component. Such a
//! tile-component has no precinct, so no packet, and adds no sample, and
//! every decode here matches `opj_decompress` 2.5.0 bit for bit: of the
//! whole image, or, for the one file OpenJPEG fails to decode whole, of
//! each tile. The files
//! and their oracles are in `tests/fixtures/empty_tile_component/` (see its
//! README).

#[path = "support/synth.rs"]
mod synth;
use std::path::PathBuf;

use rust_j2k::{
    DecodeOptions, Error, Image, IncrementalDecoder, Region, ReportEntry, Window, decode_with,
    read_header, read_header_with,
};

fn dir() -> PathBuf {
    synth::fixtures("empty_tile_component")
}

fn read(name: &str) -> Vec<u8> {
    synth::read_file(dir().join(name))
}

/// The components' samples one after another, as 8-bit values.
fn flat(image: &Image) -> Vec<u8> {
    image
        .components
        .iter()
        .flat_map(|c| c.samples.iter().map(|&s| u8::try_from(s).unwrap()))
        .collect()
}

/// The `opj_compress` encodes: name, the tile with no sample of component
/// 1 that has an oracle, and whether there is a `-r 1` oracle.
const ENCODES: &[(&str, u32, bool)] = &[
    ("x_l0_lrcp", 3, false),
    ("x_l0_rlcp", 3, false),
    ("x_l0_rpcl", 3, false),
    ("x_l0_pcrl", 3, false),
    ("x_l0_cprl", 3, false),
    ("x_l2_lrcp", 2, true),
    ("x_l2_rpcl", 2, true),
    ("x_l2_poc", 2, true),
    ("y_l0_lrcp", 12, false),
    ("y_l0_rpcl", 12, false),
    ("y_l2_lrcp", 8, true),
    ("y_l2_rpcl", 8, true),
    ("xy_l0_lrcp", 3, false),
    ("xy_l0_rpcl", 3, false),
];

/// The probes written byte by byte, every packet empty, but `probe_2pkt`,
/// which has a byte after its last packet.
const PROBES: &[&str] = &["probe_1pkt", "probe_ht_1pkt", "probe_y_1pkt", "probe_xy_l2"];

fn every_file() -> impl Iterator<Item = &'static str> {
    ENCODES
        .iter()
        .map(|&(name, _, _)| name)
        .chain(PROBES.iter().copied())
}

/// Each file decodes to OpenJPEG's image, with `strict` on and off, and
/// the decode reports nothing missing.
#[test]
fn every_file_decodes_as_openjpeg_decodes_it() {
    for name in every_file() {
        let bytes = read(&format!("{name}.j2k"));
        let oracle = read(&format!("{name}.opj.raw"));
        for strict in [true, false] {
            let options = DecodeOptions::default().with_strict(strict);
            let image = decode_with(&bytes, &options)
                .unwrap_or_else(|e| panic!("{name}, strict {strict}: {e}"));
            assert_eq!(flat(&image), oracle, "{name}, strict {strict}");
            assert!(image.report.is_complete(), "{name}: {:?}", image.report);
        }
    }
}

/// Component 1 of the 2 x 1 probe is one sample, from tile 0. Tile 1
/// carries one packet, component 0's, so it is read as the tile's last.
#[test]
fn the_one_packet_probe_gives_component_1_one_sample() {
    let image = decode_with(&read("probe_1pkt.j2k"), &DecodeOptions::default()).unwrap();
    let sizes: Vec<_> = image
        .components
        .iter()
        .map(|c| (c.width, c.height))
        .collect();
    assert_eq!(sizes, [(2, 1), (1, 1)]);
    assert_eq!(flat(&image), [128, 128, 128]);
}

/// `probe_2pkt` has a byte after tile 1's one packet, where a packet of
/// component 1 would be if the tile had one. OpenJPEG ignores it. The
/// strict decode refuses it as it refuses any byte left after the last
/// packet; with `strict` off the image is OpenJPEG's, and the byte is
/// reported.
#[test]
fn a_byte_where_an_empty_component_s_packet_would_be_is_left_over() {
    let bytes = read("probe_2pkt.j2k");
    let strict = decode_with(&bytes, &DecodeOptions::default()).map(|_| ());
    assert!(
        matches!(&strict, Err(Error::Codestream(d))
            if d.message() == "tile 1 has 1 byte(s) left after the last packet"),
        "{strict:?}"
    );
    let image = decode_with(&bytes, &DecodeOptions::default().with_strict(false)).unwrap();
    assert_eq!(flat(&image), read("probe_2pkt.opj.raw"));
    assert!(
        matches!(
            image.report.entries[..],
            [ReportEntry::LeftoverBytes {
                tile: 1,
                bytes: 1,
                ..
            }]
        ),
        "{:?}",
        image.report
    );
}

/// `read_header` reads each file and gives the components the sizes the
/// decode gives them.
#[test]
fn read_header_gives_the_decode_s_component_sizes() {
    for name in every_file().chain(["probe_2pkt"]) {
        let bytes = read(&format!("{name}.j2k"));
        let header = read_header(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let lenient = DecodeOptions::default().with_strict(false);
        let image = decode_with(&bytes, &lenient).unwrap();
        let sizes = |image: &Image| -> Vec<_> {
            image
                .components
                .iter()
                .map(|c| (c.width, c.height))
                .collect()
        };
        let read: Vec<_> = header
            .components
            .iter()
            .map(|c| (c.width, c.height))
            .collect();
        assert_eq!(read, sizes(&image), "{name}");
        assert!(read_header_with(&bytes, &lenient).is_ok(), "{name}");
    }
}

/// At a reduction of 1 the two-level encodes decode as `opj_decompress -r 1`
/// decodes them.
#[test]
fn a_reduction_decodes_as_openjpeg_decodes_it() {
    for &(name, _, reduced) in ENCODES {
        if !reduced {
            continue;
        }
        let bytes = read(&format!("{name}.j2k"));
        let options = DecodeOptions::default().with_resolution_reduction(1);
        let image = decode_with(&bytes, &options).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(flat(&image), read(&format!("{name}.r1.opj.raw")), "{name}");
    }
}

/// The tile with no sample of component 1, as a tile and as a window that
/// covers it and nothing else: component 0 alone decodes as
/// `opj_decompress -t <tile> -c 0` gives it. With component 1 asked for too
/// the decode is refused, as OpenJPEG fails to decode a component the area
/// holds no sample of.
#[test]
fn the_empty_tile_alone_decodes_as_openjpeg_decodes_it() {
    for &(name, tile, _) in ENCODES {
        let bytes = read(&format!("{name}.j2k"));
        let oracle = read(&format!("{name}.tile{tile}.opj.raw"));
        let header = read_header(&bytes).unwrap();
        let grid = header.tiles;
        let (column, row) = (tile % grid.columns, tile / grid.columns);
        let x0 = (grid.x_offset + column * grid.tile_width).max(header.image_area.x0);
        let y0 = (grid.y_offset + row * grid.tile_height).max(header.image_area.y0);
        let window = Window::new(
            x0,
            y0,
            (grid.x_offset + (column + 1) * grid.tile_width).min(header.image_area.x1),
            (grid.y_offset + (row + 1) * grid.tile_height).min(header.image_area.y1),
        );
        for region in [Region::Tile(tile), Region::Window(window)] {
            let both = DecodeOptions::default().with_region(region.clone());
            let refused = decode_with(&bytes, &both).map(|_| ());
            assert!(
                matches!(&refused, Err(Error::InvalidOptions(_))),
                "{name} {region:?}: {refused:?}"
            );
            let first = both.with_components(Some(vec![0]));
            let image =
                decode_with(&bytes, &first).unwrap_or_else(|e| panic!("{name} {region:?}: {e}"));
            assert_eq!(flat(&image), oracle, "{name} {region:?}");
        }
    }
}

/// An `IncrementalDecoder` fed each file a few bytes at a time ends on the
/// image `decode_with` gives, and its header is `read_header`'s.
#[test]
fn an_incremental_decode_ends_on_openjpeg_s_image() {
    for name in every_file() {
        let bytes = read(&format!("{name}.j2k"));
        let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
        for chunk in bytes.chunks(97) {
            decoder.feed(chunk);
            decoder.refine().unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        let header = decoder.header().unwrap().expect("the whole header is fed");
        assert_eq!(
            format!("{header:?}"),
            format!("{:?}", read_header(&bytes).unwrap()),
            "{name}"
        );
        let image = decoder.finish().unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(flat(&image), read(&format!("{name}.opj.raw")), "{name}");
    }
}

/// `side` x `side` one-sample tiles and `components` 8-bit components:
/// component 0 full size, the rest sub-sampled 255 both ways, so each holds
/// one sample, in tile 0, and no other tile holds one. `levels` wavelet
/// levels; every packet is empty.
fn one_tile_holds_the_rest(side: u32, components: usize, levels: u8) -> Vec<u8> {
    let mut comps = vec![[7, 1, 1]];
    comps.resize(components, [7, 255, 255]);
    let siz = synth::siz_body([side, side, 0, 0, 1, 1, 0, 0], &comps);
    let mut out = vec![0xFF, 0x4F];
    out.extend(synth::seg(0xFF51, &siz));
    out.extend(synth::seg(0xFF52, &synth::cod_body(1, levels, 4, 0)));
    let steps = vec![8; 1 + 3 * usize::from(levels)];
    out.extend(synth::seg(0xFF5C, &synth::qcd_none_body(2, &steps)));
    for tile in 0..side * side {
        let (x, y) = (tile % side, tile / side);
        // Component 0's resolutions that hold the tile's sample (ISO
        // B-14), and in tile 0 every resolution of the others, a packet
        // each.
        let held = (0..=levels)
            .filter(|&r| {
                let d = 1u32 << (levels - r);
                x.div_ceil(d) < (x + 1).div_ceil(d) && y.div_ceil(d) < (y + 1).div_ceil(d)
            })
            .count();
        let packets = held
            + if tile == 0 {
                (components - 1) * (usize::from(levels) + 1)
            } else {
                0
            };
        let index = u16::try_from(tile).unwrap();
        out.extend(synth::tile_part(index, 0, 1, &[], &vec![0; packets]));
    }
    out.extend([0xFF, 0xD9]);
    out
}

/// A tile-component that holds no sample has no packet, so no byte pays
/// for its setup, and each of its resolutions costs 32 steps of the packet
/// work bound. 255 tiles of 1999 such components spend 16.3 million of
/// the 2^24 a small file has and decode; 255 of 2099 spend 17.1 million
/// and are refused, by the decode and by an `IncrementalDecoder` alike.
/// Each decodes under a raised bound.
#[test]
fn the_setup_of_an_empty_tile_component_spends_the_work_bound() {
    let image = decode_with(
        &one_tile_holds_the_rest(16, 2000, 0),
        &DecodeOptions::default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(image.components.len(), 2000);
    let size = |c: &rust_j2k::Component| (c.width, c.height);
    assert_eq!(size(&image.components[1]), (1, 1));

    let bytes = one_tile_holds_the_rest(16, 2100, 0);
    let refused = match decode_with(&bytes, &DecodeOptions::default()) {
        Err(Error::Limit(m)) => m,
        other => panic!("{:?}", other.map(|_| ())),
    };
    assert_eq!(refused.kind(), rust_j2k::LimitKind::PacketWork);
    assert!(
        refused
            .message()
            .contains("holds no sample of 2099 components, whose setup costs 67168 steps"),
        "{refused}"
    );
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes);
    match decoder.refine().unwrap() {
        rust_j2k::Refined::Waiting(rust_j2k::Waiting::WorkLimit { limit, .. }) => {
            assert_eq!(format!("{limit:?}"), format!("{refused:?}"))
        }
        other => panic!("{other:?}"),
    }
    let raised = DecodeOptions::default().with_packet_work_limit(rust_j2k::WorkLimit::Raised(2));
    assert!(decode_with(&bytes, &raised).is_ok());

    // Under two levels each such component has three resolutions, so 255
    // tiles of 699 spend 17.1 million.
    let bytes = one_tile_holds_the_rest(16, 700, 2);
    assert!(decode_with(&bytes, &raised).is_ok());
    let refused = decode_with(&bytes, &DecodeOptions::default()).map(|_| ());
    assert!(
        matches!(&refused, Err(Error::Limit(m)) if m.message()
            .contains("holds no sample of 699 components, whose setup costs 67104 steps")),
        "{refused:?}"
    );
}

/// A component with no sample in the whole image is refused with `SIZ`, as
/// a header fault: by `read_header`, by the decode with `strict` on and
/// off, and by an `IncrementalDecoder`. Here the image is the one column
/// `[1, 2)`, and component 1, sub-sampled 2 across, has `x0 == x1 == 1`
/// (ISO B.2). OpenJPEG 2.5.0 fails to decode it.
#[test]
fn a_component_with_no_sample_in_the_image_is_refused_with_siz() {
    let siz = synth::siz_body([2, 1, 1, 0, 1, 1, 1, 0], &[[7, 1, 1], [7, 2, 1]]);
    let mut bytes = vec![0xFF, 0x4F];
    bytes.extend(synth::seg(0xFF51, &siz));
    bytes.extend(synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0)));
    bytes.extend(synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])));
    bytes.extend(synth::tile_part(0, 0, 1, &[], &[0]));
    bytes.extend([0xFF, 0xD9]);
    let refused = |result: Result<(), Error>| {
        assert!(
            matches!(&result, Err(Error::Marker(d))
                if d.message() == "component 1 holds no sample in the image"),
            "{result:?}"
        );
    };
    refused(read_header(&bytes).map(|_| ()));
    for strict in [true, false] {
        let options = DecodeOptions::default().with_strict(strict);
        refused(decode_with(&bytes, &options).map(|_| ()));
        refused(read_header_with(&bytes, &options).map(|_| ()));
    }
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes);
    refused(decoder.header().map(|_| ()));
    refused(decoder.refine().map(|_| ()));
    refused(decoder.finish().map(|_| ()));
}

/// `opjfail_l1_lrcp` holds tiles with no sample of components 1 and 2,
/// after tiles that hold some, under one wavelet level. OpenJPEG 2.5.0
/// fails to decode it whole (`opj_j2k_update_image_data` reads resolution
/// 0 of an empty tile-component, which starts before the output's origin).
/// Each tile, with the components it holds a sample of, decodes as
/// `opj_decompress -t <N> -c <those>` gives it, and the whole decode is
/// those tiles put in place.
#[test]
fn a_file_openjpeg_fails_whole_decodes_tile_by_tile_as_it_does() {
    let bytes = read("opjfail_l1_lrcp.j2k");
    let whole = decode_with(&bytes, &DecodeOptions::default()).unwrap();
    let header = read_header(&bytes).unwrap();
    let (grid, area) = (header.tiles, header.image_area);
    let mut tiles = Vec::new();
    // The whole image's samples, put back from the tiles' as they decode.
    let mut placed: Vec<Vec<Option<i32>>> = whole
        .components
        .iter()
        .map(|c| vec![None; c.samples.len()])
        .collect();
    for tile in 0..grid.columns * grid.rows {
        let (column, row) = (tile % grid.columns, tile / grid.columns);
        let x0 = (grid.x_offset + column * grid.tile_width).max(area.x0);
        let x1 = (grid.x_offset + (column + 1) * grid.tile_width).min(area.x1);
        let y0 = (grid.y_offset + row * grid.tile_height).max(area.y0);
        let y1 = (grid.y_offset + (row + 1) * grid.tile_height).min(area.y1);
        // The tile-component of each component (ISO B-12), where it is not
        // empty.
        let held: Vec<(u16, [u32; 4])> = whole
            .components
            .iter()
            .enumerate()
            .filter_map(|(c, comp)| {
                let (dx, dy) = (u32::from(comp.x_sampling), u32::from(comp.y_sampling));
                let rect = [
                    x0.div_ceil(dx),
                    x1.div_ceil(dx),
                    y0.div_ceil(dy),
                    y1.div_ceil(dy),
                ];
                (rect[0] < rect[1] && rect[2] < rect[3]).then_some((c as u16, rect))
            })
            .collect();
        let options = DecodeOptions::default()
            .with_region(Region::Tile(tile))
            .with_components(Some(held.iter().map(|&(c, _)| c).collect()));
        let image = decode_with(&bytes, &options).unwrap_or_else(|e| panic!("tile {tile}: {e}"));
        tiles.extend(flat(&image));
        for (part, &(c, [cx0, cx1, cy0, cy1])) in image.components.iter().zip(&held) {
            let comp = &whole.components[usize::from(c)];
            let (dx, dy) = (u32::from(comp.x_sampling), u32::from(comp.y_sampling));
            let (ox, oy) = (area.x0.div_ceil(dx), area.y0.div_ceil(dy));
            assert_eq!(
                (part.width, part.height),
                (cx1 - cx0, cy1 - cy0),
                "tile {tile}"
            );
            for y in cy0..cy1 {
                for x in cx0..cx1 {
                    let at = ((y - oy) * comp.width + (x - ox)) as usize;
                    let from = ((y - cy0) * part.width + (x - cx0)) as usize;
                    assert_eq!(placed[usize::from(c)][at], None, "tile {tile}");
                    placed[usize::from(c)][at] = Some(part.samples[from]);
                }
            }
        }
    }
    assert_eq!(tiles, read("opjfail_l1_lrcp.tiles.opj.raw"));
    for (comp, placed) in whole.components.iter().zip(placed) {
        let placed: Vec<i32> = placed.into_iter().map(Option::unwrap).collect();
        assert_eq!(comp.samples, placed);
    }
}
