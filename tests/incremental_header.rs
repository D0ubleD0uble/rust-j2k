//! `IncrementalDecoder::header` against `read_header_with` on the same
//! bytes.
//!
//! The decoder keeps what its header read found between calls, and goes on
//! from there when more bytes come in. Each call must still give what
//! `read_header_with` gives with `strict` off on the bytes fed so far, under
//! the decoder's options: the same header, the same error, and `None` where
//! that read finds the main header incomplete. This feeds every codestream,
//! JP2 and JPH file under `tests/fixtures/` (the `header` fuzz seeds among
//! them), and files built here whose tiles are over a cap under the main
//! header, in small pieces, changing the region and the reduction on the way.

use std::path::{Path, PathBuf};

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{
    DecodeOptions, Error, IncrementalDecoder, Region, Window, read_header, read_header_with,
};

/// A splitmix64 stream: small, and the same on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A value in `1..=n`.
    fn up_to(&mut self, n: usize) -> usize {
        1 + (self.next() % n.max(1) as u64) as usize
    }
}

/// What a header read gives, as text: `Header` and `Error` have no
/// `PartialEq`, and their `Debug` text holds every field.
fn shown(result: Result<Option<rust_j2k::Header>, Error>) -> String {
    format!("{result:?}")
}

/// What `IncrementalDecoder::header` must give on `prefix` under
/// `options`: `read_header_with` with `strict` off, with an incomplete main
/// header as `None`.
fn oracle(prefix: &[u8], options: &DecodeOptions) -> String {
    shown(
        match read_header_with(prefix, &options.clone().with_strict(false)) {
            Ok(header) => Ok(Some(header)),
            Err(Error::Incomplete(_)) => Ok(None),
            Err(error) => Err(error),
        },
    )
}

/// A change of options a feed can make before the read.
#[derive(Clone)]
enum Change {
    Region(Region),
    Reduction(u8),
}

/// Feed `bytes` to a decoder under `options` in the pieces `pieces` gives,
/// making each change of `changes` when the bytes fed first reach its
/// offset, and check each `header` after a feed against [`oracle`]. Returns
/// what each call gave, in order.
fn check(
    name: &str,
    bytes: &[u8],
    options: &DecodeOptions,
    mut pieces: impl FnMut(usize) -> usize,
    changes: &[(usize, Change)],
) -> Vec<String> {
    let mut decoder = IncrementalDecoder::new(options.clone()).expect("valid options");
    let mut changes = changes.to_vec();
    changes.sort_by_key(|(at, _)| *at);
    let mut changes = changes.into_iter().peekable();
    let mut fed = 0;
    let mut seen = Vec::new();
    while fed < bytes.len() {
        let to = (fed + pieces(fed)).min(bytes.len());
        decoder.feed(&bytes[fed..to]);
        fed = to;
        while let Some((_, change)) = changes.next_if(|(at, _)| *at <= fed) {
            match change {
                Change::Region(region) => decoder.set_region(region),
                Change::Reduction(levels) => decoder.set_resolution_reduction(levels),
            }
            .expect("valid options");
        }
        let got = shown(decoder.header());
        let want = oracle(&bytes[..fed], decoder.options());
        assert_eq!(got, want, "{name}: {fed} of {} bytes", bytes.len());
        // A second call on the same bytes gives the same again.
        assert_eq!(shown(decoder.header()), want, "{name}: {fed} again");
        seen.push(got);
    }
    seen
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e, "j2k" | "j2c" | "jp2" | "jph"))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Pieces for a file of `len` bytes: a few bytes at a time through the
/// first 600, where the main header usually ends, then about 40 pieces for
/// the rest, so a large file costs few reads.
fn fixture_pieces(len: usize, rng: &mut Rng) -> impl FnMut(usize) -> usize + '_ {
    let rest = (len / 40).max(64);
    move |fed| {
        if fed < 600 {
            rng.up_to(24)
        } else {
            rng.up_to(rest)
        }
    }
}

/// A window over the middle of the image `bytes` holds, or `None` when its
/// header does not read.
fn middle(bytes: &[u8]) -> Option<Region> {
    let header = read_header(bytes).ok()?;
    let (w, h) = (header.width, header.height);
    let (x0, y0) = (header.image_area.x0, header.image_area.y0);
    Some(Region::Window(Window::new(
        x0 + w / 4,
        y0 + h / 4,
        x0 + (w - w / 4).max(w / 4 + 1),
        y0 + (h - h / 4).max(h / 4 + 1),
    )))
}

