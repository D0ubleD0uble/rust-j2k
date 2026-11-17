//! `Header::image_area` and `Header::tiles` against `opj_dump`, and
//! `TileGrid::tile` against `Region::Tile` and `opj_decompress -t`.
//!
//! The expected values are `opj_dump -i <file>` (OpenJPEG 2.5.0): the image
//! area is its `x0, y0, x1, y1`, and the grid its `tdx, tdy`, `tx0, ty0` and
//! `tw, th`. The tile sizes are the size of `opj_decompress -i <file> -o
//! t.pgx -t <n>` for each tile, as its PGX header gives it. The fixtures' provenance is in `tests/fixtures/window/README.md`
//! and `tests/fixtures/README.md`.

use std::path::PathBuf;

use rust_j2k::{DecodeOptions, Error, Region, TileGrid, Window, decode_with, read_header_with};

/// One fixture and what `opj_dump` prints for it.
struct Dump {
    file: &'static str,
    /// `x0, y0, x1, y1`.
    image: (u32, u32, u32, u32),
    /// `tdx, tdy`.
    tile: (u32, u32),
    /// `tx0, ty0`.
    offset: (u32, u32),
    /// `tw, th`.
    grid: (u32, u32),
    /// Each tile's width and height from `opj_decompress -t`, where
    /// measured.
    tile_sizes: &'static [(u32, u32)],
}

const DUMPS: [Dump; 5] = [
    // The image at (300, 300), tiles from (290, 295): a window from the
    // grid's origin misses it.
    Dump {
        file: "window/offset_300.j2k",
        image: (300, 300, 370, 354),
        tile: (32, 32),
        offset: (290, 295),
        grid: (3, 2),
        tile_sizes: &[(22, 27), (32, 27), (16, 27), (22, 27), (32, 27), (16, 27)],
    },
    // Tiles from (3, 2), the image at (5, 4): a non-zero tile offset.
    Dump {
        file: "window/tiled_53.j2k",
        image: (5, 4, 75, 58),
        tile: (24, 20),
        offset: (3, 2),
        grid: (3, 3),
        tile_sizes: &[
            (22, 18),
            (24, 18),
            (24, 18),
            (22, 20),
            (24, 20),
            (24, 20),
            (22, 16),
            (24, 16),
            (24, 16),
        ],
    },
    // A non-zero tile offset, and sub-sampled components.
    Dump {
        file: "window/sub_53.j2k",
        image: (7, 5, 74, 56),
        tile: (16, 14),
        offset: (2, 3),
        grid: (5, 4),
        tile_sizes: &[],
    },
    // A JP2 file: tiles from the origin, the image at (5, 3).
    Dump {
        file: "jp2_sycc_420_tiled.jp2",
        image: (5, 3, 19, 15),
        tile: (8, 8),
        offset: (0, 0),
        grid: (3, 2),
        tile_sizes: &[],
    },
    // One tile.
    Dump {
        file: "jpeg2000_regular_latlon.j2k",
        image: (0, 0, 16, 31),
        tile: (16, 31),
        offset: (0, 0),
        grid: (1, 1),
        tile_sizes: &[(16, 31)],
    },
];

