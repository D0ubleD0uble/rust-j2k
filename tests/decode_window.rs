//! Decoding part of an image: `DecodeOptions::region` (a window or a tile)
//! and `DecodeOptions::components`.
//!
//! Each oracle is `opj_decompress -d`, `-t` or `-c` 2.5.0 on the same bytes;
//! see `tests/fixtures/window/README.md`. The 5/3 oracles are the SHA-256 of
//! the samples, graded bit-exactly. The 9/7 ones are OpenJPEG's whole decode
//! at each reduction, as PGX: the generator checks that every `-d` and `-t`
//! output is the matching crop of it, and the test grades the crop within the
//! tolerance of the whole decode.

mod support;
use support::synth;

use std::path::PathBuf;

use rust_j2k::{
    ColorOutput, DecodeOptions, Error, Image, Region, ReportEntry, Window, decode, decode_with,
    read_header_with,
};
use support::jp2::sha256_hex;
use support::pgx::parse_pgx;

fn dir() -> PathBuf {
    synth::fixtures("window")
}

fn read(name: &str) -> Vec<u8> {
    synth::read_file(dir().join(name))
}

/// Where a fixture's image and tiles sit on the reference grid.
struct Geometry {
    /// The image area, `x0, y0, x1, y1`.
    image: (u32, u32, u32, u32),
    tile_origin: (u32, u32),
    tile: (u32, u32),
    /// Tiles across.
    columns: u32,
    /// Each component's sub-sampling.
    sampling: [(u32, u32); 3],
}

/// `tiled_*`: 70 x 54 at (5, 4), 24 x 20 tiles from (3, 2).
const TILED: Geometry = Geometry {
    image: (5, 4, 75, 58),
    tile_origin: (3, 2),
    tile: (24, 20),
    columns: 3,
    sampling: [(1, 1); 3],
};

/// `sub_*`: 67 x 51 at (7, 5), 16 x 14 tiles from (2, 3), the components
/// sampled 1x1, 2x2 and 3x2.
const SUB: Geometry = Geometry {
    image: (7, 5, 74, 56),
    tile_origin: (2, 3),
    tile: (16, 14),
    columns: 5,
    sampling: [(1, 1), (2, 2), (3, 2)],
};

fn geometry(name: &str) -> &'static Geometry {
    if name.starts_with("sub_") {
        &SUB
    } else {
        &TILED
    }
}

/// One oracle line: the `opj_decompress` arguments with the spaces taken out,
/// the output size and the samples' hash.
struct Case {
    args: String,
    options: DecodeOptions,
    size: (u32, u32),
    hash: String,
}

/// The options that make the decode `args` asks `opj_decompress` for.
fn options_of(args: &str) -> DecodeOptions {
    let mut options = DecodeOptions::default();
    let mut rest = args;
    while !rest.is_empty() {
        let flag = &rest[..2];
        let end = rest[2..].find('-').map_or(rest.len(), |i| i + 2);
        let value = &rest[2..end];
        let numbers: Vec<u32> = value.split(',').map(|v| v.parse().unwrap()).collect();
        options = match flag {
            "-d" => options.with_region(Region::Window(Window::new(
                numbers[0], numbers[1], numbers[2], numbers[3],
            ))),
            "-t" => options.with_region(Region::Tile(numbers[0])),
            "-r" => options.with_resolution_reduction(numbers[0] as u8),
            "-c" => options.with_components(Some(numbers.iter().map(|&c| c as u16).collect())),
            other => panic!("unknown flag {other} in {args}"),
        };
        rest = &rest[end..];
    }
    options
}

fn cases(name: &str) -> Vec<Case> {
    let text = String::from_utf8(read(&format!("{name}.sha256"))).unwrap();
    text.lines()
        .map(|line| {
            let mut fields = line.split(' ');
            let args = fields.next().unwrap().to_string();
            let (w, h) = fields.next().unwrap().split_once('x').unwrap();
            let hash = fields.next().unwrap().to_string();
            Case {
                options: options_of(&args),
                args,
                size: (w.parse().unwrap(), h.parse().unwrap()),
                hash,
            }
        })
        .collect()
}

