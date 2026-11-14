//! Decoding the first N quality layers (`DecodeOptions::max_quality_layers`).
//!
//! Each oracle is `opj_decompress -l N` 2.5.0 on the same bytes; see
//! `tests/fixtures/layers/README.md`. The synthetic fixtures' oracles are
//! committed as PGX. The ISO/IEC 15444-4 ones are committed as the SHA-256 of
//! their samples, since the corpus may not be redistributed.

mod support;

use std::path::{Path, PathBuf};

use rust_j2k::{DecodeOptions, Error, Image, ReportEntry, WorkLimit, decode, decode_with};
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

/// The layer count `COD` declares in the main header (`SGcod`, A.6.1).
fn cod_layers(bytes: &[u8]) -> u16 {
    let mut at = 2;
    loop {
        let marker = u16::from_be_bytes([bytes[at], bytes[at + 1]]);
        let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        if marker == 0xFF52 {
            return u16::from_be_bytes([bytes[at + 6], bytes[at + 7]]);
        }
        assert_ne!(marker, 0xFF90, "no COD before the first SOT");
        at += 2 + length;
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
const SYNTHETIC: [(&str, u16, i32); 5] = [
    ("lrcp", 4, 0),
    ("rpcl", 3, 0),
    ("bypass_pterm", 4, 0),
    ("pterm_cut", 2, 0),
    ("cprl_irreversible", 3, 1),
];

/// The decodes at a reduction with an oracle: fixture, reduction, caps.
const REDUCED: [(&str, u8, &[u16]); 3] = [
    ("lrcp", 1, &[1, 2, 3, 4]),
    ("rpcl", 2, &[2]),
    ("cprl_irreversible", 1, &[1]),
];

/// Decode `name` at `layers` and `reduction` and compare each component
/// with the oracle `<stem>_<c>.pgx`.
fn grade(name: &str, layers: u16, reduction: u8, tolerance: i32) {
    let bytes = layers_file(&format!("{name}.j2k"));
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
        let bytes = layers_file(&format!("{name}.j2k"));
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
        for cap in [layers, u16::MAX] {
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

/// Where the single tile-part's `Psot` sits.
fn sot_psot_offset(bytes: &[u8]) -> usize {
    let sot = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    sot + 6
}

/// With `strict` off, a prefix that holds every packet of the layers the cap
/// keeps is complete under that cap, and decodes as the whole codestream
/// does under it. LRCP sends layer by layer, so each layer's packets end
/// before the next layer's begin.
#[test]
fn a_prefix_that_holds_the_kept_layers_is_complete_under_the_cap() {
    let whole = layers_file("lrcp.j2k");
    for n in 1..=3u16 {
        let full = sample_bytes(&decode_ok(&whole, &capped(n, 0), "whole"));
        let lenient = capped(n, 0).with_strict(false);
        let mut shortest = None;
        for length in (100..whole.len()).rev() {
            let prefix = &whole[..length];
            let image = decode_ok(prefix, &lenient, &format!("-l {n}, {length} bytes"));
            if !image.report.is_complete() {
                // Every layer past the ones whole is reported missing.
                for entry in &image.report.entries {
                    if let ReportEntry::MissingPackets {
                        complete_layers, ..
                    } = entry
                    {
                        assert!(*complete_layers < n, "-l {n}, {length} bytes");
                    }
                }
                break;
            }
            assert_eq!(sample_bytes(&image), full, "-l {n}, {length} bytes");
            shortest = Some(length);
        }
        let shortest = shortest.expect("the whole codestream is complete");
        assert!(shortest < whole.len() - 100, "-l {n}: {shortest} bytes");
        // Without the cap, the same prefix lacks the later layers.
        let uncapped = DecodeOptions::default().with_strict(false);
        let image = decode_ok(&whole[..shortest], &uncapped, "no cap");
        assert!(!image.report.is_complete(), "-l {n}: {shortest} bytes");
        assert!(
            image.report.entries.iter().any(|e| matches!(
                e,
                ReportEntry::MissingPackets { complete_layers, .. } if *complete_layers == n
            )),
            "-l {n}: {:?}",
            image.report
        );
    }
}