#[test]
fn each_header_of_a_fixture_fed_in_pieces_is_the_read_of_its_prefix() {
    let mut rng = Rng(0x5eed_0502);
    let paths = files(&synth::fixtures(""));
    assert!(paths.len() > 300, "{} fixtures", paths.len());
    assert!(
        paths
            .iter()
            .any(|path| path.ends_with("header/rescue_cut_sot.j2k")),
        "the header seeds are among them"
    );
    for path in &paths {
        let name = path.display().to_string();
        let bytes = synth::read_file(path);
        let len = bytes.len();
        let default = DecodeOptions::default();
        check(&name, &bytes, &default, fixture_pieces(len, &mut rng), &[]);
        let reduced = default.clone().with_resolution_reduction(1);
        check(&name, &bytes, &reduced, fixture_pieces(len, &mut rng), &[]);
        // A window from the start, then the whole image, a tile, the window
        // again and a reduction, each from a point picked at random.
        let Some(window) = middle(&bytes) else {
            continue;
        };
        let mut at = || (rng.next() % len as u64) as usize;
        let changes = [
            (at(), Change::Region(Region::Whole)),
            (at(), Change::Region(Region::Tile(0))),
            (at(), Change::Reduction(1)),
            (len, Change::Region(window.clone())),
        ];
        let windowed = default.clone().with_region(window);
        check(
            &name,
            &bytes,
            &windowed,
            fixture_pieces(len, &mut rng),
            &changes,
        );
    }
}

/// The main header of [`rescued`]: `tiles` tiles of 1040 x 512 in a row, one
/// gray component, one wavelet level, and precincts of 1x1 at resolution 0
/// and 2x2 at resolution 1, so 266,240 a tile, over the 2^18 cap.
fn rescued_main(tiles: u32) -> Vec<u8> {
    let mut cod = synth::cod_body(1, 1, 4, 0);
    cod[0] = 1;
    cod.extend([0x00, 0x11]);
    let siz = synth::siz_body([1040 * tiles, 512, 0, 0, 1040, 512, 0, 0], &[[7, 1, 1]]);
    [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8; 4])),
    ]
    .concat()
}

/// What tile `tile`'s first tile-part in [`rescued`] carries.
#[derive(Clone, Copy, PartialEq)]
enum Own {
    /// A `COD` of maximal precincts, which brings the tile within the caps.
    Rescued,
    /// No `COD`, so the tile is over the precinct cap.
    Nothing,
    /// A `COD` and `QCD` of no levels, which a reduction of 1 refuses.
    NoLevels,
    /// No tile-part at all.
    Missing,
}

