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

/// The rows of tiles `options` reads of `bytes`: for a window, at most
/// `(y1 - y0 - 1) / tile_height + 2` and at most the grid's rows; else `0`.
fn window_rows(bytes: &[u8], options: &DecodeOptions) -> usize {
    let rust_j2k::Region::Window(window) = &options.region else {
        return 0;
    };
    let Ok(header) = rust_j2k::read_header(bytes) else {
        // Without a header no tile is read, so no run is cut.
        return 0;
    };
    let grid = header.tiles;
    let height = u64::from(window.y1.saturating_sub(window.y0));
    let reached = height.saturating_sub(1) / u64::from(grid.tile_height.max(1)) + 2;
    reached.min(u64::from(grid.rows)) as usize
}

/// Panic when `entries` is past `entry_bound(sots, rows)`.
pub fn assert_entries(entries: usize, sots: usize, rows: usize) {
    assert!(
        entries <= entry_bound(sots, rows),
        "{entries} entries for at most {sots} tile-parts and {rows} window rows of tiles"
    );
}
