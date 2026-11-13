//! Decoding with `strict` off: a codestream with data missing, cut short or
//! left over gives the image the data holds, and a report of what is missing.
//!
//! Each oracle is an `opj_decompress -allow-partial` 2.5.0 output; see
//! `tests/fixtures/partial/README.md`.

use std::path::PathBuf;

use rust_j2k::{DecodeOptions, DecodeReport, Error, Image, ReportEntry, decode_with, read_header};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The samples of a `P5` PGM with a maximum value below 256.
fn pgm(name: &str) -> Vec<i32> {
    let data = read(name);
    let (mut fields, mut at) = (Vec::new(), 0);
    while fields.len() < 4 {
        while data[at].is_ascii_whitespace() {
            at += 1;
        }
        if data[at] == b'#' {
            while data[at] != b'\n' {
                at += 1;
            }
            continue;
        }
        let start = at;
        while !data[at].is_ascii_whitespace() {
            at += 1;
        }
        fields.push(String::from_utf8_lossy(&data[start..at]).into_owned());
    }
    assert_eq!(fields[0], "P5", "{name}");
    assert!(fields[3].parse::<u32>().unwrap() < 256, "{name}");
    data[at + 1..].iter().map(|&b| i32::from(b)).collect()
}

fn lenient(reduction: u8) -> DecodeOptions {
    DecodeOptions::default()
        .with_strict(false)
        .with_resolution_reduction(reduction)
}

fn strict(reduction: u8) -> DecodeOptions {
    DecodeOptions::default().with_resolution_reduction(reduction)
}

fn decode_ok(bytes: &[u8], options: &DecodeOptions, what: &str) -> Image {
    decode_with(bytes, options).unwrap_or_else(|e| panic!("{what}: {e}"))
}

fn samples(image: &Image) -> &[i32] {
    assert_eq!(image.components.len(), 1);
    &image.components[0].samples
}

fn has(report: &DecodeReport, entry: impl Fn(&ReportEntry) -> bool) -> bool {
    report.entries.iter().any(entry)
}

fn missing_packets(report: &DecodeReport) -> Vec<(u32, u16, Option<u8>)> {
    report
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ReportEntry::MissingPackets {
                tile,
                complete_layers,
                complete_reduction,
                ..
            } => Some((*tile, *complete_layers, *complete_reduction)),
            _ => None,
        })
        .collect()
}

/// `lrcp.prefixes`: for each reduction, the shortest prefix of `lrcp.j2k`
/// that `opj_decompress -allow-partial` decodes to the whole file's image.
fn lrcp_prefixes() -> Vec<(u8, usize)> {
    String::from_utf8(read("partial/lrcp.prefixes"))
        .unwrap()
        .lines()
        .map(|line| {
            let (r, length) = line.split_once(' ').unwrap();
            (r.parse().unwrap(), length.parse().unwrap())
        })
        .collect()
}

/// The prefix OpenJPEG needs for reduction r is the shortest one this
/// decoder reports complete at r: there it gives the whole file's image, and
/// says so; at reduction r + 1 too.
#[test]
fn a_prefix_that_holds_a_reduction_decodes_it_whole() {
    let whole = read("partial/lrcp.j2k");
    for (r, length) in lrcp_prefixes() {
        let prefix = &whole[..length];
        let image = decode_ok(prefix, &lenient(r), &format!("r{r}"));
        assert_eq!(
            samples(&image),
            pgm(&format!("partial/lrcp_r{r}.pgm")),
            "r{r}"
        );
        assert!(image.report.is_complete(), "r{r}: {:?}", image.report);
        assert_eq!(image.report.complete_reduction, Some(r), "r{r}");
        assert!(missing_packets(&image.report).is_empty(), "r{r}");
        if r < 5 {
            let coarser = decode_ok(prefix, &lenient(r + 1), &format!("r{r} at r+1"));
            assert!(coarser.report.is_complete(), "r{r} at r+1");
            assert_eq!(
                samples(&coarser),
                pgm(&format!("partial/lrcp_r{}.pgm", r + 1))
            );
        }
    }
}