fn sample_bytes(image: &Image) -> Vec<u8> {
    image
        .components
        .iter()
        .flat_map(|c| c.samples.iter().flat_map(|s| s.to_le_bytes()))
        .collect()
}

fn decode_ok(bytes: &[u8], options: &DecodeOptions, what: &str) -> Image {
    decode_with(bytes, options).unwrap_or_else(|e| panic!("{what}: {e}"))
}

/// The reference-grid area `options` asks for, cut to the image.
fn area_of(geo: &Geometry, options: &DecodeOptions) -> (u32, u32, u32, u32) {
    let (x0, y0, x1, y1) = geo.image;
    if let Region::Window(w) = options.region {
        return (w.x0.max(x0), w.y0.max(y0), w.x1.min(x1), w.y1.min(y1));
    }
    if let Region::Tile(t) = options.region {
        let (p, q) = (t % geo.columns, t / geo.columns);
        let ((tx, ty), (tw, th)) = (geo.tile_origin, geo.tile);
        return (
            (tx + tw * p).max(x0),
            (ty + th * q).max(y0),
            (tx + tw * (p + 1)).min(x1),
            (ty + th * (q + 1)).min(y1),
        );
    }
    geo.image
}

/// The image size `options` decodes to: the area at the reduction.
fn size_of(geo: &Geometry, options: &DecodeOptions) -> (u32, u32) {
    let up = |v: u32| v.div_ceil(1 << options.resolution_reduction);
    let (x0, y0, x1, y1) = area_of(geo, options);
    (up(x1) - up(x0), up(y1) - up(y0))
}

/// The samples of component `c`'s `plane` (`width` wide, the whole image at
/// reduction `r`) inside the area `options` asks for: on the component's
/// grid, from `ceil(x0 / dx)` to `ceil(x1 / dx)`, then reduced.
fn crop(
    geo: &Geometry,
    c: usize,
    plane: &[i32],
    width: u32,
    options: &DecodeOptions,
) -> (u32, u32, Vec<i32>) {
    let r = options.resolution_reduction;
    let (dx, dy) = geo.sampling[c];
    let at = |v: u32, d: u32| v.div_ceil(d).div_ceil(1 << r);
    let (x0, y0, x1, y1) = area_of(geo, options);
    let (ax0, ay0, ax1, ay1) = (at(x0, dx), at(y0, dy), at(x1, dx), at(y1, dy));
    let (ox, oy) = (at(geo.image.0, dx), at(geo.image.1, dy));
    let mut out = Vec::new();
    for y in ay0..ay1 {
        for x in ax0..ax1 {
            out.push(plane[((y - oy) * width + (x - ox)) as usize]);
        }
    }
    (ax1 - ax0, ay1 - ay0, out)
}

/// The components `options` returns, as SIZ indices.
fn picked(options: &DecodeOptions) -> Vec<usize> {
    options.components.as_ref().map_or_else(
        || vec![0, 1, 2],
        |c| c.iter().map(|&c| usize::from(c)).collect(),
    )
}

/// The 5/3 codestreams decode every case to OpenJPEG's samples, bit for bit:
/// windows off the tile grid, at the image's edges, clipped, at a reduction,
/// single tiles, and component subsets with and without the color
/// transform. Under the transform the oracle is the whole decode's
/// components, since OpenJPEG's `-c` skips the transform. `sub_53` has
/// sub-sampled components and its image and tiles off the grid's origin.
#[test]
fn the_53_cases_match_openjpeg_bit_exactly() {
    for (name, count) in [("tiled_53", 18), ("tiled_53_nomct", 18), ("sub_53", 17)] {
        let bytes = read(&format!("{name}.j2k"));
        let all = cases(name);
        assert_eq!(all.len(), count, "{name}");
        for case in all {
            let what = format!("{name} {}", case.args);
            let image = decode_ok(&bytes, &case.options, &what);
            assert_eq!(
                (image.width, image.height),
                size_of(geometry(name), &case.options),
                "{what}"
            );
            // The size OpenJPEG gives is its first component's.
            let first = &image.components[0];
            assert_eq!((first.width, first.height), case.size, "{what}");
            assert_eq!(
                image.components.len(),
                picked(&case.options).len(),
                "{what}"
            );
            assert_eq!(sha256_hex(&sample_bytes(&image)), case.hash, "{what}");
        }
    }
}

