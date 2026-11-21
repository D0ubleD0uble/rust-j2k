//! The report bound of the `decode_partial` fuzz oracle
//! (`fuzz/common/partial.rs`) under a window: one `MissingTiles` run per row
//! of tiles the window reads.
//!
//! The seed is an 80-byte input the fuzzer found (see `tests/fixtures/README.md`).

#[path = "../fuzz/common/budget.rs"]
mod budget;
#[path = "support/inconsistent.rs"]
mod inconsistent;
#[path = "../fuzz/common/partial.rs"]
mod partial;

use rust_j2k::{Region, Window};

fn seed() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/partial/window_rows.j2k"),
    )
    .unwrap()
}

/// The window the `decode_partial` target picks for the seed, from its
/// 64-bit FNV-1a hash: 24..55 across by 11..34 down.
fn window() -> Region {
    Region::Window(Window::new(24, 11, 55, 34))
}

/// The seed through the oracle, as the target runs it: through the window
/// its hash picks, at reduction 0.
#[test]
fn the_seed_passes_the_oracle_through_its_window() {
    let bytes = seed();
    let options = budget::options(4).with_region(window());
    partial::check(
        &bytes,
        &options,
        &inconsistent::checked(rust_j2k::decode_with(&bytes, &options)),
    );
}

/// The report holds one run per window row: more than five entries per
/// tile-part plus two, and within what the window's rows of tiles add.
#[test]
fn the_window_rows_cover_the_report_and_no_more() {
    let bytes = seed();
    let options = budget::options(4).with_region(window()).with_strict(false);
    let image = rust_j2k::decode_with(&bytes, &options).unwrap();
    let entries = image.report.entries.len();
    assert!(entries > partial::entry_bound(1, 0), "{entries} entries");
    let grid = rust_j2k::read_header(&bytes).unwrap().tiles;
    let Region::Window(w) = window() else {
        panic!("a window")
    };
    let rows = partial::rows_read(&w, grid.tile_height, grid.y_offset, grid.rows);
    assert!(
        entries <= partial::entry_bound(1, rows),
        "{entries} entries"
    );
}

/// The bound is five entries per tile-part, two, and a row each.
#[test]
fn the_bound_is_strict() {
    assert_eq!(partial::entry_bound(1, 0), 7);
    assert_eq!(partial::entry_bound(1, 23), 30);
    assert_eq!(partial::entry_bound(3, 2), 19);
}

/// A report one entry past the bound for its tile-parts and window rows
/// fails the oracle; one at the bound passes.
#[test]
fn an_entry_at_the_bound_passes() {
    partial::assert_entries(partial::entry_bound(2, 5), 2, 5);
}

#[test]
#[should_panic(expected = "entries for at most 2 tile-parts and 5 window rows")]
fn one_entry_over_the_bound_panics() {
    partial::assert_entries(partial::entry_bound(2, 5) + 1, 2, 5);
}

/// A window that starts and ends inside tiles of two lines reads the two rows
/// the lines 1 and 2 fall in, not one row more.
#[test]
fn rows_read_counts_the_rows_the_window_touches() {
    assert_eq!(partial::rows_read(&Window::new(0, 1, 1, 3), 2, 0, 8), 2);
    // A window on the tile edges, one row; the grid's rows cap it.
    assert_eq!(partial::rows_read(&Window::new(0, 2, 1, 4), 2, 0, 8), 1);
    assert_eq!(partial::rows_read(&Window::new(0, 0, 1, 100), 2, 0, 8), 8);
    // A tile grid offset up shifts the rows.
    assert_eq!(partial::rows_read(&Window::new(0, 2, 1, 5), 2, 1, 8), 2);
}

/// `header_refused.j2k` has a tile over the precinct cap and no tile-part for
/// it, so a header read with `strict` on refuses it while the decode with
/// `strict` off reports 25 entries through its window (22..39 by 9..34).
#[test]
fn the_oracle_bounds_a_report_whose_strict_header_is_refused() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/partial/header_refused.j2k"),
    )
    .unwrap();
    assert!(rust_j2k::read_header(&bytes).is_err());
    let options = budget::options(4)
        .with_resolution_reduction(1)
        .with_region(Region::Window(Window::new(22, 9, 39, 34)));
    partial::check(
        &bytes,
        &options,
        &inconsistent::checked(rust_j2k::decode_with(&bytes, &options)),
    );
}