/// One byte short of that prefix, the strict decode refuses; with `strict`
/// off the decode gives what OpenJPEG gives, reports the missing packets,
/// and names the next reduction as complete, where the decode is the whole
/// file's with a complete report.
#[test]
fn one_byte_short_is_refused_strict_and_reported_otherwise() {
    let whole = read("partial/lrcp.j2k");
    for (r, length) in lrcp_prefixes() {
        let prefix = &whole[..length - 1];
        match decode_with(prefix, &strict(r)) {
            Err(Error::Codestream(_)) => {}
            other => panic!("r{r}: strict gave {other:?}"),
        }
        let image = decode_ok(prefix, &lenient(r), &format!("r{r}"));
        assert_eq!(
            samples(&image),
            pgm(&format!("partial/lrcp_r{r}_short.pgm")),
            "r{r}"
        );
        assert!(!image.report.is_complete(), "r{r}");
        let next = if r < 5 { Some(r + 1) } else { None };
        assert_eq!(image.report.complete_reduction, next, "r{r}");
        assert_eq!(missing_packets(&image.report), [(0, 0, next)], "r{r}");
        assert!(
            has(&image.report, |e| matches!(
                e,
                ReportEntry::MissingEoc { .. }
            )),
            "r{r}"
        );
        if let Some(next) = next {
            let coarser = decode_ok(prefix, &lenient(next), &format!("r{r} at r+1"));
            assert_eq!(coarser.report.complete_reduction, Some(next));
            assert!(coarser.report.is_complete());
            assert_eq!(samples(&coarser), pgm(&format!("partial/lrcp_r{next}.pgm")));
        }
    }
}

/// The whole file decodes the same either way, with the complete report;
/// a strict decode always has the complete report.
#[test]
fn a_whole_file_decodes_the_same_with_strict_off() {
    let whole = read("partial/lrcp.j2k");
    for r in 0..=5 {
        let a = decode_ok(&whole, &strict(r), "strict");
        let b = decode_ok(&whole, &lenient(r), "lenient");
        assert_eq!(a.report, DecodeReport::default());
        assert_eq!(b.report.entries, []);
        assert_eq!(b.report.complete_reduction, Some(0));
        assert!(b.report.is_complete());
        assert_eq!(a.components, b.components, "r{r}");
    }
}

/// The report's default is the complete one.
#[test]
fn the_default_report_is_complete() {
    let report = DecodeReport::default();
    assert!(report.entries.is_empty());
    assert_eq!(report.complete_reduction, Some(0));
    assert!(report.is_complete());
}

/// Every prefix from the first `SOT` marker on decodes with `strict` off; the
/// report never says more is there than is: a prefix it reports complete at
/// r gives the whole file's image at r.
#[test]
fn every_prefix_reports_no_more_than_it_holds() {
    let whole = read("partial/lrcp.j2k");
    let full: Vec<Vec<i32>> = (0..=5)
        .map(|r| pgm(&format!("partial/lrcp_r{r}.pgm")))
        .collect();
    let sot = whole.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    // A prefix that ends before it does not show where the main header ends.
    assert!(decode_with(&whole[..sot + 1], &lenient(0)).is_err());
    for length in sot + 2..=whole.len() {
        let prefix = &whole[..length];
        let image = decode_ok(prefix, &lenient(0), &format!("{length}"));
        if let Some(c) = image.report.complete_reduction {
            let coarse = decode_ok(prefix, &lenient(c), &format!("{length} at {c}"));
            assert!(coarse.report.is_complete(), "{length}");
            assert_eq!(samples(&coarse), full[usize::from(c)], "{length} at {c}");
        }
    }
}

/// `max_reduction` is the most levels the main header lets a decode drop.
#[test]
fn max_reduction_is_the_main_headers_limit() {
    let whole = read("partial/lrcp.j2k");
    let header = read_header(&whole).unwrap();
    assert_eq!(header.max_reduction, 5);
    assert!(decode_with(&whole, &strict(5)).is_ok());
    assert!(matches!(
        decode_with(&whole, &strict(6)),
        Err(Error::InvalidOptions(_))
    ));
}