/// `tiles` tiles of [`rescued_main`] of `parts` tile-parts each, whose
/// first tile-parts carry what `own` says, each tile's tile-parts together
/// or, with `interleaved`, all the first tile-parts first.
fn rescued(tiles: u32, parts: u8, own: impl Fn(u32) -> Own, interleaved: bool) -> Vec<u8> {
    let rescue = synth::seg(0xFF52, &synth::cod_body(1, 1, 4, 0));
    let no_levels = [
        synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0)),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
    ]
    .concat();
    let part = |tile: u32, part: u8| {
        let extra: &[u8] = match (part, own(tile)) {
            (0, Own::Rescued) => &rescue,
            (0, Own::NoLevels) => &no_levels,
            _ => &[],
        };
        let data: &[u8] = if part == 0 { &[0] } else { &[] };
        synth::tile_part(tile as u16, part, parts, extra, data)
    };
    let present: Vec<u32> = (0..tiles).filter(|&t| own(t) != Own::Missing).collect();
    let mut bytes = rescued_main(tiles);
    if interleaved {
        for p in 0..parts {
            for &tile in &present {
                bytes.extend(part(tile, p));
            }
        }
    } else {
        for &tile in &present {
            for p in 0..parts {
                bytes.extend(part(tile, p));
            }
        }
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// `components` gray components over a row of `width` tiles of 1x1 and a
/// row of `width` tiles of 1 x `tall`, with one level and precincts of 1x1
/// at resolution 0 and 2x2 above. Every tile has a tile-part with no `COD`
/// or `COC` and no data. (`tests/tile_geometry_cap.rs` has the same.)
fn small_row_over_tall_row(components: usize, width: u32, tall: u32) -> Vec<u8> {
    let siz = synth::siz_body(
        [width, 2 * tall, 0, tall - 1, 1, tall, 0, 0],
        &vec![[7, 1, 1]; components],
    );
    let mut cod = synth::cod_body(1, 1, 0, 0);
    cod[0] = 1;
    cod.extend([0x00, 0x11]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8; 4])),
    ]
    .concat();
    for tile in 0..2 * width {
        bytes.extend(synth::tile_part(tile as u16, 0, 1, &[], &[]));
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// `codestream` in a JP2 file with a gray `colr`, and the other `jp2h`
/// boxes `boxes`.
fn in_jp2(codestream: &[u8], boxes: &[Vec<u8>]) -> Vec<u8> {
    let mut header = vec![
        synth::ihdr_for(codestream),
        synth::bx(b"colr", &synth::colr_enum(17)),
    ];
    header.extend_from_slice(boxes);
    synth::jp2_file(&header, &[], codestream, &[])
}

/// Feed `bytes` under each of the options a tiled file is read with, in
/// pieces of 1 to 20 bytes and of 1 to 200, with region and reduction
/// changes on the way, and return what the default read gave at each feed.
fn check_tiled(name: &str, bytes: &[u8], seed: u64) -> Vec<String> {
    let mut rng = Rng(seed);
    let len = bytes.len();
    let window = Region::Window(Window::new(10 * 1040, 0, 20 * 1040, 512));
    let default = DecodeOptions::default();
    let mut seen = Vec::new();
    for most in [20, 200] {
        let mut pieces = |_| rng.up_to(most);
        let found = check(name, bytes, &default, &mut pieces, &[]);
        if most == 20 {
            seen = found;
        }
        let reduced = default.clone().with_resolution_reduction(1);
        check(name, bytes, &reduced, &mut pieces, &[]);
        let windowed = default.clone().with_region(window.clone());
        check(name, bytes, &windowed, &mut pieces, &[]);
        let tile = default.clone().with_region(Region::Tile(25));
        check(name, bytes, &tile, &mut pieces, &[]);
        let changes = [
            (len / 5, Change::Region(window.clone())),
            (len / 3, Change::Reduction(1)),
            (len / 2, Change::Region(Region::Whole)),
            (2 * len / 3, Change::Reduction(0)),
            (3 * len / 4, Change::Region(Region::Tile(25))),
        ];
        check(name, bytes, &default, &mut pieces, &changes);
    }
    seen
}

#[test]
fn each_header_of_a_tiled_file_over_a_cap_is_the_read_of_its_prefix() {
    use Own::{Missing, NoLevels, Nothing, Rescued};
    let tiles = 64;
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "every tile rescued, tile-parts together",
            rescued(tiles, 16, |_| Rescued, false),
            "Ok(Some(",
        ),
        (
            "every tile rescued, first tile-parts first",
            rescued(tiles, 16, |_| Rescued, true),
            "Ok(Some(",
        ),
        (
            "tile 40 over the cap",
            rescued(tiles, 4, |t| if t == 40 { Nothing } else { Rescued }, false),
            "Err(Limit(",
        ),
        (
            "tile 30 of no levels",
            rescued(
                tiles,
                4,
                |t| if t == 30 { NoLevels } else { Rescued },
                false,
            ),
            "Ok(Some(",
        ),
        (
            "tile 20 missing",
            rescued(tiles, 4, |t| if t == 20 { Missing } else { Rescued }, true),
            "Ok(Some(",
        ),
    ];
    for (seed, (name, bytes, want)) in cases.iter().enumerate() {
        let seen = check_tiled(name, bytes, seed as u64);
        assert!(seen.iter().any(|s| s == "Ok(None)"), "{name}: {seen:?}");
        assert!(
            seen.last().is_some_and(|last| last.starts_with(want)),
            "{name}: {:?}",
            seen.last()
        );
    }
    // The file of a tile of no levels is refused at a reduction of 1 once
    // that tile's tile-part `COD` comes in, for that tile.
    let bytes = &cases[3].1;
    let reduced = DecodeOptions::default().with_resolution_reduction(1);
    let mut rng = Rng(7);
    let seen = check("no levels", bytes, &reduced, |_| rng.up_to(30), &[]);
    let refused = seen
        .iter()
        .position(|s| s.starts_with("Err(InvalidOptions(") && s.contains("in tile 30"))
        .expect("the reduction refusal");
    assert!(seen[..refused].iter().any(|s| s.starts_with("Ok(Some(")));
}

