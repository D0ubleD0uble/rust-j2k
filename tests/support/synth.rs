//! Synthetic-bytes builders shared by the integration tests, the unit tests
//! and the fuzz targets. Std only, so a fuzz target can include it with
//! `#[path]` without pulling in the test dependencies.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// `tests/fixtures`, or a directory under it. It reads the including test
/// crate's `CARGO_MANIFEST_DIR`, so fuzz targets (whose manifest is `fuzz/`)
/// must not call it.
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

/// The JP2 signature box.
pub const JP2_SIGNATURE: [u8; 12] = [0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];

/// The `ftyp` box of a plain JP2 file: brand `jp2 `, compatible with `jp2 `.
pub fn ftyp_jp2() -> Vec<u8> {
    bx(b"ftyp", b"jp2 \0\0\0\0jp2 ")
}

/// An `ihdr` payload: height, width, component count, then `BPC`, `C`,
/// `UnkC` and `IPR`.
pub fn ihdr_payload(height: u32, width: u32, components: u16, tail: [u8; 4]) -> Vec<u8> {
    let mut out = height.to_be_bytes().to_vec();
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&components.to_be_bytes());
    out.extend_from_slice(&tail);
    out
}

/// A `colr` payload with an enumerated color space.
pub fn colr_enum(enum_cs: u32) -> Vec<u8> {
    let mut out = vec![1, 0, 0];
    out.extend_from_slice(&enum_cs.to_be_bytes());
    out
}

/// A `colr` payload that carries an ICC `profile`.
pub fn colr_icc(profile: &[u8]) -> Vec<u8> {
    let mut out = vec![2, 0, 0];
    out.extend_from_slice(profile);
    out
}

/// A `cdef` box of (`Cn`, `Typ`, `Asoc`) entries.
pub fn cdef(entries: &[(u16, u16, u16)]) -> Vec<u8> {
    let count = u16::try_from(entries.len()).expect("too many cdef entries");
    let mut payload = count.to_be_bytes().to_vec();
    for &(cn, typ, asoc) in entries {
        for v in [cn, typ, asoc] {
            payload.extend_from_slice(&v.to_be_bytes());
        }
    }
    bx(b"cdef", &payload)
}

/// A JP2 file: the signature, `ftyp`, a `jp2h` of `header_boxes`, the
/// `between` boxes, a `jp2c` of `codestream`, then the `trailing` boxes.
pub fn jp2_file(
    header_boxes: &[Vec<u8>],
    between: &[Vec<u8>],
    codestream: &[u8],
    trailing: &[Vec<u8>],
) -> Vec<u8> {
    let mut out = JP2_SIGNATURE.to_vec();
    out.extend(ftyp_jp2());
    out.extend(bx(b"jp2h", &header_boxes.concat()));
    out.extend(between.concat());
    out.extend(bx(b"jp2c", codestream));
    out.extend(trailing.concat());
    out
}

/// A `SIZ` body: `Rsiz` 0, the eight grid fields (`Xsiz`, `Ysiz`, `XOsiz`,
/// `YOsiz`, `XTsiz`, `YTsiz`, `XTOsiz`, `YTOsiz`), `Csiz` and each
/// component's (`Ssiz`, `XRsiz`, `YRsiz`).
pub fn siz_body(grid: [u32; 8], components: &[[u8; 3]]) -> Vec<u8> {
    let mut out = 0u16.to_be_bytes().to_vec();
    for field in grid {
        out.extend_from_slice(&field.to_be_bytes());
    }
    let count = u16::try_from(components.len()).expect("too many components");
    out.extend_from_slice(&count.to_be_bytes());
    out.extend(components.concat());
    out
}
