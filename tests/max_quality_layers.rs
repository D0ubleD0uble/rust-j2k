//! Decoding the first N quality layers (`DecodeOptions::max_quality_layers`).
//!
//! Each oracle is `opj_decompress -l N` 2.5.0 on the same bytes; see
//! `tests/fixtures/layers/README.md`. The synthetic fixtures' oracles are
//! committed as PGX, or as the SHA-256 of their samples where there are many
//! (the prefixes of `prefixes.sha256`, which are `-allow-partial` outputs).
//! The ISO/IEC 15444-4 ones are committed as hashes too: `layers/` ships in
//! the crate, and the conformance corpus is excluded from it.
//!
//! Two tests read that corpus from `tests/fixtures/conformance/codestreams/`,
//! so they need the repository, not the published crate:
//! `part4_codestreams_match_openjpeg_at_each_cap` and
//! `a_cap_at_or_past_the_layer_count_is_no_cap`. The others read only
//! `tests/fixtures/layers/` and `tests/fixtures/`.

mod support;

use std::path::{Path, PathBuf};

use rust_j2k::{
    DecodeOptions, Error, Image, ReportEntry, WorkLimit, decode, decode_with, read_header,
};
use support::jp2::sha256_hex;
use support::pgx::parse_pgx;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn layers_file(name: &str) -> Vec<u8> {
    read(&fixtures().join("layers").join(name))
}

fn capped(layers: u16, reduction: u8) -> DecodeOptions {
    DecodeOptions::default()
        .with_max_quality_layers(Some(layers))
        .with_resolution_reduction(reduction)
}

fn decode_ok(bytes: &[u8], options: &DecodeOptions, what: &str) -> Image {
    decode_with(bytes, options).unwrap_or_else(|e| panic!("{what}: {e}"))
}

/// The layer count the main header's `COD` declares, through `read_header`.
fn cod_layers(bytes: &[u8]) -> u16 {
    read_header(bytes).expect("header").quality_layers
}

/// `read_header` reports the main-header layer count that `opj_dump` prints
/// as `numlayers`, for raw codestreams and a JP2 file, single-layer and
/// multi-layer. Measured with `opj_dump` 2.5.0: `layers/lrcp.j2k` 4,
/// `layers/pterm_cut.j2k` 2, `multilayer_lossless.j2k` 3,
/// `irreversible_lossy.j2k` 1, `jp2_cdef_rgba.jp2` 1,
/// `jp2_real/byte.jp2` 12, `jp2_palette_8bit.jp2` 3.
#[test]
fn read_header_reports_the_layer_count_opj_dump_prints() {
    let cases: [(PathBuf, u16); 7] = [
        (fixtures().join("layers/lrcp.j2k"), 4),
        (fixtures().join("layers/pterm_cut.j2k"), 2),
        (fixtures().join("multilayer_lossless.j2k"), 3),
        (fixtures().join("irreversible_lossy.j2k"), 1),
        (fixtures().join("jp2_cdef_rgba.jp2"), 1),
        (fixtures().join("jp2_real/byte.jp2"), 12),
        (fixtures().join("jp2_palette_8bit.jp2"), 3),
    ];
    for (path, numlayers) in cases {
        let header =
            read_header(&read(&path)).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(header.quality_layers, numlayers, "{}", path.display());
    }
}

/// Every component's samples, in order, each as a little-endian `i32`.
fn sample_bytes(image: &Image) -> Vec<u8> {
    image
        .components
        .iter()
        .flat_map(|c| c.samples.iter().flat_map(|s| s.to_le_bytes()))
        .collect()
}

/// The synthetic fixtures: name, layer count, and the most a sample may
/// differ from OpenJPEG's. 5/3 is exact integer arithmetic. The 9/7 one is
/// float: OpenJPEG's decode of the whole codestream differs from this one by
/// at most 1 too.
///
/// `zero_length_contribution` and `zero_length_contribution_bypass` sit in
/// `tests/fixtures/`: a cap of 1 decodes passes whose bytes come in layer 1
/// from the fill, as OpenJPEG does, not from those bytes.
const SYNTHETIC: [(&str, u16, i32); 8] = [
    ("lrcp", 4, 0),
    ("rpcl", 3, 0),
    ("bypass_pterm", 4, 0),
    ("precincts_pterm", 3, 0),
    ("pterm_cut", 2, 0),
    ("cprl_irreversible", 3, 1),
    ("zero_length_contribution", 2, 0),
    ("zero_length_contribution_bypass", 2, 0),
];

