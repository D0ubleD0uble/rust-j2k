//! `Header::tiles` against `opj_dump` and against `Region::Tile`.
//!
//! The expected values are `opj_dump -i <file>` (OpenJPEG 2.5.0): the image
//! area is its `x0, y0, x1, y1`, and the grid its `tdx, tdy`, `tx0, ty0` and
//! `tw, th`. The fixtures' provenance is in `tests/fixtures/window/README.md`
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
}

const DUMPS: [Dump; 4] = [
    // Tiles from (3, 2), the image at (5, 4): a non-zero tile offset.
    Dump {
        file: "window/tiled_53.j2k",
        image: (5, 4, 75, 58),
        tile: (24, 20),
        offset: (3, 2),
        grid: (3, 3),
    },
    // A non-zero tile offset, and sub-sampled components.
    Dump {
        file: "window/sub_53.j2k",
        image: (7, 5, 74, 56),
        tile: (16, 14),
        offset: (2, 3),
        grid: (5, 4),
    },
    // A JP2 file: tiles from the origin, the image at (5, 3).
    Dump {
        file: "jp2_sycc_420_tiled.jp2",
        image: (5, 3, 19, 15),
        tile: (8, 8),
        offset: (0, 0),
        grid: (3, 2),
    },
    // One tile.
    Dump {
        file: "jpeg2000_regular_latlon.j2k",
        image: (0, 0, 16, 31),
        tile: (16, 31),
        offset: (0, 0),
        grid: (1, 1),
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
        let tiles: TileGrid = read_header_with(&bytes, &DecodeOptions::default())
            .unwrap()
            .tiles;
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

/// Every index below `count()` decodes to the tile the grid places there,
/// cut to the image area, and `count()` itself is out of range.
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
            let x0 = (tiles.x_offset + p * tiles.tile_width).max(ix0);
            let y0 = (tiles.y_offset + q * tiles.tile_height).max(iy0);
            let x1 = (tiles.x_offset + (p + 1) * tiles.tile_width).min(ix1);
            let y1 = (tiles.y_offset + (q + 1) * tiles.tile_height).min(iy1);
            let tile = DecodeOptions::default().with_region(Region::Tile(index));
            let image = decode_with(&bytes, &tile).unwrap();
            assert_eq!(
                (image.width, image.height),
                (x1 - x0, y1 - y0),
                "{} tile {index}",
                dump.file
            );
            // The same samples as a window on that rectangle.
            let window =
                DecodeOptions::default().with_region(Region::Window(Window::new(x0, y0, x1, y1)));
            assert_eq!(
                decode_with(&bytes, &window).unwrap().components,
                image.components,
                "{} tile {index}",
                dump.file
            );
        }
        let past = DecodeOptions::default().with_region(Region::Tile(tiles.count()));
        assert!(
            matches!(decode_with(&bytes, &past), Err(Error::InvalidOptions(_))),
            "{}",
            dump.file
        );
    }
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
    assert_ne!(header.width, 70);
}