/// The 9/7 codestream decodes every case to the crop of OpenJPEG's whole
/// decode, which is its `-d` and `-t` output, within 1, as the whole decode
/// is.
#[test]
fn the_97_cases_match_openjpeg_within_one() {
    for name in ["tiled_97", "sub_97"] {
        let geo = geometry(name);
        let bytes = read(&format!("{name}.j2k"));
        for case in cases(name) {
            let what = format!("{name} {}", case.args);
            let image = decode_ok(&bytes, &case.options, &what);
            assert_eq!(
                (image.width, image.height),
                size_of(geo, &case.options),
                "{what}"
            );
            assert_eq!(
                (image.components[0].width, image.components[0].height),
                case.size,
                "{what}"
            );
            let r = case.options.resolution_reduction;
            for (component, &c) in image.components.iter().zip(&picked(&case.options)) {
                let whole = parse_pgx(&read(&format!("{name}_r{r}_{c}.pgx"))).unwrap();
                let (w, h, expected) = crop(geo, c, &whole.samples, whole.width, &case.options);
                assert_eq!((component.width, component.height), (w, h), "{what}");
                let worst = component
                    .samples
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| (a - b).abs())
                    .max()
                    .unwrap();
                assert!(worst <= 1, "{what}: component {c} differs by {worst}");
            }
        }
    }
}

/// Every case is the crop of this decoder's own whole decode at the same
/// reduction, bit for bit, on all three codestreams: the window and the
/// component subset change what is returned, never a sample.
#[test]
fn every_case_is_the_crop_of_the_whole_decode() {
    for name in ["tiled_53", "tiled_53_nomct", "tiled_97", "sub_53", "sub_97"] {
        let geo = geometry(name);
        let bytes = read(&format!("{name}.j2k"));
        let wholes: Vec<Image> = (0..3)
            .map(|r| {
                decode_ok(
                    &bytes,
                    &DecodeOptions::default().with_resolution_reduction(r),
                    name,
                )
            })
            .collect();
        for case in cases(name) {
            let what = format!("{name} {}", case.args);
            let image = decode_ok(&bytes, &case.options, &what);
            let whole = &wholes[usize::from(case.options.resolution_reduction)];
            for (component, &c) in image.components.iter().zip(&picked(&case.options)) {
                let plane = &whole.components[c];
                let (w, h, expected) = crop(geo, c, &plane.samples, plane.width, &case.options);
                assert_eq!((component.width, component.height), (w, h), "{what}");
                assert_eq!(component.samples, expected, "{what}: component {c}");
                assert_eq!(component.bit_depth, plane.bit_depth, "{what}");
            }
        }
    }
}

/// `read_header_with` describes the image each case decodes to.
#[test]
fn the_header_describes_each_case() {
    for name in ["tiled_53", "tiled_97", "sub_53", "sub_97"] {
        let bytes = read(&format!("{name}.j2k"));
        for case in cases(name) {
            let what = format!("{name} {}", case.args);
            let header = read_header_with(&bytes, &case.options).unwrap();
            let image = decode_ok(&bytes, &case.options, &what);
            assert_eq!((header.width, header.height), (image.width, image.height));
            let sizes: Vec<_> = header
                .components
                .iter()
                .map(|c| (c.width, c.height))
                .collect();
            let decoded: Vec<_> = image
                .components
                .iter()
                .map(|c| (c.width, c.height))
                .collect();
            assert_eq!(sizes, decoded, "{what}");
        }
    }
}

fn invalid(bytes: &[u8], options: &DecodeOptions) -> String {
    let header = read_header_with(bytes, options);
    let decoded = decode_with(bytes, options);
    match (header, decoded) {
        (Err(Error::InvalidOptions(a)), Err(Error::InvalidOptions(b))) => {
            assert_eq!(a.message(), b.message());
            a.message().to_owned()
        }
        other => panic!("{options:?}: {other:?}"),
    }
}