/// The codestream `name`, in `tests/fixtures/layers/` or `tests/fixtures/`.
fn codestream(name: &str) -> Vec<u8> {
    let path = fixtures().join("layers").join(format!("{name}.j2k"));
    if path.exists() {
        read(&path)
    } else {
        read(&fixtures().join(format!("{name}.j2k")))
    }
}

/// The decodes at a reduction with an oracle: fixture, reduction, caps.
const REDUCED: [(&str, u8, &[u16]); 3] = [
    ("lrcp", 1, &[1, 2, 3, 4]),
    ("rpcl", 2, &[2]),
    ("cprl_irreversible", 1, &[1]),
];

/// Decode `name` at `layers` and `reduction` and compare each component
/// with the oracle `<stem>_<c>.pgx`.
fn grade(name: &str, layers: u16, reduction: u8, tolerance: i32) {
    let bytes = codestream(name);
    let what = format!("{name} -l {layers} -r {reduction}");
    let image = decode_ok(&bytes, &capped(layers, reduction), &what);
    let stem = if reduction == 0 {
        format!("{name}_l{layers}")
    } else {
        format!("{name}_l{layers}_r{reduction}")
    };
    for (c, component) in image.components.iter().enumerate() {
        let oracle = parse_pgx(&layers_file(&format!("{stem}_{c}.pgx")))
            .unwrap_or_else(|e| panic!("{stem}_{c}.pgx: {e}"));
        assert_eq!(
            (component.width, component.height),
            (oracle.width, oracle.height),
            "{what}, component {c}"
        );
        let worst = component
            .samples
            .iter()
            .zip(&oracle.samples)
            .map(|(a, b)| (a - b).abs())
            .max()
            .unwrap_or(0);
        assert!(worst <= tolerance, "{what}, component {c}: off by {worst}");
    }
    assert!(
        layers_file_exists(&format!("{stem}_{}.pgx", image.components.len())).is_none(),
        "{what}: the oracle has more components"
    );
}

fn layers_file_exists(name: &str) -> Option<PathBuf> {
    let path = fixtures().join("layers").join(name);
    path.exists().then_some(path)
}

/// Each synthetic fixture at each cap from 1 to its layer count matches
/// `opj_decompress -l N`, and each cap gives other samples than the next, so
/// the cap is graded where it changes the image.
#[test]
fn each_cap_matches_openjpeg() {
    for (name, layers, tolerance) in SYNTHETIC {
        let bytes = codestream(name);
        assert_eq!(cod_layers(&bytes), layers, "{name}");
        let mut previous: Option<Vec<u8>> = None;
        for n in 1..=layers {
            grade(name, n, 0, tolerance);
            let samples = sample_bytes(&decode_ok(&bytes, &capped(n, 0), name));
            assert_ne!(
                previous.as_ref(),
                Some(&samples),
                "{name}: -l {n} is -l {}",
                n - 1
            );
            previous = Some(samples);
        }
        // The last cap is the whole decode.
        assert_eq!(
            previous,
            Some(sample_bytes(&decode(&bytes).unwrap())),
            "{name}"
        );
    }
}

/// A cap combines with a resolution reduction.
#[test]
fn a_cap_at_a_reduction_matches_openjpeg() {
    for (name, reduction, caps) in REDUCED {
        let tolerance = SYNTHETIC.iter().find(|f| f.0 == name).unwrap().2;
        for &n in caps {
            grade(name, n, reduction, tolerance);
        }
    }
}

