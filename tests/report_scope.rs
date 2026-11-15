//! With `strict` off, under a window, a tile or a component subset, the
//! report describes what the decode returns: the tiles it reads, the
//! components it reconstructs, and the code-blocks the window's samples
//! depend on. Data the returned samples do not need does not make the image
//! look incomplete, and data they do need is still reported.

use std::path::PathBuf;

use rust_j2k::{DecodeOptions, Image, ReportEntry, Window, decode_with, read_header};

fn read(path: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn lenient() -> DecodeOptions {
    DecodeOptions::default().with_strict(false)
}

fn decode_ok(bytes: &[u8], options: &DecodeOptions, what: &str) -> Image {
    decode_with(bytes, options).unwrap_or_else(|e| panic!("{what}: {e}"))
}

/// A cut of `bytes` at `percent` of its length.
fn cut(bytes: &[u8], percent: usize) -> &[u8] {
    &bytes[..bytes.len() * percent / 100]
}

/// `precincts_cprl_lossless` is one tile of three components, no color
/// transform, in component-major order: by 70% of the file every packet of
/// component 0 is there. A decode of component 0 alone gives the uncut
/// samples, and the report says it is complete; the whole image's report
/// still has packets missing.
#[test]
fn a_component_whose_packets_are_all_there_is_complete() {
    let bytes = read("tests/fixtures/precincts_cprl_lossless.j2k");
    let first = lenient().with_components(Some(vec![0]));
    let uncut = decode_ok(&bytes, &first, "uncut");
    for percent in 70..=97 {
        let prefix = cut(&bytes, percent);
        let part = decode_ok(prefix, &first, "component 0");
        assert_eq!(part.components, uncut.components, "{percent}%");
        assert!(part.report.is_complete(), "{percent}%: {:?}", part.report);
        assert_eq!(part.report.complete_reduction, Some(0), "{percent}%");
        let whole = decode_ok(prefix, &lenient(), "the whole image");
        assert!(!whole.report.is_complete(), "{percent}%");
        // The components whose packets are cut are still reported.
        let last = lenient().with_components(Some(vec![2]));
        let part = decode_ok(prefix, &last, "component 2");
        assert!(!part.report.is_complete(), "{percent}%");
        assert_ne!(
            part.components,
            decode_ok(&bytes, &last, "uncut").components
        );
    }
    // Cut inside component 0's packets, its samples differ, and the report
    // says so.
    let prefix = cut(&bytes, 20);
    let part = decode_ok(prefix, &first, "component 0 at 20%");
    assert_ne!(part.components, uncut.components);
    assert!(!part.report.is_complete(), "{:?}", part.report);
}

/// A window of the top-left 8 x 8 samples of component 0 needs only the
/// first precinct of each resolution, which are the first packets of the
/// file: cut at 15%, the samples are the uncut ones and the report is
/// complete, at reduction 0 as at every coarser one. The whole of component
/// 0 is not.
#[test]
fn a_window_whose_code_blocks_are_all_there_is_complete() {
    let bytes = read("tests/fixtures/precincts_cprl_lossless.j2k");
    let prefix = cut(&bytes, 15);
    let first = lenient().with_components(Some(vec![0]));
    let window = first.clone().with_window(Some(Window::new(0, 0, 8, 8)));
    let part = decode_ok(prefix, &window, "the window");
    assert_eq!(
        part.components,
        decode_ok(&bytes, &window, "uncut").components
    );
    assert!(part.report.is_complete(), "{:?}", part.report);
    assert_eq!(part.report.complete_reduction, Some(0));
    let all = decode_ok(prefix, &first, "component 0");
    assert!(!all.report.is_complete());
    // A window over the bottom-right corner needs the last precincts of
    // component 0, which the cut leaves out.
    let far = first.with_window(Some(Window::new(100, 80, 128, 96)));
    let part = decode_ok(prefix, &far, "the far window");
    assert!(!part.report.is_complete(), "{:?}", part.report);
    assert_ne!(part.components, decode_ok(&bytes, &far, "uncut").components);
}

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

    /// A value in `0..n`.
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % u64::from(n.max(1))) as u32
    }
}

/// The tile grid of a raw codestream, from its SIZ marker, which follows
/// SOC: the image area `XOsiz..Xsiz` by `YOsiz..Ysiz`, the tile size and the
/// tile grid's origin.
struct Grid {
    image: (u32, u32, u32, u32),
    tile: (u32, u32),
    origin: (u32, u32),
}

impl Grid {
    fn of(bytes: &[u8]) -> Self {
        assert_eq!(&bytes[..4], &[0xff, 0x4f, 0xff, 0x51], "SOC then SIZ");
        let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        Grid {
            image: (word(16), word(20), word(8), word(12)),
            tile: (word(24), word(28)),
            origin: (word(32), word(36)),
        }
    }

    /// The tiles across.
    fn across(&self) -> u32 {
        (self.image.2 - self.origin.0).div_ceil(self.tile.0)
    }