fn read(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn tile_grid_matches_opj_dump() {
    for dump in &DUMPS {
        let bytes = read(dump.file);
        let header = read_header_with(&bytes, &DecodeOptions::default()).unwrap();
        let (x0, y0, x1, y1) = dump.image;
        assert_eq!(
            header.image_area,
            Window::new(x0, y0, x1, y1),
            "{}: x0, y0, x1, y1",
            dump.file
        );
        let tiles: TileGrid = header.tiles;
        assert_eq!(
            (tiles.columns, tiles.rows),
            dump.grid,
            "{}: tw, th",
            dump.file
        );
        assert_eq!(
            (tiles.tile_width, tiles.tile_height),
            dump.tile,
            "{}: tdx, tdy",
            dump.file
        );
        assert_eq!(
            (tiles.x_offset, tiles.y_offset),
            dump.offset,
            "{}: tx0, ty0",
            dump.file
        );
        assert_eq!(tiles.count(), dump.grid.0 * dump.grid.1, "{}", dump.file);
    }
}

/// Every index below `count()` has a rectangle: the tile the grid places
/// there, cut to the image area. `Region::Tile` decodes that rectangle, to
/// the size `opj_decompress -t` gives, and to the same samples as a window on
/// it. `count()` itself has none and is out of range.
#[test]
fn tile_grid_indexes_region_tile() {
    for dump in &DUMPS {
        let bytes = read(dump.file);
        let tiles = read_header_with(&bytes, &DecodeOptions::default())
            .unwrap()
            .tiles;
        let (ix0, iy0, ix1, iy1) = dump.image;
        for index in 0..tiles.count() {
            let (p, q) = (index % tiles.columns, index / tiles.columns);
            let expected = Window::new(
                (tiles.x_offset + p * tiles.tile_width).max(ix0),
                (tiles.y_offset + q * tiles.tile_height).max(iy0),
                (tiles.x_offset + (p + 1) * tiles.tile_width).min(ix1),
                (tiles.y_offset + (q + 1) * tiles.tile_height).min(iy1),
            );
            let rect = tiles.tile(index).unwrap();
            assert_eq!(rect, expected, "{} tile {index}", dump.file);
            let size = (rect.x1 - rect.x0, rect.y1 - rect.y0);
            if let Some(&opj) = dump.tile_sizes.get(index as usize) {
                assert_eq!(size, opj, "{} tile {index}: opj_decompress -t", dump.file);
            }
            let tile = DecodeOptions::default().with_region(Region::Tile(index));
            let image = decode_with(&bytes, &tile).unwrap();
            assert_eq!(
                (image.width, image.height),
                size,
                "{} tile {index}",
                dump.file
            );
            let window = DecodeOptions::default().with_region(Region::Window(rect));
            assert_eq!(
                decode_with(&bytes, &window).unwrap().components,
                image.components,
                "{} tile {index}",
                dump.file
            );
        }
        assert!(dump.tile_sizes.is_empty() || dump.tile_sizes.len() == tiles.count() as usize);
        assert_eq!(tiles.tile(tiles.count()), None, "{}", dump.file);
        let past = DecodeOptions::default().with_region(Region::Tile(tiles.count()));
        assert!(
            matches!(decode_with(&bytes, &past), Err(Error::InvalidOptions(_))),
            "{}",
            dump.file
        );
    }
}

/// A window placed from the image area's origin decodes; the same window
/// from the grid's origin misses an image that starts at (300, 300).
#[test]
fn a_window_from_the_image_area_origin() {
    let bytes = read("window/offset_300.j2k");
    let area = read_header_with(&bytes, &DecodeOptions::default())
        .unwrap()
        .image_area;
    let at = |x0: u32, y0: u32| {
        let window = Window::new(x0, y0, x0 + 16, y0 + 16);
        decode_with(
            &bytes,
            &DecodeOptions::default().with_region(Region::Window(window)),
        )
    };
    let image = at(area.x0, area.y0).unwrap();
    assert_eq!((image.width, image.height), (16, 16));
    assert!(matches!(at(0, 0), Err(Error::InvalidOptions(_))));
}

/// A tile past a one-tile image names one tile, in the singular.
#[test]
fn a_tile_past_one_tile_says_one_tile() {
    let bytes = read("jpeg2000_regular_latlon.j2k");
    let options = DecodeOptions::default().with_region(Region::Tile(1));
    let Err(Error::InvalidOptions(why)) = decode_with(&bytes, &options) else {
        panic!("tile 1 of a one-tile image is out of range");
    };
    assert_eq!(why.message(), "tile 1 is past the image's 1 tile (tile 0)");
}

/// The grid is the file's, whatever region or reduction the options ask for.
#[test]
fn tile_grid_ignores_the_options() {
    let bytes = read("window/tiled_53.j2k");
    let whole = read_header_with(&bytes, &DecodeOptions::default())
        .unwrap()
        .tiles;
    let options = DecodeOptions::default()
        .with_region(Region::Tile(4))
        .with_resolution_reduction(1);
    let header = read_header_with(&bytes, &options).unwrap();
    assert_eq!(header.tiles, whole);
    assert_eq!(header.image_area, Window::new(5, 4, 75, 58));
    assert_ne!(header.width, 70);
}
