//! Print what `read_header` says about a JPEG 2000 file, without decoding it.
//!
//! It shows `Header`: the image size, each component's depth, sign and
//! subsampling, the tile grid, the resolution levels and quality layers, and,
//! for a JP2 file, the metadata from its boxes.
//!
//! ```text
//! cargo run --example info -- tests/fixtures/jp2_metadata_boxes.jp2
//! cargo run --example info -- tests/fixtures/multilayer_lossless.j2k
//! ```
//!
//! The output is for reading, not parsing: its layout can change. The example
//! uses the default limits, so a header that needs a raised limit is refused
//! with an error that names it; pass a `Raised` limit through
//! `read_header_with` in your own code.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use rust_j2k::{ColorSpace, Header, Jp2Metadata, Ratio, Res, read_header};

fn print_header(header: &Header) {
    println!("size: {} x {}", header.width, header.height);
    let area = header.image_area;
    println!(
        "image area: x {}..{}, y {}..{}",
        area.x0, area.x1, area.y0, area.y1
    );
    println!("components: {}", header.components.len());
    for (i, c) in header.components.iter().enumerate() {
        println!(
            "  {i}: {}x{}, {}-bit {}, subsampling {}x{}",
            c.width,
            c.height,
            c.bit_depth,
            if c.signed { "signed" } else { "unsigned" },
            c.x_sampling,
            c.y_sampling
        );
    }
    let t = &header.tiles;
    println!(
        "tiles: {} x {} = {}, each {} x {}, grid starts at ({}, {})",
        t.columns,
        t.rows,
        t.count(),
        t.tile_width,
        t.tile_height,
        t.x_offset,
        t.y_offset
    );
    println!(
        "resolution levels: at least {} (the largest reduction is {})",
        u16::from(header.max_reduction) + 1,
        header.max_reduction
    );
    println!("quality layers: {}", header.quality_layers);
}

fn fourcc(code: [u8; 4]) -> String {
    format!("{:?}", String::from_utf8_lossy(&code))
}

/// A resolution in grid points per meter, or a note that the file gives none.
fn describe(res: Option<Res>) -> String {
    let Some(res) = res else {
        return "not given".to_string();
    };
    let per_meter = |r: Ratio| {
        r.grid_points_per_meter()
            .map_or("n/a".into(), |v| format!("{v:.2}"))
    };
    format!(
        "{} horizontal, {} vertical grid points per meter",
        per_meter(res.horizontal),
        per_meter(res.vertical)
    )
}

fn print_jp2(jp2: &Jp2Metadata) {
    println!("JP2 metadata:");
    match &jp2.color {
        ColorSpace::RestrictedIcc(profile) => {
            println!(
                "  color space: restricted ICC profile, {} bytes",
                profile.len()
            );
        }
        other => println!("  color space: {other:?}"),
    }
    println!("  channels:");
    for (i, c) in jp2.channels.iter().enumerate() {
        println!("    {i}: {:?}, {:?}", c.kind, c.association);
    }
    println!("  palette applied: {}", jp2.palette_applied);
    if let Some(palette) = &jp2.palette {
        println!(
            "  palette: {} entries, {} columns",
            palette.entry_count(),
            palette.columns().len()
        );
    }
    println!("  capture resolution: {}", describe(jp2.resolution.capture));
    println!("  display resolution: {}", describe(jp2.resolution.display));
    let counts = [
        ("xml", jp2.xml.len()),
        ("uuid", jp2.uuid.len()),
        ("intellectual property", jp2.ipr.len()),
        ("uuid info", jp2.uuid_info.len()),
    ];
    for (kind, count) in counts {
        println!("  {kind} boxes: {count}");
    }
    let ft = &jp2.file_type;
    let codes = ft.compatibility.iter().map(|b| fourcc(*b));
    let compatible: Vec<_> = codes.collect();
    println!(
        "  file type: brand {}, minor version {}, compatible with [{}]",
        fourcc(ft.brand),
        ft.minor_version,
        compatible.join(", ")
    );
}

fn run(args: &[OsString]) -> Result<(), String> {
    let [path] = args else {
        return Err("usage: info <file.jp2|file.j2k>".into());
    };
    let path = PathBuf::from(path);
    let name = path.display();
    let bytes = std::fs::read(&path).map_err(|e| format!("{name}: {e}"))?;
    let header = read_header(&bytes).map_err(|e| format!("{name}: {e}"))?;
    print_header(&header);
    match &header.jp2 {
        Some(jp2) => print_jp2(jp2),
        None => println!("JP2 metadata: none (a raw codestream)"),
    }
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