/// The ISO/IEC 15444-4 codestreams of several 5/3 layers, at each cap,
/// bit-exactly: LRCP (`p0_02`, `p1_01`, `p0_10`), RLCP (`p0_16`) and PCRL
/// (`p0_03`, also at reduction 1). `p0_02` and `p1_01` are under `restart`,
/// `predictable termination` and `segmentation symbols`.
#[test]
fn part4_codestreams_match_openjpeg_at_each_cap() {
    let oracles = String::from_utf8(layers_file("part4.sha256")).unwrap();
    let mut graded = 0;
    for line in oracles.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [name, n, r, hash] = fields[..] else {
            panic!("bad line {line:?}");
        };
        let path = fixtures().join(format!("conformance/codestreams/{name}.j2k"));
        let bytes = read(&path);
        let (n, r) = (n.parse().unwrap(), r.parse().unwrap());
        let image = decode_ok(&bytes, &capped(n, r), line);
        assert_eq!(
            sha256_hex(&sample_bytes(&image)),
            hash,
            "{name} -l {n} -r {r}"
        );
        graded += 1;
    }
    assert_eq!(graded, 32, "the oracle count changed");
}

/// No cap, a cap at the layer count and a cap past it decode every
/// codestream in the repository, the ISO/IEC 15444-4 corpus with it, to the
/// same result. `opj_decompress -l` past the layer count decodes them all.
/// A cap past the count takes the same path as one at it, so it runs only on
/// the small codestreams, to keep the test short.
#[test]
fn a_cap_at_or_past_the_layer_count_is_no_cap() {
    let mut paths: Vec<PathBuf> = Vec::new();
    for dir in ["", "conformance/codestreams", "layers"] {
        for entry in std::fs::read_dir(fixtures().join(dir)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "j2k") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    assert!(paths.len() > 60, "{} codestreams", paths.len());
    let unlimited = |options: DecodeOptions| {
        options
            .with_packet_work_limit(WorkLimit::Unlimited)
            .with_code_block_work_limit(WorkLimit::Unlimited)
            .with_sample_limit(WorkLimit::Unlimited)
    };
    for path in paths {
        let bytes = read(&path);
        let what = path.file_name().unwrap().to_string_lossy();
        let layers = cod_layers(&bytes);
        let outcome = |options: DecodeOptions| {
            decode_with(&bytes, &unlimited(options)).map(|image| sample_bytes(&image))
        };
        let whole = outcome(DecodeOptions::default());
        let caps: &[u16] = if bytes.len() <= 16 * 1024 {
            &[layers, u16::MAX]
        } else {
            &[layers]
        };
        for &cap in caps {
            let got = outcome(DecodeOptions::default().with_max_quality_layers(Some(cap)));
            match (&whole, &got) {
                (Ok(a), Ok(b)) => assert!(a == b, "{what} at a cap of {cap}"),
                (Err(a), Err(b)) => assert_eq!(a.to_string(), b.to_string(), "{what}"),
                _ => panic!("{what} at a cap of {cap}: {whole:?} against {got:?}"),
            }
        }
    }
}

/// A cap of 0 keeps nothing to decode.
#[test]
fn a_cap_of_zero_is_invalid_options() {
    let bytes = layers_file("lrcp.j2k");
    let options = DecodeOptions::default().with_max_quality_layers(Some(0));
    match decode_with(&bytes, &options) {
        Err(Error::InvalidOptions(why)) => assert!(why.contains("max_quality_layers"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        rust_j2k::read_header_with(&bytes, &options),
        Err(Error::InvalidOptions(_))
    ));
}

/// A later layer is still read under a cap, so damage in it is still found:
/// a packet body that runs past the tile, and bytes left after the last
/// packet.
#[test]
fn a_damaged_later_layer_is_still_an_error() {
    let whole = layers_file("lrcp.j2k");
    // Drop the last byte before `EOC`: the last layer's last packet is cut.
    let mut short = whole[..whole.len() - 3].to_vec();
    short.extend_from_slice(&whole[whole.len() - 2..]);
    let psot = sot_psot_offset(&short);
    let declared = u32::from_be_bytes(short[psot..psot + 4].try_into().unwrap());
    short[psot..psot + 4].copy_from_slice(&(declared - 1).to_be_bytes());
    // One byte more than the packets hold.
    let mut long = whole[..whole.len() - 2].to_vec();
    long.extend_from_slice(&[0, 0xFF, 0xD9]);
    let psot = sot_psot_offset(&long);
    let declared = u32::from_be_bytes(long[psot..psot + 4].try_into().unwrap());
    long[psot..psot + 4].copy_from_slice(&(declared + 1).to_be_bytes());
    for (what, bytes) in [("short", &short), ("long", &long)] {
        assert!(decode(bytes).is_err(), "{what}");
        match decode_with(bytes, &capped(1, 0)) {
            Err(Error::Codestream(_)) => {}
            other => panic!("{what}: {other:?}"),
        }
    }
}

/// Under `bypass` and `predictable termination`, a cap cuts only the last
/// segment of a block short; the block's earlier MQ segments are whole and
/// still checked. Each flip lands in a segment the cap keeps whole, and leaves
/// it with more unconsumed bytes than the check allows.
#[test]
fn a_damaged_whole_segment_before_a_cut_is_still_an_error() {
    let whole = layers_file("bypass_pterm.j2k");
    for (offset, layers) in [(188, 1), (128, 3)] {
        let mut bad = whole.clone();
        bad[offset] ^= 0x10;
        assert!(decode(&bad).is_err(), "byte {offset}: uncapped");
        match decode_with(&bad, &capped(layers, 0)) {
            Err(Error::Codestream(message)) => {
                assert!(
                    message.contains("predictable termination"),
                    "byte {offset}: {message}"
                );
            }
            other => panic!("byte {offset} at a cap of {layers}: {other:?}"),
        }
    }
}

/// A cap cuts the last kept segment of a block short, and only that one is
/// exempt from `predictable termination`. Here the flip damages a whole
/// segment that still decodes within its bytes, so the decode is sound and
/// matches OpenJPEG's, while the cut segment after it would fail the check.
#[test]
fn a_cut_segment_is_exempt_beside_a_damaged_whole_one() {
    let oracles = String::from_utf8(layers_file("damaged.sha256")).unwrap();
    let mut graded = 0;
    for line in oracles.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [name, offset, n, hash] = fields[..] else {
            panic!("bad line {line:?}");
        };
        let mut bad = layers_file(&format!("{name}.j2k"));
        let (offset, n): (usize, u16) = (offset.parse().unwrap(), n.parse().unwrap());
        bad[offset] ^= 0x10;
        let image = decode_ok(&bad, &capped(n, 0), line);
        assert_eq!(sha256_hex(&sample_bytes(&image)), hash, "{line}");
        graded += 1;
    }
    assert_eq!(graded, 1, "the oracle count changed");
}

/// A block no cap cuts keeps the check on its last segment: `pterm_cut.j2k`
/// with byte 84 flipped leaves its one segment with bytes unconsumed, which is
/// refused whether no cap or a cap at the layer count is set. OpenJPEG only
/// warns.
#[test]
fn an_uncut_last_segment_is_still_checked() {
    let mut bad = layers_file("pterm_cut.j2k");
    bad[84] ^= 0x10;
    for options in [DecodeOptions::default(), capped(2, 0)] {
        match decode_with(&bad, &options) {
            Err(Error::Codestream(message)) => {
                assert!(message.contains("predictable termination"), "{message}");
            }
            other => panic!("{options:?}: {other:?}"),
        }
    }
}

/// The other side of the exemption: under the default style a block has one
/// segment, so with `strict` off a prefix that lacks a later layer does not
/// check it, and damage in it goes unreported, as under a cap. `pterm_cut.j2k`
/// with byte 100 flipped is refused whole, strict or not, and its 228-byte
/// prefix, which lacks the last packet's header, decodes.
#[test]
fn a_cut_prefix_does_not_check_a_blocks_only_segment() {
    let mut bad = layers_file("pterm_cut.j2k");
    bad[100] ^= 0x10;
    let lenient = DecodeOptions::default().with_strict(false);
    for options in [DecodeOptions::default(), lenient.clone()] {
        assert!(
            matches!(decode_with(&bad, &options), Err(Error::Codestream(_))),
            "{options:?}"
        );
    }
    let image = decode_ok(&bad[..228], &lenient, "228 bytes");
    assert!(!image.report.is_complete());
}

/// Where the single tile-part's `Psot` sits.
fn sot_psot_offset(bytes: &[u8]) -> usize {
    let sot = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    sot + 6
}

/// With `strict` off, a prefix that holds every packet of the layers the cap
/// keeps is complete under that cap, and decodes as the whole codestream
/// does under it. LRCP sends layer by layer, so each layer's packets end
/// before the next layer's begin. `bypass_pterm` and `pterm_cut` are under
/// `predictable termination`: the layers past the cap that the prefix lacks
/// would add passes to segments the prefix holds, so those segments are cut.
#[test]
fn a_prefix_that_holds_the_kept_layers_is_complete_under_the_cap() {
    for (name, layers) in [("lrcp", 4u16), ("bypass_pterm", 4), ("pterm_cut", 2)] {
        let whole = layers_file(&format!("{name}.j2k"));
        assert_eq!(cod_layers(&whole), layers, "{name}");
        for n in 1..layers {
            let what = format!("{name} -l {n}");
            let full = sample_bytes(&decode_ok(&whole, &capped(n, 0), &what));
            let lenient = capped(n, 0).with_strict(false);
            let mut shortest = None;
            for length in (1..whole.len()).rev() {
                let prefix = &whole[..length];
                let image = decode_ok(prefix, &lenient, &format!("{what}, {length} bytes"));
                if !image.report.is_complete() {
                    // Every layer past the ones whole is reported missing.
                    for entry in &image.report.entries {
                        if let ReportEntry::MissingPackets {
                            complete_layers, ..
                        } = entry
                        {
                            assert!(*complete_layers < n, "{what}, {length} bytes");
                        }
                    }
                    break;
                }
                assert_eq!(sample_bytes(&image), full, "{what}, {length} bytes");
                shortest = Some(length);
            }
            let shortest = shortest.expect("the whole codestream is complete");
            // The prefix lacks more than the `EOC` marker.
            assert!(shortest < whole.len() - 2, "{what}: {shortest} bytes");
            // Without the cap, the same prefix lacks the later layers.
            let uncapped = DecodeOptions::default().with_strict(false);
            let image = decode_ok(&whole[..shortest], &uncapped, "no cap");
            assert!(!image.report.is_complete(), "{what}: {shortest} bytes");
            assert!(
                image.report.entries.iter().any(|e| matches!(
                    e,
                    ReportEntry::MissingPackets { complete_layers, .. } if *complete_layers == n
                )),
                "{what}: {:?}",
                image.report
            );
        }
    }
}

/// The prefixes that end inside the header of a packet of a later layer,
/// whose code-blocks hold earlier layers. OpenJPEG reads the missing header
/// bits as 0 and decodes what they say; this decoder drops the packet, as
/// B.11 drops a cut packet. Each is one run of `prefixes.sha256` that OpenJPEG
/// flags as `whole`.
const HEADER_CUTS: [(&str, &[usize]); 2] = [
    ("pterm_cut", &[224, 225, 226, 227, 228]),
    ("bypass_pterm", &[215, 733, 975, 2008, 2047]),
];

/// Every prefix of the `predictable termination` fixtures decodes with
/// `strict` off, with no cap and with a cap of 1, once the main header and
/// the tile-part header are there, and matches `opj_decompress
/// -allow-partial` on the same prefix bit-exactly (both are 5/3).
///
/// Before that header, OpenJPEG writes no image and this decoder gives
/// `Error::Incomplete` or an incomplete image of no tile data. Without a cap,
/// two kinds of prefix differ, as `docs/openjpeg-departures.md` records: those
/// that end inside a packet's body where OpenJPEG warns that a segment is too
/// long and drops the cut code-block, its earlier layers too, while this
/// decoder keeps those layers; and those in [`HEADER_CUTS`]. Both are graded
/// as decoding to an incomplete image.
#[test]
fn every_strict_off_prefix_of_a_pterm_codestream_matches_openjpeg() {
    let oracles = String::from_utf8(layers_file("prefixes.sha256")).unwrap();
    let mut exact = 0;
    let mut departures = 0;
    for line in oracles.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [name, cap, first, last, hash, flag] = fields[..] else {
            panic!("bad line {line:?}");
        };
        let whole = layers_file(&format!("{name}.j2k"));
        let cap: u16 = cap.parse().unwrap();
        let (first, last): (usize, usize) = (first.parse().unwrap(), last.parse().unwrap());
        assert!(flag == "whole" || flag == "cut", "{line}");
        let mut options = DecodeOptions::default().with_strict(false);
        if cap > 0 {
            options = options.with_max_quality_layers(Some(cap));
        }
        let header_cuts = HEADER_CUTS.iter().find(|h| h.0 == name).unwrap().1;
        for length in first..=last {
            let what = format!("{name} -l {cap}, {length} bytes");
            let decoded = decode_with(&whole[..length], &options);
            if hash == "refused" {
                match decoded {
                    Err(Error::Incomplete(_)) => {}
                    Ok(image) => assert!(!image.report.is_complete(), "{what}"),
                    Err(error) => panic!("{what}: {error}"),
                }
                continue;
            }
            let image = decoded.unwrap_or_else(|e| panic!("{what}: {e}"));
            let got = sha256_hex(&sample_bytes(&image));
            let departs = cap == 0 && (flag == "cut" || header_cuts.contains(&length));
            if departs && got != hash {
                assert!(!image.report.is_complete(), "{what}");
                departures += 1;
            } else {
                assert_eq!(got, hash, "{what}");
                exact += 1;
            }
        }
    }
    // Every prefix of both files, under both caps, is graded.
    assert_eq!(exact + departures, 2 * (231 + 2558) - 2 * (78 + 123));
    // How many depart, so a change that widens the difference shows.
    assert_eq!((exact, departures), (3222, 1954));
}