    /// The indices of the tiles that hold part of `window`.
    fn tiles(&self, window: Window) -> Vec<u32> {
        let (ix0, iy0, ix1, iy1) = self.image;
        let (x0, y0) = (window.x0.max(ix0), window.y0.max(iy0));
        let (x1, y1) = (window.x1.min(ix1), window.y1.min(iy1));
        let (tw, th) = self.tile;
        let (ox, oy) = self.origin;
        let mut out = Vec::new();
        for ty in (y0 - oy) / th..(y1 - oy).div_ceil(th) {
            for tx in (x0 - ox) / tw..(x1 - ox).div_ceil(tw) {
                out.push(ty * self.across() + tx);
            }
        }
        out
    }
}

/// Where each tile's data ends in `bytes`: the end of its last tile-part,
/// found by stepping from the first `SOT` by each `Psot`.
fn tile_ends(bytes: &[u8]) -> Vec<usize> {
    let mut at = bytes
        .windows(2)
        .position(|w| w == [0xFF, 0x90])
        .expect("an SOT");
    let mut ends = Vec::new();
    while bytes[at..at + 2] == [0xFF, 0x90] {
        let tile = usize::from(u16::from_be_bytes([bytes[at + 4], bytes[at + 5]]));
        let psot = u32::from_be_bytes(bytes[at + 6..at + 10].try_into().unwrap()) as usize;
        assert_ne!(psot, 0, "the fixtures give every Psot");
        if ends.len() <= tile {
            ends.resize(tile + 1, 0);
        }
        ends[tile] = at + psot;
        at += psot;
    }
    ends
}

/// The samples of `whole`'s component `c` inside `window` cut to `image`,
/// at reduction `r`.
fn crop(whole: &Image, c: usize, image: (u32, u32, u32, u32), window: Window, r: u8) -> Vec<i32> {
    let at = |v: u32, d: u32| v.div_ceil(d).div_ceil(1 << r);
    let plane = &whole.components[c];
    let (dx, dy) = (u32::from(plane.x_sampling), u32::from(plane.y_sampling));
    let (x0, y0) = (window.x0.max(image.0), window.y0.max(image.1));
    let (x1, y1) = (window.x1.min(image.2), window.y1.min(image.3));
    let (ox, oy) = (at(image.0, dx), at(image.1, dy));
    let mut out = Vec::new();
    for y in at(y0, dy)..at(y1, dy) {
        for x in at(x0, dx)..at(x1, dx) {
            out.push(plane.samples[((y - oy) * plane.width + (x - ox)) as usize]);
        }
    }
    out
}

/// A random window on the image area: mostly small, so it falls inside a
/// tile and a few of its precincts, sometimes wide.
fn window(rng: &mut Rng, image: (u32, u32, u32, u32)) -> Window {
    let (x0, y0, x1, y1) = image;
    let mut axis = |lo: u32, hi: u32| {
        let start = lo + rng.below(hi - lo);
        let reach = [4, 16, 48, hi - lo][rng.below(4) as usize];
        (start, (start + 1 + rng.below(reach)).min(hi))
    };
    let (wx0, wx1) = axis(x0, x1);
    let (wy0, wy1) = axis(y0, y1);
    Window::new(wx0, wy0, wx1, wy1)
}

/// How the decodes of [`check`] came out.
struct Counts {
    /// Reported complete.
    complete: usize,
    /// Reported incomplete, with samples other than the uncut ones.
    incomplete: usize,
    /// Reported incomplete, with the uncut samples.
    same_incomplete: usize,
}

