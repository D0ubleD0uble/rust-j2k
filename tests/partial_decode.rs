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