/// What OpenJPEG refuses, this refuses as `Error::InvalidOptions`, from the
/// header and from the decode alike: an empty window, one outside the image
/// area, one that holds no sample at the reduction, a tile past the grid, and
/// a component past SIZ's or named twice. An empty list, a window with a
/// tile, and a subset with sRGB output contradict themselves.
#[test]
fn bad_options_are_invalid() {
    let bytes = read("tiled_53.j2k");
    let window = |x0, y0, x1, y1| {
        DecodeOptions::default().with_region(Region::Window(Window::new(x0, y0, x1, y1)))
    };
    let message = invalid(&bytes, &window(20, 10, 20, 31));
    assert_eq!(message, "the window 20..20 by 10..31 is empty");
    invalid(&bytes, &window(40, 10, 30, 31));
    // `opj_decompress -d 76,10,80,20`: "is outside the image area".
    let message = invalid(&bytes, &window(76, 10, 80, 20));
    assert_eq!(
        message,
        "the window 76..80 by 10..20 is outside the image area 5..75 by 4..58"
    );
    // Ends before XOsiz, and at it.
    invalid(&bytes, &window(0, 0, 4, 20));
    invalid(&bytes, &window(0, 0, 5, 20));
    // Starts at Xsiz.
    invalid(&bytes, &window(75, 0, 80, 20));
    // One column at an odd x: no sample at reduction 1, as OpenJPEG fails it.
    let message = invalid(&bytes, &window(21, 10, 22, 31).with_resolution_reduction(1));
    assert_eq!(
        message,
        "the area 21..22 by 10..31 holds no sample of component 0 at resolution reduction 1"
    );
    // The same column decodes at full resolution.
    assert!(decode_with(&bytes, &window(21, 10, 22, 31)).is_ok());

    let message = invalid(
        &bytes,
        &DecodeOptions::default().with_region(Region::Tile(9)),
    );
    assert_eq!(message, "tile 9 is past the image's 9 tiles (0 to 8)");

    let components = |list: Vec<u16>| DecodeOptions::default().with_components(Some(list));
    let message = invalid(&bytes, &components(vec![3]));
    assert_eq!(
        message,
        "`DecodeOptions::components` names component 3, but the image has 3"
    );
    let message = invalid(&bytes, &components(vec![1, 0, 1]));
    assert_eq!(
        message,
        "`DecodeOptions::components` names component 1 more than once"
    );
    invalid(&bytes, &components(vec![]));
    invalid(&bytes, &components(vec![0]).with_color(ColorOutput::Srgb));
}

/// A window keeps the samples it covers whatever its reduction, so a window
/// of the whole image is the whole decode, report and all.
#[test]
fn a_window_of_the_whole_image_is_the_whole_decode() {
    for name in ["tiled_53", "tiled_97"] {
        let bytes = read(&format!("{name}.j2k"));
        for r in 0..3 {
            let options = DecodeOptions::default().with_resolution_reduction(r);
            let whole = decode_ok(&bytes, &options, name);
            let windowed = decode_ok(
                &bytes,
                &options
                    .clone()
                    .with_region(Region::Window(Window::new(0, 0, u32::MAX, u32::MAX))),
                name,
            );
            assert_eq!(windowed, whole, "{name} -r {r}");
        }
    }
}

/// Where tile `index`'s first tile-part starts in `bytes`.
fn sot_of(bytes: &[u8], index: u16) -> usize {
    (0..bytes.len() - 6)
        .find(|&at| {
            bytes[at..at + 2] == [0xFF, 0x90]
                && bytes[at + 2..at + 4] == [0x00, 0x0A]
                && u16::from_be_bytes([bytes[at + 4], bytes[at + 5]]) == index
        })
        .unwrap_or_else(|| panic!("no SOT of tile {index}"))
}

