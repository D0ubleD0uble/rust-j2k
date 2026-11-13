//! The decode's work bound ([`super::decode_visits`]) against the committed
//! files: what each one's packet walk spends, as a fraction of its bound.

use std::path::{Path, PathBuf};

use super::{decode_packets, decode_visits};

/// How far below its bound every committed file's walk must stay: at most
/// this fraction of it.
const MARGIN: f64 = 1.0 / 64.0;

/// Every `.j2k` and `.jp2` under `dir`, recursively.
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read fixtures").flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(&path, out);
        } else if path.extension().is_some_and(|e| e == "j2k" || e == "jp2") {
            out.push(path);
        }
    }
}

/// The visits an unbounded walk of every tile's packets spends, and the bytes
/// of tile data the decode's bound is scaled by; `None` when the file has no codestream to
/// walk or a tile's packets do not parse.
pub(crate) fn walk(bytes: &[u8]) -> Option<(u64, u64)> {
    let codestream = if bytes.starts_with(&[0xFF, 0x4F]) {
        bytes
    } else {
        crate::jp2::parse(bytes, &crate::WorkLimit::Default)
            .ok()?
            .codestream
    };
    let parsed = crate::codestream::parse_header(codestream).ok()?;
    let cs = crate::codestream::parse_tiles(codestream, parsed).ok()?;
    let mut left = u64::MAX;
    for (index, tile) in cs.tiles.iter().enumerate() {
        let header = cs.tile_header(index).ok()?;
        decode_packets(&header, tile, &mut left, u64::MAX).ok()?;
    }
    Some((u64::MAX - left, crate::tile_bytes(&cs)))
}

/// The bound is 2^24 visits, or 32 per byte of tile data past 2^19 bytes.
#[test]
fn the_work_bound_grows_with_the_tile_data() {
    assert_eq!(decode_visits(0), 1 << 24);
    assert_eq!(decode_visits(1 << 19), 1 << 24);
    assert_eq!(decode_visits((1 << 19) + 1), (1 << 24) + 32);
    assert_eq!(decode_visits(1 << 24), 1 << 29);
    assert_eq!(decode_visits(u64::MAX), u64::MAX);
}

/// Every committed file whose packets parse, conformance, real-world and
/// `opj_compress` many-layer files included, walks them in a small fraction
/// of the decode's bound, so the bound refuses none of them. Run with
/// `--nocapture` for the table.
#[test]
fn every_committed_file_walks_far_below_the_work_bound() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut paths = Vec::new();
    files(&root, &mut paths);
    paths.sort();
    let mut rows = Vec::new();
    for path in &paths {
        let name = path.strip_prefix(&root).unwrap().display().to_string();
        // These are made to go past the bound, but for the encoder output.
        if name.starts_with("work_bound/") && !name.starts_with("work_bound/opj_") {
            continue;
        }
        let bytes = std::fs::read(path).unwrap();
        if let Some((spent, data)) = walk(&bytes) {
            let bound = decode_visits(data);
            rows.push((spent as f64 / bound as f64, spent, data, name));
        }
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    eprintln!("fraction of bound\tvisits\tdata bytes\tvisits per byte\tfile");
    for (fraction, spent, data, name) in &rows {
        let per_byte = *spent as f64 / (*data).max(1) as f64;
        eprintln!("{fraction:.6}\t{spent}\t{data}\t{per_byte:.3}\t{name}");
    }
    eprintln!("{} files walked of {}", rows.len(), paths.len());
    // The conformance corpus and the real-world files are among them.
    assert!(rows.len() > 100, "{} files walked", rows.len());
    assert!(rows.iter().any(|row| row.3.starts_with("work_bound/opj_")));
    for (fraction, _, _, name) in &rows {
        assert!(*fraction <= MARGIN, "{name} spends {fraction} of its bound");
    }
}
