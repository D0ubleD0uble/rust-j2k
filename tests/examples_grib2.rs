//! The `grib2` example prints the values eccodes reads from the source
//! message.
//!
//! `jpeg2000_regular_latlon.j2k` is section 7 of
//! `crates/fieldglass-grib2/tests/fixtures/jpeg2000_regular_latlon.grib2` in
//! the sibling `fieldglass` repo (see `tests/fixtures/README.md`), without
//! its 5-byte header (`00 00 04 11 07`: length 1041, section 7). The
//! codestream starts at byte 194 of the message. The message's SHA-256 is
//! `1e187d5708640f3abe963f984eeb0422eb05a5d0a12581427f3aef22df196948`.
//!
//! The reference values come from eccodes 2.34.1 on that message:
//!
//! ```text
//! grib_get -F%.17g -p referenceValue,binaryScaleFactor,decimalScaleFactor
//! grib_get -F%.17g -p min,max
//! grib_get_data -F%.17g
//! ```
//!
//! R = 270.466796875, E = -10, D = 0, 16 bits per value, no bitmap, a 16 x 31
//! grid of 496 values. All 496 values `grib_get_data` printed equal the
//! formula applied to this fixture's `.expected.json` samples (the OpenJPEG
//! oracle). The test keeps the minimum, the maximum and the first eight.

mod support;

#[cfg(not(target_family = "wasm"))]
#[path = "support/run_example.rs"]
mod run_example;

use std::path::{Path, PathBuf};

use support::Expected;

/// R, E and D from section 5, as eccodes prints them.
const RED: [&str; 3] = ["270.466796875", "-10", "0"];
const MIN: f64 = 270.466796875;
const MAX: f64 = 311.0986328125;
/// `grib_get_data`'s first eight values, in scanning order.
const FIRST: [f64; 8] = [
    279.0,
    279.9609375,
    278.53125,
    275.1650390625,
    270.466796875,
    274.19140625,
    274.4013671875,
    274.201171875,
];

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The eccodes figures hold for the oracle's samples under the formula, so
/// the reference values and the fixture belong together.
#[test]
fn the_eccodes_values_follow_from_the_oracle_samples() {
    let path = fixture("jpeg2000_regular_latlon.expected.json");
    let json = std::fs::read_to_string(path).unwrap();
    let expected = Expected::from_json(&json).unwrap();
    let samples = &expected.components[0].samples;
    let values: Vec<f64> = samples
        .iter()
        .map(|&x| 270.466796875 + f64::from(x) * 2f64.powi(-10))
        .collect();
    assert_eq!(values.len(), 496);
    assert_eq!(values.iter().copied().fold(f64::INFINITY, f64::min), MIN);
    assert_eq!(
        values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        MAX
    );
    assert_eq!(values[..8], FIRST);
}

#[cfg(not(target_family = "wasm"))]
fn run_grib2(args: &[&std::ffi::OsStr]) -> std::process::Output {
    run_example::run_example("grib2", args)
}

/// A printed line's value, or values, after its label.
#[cfg(not(target_family = "wasm"))]
fn field(out: &str, label: &str) -> Vec<f64> {
    let line = out
        .lines()
        .find_map(|line| line.strip_prefix(label))
        .unwrap_or_else(|| panic!("no {label:?} line in {out}"));
    line.split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect()
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn the_grib2_example_prints_the_values_eccodes_reads() {
    let input = fixture("jpeg2000_regular_latlon.j2k");
    let mut args = vec![input.as_os_str()];
    args.extend(RED.iter().map(std::ffi::OsStr::new));
    let run = run_grib2(&args);
    assert!(run.status.success(), "{run:?}");
    let out = String::from_utf8_lossy(&run.stdout);
    assert!(out.contains("grid: 16 x 31"), "{out}");
    assert_eq!(field(&out, "min: "), [MIN]);
    assert_eq!(field(&out, "max: "), [MAX]);
    assert_eq!(field(&out, "first values: "), FIRST);
    // R is exactly the 32-bit float section 5 holds: no note.
    assert!(run.stderr.is_empty(), "{run:?}");
}

/// R as eccodes' default format rounds it is not a 32-bit float: the
/// example decodes, and notes it.
#[cfg(not(target_family = "wasm"))]
#[test]
fn the_grib2_example_notes_a_rounded_r() {
    let input = fixture("jpeg2000_regular_latlon.j2k");
    let os = std::ffi::OsStr::new;
    let args = [input.as_os_str(), os("270.467"), os("-10"), os("0")];
    let run = run_grib2(&args);
    assert!(run.status.success(), "{run:?}");
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(err.contains("R is not a 32-bit float"), "{err}");
}

/// D scales by a power of ten: dividing for a positive D, multiplying for a
/// negative one. The message has D = 0, so these are not eccodes figures;
/// they check the formula against the values above.
#[cfg(not(target_family = "wasm"))]
#[test]
fn the_grib2_example_applies_the_decimal_scale_factor() {
    let input = fixture("jpeg2000_regular_latlon.j2k");
    let down: fn(f64) -> f64 = |v| v / 100.0;
    let up: fn(f64) -> f64 = |v| v * 10.0;
    for (d, scale) in [("2", down), ("-1", up)] {
        let os = std::ffi::OsStr::new;
        let args = [input.as_os_str(), os(RED[0]), os(RED[1]), os(d)];
        let run = run_grib2(&args);
        assert!(run.status.success(), "{run:?}");
        let out = String::from_utf8_lossy(&run.stdout);
        let want: Vec<f64> = FIRST.iter().map(|&v| scale(v)).collect();
        assert_eq!(field(&out, "first values: "), want, "D = {d}");
        assert_eq!(field(&out, "min: "), [scale(MIN)], "D = {d}");
        assert_eq!(field(&out, "max: "), [scale(MAX)], "D = {d}");
    }
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn the_grib2_example_refuses_bad_arguments() {
    let input = fixture("jpeg2000_regular_latlon.j2k");
    let rgb = fixture("multicomponent_lossless.j2k");
    let os = std::ffi::OsStr::new;
    let range = "not a whole number from -32767 to 32767";
    let e_range = format!("E: {range}: 40000");
    let d_range = format!("D: {range}: -32768");
    let cases: [(Vec<&std::ffi::OsStr>, &str); 9] = [
        (vec![input.as_os_str()], "usage: grib2"),
        (vec![input.as_os_str(), os("x"), os("0"), os("0")], "R: "),
        (vec![input.as_os_str(), os("1e39"), os("0"), os("0")], "R: "),
        (
            vec![input.as_os_str(), os("1"), os("40000"), os("0")],
            &e_range,
        ),
        (
            vec![input.as_os_str(), os("1"), os("0"), os("-32768")],
            &d_range,
        ),
        (vec![input.as_os_str(), os("1"), os("0"), os("1e9")], "D: "),
        // 2^E or 10^-D past an f64.
        (
            vec![input.as_os_str(), os("1"), os("1100"), os("0")],
            "not finite",
        ),
        (
            vec![input.as_os_str(), os("1"), os("0"), os("-400")],
            "not finite",
        ),
        (
            vec![rgb.as_os_str(), os("1"), os("0"), os("0")],
            "multicomponent_lossless.j2k: a GRIB2 grid has one channel",
        ),
    ];
    for (args, message) in cases {
        let run = run_grib2(&args);
        assert!(!run.status.success(), "{run:?}");
        let err = String::from_utf8_lossy(&run.stderr);
        assert!(err.contains(message), "{err}");
    }
}
