//! Synthetic-bytes builders shared by the integration tests, the unit tests
//! and the fuzz targets. Std only, so a fuzz target can include it with
//! `#[path]` without pulling in the test dependencies.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// `tests/fixtures`, or a directory under it.
pub fn fixtures(sub: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    if sub.is_empty() { root } else { root.join(sub) }
}

/// The bytes of a file; a failure names the path.
pub fn read_file(path: impl AsRef<Path>) -> Vec<u8> {
    let path = path.as_ref();
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A marker segment: the marker code, `Lxxx` (the body length plus the two
/// length bytes), then the body.
pub fn seg(code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = code.to_be_bytes().to_vec();
    let length = u16::try_from(body.len() + 2).expect("marker segment body too long");
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// A JP2 box: `LBox` (the payload length plus the eight header bytes), the
/// four-byte type, then the payload.
pub fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let length = u32::try_from(8 + payload.len()).expect("box payload too long");
    let mut out = length.to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}
