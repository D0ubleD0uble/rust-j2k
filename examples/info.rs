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

use std::process::ExitCode;

use rust_j2k::{Header, Jp2Metadata, Ratio, Res, read_header};

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
        "resolution levels: {} (the largest reduction is {})",
        u16::from(header.max_reduction) + 1,
        header.max_reduction
    );
    println!("quality layers: {}", header.quality_layers);
}

fn fourcc(code: [u8; 4]) -> String {
    String::from_utf8_lossy(&code).into_owned()
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
    println!("  color space: {:?}", jp2.color);
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
    println!(
        "  boxes: {} xml, {} uuid, {} ipr, {} uuid info",
        jp2.xml.len(),
        jp2.uuid.len(),
        jp2.ipr.len(),
        jp2.uuid_info.len()
    );
    let ft = &jp2.file_type;
    let compatible: Vec<_> = ft.compatibility.iter().map(|b| fourcc(*b)).collect();
    println!(
        "  file type: brand {:?}, minor version {}, compatible with {}",
        fourcc(ft.brand),
        ft.minor_version,
        compatible.join(", ")
    );
}

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [path] = args else {
        return Err("usage: info <file.jp2|file.j2k>".into());
    };
    let bytes = std::fs::read(path)?;
    let header = read_header(&bytes)?;
    print_header(&header);
    match &header.jp2 {
        Some(jp2) => print_jp2(jp2),
        None => println!("JP2 metadata: none (a raw codestream)"),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("info: {e}");
            ExitCode::FAILURE
        }
    }
}
