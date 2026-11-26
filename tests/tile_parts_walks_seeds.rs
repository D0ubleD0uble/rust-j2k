//! The `header` fuzz target's seeds (the recipe in `fuzz/README.md`) through
//! `rust_j2k::fuzz::tile_parts_walks_agree`, which holds the walk of the
//! tile-parts that `read_header_with` runs where it refuses a tile to the
//! decode's parse, on each seed and on prefixes of it. The seeds are whole
//! files, so the prefixes are what reach the walks' handling of input that
//! ends early.

#![cfg(feature = "__fuzz")]

use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The files in `dir` whose names `keep` accepts, sorted.
fn files(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file() && keep(path.file_name().unwrap().to_str().unwrap()))
        .collect();
    paths.sort();
    paths
}

/// The `header` target's seeds: `tests/fixtures/*.j2k`,
/// `tests/fixtures/jp2_*.jp2`, `tests/fixtures/header/*` and
/// `tests/fixtures/packed_overrun/*.j2k`. Keep this list the same as the
/// `header` recipe in `fuzz/README.md` and the seeds the CI fuzz smoke
/// copies for `header`.
fn header_seeds() -> Vec<PathBuf> {
    let root = fixtures();
    let mut seeds = files(&root, |name| {
        name.ends_with(".j2k") || (name.starts_with("jp2_") && name.ends_with(".jp2"))
    });
    seeds.extend(files(&root.join("header"), |_| true));
    seeds.extend(files(&root.join("packed_overrun"), |name| {
        name.ends_with(".j2k")
    }));
    seeds
}

fn walks_agree(path: &Path) {
    let bytes = std::fs::read(path).unwrap();
    let walk = std::panic::catch_unwind(|| rust_j2k::fuzz::tile_parts_walks_agree(&bytes));
    assert!(walk.is_ok(), "{}", path.display());
}

#[test]
fn the_tile_part_walks_agree_on_every_header_seed() {
    let seeds = header_seeds();
    assert!(seeds.len() >= 80, "only {} seeds", seeds.len());
    for seed in &seeds {
        walks_agree(seed);
    }
}

/// `ppm_two_parts.j2k` puts the main header's packet headers in a `PPM`
/// with a chunk for each of two tile-parts. Its prefixes end in the `SOT`
/// of a tile-part, in a tile-part header, in a tile-part's data and one
/// byte after a tile-part, so with `strict` off each of the parse's four
/// ways of ending early leaves a `PPM` chunk over, which only that ending
/// excuses.
#[test]
fn the_walks_agree_where_a_cut_leaves_ppm_chunks_over() {
    walks_agree(&fixtures().join("packed_overrun/ppm_two_parts.j2k"));
}