/// With `strict` off, a codestream cut short decodes a window whose tiles are
/// all there to the same samples as the whole codestream, with a complete
/// report: the missing tiles outside it are not reported. A window over a
/// missing tile reports the run of missing tiles cut to it.
#[test]
fn a_cut_codestream_reports_only_the_tiles_a_window_reads() {
    let bytes = read("tiled_53.j2k");
    let prefix = &bytes[..sot_of(&bytes, 2)];
    let loose = DecodeOptions::default().with_strict(false);

    let tile = |index| loose.clone().with_region(Region::Tile(index));
    let whole = decode_ok(
        &bytes,
        &DecodeOptions::default().with_region(Region::Tile(1)),
        "tile 1",
    );
    let cut = decode_ok(prefix, &tile(1), "the prefix's tile 1");
    assert!(cut.report.is_complete(), "{:?}", cut.report);
    assert_eq!(cut.components, whole.components);
    // Strict still refuses the cut codestream, whatever the tile.
    assert!(matches!(
        decode_with(
            prefix,
            &DecodeOptions::default().with_region(Region::Tile(1))
        ),
        Err(Error::Codestream(_))
    ));
    // The whole prefix misses tiles 2 to 8.
    let all = decode_ok(prefix, &loose, "the prefix");
    assert_eq!(missing_tiles(&all), [(2, 9)]);

    let cut = decode_ok(prefix, &tile(5), "the prefix's tile 5");
    assert!(!cut.report.is_complete());
    assert_eq!(missing_tiles(&cut), [(5, 6)]);
    assert!(
        cut.components
            .iter()
            .all(|c| c.samples.iter().all(|&s| s == 0))
    );

    // A window over tiles 1 and 4 reads tile 4 of the missing run.
    let window = loose.with_region(Region::Window(Window::new(30, 10, 40, 30)));
    let cut = decode_ok(prefix, &window, "the prefix's window");
    assert_eq!(missing_tiles(&cut), [(4, 5)]);
}