/// For random windows and component subsets of `name`, each decoded from
/// a series of cuts at reductions 0 and 1:
///
/// - the report is complete only when the samples are the uncut ones;
/// - it is complete whenever the whole image's report at that cut is;
/// - it is complete whenever every tile the window reads is wholly there;
/// - once complete at a cut, it stays complete at every longer one;
/// - few decodes that give the uncut samples are reported incomplete.
///
/// Returns how the decodes came out, so the caller can check each outcome
/// happened, and how often the report missed data the samples did not need.
fn check(name: &str, draws: usize, seed: u64) -> Counts {
    let bytes = read(name);
    let grid = Grid::of(&bytes);
    let ends = tile_ends(&bytes);
    let components = read_header(&bytes).unwrap().components.len() as u16;
    let start = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap() + 14;
    let cuts: Vec<usize> = (0..12)
        .map(|i| start + (bytes.len() - start) * i / 12)
        .chain([bytes.len()])
        .collect();
    let mut rng = Rng(seed);
    let (mut complete, mut incomplete, mut same_incomplete) = (0, 0, 0);
    for r in 0..2u8 {
        let base = lenient().with_resolution_reduction(r);
        let uncut = decode_ok(&bytes, &base, name);
        let wholes: Vec<bool> = cuts
            .iter()
            .map(|&at| decode_ok(&bytes[..at], &base, name).report.is_complete())
            .collect();
        for _ in 0..draws {
            let window = (rng.below(4) != 0).then(|| window(&mut rng, grid.image));
            let subset = match rng.below(3) {
                0 => None,
                1 => Some(vec![rng.below(u32::from(components)) as u16]),
                _ => Some(vec![components - 1, 0]),
            };
            let options = base
                .clone()
                .with_window(window)
                .with_components(subset.clone());
            let what = format!("{name} -r {r} {window:?} {subset:?}");
            let returned: Vec<usize> = subset.as_ref().map_or_else(
                || (0..usize::from(components)).collect(),
                |s| s.iter().map(|&c| usize::from(c)).collect(),
            );
            let expected: Vec<Vec<i32>> = returned
                .iter()
                .map(|&c| match window {
                    Some(w) => crop(&uncut, c, grid.image, w, r),
                    None => uncut.components[c].samples.clone(),
                })
                .collect();
            // The tiles read, and where the last of them ends.
            let read_end = match window {
                Some(w) => grid
                    .tiles(w)
                    .iter()
                    .map(|&t| ends[t as usize])
                    .max()
                    .unwrap(),
                None => *ends.iter().max().unwrap(),
            };
            let mut was_complete = false;
            for (&at, &whole_complete) in cuts.iter().zip(&wholes) {
                let part = match decode_with(&bytes[..at], &options) {
                    Ok(part) => part,
                    // A window holding no sample of a component at this
                    // reduction is refused, whatever the cut.
                    Err(rust_j2k::Error::InvalidOptions(_)) => break,
                    Err(e) => panic!("{what} at {at}: {e}"),
                };
                let samples: Vec<&Vec<i32>> = part.components.iter().map(|c| &c.samples).collect();
                let same = samples.iter().zip(&expected).all(|(a, b)| *a == b);
                let is = part.report.is_complete();
                let report = &part.report;
                assert!(
                    !is || same,
                    "{what} at {at}: complete, but differs: {report:?}"
                );
                assert!(
                    !whole_complete || is,
                    "{what} at {at}: the whole is complete: {report:?}"
                );
                assert!(
                    at < read_end || is,
                    "{what} at {at}: every tile read is there: {report:?}"
                );
                assert!(
                    !was_complete || is,
                    "{what} at {at}: was complete at a shorter cut"
                );
                was_complete = is;
                if is {
                    complete += 1;
                } else if same {
                    same_incomplete += 1;
                } else {
                    incomplete += 1;
                }
            }
        }
    }
    Counts {
        complete,
        incomplete,
        same_incomplete,
    }
}

/// The randomized check on fixtures with precincts and layers, with tiles,
/// with a color transform, and on 9/7 with sub-sampled components.
#[test]
fn random_windows_and_subsets_of_cut_codestreams_report_what_they_return() {
    for (seed, (name, draws)) in [
        ("tests/fixtures/precincts_cprl_lossless.j2k", 8),
        ("tests/fixtures/precincts_lrcp_lossless.j2k", 6),
        ("tests/fixtures/precincts_tiled_cprl_lossless.j2k", 8),
        ("tests/fixtures/window/tiled_53.j2k", 6),
        ("tests/fixtures/window/sub_97.j2k", 6),
    ]
    .into_iter()
    .enumerate()
    {
        let Counts {
            complete,
            incomplete,
            same_incomplete,
        } = check(name, draws, 500 + seed as u64);
        assert!(
            complete > 0 && incomplete > 0,
            "{name}: {complete} complete, {incomplete} not"
        );
        // Data the samples do not need can still be reported missing: a
        // packet that adds nothing, or a code-block of the footprint's
        // margin whose samples the window drops in the end. That is rare;
        // a report of whole tiles or every component makes it common.
        assert!(
            same_incomplete * 10 <= complete + same_incomplete,
            "{name}: {same_incomplete} of {} decodes with the uncut samples are reported \
             incomplete",
            complete + same_incomplete
        );
    }
}

/// The runs of missing tiles on `image`'s report.
fn missing_tiles(image: &Image) -> Vec<(u32, u32)> {
    image
        .report
        .entries
        .iter()
        .filter_map(|e| match e {
            ReportEntry::MissingTiles { tiles, .. } => Some((tiles.start, tiles.end)),
            _ => None,
        })
        .collect()
}

/// A window down the first column of `tiled_53`'s 3 x 3 tiles, cut before
/// tile 3, reads tiles 3 and 6 of the missing run 3..9: the report names
/// those two alone, not the tiles between them it does not read.
#[test]
fn a_run_of_missing_tiles_is_split_at_the_tiles_a_window_does_not_read() {
    let bytes = read("tests/fixtures/window/tiled_53.j2k");
    let ends = tile_ends(&bytes);
    let prefix = &bytes[..ends[2]];
    let window = lenient().with_window(Some(Window::new(5, 4, 20, 58)));
    let part = decode_ok(prefix, &window, "the first column");
    assert_eq!(missing_tiles(&part), [(3, 4), (6, 7)]);
    assert!(!part.report.is_complete());
}