/// Packets the `POC` volumes leave out are missing like packets the data
/// never reached: `pterm_poc_gap.j2k` is `pterm_cut.j2k` with a `POC` that
/// ends at layer 1, so layer 1 would add passes to each block's one segment.
/// A strict decode refuses the gap. With `strict` off it decodes to OpenJPEG's
/// `-allow-partial` image, bit-exactly, which is the `-l 1` image of
/// `pterm_cut.j2k`, and reports the layer missing.
#[test]
fn a_segment_a_poc_gap_leaves_short_is_not_checked_with_strict_off() {
    let bytes = layers_file("pterm_poc_gap.j2k");
    match decode(&bytes) {
        Err(Error::Codestream(message)) => assert!(message.contains("POC"), "{message}"),
        other => panic!("{other:?}"),
    }
    let lenient = DecodeOptions::default().with_strict(false);
    let image = decode_ok(&bytes, &lenient, "strict off");
    let oracle = parse_pgx(&layers_file("pterm_poc_gap_0.pgx")).unwrap();
    assert_eq!(image.components.len(), 1);
    assert_eq!(image.components[0].samples, oracle.samples);
    assert!(
        image.report.entries.iter().any(|e| matches!(
            e,
            ReportEntry::MissingPackets {
                complete_layers: 1,
                ..
            }
        )),
        "{:?}",
        image.report
    );
}