/// A codestream cut inside tile 1 of four: tile 1 decodes from the packets
/// it has, and tiles 2 and 3 are 0, as OpenJPEG writes them.
#[test]
fn missing_tiles_are_zero_and_reported() {
    let whole = read("partial/tiles.j2k");
    let cut: usize = String::from_utf8(read("partial/tiles.cut"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let prefix = &whole[..cut];
    assert!(matches!(
        decode_with(prefix, &strict(0)),
        Err(Error::Codestream(_))
    ));
    let image = decode_ok(prefix, &lenient(0), "cut");
    assert_eq!(samples(&image), pgm("partial/tiles_cut.pgm"));
    let entries = &image.report.entries;
    assert!(has(&image.report, |e| matches!(
        e,
        ReportEntry::MissingEoc { .. }
    )));
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, ReportEntry::MissingTiles { tiles, .. } if *tiles == (2..4)))
    );
    assert!(entries.iter().any(|e| matches!(
        e,
        ReportEntry::TruncatedTilePart { tile: 1, tile_part: 0, declared, present, .. }
            if present < declared
    )));
    assert!(!image.report.is_complete());
    assert_eq!(image.report.complete_reduction, None);
    // Tiles 2 and 3 are the bottom half: 0, though the image is mid-gray
    // around 128 there.
    assert!(samples(&image)[32 * 64..].iter().all(|&s| s == 0));
}

/// `opj_compress` writes the packets of overlapping POC volumes twice. The
/// tile's bytes after its last packet are left over: the decode reads the
/// first copy, which is the source, and reports the rest.
#[test]
fn overlapping_poc_volumes_leave_bytes_over() {
    let bytes = read("partial/overlap.j2k");
    assert!(matches!(
        decode_with(&bytes, &strict(0)),
        Err(Error::Codestream(_))
    ));
    let image = decode_ok(&bytes, &lenient(0), "overlap");
    assert_eq!(samples(&image), pgm("partial/source.pgm"));
    assert!(matches!(
        image.report.entries.as_slice(),
        [ReportEntry::LeftoverBytes {
            tile: 0,
            in_packed_headers: false,
            ..
        }]
    ));
    assert!(!image.report.is_complete());
    assert_eq!(image.report.complete_reduction, None);
}

/// `-TP R` with empty resolutions: `opj_compress` writes a stray byte that
/// leaves the rest of the tile unread. The decode gives what OpenJPEG gives,
/// which is not the source, and reports the bytes left over.
#[test]
fn a_tp_r_phantom_packet_leaves_bytes_over() {
    let bytes = read("partial/tp_r.j2k");
    assert!(matches!(
        decode_with(&bytes, &strict(0)),
        Err(Error::Codestream(_))
    ));
    let image = decode_ok(&bytes, &lenient(0), "tp_r");
    assert_eq!(samples(&image), pgm("partial/tp_r.pgm"));
    let source: Vec<i32> = read("partial/tp_r_source.raw")
        .iter()
        .map(|&b| i32::from(b))
        .collect();
    assert_ne!(samples(&image), source);
    assert!(
        image
            .report
            .entries
            .iter()
            .all(|e| matches!(e, ReportEntry::LeftoverBytes { .. }))
    );
    assert!(!image.report.entries.is_empty());
    assert!(!image.report.is_complete());
    assert_eq!(image.report.complete_reduction, None);
}

/// The `Psot = 0` shapes OpenJPEG refuses unless told to allow a partial
/// decode: its output, with the missing packets reported.
#[test]
fn psot_zero_without_eoc_decodes_as_openjpeg_allows() {
    for (name, oracle) in [
        (
            "psot0/single_psot0_no_eoc.j2k",
            "partial/psot0_single_no_eoc.pgm",
        ),
        (
            "psot0/tileparts_psot0_no_eoc.j2k",
            "partial/psot0_tileparts_no_eoc.pgm",
        ),
    ] {
        let bytes = read(name);
        assert!(decode_with(&bytes, &strict(0)).is_err(), "{name}");
        let image = decode_ok(&bytes, &lenient(0), name);
        assert_eq!(samples(&image), pgm(oracle), "{name}");
        assert!(!missing_packets(&image.report).is_empty(), "{name}");
        assert!(!image.report.is_complete(), "{name}");
    }
}