/// With `strict` off, a tile of a cut codestream decodes to what
/// `opj_decompress -allow-partial -t` 2.5.0 gives on the same prefix. The
/// prefix is `tiled_53.j2k` cut at 7000 bytes, inside tile 6, so tiles 0 to
/// 5 are whole and tile 6 is cut. `-allow-partial -d` refuses the same prefix ("Stream too
/// short"), even for a window inside tile 2, so `-t` is the oracle here and a
/// window is checked as the crop of its tile.
#[test]
fn a_tile_of_a_cut_codestream_matches_openjpeg_allow_partial() {
    let bytes = read("tiled_53.j2k");
    let prefix = &bytes[..7000];
    let loose = DecodeOptions::default().with_strict(false);
    // `opj_decompress -i cut.j2k -o t.pgx -allow-partial -t N`, hashed as
    // the `.sha256` lines are. Tile 4's is the uncut file's `-t4` line;
    // tile 6 is the cut one.
    for (tile, hash) in [
        (
            2,
            "cad0b587265586e577841bee16f6b881555a6a41257a4578f84cb6fe0b1aabb8",
        ),
        (
            4,
            "0888ba413060d42436dcb8c831a7ac278d96a566bd1e4bb2bfc3838a6334c940",
        ),
        (
            6,
            "22bdd0ff24cdd365337be614620bf80a1eb60401ee437c9c0710c57acaa67dbb",
        ),
    ] {
        let image = decode_ok(
            prefix,
            &loose.clone().with_region(Region::Tile(tile)),
            "tile",
        );
        assert_eq!(sha256_hex(&sample_bytes(&image)), hash, "tile {tile}");
    }

    // A window inside tile 2 (x 51..75, y 4..22) returns that tile's samples
    // and reports only the missing end of the codestream.
    let options = loose
        .clone()
        .with_region(Region::Window(Window::new(52, 5, 74, 21)));
    let window = decode_ok(prefix, &options, "window");
    let tile = decode_ok(prefix, &loose.with_region(Region::Tile(2)), "tile 2");
    // Tile 2's area is x 51..75 by y 4..22, so the window is its columns
    // 1..23 and rows 1..17.
    for (c, (got, of)) in window.components.iter().zip(&tile.components).enumerate() {
        assert_eq!((got.width, got.height), (22, 16));
        let want: Vec<i32> = (1..17)
            .flat_map(|y| (1..23).map(move |x| of.samples[y * 24 + x]))
            .collect();
        assert_eq!(got.samples, want, "component {c}");
    }
    assert!(
        matches!(
            window.report.entries.as_slice(),
            [ReportEntry::MissingEoc { .. }]
        ),
        "{:?}",
        window.report
    );
    assert_eq!(window.report.complete_reduction, Some(0));
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

/// On a JP2 file a component subset names codestream components and returns
/// them as coded, as `raw_channels` does, and the channel labels follow the
/// subset. A window cuts the default decode.
#[test]
fn a_jp2_subset_is_the_raw_components() {
    let bytes = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jp2_cdef_rgba.jp2"),
    )
    .unwrap();
    let raw = decode_ok(
        &bytes,
        &DecodeOptions::default().with_raw_channels(true),
        "raw",
    );
    let options = DecodeOptions::default().with_components(Some(vec![3, 0]));
    let some = decode_ok(&bytes, &options, "subset");
    assert_eq!(
        some.components,
        [raw.components[3].clone(), raw.components[0].clone()]
    );
    let raw_meta = raw.jp2.unwrap();
    let meta = some.jp2.as_ref().unwrap();
    assert_eq!(meta.channels, [raw_meta.channels[3], raw_meta.channels[0]]);
    assert!(!meta.palette_applied);
    let header = read_header_with(&bytes, &options).unwrap();
    assert_eq!(header.components.len(), 2);
    assert_eq!(header.jp2.as_ref(), some.jp2.as_ref());

    let whole = decode(&bytes).unwrap();
    let window = DecodeOptions::default().with_region(Region::Window(Window::new(3, 2, 11, 9)));
    let part = decode_ok(&bytes, &window, "window");
    assert_eq!((part.width, part.height), (8, 7));
    assert_eq!(part.jp2, whole.jp2);
    for (p, w) in part.components.iter().zip(&whole.components) {
        let expected: Vec<i32> = (2..9)
            .flat_map(|y| (3..11).map(move |x| (x, y)))
            .map(|(x, y)| w.sample(x, y).unwrap())
            .collect();
        assert_eq!(p.samples, expected);
    }
}

/// sRGB output from sub-sampled sYCC replicates the chroma by the parity of
/// the decoded area's origin, so part of the image would not be the crop of
/// the whole image's conversion: a window smaller than the image is refused, from
/// the header and from the decode alike. The samples as coded decode, and so
/// does sRGB output that passes the channels through.
#[test]
fn srgb_from_sub_sampled_sycc_refuses_a_window() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let srgb = DecodeOptions::default().with_color(ColorOutput::Srgb);
    for name in [
        "jp2_sycc_420_even",
        "jp2_sycc_420_odd",
        "jp2_sycc_422_even",
        "jp2_sycc_422_odd",
    ] {
        let bytes = std::fs::read(fixtures.join(format!("{name}.jp2"))).unwrap();
        assert!(decode_with(&bytes, &srgb).is_ok(), "{name}");
        let window = Window::new(1, 1, 7, 5);
        let options = srgb.clone().with_region(Region::Window(window));
        assert!(
            matches!(decode_with(&bytes, &options), Err(Error::Unsupported(_))),
            "{name}"
        );
        assert!(
            matches!(
                read_header_with(&bytes, &options),
                Err(Error::Unsupported(_))
            ),
            "{name}"
        );
        let coded = DecodeOptions::default().with_region(Region::Window(window));
        assert!(decode_with(&bytes, &coded).is_ok(), "{name}");
    }

    // RGB passes through sRGB output, so a window of it is the crop.
    let bytes = std::fs::read(fixtures.join("jp2_cdef_rgba.jp2")).unwrap();
    let whole = decode_ok(&bytes, &srgb, "whole");
    let part = decode_ok(
        &bytes,
        &srgb
            .clone()
            .with_region(Region::Window(Window::new(3, 2, 11, 9))),
        "window",
    );
    for (p, w) in part.components.iter().zip(&whole.components) {
        let expected: Vec<i32> = (2..9)
            .flat_map(|y| (3..11).map(move |x| (x, y)))
            .map(|(x, y)| w.sample(x, y).unwrap())
            .collect();
        assert_eq!(p.samples, expected);
    }
}