/// With `strict` off, a block whose precinct the data ended in is exempt from
/// `predictable termination` only on its last segment, and only when that
/// segment could still take passes. `bypass_pterm` with byte 188 or 128
/// flipped damages an MQ segment of the LL block that is not its last, which
/// layer 0 brings whole: every prefix that holds it is refused, as the whole
/// file is, though later packets of that precinct are missing. The same
/// prefixes of the undamaged file decode.
#[test]
fn a_damaged_whole_segment_before_the_data_ends_is_still_an_error() {
    let whole = layers_file("bypass_pterm.j2k");
    let lenient = DecodeOptions::default().with_strict(false);
    for offset in [188, 128] {
        let mut bad = whole.clone();
        bad[offset] ^= 0x10;
        let lengths = [212, 213, 214, 215, 216, 281, 282, 700, 1500, 2555];
        for length in lengths.into_iter().chain([whole.len()]) {
            decode_ok(&whole[..length], &lenient, &format!("{length} bytes"));
            match decode_with(&bad[..length], &lenient) {
                Err(Error::Codestream(message)) => assert!(
                    message.contains("predictable termination"),
                    "byte {offset}, {length} bytes: {message}"
                ),
                other => panic!("byte {offset}, {length} bytes: {other:?}"),
            }
        }
    }
}