/// `lrcp.j2k` with its `EOC` removed: strict refuses it, and with `strict`
/// off nothing is missing but the marker, so the decode is complete.
#[test]
fn a_missing_eoc_alone_leaves_the_decode_complete() {
    let whole = read("partial/lrcp.j2k");
    assert_eq!(whole[whole.len() - 2..], [0xFF, 0xD9]);
    let bytes = &whole[..whole.len() - 2];
    assert!(decode_with(bytes, &strict(0)).is_err());
    let image = decode_ok(bytes, &lenient(0), "no EOC");
    assert!(matches!(
        image.report.entries.as_slice(),
        [ReportEntry::MissingEoc { .. }]
    ));
    assert!(image.report.is_complete());
    assert_eq!(samples(&image), pgm("partial/lrcp_r0.pgm"));
}

/// `lrcp.j2k`'s one tile-part with `TNsot` raised from 1 to 2: the count is
/// advisory, so the decode reads the part there is, reports the mismatch,
/// and is complete.
#[test]
fn a_tile_part_count_mismatch_alone_leaves_the_decode_complete() {
    let mut bytes = read("partial/lrcp.j2k");
    let sot = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    assert_eq!(bytes[sot + 11], 1, "TNsot");
    bytes[sot + 11] = 2;
    assert!(decode_with(&bytes, &strict(0)).is_err());
    let image = decode_ok(&bytes, &lenient(0), "TNsot 2");
    assert!(
        matches!(
            image.report.entries.as_slice(),
            [ReportEntry::TilePartCountMismatch {
                tile: 0,
                declared: 2,
                found: 1,
                ..
            }]
        ),
        "{:?}",
        image.report
    );
    assert!(image.report.is_complete());
    assert_eq!(samples(&image), pgm("partial/lrcp_r0.pgm"));
}

/// The POC volumes leave out resolution 1: the packets are missing data, so
/// the decode at reduction 1, which needs none of them, is complete and is
/// the decode of the same file with a second volume that covers them.
#[test]
fn packets_a_poc_leaves_out_are_missing() {
    let missing = read("poc_coverage/poc_res_missing.j2k");
    assert!(matches!(
        decode_with(&missing, &strict(0)),
        Err(Error::Codestream(_))
    ));
    let image = decode_ok(&missing, &lenient(0), "r0");
    assert_eq!(missing_packets(&image.report), [(0, 0, Some(1))]);
    assert!(!image.report.is_complete());
    let coarse = decode_ok(&missing, &lenient(1), "r1");
    assert!(coarse.report.is_complete());
    let covered = decode_ok(
        &read("poc_coverage/poc_res_covered.j2k"),
        &strict(1),
        "covered",
    );
    assert_eq!(coarse.components, covered.components);
}

/// The report holds at most five entries per tile-part found, plus two,
/// for every prefix of a four-tile codestream of several tile-parts each.
#[test]
fn the_report_is_bounded_by_the_tile_parts() {
    for name in ["tiles_multipart_lossless.j2k", "partial/tiles.j2k"] {
        let whole = read(name);
        let sot = whole.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
        for length in sot..=whole.len() {
            let prefix = &whole[..length];
            let Ok(image) = decode_with(prefix, &lenient(0)) else {
                continue;
            };
            let parts = count_tile_parts(prefix, sot);
            assert!(
                image.report.entries.len() <= 5 * parts + 2,
                "{name} {length}: {} entries for {parts} tile-parts",
                image.report.entries.len()
            );
        }
    }
}

