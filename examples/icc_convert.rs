//! Convert a codestream to sRGB through an ICC profile and write the samples.
//!
//! The tool behind `scripts/check-icc-exhaustive.py`, which compares the
//! output with the system LittleCMS. It wraps the codestream in a JP2 file
//! whose `colr` box holds the profile, decodes it with
//! `ColorOutput::Srgb`, and writes the samples pixel by pixel, each channel
//! as one byte for 8-bit samples or two little-endian bytes for 16-bit ones.
//!
//! ```text
//! cargo run --release --example icc_convert -- <codestream> <profile> <out>
//! ```
//!
//! On success it prints `channels=<n> depth=<bits>` to stdout. Exit status 3
//! means the decoder declines the profile with `Error::Unsupported`, and 1
//! any other failure.

use std::process::ExitCode;

use rust_j2k::{ColorOutput, DecodeOptions, Error, decode_with, read_header};

fn jp2_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// A JP2 file around `codestream` whose `colr` box carries `profile`.
fn wrap(codestream: &[u8], profile: &[u8]) -> Result<Vec<u8>, Error> {
    let header = read_header(codestream)?;
    let mut ihdr = header.height.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&header.width.to_be_bytes());
    ihdr.extend_from_slice(&(header.components.len() as u16).to_be_bytes());
    ihdr.extend_from_slice(&[header.components[0].bit_depth - 1, 7, 0, 0]);
    let mut colr = vec![2, 0, 0];
    colr.extend_from_slice(profile);
    let mut jp2h = jp2_box(b"ihdr", &ihdr);
    jp2h.extend(jp2_box(b"colr", &colr));
    let mut out = b"\x00\x00\x00\x0cjP  \r\n\x87\n".to_vec();
    out.extend(jp2_box(b"ftyp", b"jp2 \0\0\0\0jp2 "));
    out.extend(jp2_box(b"jp2h", &jp2h));
    out.extend(jp2_box(b"jp2c", codestream));
    Ok(out)
}

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [codestream, profile, out] = args else {
        return Err("usage: icc_convert <codestream> <profile> <out>".into());
    };
    let codestream = std::fs::read(codestream)?;
    let profile = std::fs::read(profile)?;
    let options = DecodeOptions::default().with_color(ColorOutput::Srgb);
    let image = decode_with(&wrap(&codestream, &profile)?, &options)?;
    let depth = image.components[0].bit_depth;
    let pixels = image.components[0].samples.len();
    let mut bytes = Vec::with_capacity(pixels * image.components.len() * 2);
    for i in 0..pixels {
        for component in &image.components {
            let sample = component.samples[i];
            if depth <= 8 {
                bytes.push(sample as u8);
            } else {
                bytes.extend_from_slice(&(sample as u16).to_le_bytes());
            }
        }
    }
    std::fs::write(out, bytes)?;
    println!("channels={} depth={depth}", image.components.len());
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            match e.downcast_ref::<Error>() {
                Some(Error::Unsupported(_)) => ExitCode::from(3),
                _ => ExitCode::FAILURE,
            }
        }
    }
}
