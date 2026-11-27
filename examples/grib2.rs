//! Decode the JPEG 2000 codestream of a GRIB2 message and scale it to
//! physical values.
//!
//! ```text
//! cargo run --example grib2 -- \
//!     tests/fixtures/jpeg2000_regular_latlon.j2k 270.466796875 -10 0
//! ```
//!
//! A GRIB2 message that uses data representation template 5.40 stores its
//! grid as a JPEG 2000 codestream in section 7, the data section. Section 7
//! starts with a 5-byte header: its length (4 bytes) and its number, 7 (1
//! byte). The codestream is the rest of the section, so the input here is
//! section 7 without those 5 bytes. This example does not parse a GRIB2
//! message; take the bytes and the three numbers from section 5 with your
//! own reader or a tool such as eccodes.
//!
//! The decode gives one channel of whole numbers X, in the grid's scanning
//! order. Section 5 gives the reference value R (an IEEE 32-bit float), the
//! binary scale factor E and the decimal scale factor D, and the physical
//! value is Y = (R + X × 2^E) / 10^D (WMO regulation 92.9.4, note 4). Pass R
//! with all its digits: `grib_get -F%.17g -p referenceValue` prints them,
//! while eccodes' default format rounds it. eccodes works out 10^-D by
//! repeated division and multiplies by it, so where D is positive a value
//! can differ from eccodes' in its last few bits.
//!
//! If section 5 gives 0 bits per value (octet 20), the field is constant and
//! section 7 may hold no codestream at all. Don't decode it then: every
//! point has the value R, and eccodes ignores E and D. If the message has
//! a bitmap in section 6, the values fill only the points it marks, in
//! order; this example ignores the bitmap.
//!
//! It prints the grid size, the smallest and largest value, and the first
//! few values. The output is for reading, not parsing: its layout can change.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

/// How many values to print from the start of the grid.
const FIRST: usize = 8;

/// Parse one number argument, naming it in the error.
fn number<T: FromStr>(arg: &OsString, what: &str) -> Result<T, String> {
    arg.to_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("{what}: not a number: {}", arg.display()))
}

/// Parse E or D: GRIB2 stores each in 16 bits as sign and magnitude, so it
/// is a whole number from -32767 to 32767.
fn factor(arg: &OsString, what: &str) -> Result<i16, String> {
    arg.to_str()
        .and_then(|s| s.parse::<i16>().ok())
        .filter(|&v| v != i16::MIN)
        .ok_or_else(|| {
            format!(
                "{what}: not a whole number from -32767 to 32767: {}",
                arg.display()
            )
        })
}

fn run(args: &[OsString]) -> Result<(), String> {
    let [input, r, e, d] = args else {
        return Err("usage: grib2 <section7.j2k> <R> <E> <D>".into());
    };
    let reference: f64 = number(r, "R")?;
    // Section 5 stores R as an IEEE 32-bit float.
    if !(reference as f32).is_finite() {
        return Err("R: must be a finite 32-bit float".into());
    }
    if f64::from(reference as f32) != reference {
        eprintln!("note: R is not a 32-bit float; pass all its digits");
    }
    let binary = factor(e, "E")?;
    let decimal = factor(d, "D")?;

    let input = PathBuf::from(input);
    let name = input.display();
    let bytes = std::fs::read(&input).map_err(|e| format!("{name}: {e}"))?;
    let image = rust_j2k::decode(&bytes).map_err(|e| format!("{name}: {e}"))?;
    let [grid] = image.components.as_slice() else {
        return Err(format!("{name}: a GRIB2 grid has one channel"));
    };

    // The formula as WMO gives it. 2^E is exact while it is a normal
    // f64, and 10^D is exact for |D| <= 22.
    let two_e = 2f64.powi(i32::from(binary));
    let ten_d = 10f64.powi(i32::from(decimal.unsigned_abs()));
    let scale = |x: i32| {
        let v = reference + f64::from(x) * two_e;
        if decimal >= 0 { v / ten_d } else { v * ten_d }
    };
    let values: Vec<f64> = grid.samples.iter().map(|&x| scale(x)).collect();
    if values.is_empty() {
        return Err(format!("{name}: the grid holds no values"));
    }
    // A large E or a negative D past about -300 overflows an f64.
    if let Some(i) = values.iter().position(|v| !v.is_finite()) {
        return Err(format!(
            "{name}: value {i} is not finite with this R, E and D"
        ));
    }

    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    println!("grid: {} x {}", grid.width, grid.height);
    println!("min: {min}");
    println!("max: {max}");
    let first = values.iter().take(FIRST).map(f64::to_string);
    let first: Vec<String> = first.collect();
    println!("first values: {}", first.join(" "));
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
