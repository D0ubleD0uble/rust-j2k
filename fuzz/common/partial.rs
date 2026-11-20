//! The differential `decode_partial` and `tile_body` run on a decode with
//! `strict` off.

use rust_j2k::{DecodeOptions, Image, ReportEntry, Result};

/// Decode `bytes` with `strict` off under `options` (which must be strict),
/// given `strict`, the strict decode of the same bytes under `options`, and
/// check what the two must agree on:
///
/// - when the strict decode gives an image, the decode with `strict` off
///   gives the same image with the complete report;
/// - a strict image always has the complete report;
/// - the report stays within `entry_bound`: five entries per tile-part plus
///   two, plus a piece for each row of tiles a window reads. Every tile-part
///   starts with the bytes `FF 90`, so their count bounds the first part.
pub fn check(bytes: &[u8], options: &DecodeOptions, strict: &Result<Image>) {
    let lenient = rust_j2k::decode_with(bytes, &options.clone().with_strict(false));
    if let Ok(image) = strict {
        assert_eq!(image.report, rust_j2k::DecodeReport::default());
        let partial = lenient.as_ref().expect("strict decodes, so partial does");
        assert_eq!(
            (image.width, image.height, &image.components, &image.jp2),
            (
                partial.width,
                partial.height,
                &partial.components,
                &partial.jp2
            ),
            "the same image"
        );
        assert_eq!(partial.report, rust_j2k::DecodeReport::default());
        assert!(partial.report.is_complete());
        assert_eq!(image, partial, "the same Image, report included");
    }
    if let Ok(image) = &lenient {
        let report = &image.report;
        let sots = bytes.windows(2).filter(|w| *w == [0xFF, 0x90]).count();
        let rows = window_rows(bytes, options);
        assert_entries(report.entries.len(), sots, rows);
        // A tile with bytes left over is complete at no reduction.
        let leftover = report
            .entries
            .iter()
            .any(|e| matches!(e, ReportEntry::LeftoverBytes { .. }));
        if leftover {
            assert_eq!(report.complete_reduction, None);
        }
        if report.is_complete() {
            assert!(
                !report
                    .entries
                    .iter()
                    .any(|e| matches!(e, ReportEntry::MissingTiles { .. }))
            );
        }
    }
}

/// The most entries a report can hold, for a codestream with at most `sots`
/// tile-parts decoded through a window that reads at most `rows` rows of
/// tiles (`0` without a window).
///
/// Without a window the report holds at most five entries per tile-part, plus
/// two. A `MissingTiles` run is a stretch of tile indices with no tile-part,
/// so the runs are at most one more than the tiles that have one. A window
/// reads a rectangle of tiles, and `keep_entries` (`src/window.rs`) cuts each
/// run into one piece for each row of tiles it touches. The runs lie in order
/// and share at most a boundary row each, so the pieces are at most the
/// runs plus the rows: the window's rows add at most `rows` to the bound.
pub fn entry_bound(sots: usize, rows: usize) -> usize {
    5 * sots + 2 + rows
}

/// The rows of tiles `options` reads of `bytes`: `0` without a window. Under
/// a window the grid comes from a header read with `strict` off, as the
/// decode reads it. If even that fails, the grid is unknown and the worst
/// case, tiles one row high, bounds the rows by the window's height plus one.
fn window_rows(bytes: &[u8], options: &DecodeOptions) -> usize {
    let rust_j2k::Region::Window(window) = &options.region else {
        return 0;
    };
    match rust_j2k::read_header_with(bytes, &options.clone().with_strict(false)) {
        Ok(header) => {
            let grid = header.tiles;
            rows_read(window, grid.tile_height, grid.y_offset, grid.rows)
        }
        Err(_) => window.y1.saturating_sub(window.y0) as usize + 1,
    }
}

/// The rows of tiles, `tile_height` high from `y_offset` on the reference
/// grid, that `window` reaches in a grid of `grid_rows` rows: from the row of
/// the window's first line to the row of its last, at most `grid_rows`. The
/// window is not cut to the image, so this can count a row more than the
/// decode reads, never fewer.
pub fn rows_read(
    window: &rust_j2k::Window,
    tile_height: u32,
    y_offset: u32,
    grid_rows: u32,
) -> usize {
    let row = |y: u32| u64::from(y.saturating_sub(y_offset)) / u64::from(tile_height.max(1));
    let last = row(window.y1.saturating_sub(1).max(window.y0));
    let reached = last - row(window.y0) + 1;
    reached.min(u64::from(grid_rows)) as usize
}

/// Panic when `entries` is past `entry_bound(sots, rows)`.
pub fn assert_entries(entries: usize, sots: usize, rows: usize) {
    assert!(
        entries <= entry_bound(sots, rows),
        "{entries} entries for at most {sots} tile-parts and {rows} window rows of tiles"
    );
}