#[test]
fn each_header_of_many_components_under_a_reduction_is_the_read_of_its_prefix() {
    let bytes = small_row_over_tall_row(1024, 24, 4096);
    let reduced = DecodeOptions::default().with_resolution_reduction(1);
    let mut rng = Rng(11);
    let seen = check("many components", &bytes, &reduced, |_| rng.up_to(40), &[]);
    let last = seen.last().expect("a call");
    assert!(last.starts_with("Err(Limit("), "{last}");
    assert!(seen.iter().any(|s| s.starts_with("Ok(Some(")));
}

/// A JP2 file's box layout fault is refused once the `jp2c` box header is
/// in: before it, each call finds the header incomplete, and from it on,
/// each gives the read's `FileFormat` error, however the codestream goes on.
#[test]
fn each_header_of_a_jp2_file_is_the_read_of_its_prefix() {
    let codestream = rescued(
        64,
        4,
        |t| if t == 40 { Own::Nothing } else { Own::Rescued },
        false,
    );
    let whole = in_jp2(&codestream, &[]);
    let seen = check_tiled("jp2", &whole, 21);
    assert!(seen.last().is_some_and(|s| s.starts_with("Err(Limit(")));
    let cdef = synth::cdef(&[(0, 0, 1)]);
    let two_cdefs = in_jp2(&codestream, &[cdef.clone(), cdef]);
    let seen = check_tiled("jp2 of two cdefs", &two_cdefs, 22);
    let first = seen
        .iter()
        .position(|s| s != "Ok(None)")
        .expect("a call past the header");
    assert!(first > 0);
    assert!(
        seen[first..]
            .iter()
            .all(|s| s.starts_with("Err(FileFormat(") && s.contains("`cdef`")),
        "{:?}",
        &seen[first..]
    );
}