/// An area equal to the image has the whole image's chroma phase, so sRGB
/// output from sub-sampled sYCC decodes it as the plain decode does: a window
/// clipped to the whole image, and the only tile, at any reduction. An area
/// that differs in origin only, or in size only, is still refused, from the
/// header and from the decode alike.
#[test]
fn srgb_from_sub_sampled_sycc_decodes_an_area_equal_to_the_image() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let srgb = DecodeOptions::default().with_color(ColorOutput::Srgb);
    let everything = Window::new(0, 0, u32::MAX, u32::MAX);
    for name in [
        "jp2_sycc_420_even",
        "jp2_sycc_420_odd",
        "jp2_sycc_422_even",
        "jp2_sycc_422_odd",
    ] {
        let bytes = std::fs::read(fixtures.join(format!("{name}.jp2"))).unwrap();
        for reduction in [0, 1] {
            let plain = srgb.clone().with_resolution_reduction(reduction);
            let whole = decode_ok(&bytes, &plain, name);
            for options in [
                plain.clone().with_region(Region::Window(everything)),
                plain.clone().with_region(Region::Tile(0)),
            ] {
                let part = decode_ok(&bytes, &options, name);
                assert_eq!((part.width, part.height), (whole.width, whole.height));
                assert!(
                    part.components
                        .iter()
                        .zip(&whole.components)
                        .all(|(p, w)| p.samples == w.samples),
                    "{name} r{reduction} {options:?}"
                );
                assert_eq!(part.jp2, whole.jp2, "{name}");
                read_header_with(&bytes, &options).unwrap();
            }
        }
        // Only the origin differs: the window runs to the image's far corner.
        // Only the size differs: it starts at the image's origin.
        for window in [
            Window::new(8, 5, u32::MAX, u32::MAX),
            Window::new(0, 0, 10, 6),
        ] {
            let options = srgb.clone().with_region(Region::Window(window));
            assert!(
                matches!(decode_with(&bytes, &options), Err(Error::Unsupported(_))),
                "{name} {window:?}"
            );
            assert!(
                matches!(
                    read_header_with(&bytes, &options),
                    Err(Error::Unsupported(_))
                ),
                "{name} {window:?}"
            );
        }
    }
}

/// On a tiled sYCC image (14x12, 4:2:0, six tiles) a window over the whole
/// image decodes as the plain sRGB decode does, at reductions 0 and 1, and
/// a tile, which is part of the image, is refused at reduction 0, from the
/// header and from the decode alike.
#[test]
fn srgb_from_tiled_sycc_refuses_a_tile() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let bytes = std::fs::read(dir.join("jp2_sycc_420_tiled.jp2")).unwrap();
    let srgb = DecodeOptions::default().with_color(ColorOutput::Srgb);
    for reduction in [0, 1] {
        let plain = srgb.clone().with_resolution_reduction(reduction);
        let whole = decode_ok(&bytes, &plain, "plain");
        let options = plain.with_region(Region::Window(Window::new(0, 0, u32::MAX, u32::MAX)));
        let part = decode_ok(&bytes, &options, "window");
        assert_eq!((part.width, part.height), (whole.width, whole.height));
        for (p, w) in part.components.iter().zip(&whole.components) {
            assert_eq!(p.samples, w.samples, "r{reduction}");
        }
    }
    for tile in 0..6 {
        let options = srgb.clone().with_region(Region::Tile(tile));
        assert!(
            matches!(decode_with(&bytes, &options), Err(Error::Unsupported(_))),
            "tile {tile}"
        );
        assert!(
            matches!(
                read_header_with(&bytes, &options),
                Err(Error::Unsupported(_))
            ),
            "tile {tile}"
        );
    }
}