/// The tile-parts whose `SOT` marker segment the codestream holds, walking
/// the `Psot` chain from `sot`.
fn count_tile_parts(bytes: &[u8], mut sot: usize) -> usize {
    let mut parts = 0;
    while sot + 12 <= bytes.len() && bytes[sot..sot + 2] == [0xFF, 0x90] {
        parts += 1;
        let psot = u32::from_be_bytes(bytes[sot + 6..sot + 10].try_into().unwrap()) as usize;
        if psot == 0 {
            break;
        }
        sot += psot;
    }
    parts
}

/// A run of missing tiles is one entry, however many tiles it spans.
#[test]
fn a_run_of_missing_tiles_is_one_entry() {
    let whole = read("partial/tiles.j2k");
    let first = whole.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    let psot = u32::from_be_bytes(whole[first + 6..first + 10].try_into().unwrap()) as usize;
    // Tile 0 whole, then nothing.
    let image = decode_ok(&whole[..first + psot], &lenient(0), "tile 0");
    let runs: Vec<_> = image
        .report
        .entries
        .iter()
        .filter_map(|e| match e {
            ReportEntry::MissingTiles { tiles, .. } => Some(tiles.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        runs,
        vec![std::ops::Range { start: 1, end: 4 }],
        "{:?}",
        image.report
    );
    assert!(!image.report.is_complete());
    assert!(samples(&image)[..32].iter().any(|&s| s != 0));
}

/// A marker segment: `code`, its length counting itself, then `body`.
fn segment(code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = code.to_be_bytes().to_vec();
    out.extend_from_slice(&(body.len() as u16 + 2).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// A main header: an image of `width` x `height` in tiles of `tile_width` x
/// `tile_height`, `components` 8-bit unsigned components, `levels`
/// decomposition levels, one layer, LRCP, the 5/3 wavelet, code-blocks of
/// `4 << block` samples square, no quantization.
fn main_header(
    (width, height): (u32, u32),
    (tile_width, tile_height): (u32, u32),
    components: u16,
    levels: u8,
    block: u8,
) -> Vec<u8> {
    let mut siz = Vec::new();
    siz.extend_from_slice(&0u16.to_be_bytes());
    for value in [width, height, 0, 0, tile_width, tile_height, 0, 0] {
        siz.extend_from_slice(&value.to_be_bytes());
    }
    siz.extend_from_slice(&components.to_be_bytes());
    for _ in 0..components {
        siz.extend_from_slice(&[7, 1, 1]);
    }
    let cod = [0, 0, 0, 1, 0, levels, block, block, 0, 1];
    let mut qcd = vec![0x20];
    qcd.extend(std::iter::repeat_n(9 << 3, 3 * usize::from(levels) + 1));
    let mut out = vec![0xFF, 0x4F];
    out.extend(segment(0xFF51, &siz));
    out.extend(segment(0xFF52, &cod));
    out.extend(segment(0xFF5C, &qcd));
    out
}

/// A tile-part: `SOT` for tile `tile`, part `part` of `parts`, then `extra`
/// (marker segments of its header), `SOD` and `data`.
fn tile_part(tile: u16, part: u8, parts: u8, extra: &[u8], data: &[u8]) -> Vec<u8> {
    let psot = (12 + extra.len() + 2 + data.len()) as u32;
    let mut sot = Vec::new();
    sot.extend_from_slice(&tile.to_be_bytes());
    sot.extend_from_slice(&psot.to_be_bytes());
    sot.extend_from_slice(&[part, parts]);
    let mut out = segment(0xFF90, &sot);
    out.extend_from_slice(extra);
    out.extend_from_slice(&[0xFF, 0x93]);
    out.extend_from_slice(data);
    out
}

/// One 8 x 4 tile of two 4 x 4 code-blocks, no decomposition, its one packet
/// header packed in a `PPT` as `header` and its body `body`.
fn packed(header: &[u8], body: &[u8]) -> Vec<u8> {
    let mut ppt = vec![0];
    ppt.extend_from_slice(header);
    let mut out = main_header((8, 4), (8, 4), 1, 0, 0);
    out.extend(tile_part(0, 0, 1, &segment(0xFF61, &ppt), body));
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

/// A packed packet header that runs short is read as a strict decode reads
/// it, with the bits past its end as fill, so the decode with `strict` off
/// gives the same image: the stream does not end there, since the tile's
/// body is whole.
#[test]
fn a_packed_header_that_runs_short_decodes_as_strict() {
    let cases = [
        // A one-block header whose last byte is 0xFF, with and without the
        // stuffed byte the alignment after it reads.
        (
            "0xFF last, no stuffed byte",
            vec![0xC4, 0xFF],
            vec![0; 7],
            1,
            1,
        ),
        (
            "0xFF last, stuffed byte",
            vec![0xC4, 0xFF, 0x00],
            vec![0; 7],
            1,
            1,
        ),
        // Two blocks, the second's last length bit (a 1) past the end.
        ("a bit short", vec![0xF8, 0x30], vec![0x00], 8, 4),
        ("whole", vec![0xF8, 0x30, 0x80], vec![0x00], 8, 4),
    ];
    for (what, header, body, width, height) in cases {
        let bytes = if width == 1 {
            let mut ppt = vec![0];
            ppt.extend_from_slice(&header);
            let mut out = main_header((1, 1), (1, 1), 1, 0, 4);
            out.extend(tile_part(0, 0, 1, &segment(0xFF61, &ppt), &body));
            out.extend_from_slice(&[0xFF, 0xD9]);
            out
        } else {
            packed(&header, &body)
        };
        let strict = decode_ok(&bytes, &strict(0), what);
        let partial = decode_ok(&bytes, &lenient(0), what);
        assert_eq!(strict, partial, "{what}");
        assert_eq!(partial.report, DecodeReport::default(), "{what}");
        assert_eq!(samples(&partial).len(), (width * height) as usize, "{what}");
    }
}

/// A decode with `strict` off of a whole file, at a reduction, is the strict
/// decode: the same `Image`, report included.
#[test]
fn a_whole_file_at_a_reduction_is_the_strict_image() {
    for name in [
        "partial/lrcp.j2k",
        "partial/tiles.j2k",
        "multilayer_lossless.j2k",
    ] {
        let bytes = read(name);
        for r in 0..=2 {
            let strict = decode_ok(&bytes, &strict(r), name);
            let partial = decode_ok(&bytes, &lenient(r), name);
            assert_eq!(strict, partial, "{name} at r{r}");
        }
    }
}

/// A `Psot = 0` tile-part takes the codestream to two bytes before its end.
/// With `strict` off, a prefix that leaves it fewer than two bytes is a
/// tile-part with no data, not an error: every prefix from the first `SOT`
/// marker on decodes.
#[test]
fn every_prefix_of_a_psot_zero_tile_part_decodes() {
    let whole = read("psot0/single_psot0_eoc.j2k");
    let sot = whole.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    let sod = sot + 12;
    assert_eq!(whole[sod..sod + 2], [0xFF, 0x93]);
    for length in sot + 2..=whole.len() {
        let image = decode_ok(&whole[..length], &lenient(0), &format!("{length}"));
        if let Some(c) = image.report.complete_reduction {
            let coarse = decode_ok(&whole[..length], &lenient(c), &format!("{length} at {c}"));
            let full = decode_ok(&whole, &strict(c), "whole");
            assert_eq!(coarse.components, full.components, "{length} at {c}");
        }
    }
    // Zero and one bytes after SOD: strict refuses, partial has no data.
    for length in [sod + 2, sod + 3] {
        assert!(
            decode_with(&whole[..length], &strict(0)).is_err(),
            "{length}"
        );
        let image = decode_ok(&whole[..length], &lenient(0), &format!("{length}"));
        assert!(!image.report.is_complete(), "{length}");
    }
}

/// With SOP and EPH markers, a prefix may end inside a packet's SOP segment,
/// header or EPH marker: every prefix decodes, and the report never says more
/// is there than is.
#[test]
fn every_prefix_with_sop_and_eph_reports_no_more_than_it_holds() {
    for name in [
        "delimiters_sop_eph_lossless.j2k",
        "delimiters_sop_lossless.j2k",
        "delimiters_eph_lossless.j2k",
    ] {
        let whole = read(name);
        let levels = read_header(&whole).unwrap().max_reduction;
        let full: Vec<Image> = (0..=levels)
            .map(|r| decode_ok(&whole, &strict(r), name))
            .collect();
        let sot = whole.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
        for length in sot + 2..=whole.len() {
            let prefix = &whole[..length];
            let image = decode_ok(prefix, &lenient(0), &format!("{name} {length}"));
            if let Some(c) = image.report.complete_reduction {
                let coarse = decode_ok(prefix, &lenient(c), &format!("{name} {length} at {c}"));
                assert!(coarse.report.is_complete(), "{name} {length}");
                assert_eq!(
                    coarse.components,
                    full[usize::from(c)].components,
                    "{name} {length} at {c}"
                );
            }
        }
    }
}

/// With `strict` off a damaged packet header is still an error: only a
/// header that runs past the end of the data is taken as the data ending.
#[test]
fn a_damaged_packet_header_is_refused_with_strict_off() {
    // One 4 x 4 code-block: nonempty 1, included 1 1, zero bit-planes 1 1,
    // one pass 0, then an Lblock indicator of 64 ones, longer than any
    // Lblock, and whole bytes after it.
    let mut bits = vec![1, 1, 1, 1, 1, 0];
    bits.extend(std::iter::repeat_n(1, 64));
    bits.extend([0, 0, 0, 1]);
    let header = pack_bits(&bits);
    let mut data = header;
    data.extend_from_slice(&[0x00; 4]);
    let mut bytes = main_header((4, 4), (4, 4), 1, 0, 0);
    bytes.extend(tile_part(0, 0, 1, &[], &data));
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    let strict_error = decode_with(&bytes, &strict(0)).expect_err("strict refuses");
    let partial_error = decode_with(&bytes, &lenient(0)).expect_err("partial refuses");
    assert!(
        matches!(&partial_error, Error::Codestream(m) if m.contains("Lblock")),
        "{partial_error}"
    );
    assert_eq!(strict_error, partial_error);
}

/// Each tile-component of a missing tile is charged against the packet work
/// bound. A main header that declares 65535 tiles of 257 components, followed
/// by nothing, is refused at once rather than zeroing 16.8 million
/// tile-components.
#[test]
fn missing_tiles_are_charged_against_the_work_bound() {
    let mut bytes = main_header((65535, 1), (1, 1), 257, 0, 0);
    bytes.extend_from_slice(&[0xFF, 0x90]);
    match decode_with(&bytes, &lenient(0)) {
        Err(Error::Limit(message)) => assert!(message.contains("missing tiles"), "{message}"),
        other => panic!("expected Limit: {other:?}"),
    }
}

/// The end of a tile's data is charged 32 visits for each (component,
/// resolution): 6000 empty tiles of 16 components and 6 resolutions come to
/// 18.4 million, past the bound of 2^24, while their packets alone are 576000
/// visits.
#[test]
fn the_end_of_each_tiles_data_is_charged_against_the_work_bound() {
    let mut bytes = main_header((6000, 1), (1, 1), 16, 5, 0);
    for tile in 0..6000 {
        bytes.extend(tile_part(tile, 0, 1, &[], &[]));
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    match decode_with(&bytes, &lenient(0)) {
        Err(Error::Limit(message)) => {
            assert!(message.contains("packet_work_limit"), "{message}")
        }
        other => panic!("expected Limit: {other:?}"),
    }
}

/// `bits` as packet header bytes, MSB first, with the bit stuffing B.10.1
/// asks for: a byte after a 0xFF holds seven bits. The last byte is padded
/// with zeros.
fn pack_bits(bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut bits = bits.iter().copied().peekable();
    while bits.peek().is_some() {
        let width = if out.last() == Some(&0xFF) { 7 } else { 8 };
        let mut byte = 0u8;
        for _ in 0..width {
            byte = byte << 1 | bits.next().unwrap_or(0);
        }
        out.push(byte);
    }
    out
}