/// [`rescued_main`] of `tiles` tiles, with `main` after its `QCD`, and the
/// tile-parts `parts` in order, each `(tile, TPsot, TNsot, markers)`; a
/// first tile-part has a byte of data.
fn tile_parts(tiles: u32, main: &[u8], parts: &[(u16, u8, u8, Vec<u8>)]) -> Vec<u8> {
    let mut bytes = rescued_main(tiles);
    bytes.extend_from_slice(main);
    for (tile, part, count, markers) in parts {
        let data: &[u8] = if *part == 0 { &[0] } else { &[] };
        bytes.extend(synth::tile_part(*tile, *part, *count, markers, data));
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// Where tile 0 is over the cap, each call refuses it, and gives the
/// decode's error for the tile-parts first, from the walk for it that the
/// last call kept. These files put in that walk's way what it keeps
/// between calls, or what a longer input changes:
/// - the `TPsot == TNsot` look-ahead, whose answer changes once tile 0's
///   third tile-part comes in, and raises both counts in the error for tile
///   5, which declares 2 tile-parts and then 1;
/// - `PPM` chunks, which run out at the eleventh of 16 tile-parts, or not;
/// - a first tile-part header over 1 KiB, whose coding is kept for the
///   later tile-parts that restate it, and a short one, read again for
///   each, with a later `COD` that differs;
/// - bytes after a tile-part that are neither `SOT` nor `EOC`.
#[test]
fn each_header_with_the_fault_walk_kept_is_the_read_of_its_prefix() {
    let rescue = synth::seg(0xFF52, &synth::cod_body(1, 1, 4, 0));
    let other = synth::seg(0xFF52, &synth::cod_body(2, 1, 4, 0));
    let own = |tile: u16| {
        if tile == 0 {
            Vec::new()
        } else {
            rescue.clone()
        }
    };
    // Eight tiles of two tile-parts, tile 0's first.
    let two_each = |count: &dyn Fn(u16, u8) -> u8, later: &dyn Fn(u16, u8) -> Vec<u8>| {
        let mut parts = Vec::new();
        for tile in 0..8u16 {
            for part in 0..2u8 {
                let markers = if part == 0 {
                    own(tile)
                } else {
                    later(tile, part)
                };
                parts.push((tile, part, count(tile, part), markers));
            }
        }
        parts
    };
    let nothing = |_: u16, _: u8| Vec::new();
    let mut cases: Vec<(&str, Vec<u8>, &str)> = Vec::new();
    for (name, mismatch) in [("look-ahead", true), ("look-ahead, no fault", false)] {
        let mut parts = two_each(
            &|tile, part| {
                if mismatch && tile == 5 && part == 1 {
                    1
                } else {
                    2
                }
            },
            &nothing,
        );
        parts.push((0, 2, 2, Vec::new()));
        let want = if mismatch {
            "Err(Codestream(\"tile 5 declares 2 tile-parts in one `SOT` and 3 in another"
        } else {
            "Err(Limit("
        };
        cases.push((name, tile_parts(8, &[], &parts), want));
    }
    for (name, chunks, want) in [
        ("PPM short", 10u32, "Err(Codestream(\"`PPM` carries fewer"),
        ("PPM whole", 16, "Err(Limit("),
    ] {
        let mut ppm = vec![0u8];
        for _ in 0..chunks {
            ppm.extend([0, 0, 0, 1, 0]);
        }
        let parts = two_each(&|_, _| 2, &nothing);
        cases.push((name, tile_parts(8, &synth::seg(0xFF60, &ppm), &parts), want));
    }
    let mut long = rescue.clone();
    for _ in 0..300 {
        long.extend([0xFF, 0x64, 0, 2]);
    }
    for (name, differs, want) in [
        ("restated", false, "Err(Limit("),
        (
            "restated, then differs",
            true,
            "Err(Codestream(\"a later tile-part of tile 3 carries a `COD`",
        ),
    ] {
        let mut parts = Vec::new();
        for tile in 0..8u16 {
            for part in 0..4u8 {
                let markers = match (tile, part) {
                    (2, 0) => long.clone(),
                    (_, 0) => own(tile),
                    (3, 3) if differs => other.clone(),
                    (2 | 3, _) => rescue.clone(),
                    _ => Vec::new(),
                };
                parts.push((tile, part, 4, markers));
            }
        }
        cases.push((name, tile_parts(8, &[], &parts), want));
    }
    // Tile 1's own `COD` of two levels, under the main `QCD`'s steps for
    // one: its header is gathered once and kept.
    let at_fault = synth::seg(0xFF52, &synth::cod_body(1, 2, 4, 0));
    let parts: Vec<_> = two_each(&|_, _| 2, &nothing)
        .into_iter()
        .map(|(tile, part, count, markers)| match (tile, part) {
            (1, 0) => (tile, part, count, at_fault.clone()),
            _ => (tile, part, count, markers),
        })
        .collect();
    cases.push((
        "a tile header at fault",
        tile_parts(8, &[], &parts),
        "Err(Marker(\"component 0 carries 4 quantization step entries",
    ));
    // Tile 3's own `COD` of two levels, and tile 1's of three, whose
    // tile-parts come last: the first tile at fault turns from tile 3 to
    // tile 1 once tile 1 is fed, and its header is gathered afresh.
    let three = synth::seg(0xFF52, &synth::cod_body(1, 3, 4, 0));
    let mut parts: Vec<_> = two_each(&|_, _| 2, &nothing)
        .into_iter()
        .map(|(tile, part, count, markers)| match (tile, part) {
            (3, 0) => (tile, part, count, at_fault.clone()),
            (1, 0) => (tile, part, count, three.clone()),
            _ => (tile, part, count, markers),
        })
        .collect();
    parts.sort_by_key(|&(tile, _, _, _)| tile == 1);
    cases.push((
        "a lower tile at fault later",
        tile_parts(8, &[], &parts),
        "Err(Marker(\"component 0 carries 4 quantization step entries but its 3",
    ));
    // 20 `POC` volumes in the main header, and 4 in each later tile-part of
    // tile 1: the tile goes over 32 at its fourth tile-part, and the volumes
    // the gathered header holds grow with each tile-part after.
    let poc = |volumes: usize| {
        let mut body = Vec::new();
        for _ in 0..volumes {
            body.extend([0, 0, 0, 1, 1, 1, 0]);
        }
        synth::seg(0xFF5F, &body)
    };
    let mut parts = Vec::new();
    for tile in 0..8u16 {
        for part in 0..6u8 {
            let markers = match (tile, part) {
                (_, 0) => own(tile),
                (1, _) => poc(4),
                _ => Vec::new(),
            };
            parts.push((tile, part, 6, markers));
        }
    }
    cases.push((
        "POC volumes joined",
        tile_parts(8, &poc(20), &parts),
        "Err(Limit(LimitDetail { kind: PocVolumes",
    ));
    let mut junk = tile_parts(8, &[], &two_each(&|_, _| 2, &nothing));
    let end = junk.len() - 2;
    junk[end..].copy_from_slice(&[0x12, 0x34]);
    cases.push((
        "junk after a tile-part",
        junk,
        "Err(Codestream(\"expected `SOT` or `EOC` after a tile-part, found 0x1234",
    ));

    let options = DecodeOptions::default();
    let lenient = options.clone().with_strict(false);
    for (seed, (name, bytes, want)) in cases.iter().enumerate() {
        let mut rng = Rng(100 + seed as u64);
        for most in [1, 20] {
            let seen = check(name, bytes, &options, |_| rng.up_to(most), &[]);
            let last = seen.last().expect("a call");
            assert!(last.starts_with(want), "{name}: {last}");
            // The walk is kept over many calls that refuse tile 0.
            let refusals = seen.iter().filter(|s| s.starts_with("Err(")).count();
            assert!(refusals * 2 > seen.len(), "{name}: {seen:?}");
            if *name == "look-ahead" {
                // Before tile 0's third tile-part is in, the counts stand.
                assert!(
                    seen.iter()
                        .any(|s| s.contains("declares 1 tile-parts in one"))
                );
            }
        }
        // A change of options keeps the walk, which depends on neither the
        // selection nor the reduction.
        let len = bytes.len();
        let changes = [
            (len / 4, Change::Region(Region::Tile(0))),
            (len / 2, Change::Reduction(1)),
            (3 * len / 4, Change::Region(Region::Whole)),
        ];
        let seen = check(name, bytes, &options, |_| rng.up_to(20), &changes);
        let last = seen.last().expect("a call");
        assert!(last.starts_with(want), "{name}, with changes: {last}");
        let decoded = format!("{:?}", rust_j2k::decode_with(bytes, &lenient).map(|_| ()));
        assert!(decoded.starts_with(want), "{name}: the decode, {decoded}");
    }
}

/// 100 tiles of 4096 components over the precinct cap under the main
/// header, each but `refused` brought within by a tile-part `COD`, in index
/// order (`tests/tile_geometry_cap.rs` has the same). The count of the
/// tiles' own headers runs out of its budget at tile 85: each costs 45,056
/// steps to read and 4096 to count, of the 2^22.
fn past_the_budget(refused: u16) -> Vec<u8> {
    let tiles = 100u16;
    let mut cod = synth::cod_body(1, 0, 0, 0);
    cod[0] = 1;
    cod.push(0x00);
    let siz = synth::siz_body(
        [8 * u32::from(tiles), 16, 0, 0, 8, 16, 0, 0],
        &vec![[7, 1, 1]; 4096],
    );
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
    ]
    .concat();
    let own = synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0));
    let packets = vec![0; 4096];
    for tile in 0..tiles {
        let (extra, data): (&[u8], &[u8]) = if tile == refused {
            (&[], &[])
        } else {
            (&own, &packets)
        };
        bytes.extend(synth::tile_part(tile, 0, 1, extra, data));
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// A call on more bytes is charged the budget for the tiles an earlier call
/// read, as a read from the start is, so the budget runs out at the same
/// tile: tile 66 is refused, and tile 90, past where the budget runs out, is
/// left to the decode. Were either charge left out for the tiles read
/// before, the budget would last past tile 90.
#[test]
fn each_header_of_a_file_past_the_budget_is_the_read_of_its_prefix() {
    for (refused, want) in [(66, "Err(Limit("), (90, "Ok(Some(")] {
        let bytes = past_the_budget(refused);
        let mut rng = Rng(u64::from(refused));
        let name = format!("tile {refused} refused");
        let options = DecodeOptions::default();
        let seen = check(&name, &bytes, &options, |_| rng.up_to(9000), &[]);
        let last = seen.last().expect("a call");
        assert!(last.starts_with(want), "{name}: {last}");
    }
}
