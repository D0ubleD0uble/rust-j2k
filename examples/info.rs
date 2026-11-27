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

#[path = "common/say.rs"]
mod say;
use say::{say, say_err};

fn print_header(header: &Header) {
    say!("size: {} x {}", header.width, header.height);
    let area = header.image_area;
    let (x0, x1, y0, y1) = (area.x0, area.x1, area.y0, area.y1);
    say!("image area: x {x0}..{x1}, y {y0}..{y1}");
    say!("components: {}", header.components.len());
    for (i, c) in header.components.iter().enumerate() {
        say!(
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
    say!(
        "tiles: {} x {} = {}, each {} x {}, grid starts at ({}, {})",
        t.columns,
        t.rows,
        t.count(),
        t.tile_width,
        t.tile_height,
        t.x_offset,
        t.y_offset
    );
    say!(
        "resolution levels: at least {} (the largest reduction is {})",
        u16::from(header.max_reduction) + 1,
        header.max_reduction
    );
    say!("quality layers: {}", header.quality_layers);
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

fn show_count<T>(what: &str, boxes: &[T]) {
    say!("  {what}: {}", boxes.len());
}

fn print_jp2(jp2: &Jp2Metadata) {
    say!("JP2 metadata:");
    match &jp2.color {
        ColorSpace::RestrictedIcc(profile) => {
            say!(
                "  color space: restricted ICC profile, {} bytes",
                profile.len()
            );
        }
        other => say!("  color space: {other:?}"),
    }
    say!("  channels:");
    for (i, c) in jp2.channels.iter().enumerate() {
        say!("    {i}: {:?}, {:?}", c.kind, c.association);
    }
    say!("  palette applied: {}", jp2.palette_applied);
    if let Some(palette) = &jp2.palette {
        say!(
            "  palette: {} entries, {} columns",
            palette.entry_count(),
            palette.columns().len()
        );
    }
    say!("  capture resolution: {}", describe(jp2.resolution.capture));
    say!("  display resolution: {}", describe(jp2.resolution.display));
    show_count("xml boxes", &jp2.xml);
    show_count("UUID boxes", &jp2.uuid);
    show_count("intellectual property boxes", &jp2.ipr);
    show_count("UUID info boxes", &jp2.uuid_info);
    let ft = &jp2.file_type;
    let codes = ft.compatibility.iter().map(|b| fourcc(*b));
    let compatible: Vec<_> = codes.collect();
    say!(
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
        None => say!("JP2 metadata: none (a raw codestream)"),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            say_err!("{e}");
            ExitCode::FAILURE
        }
    }
}
