//! Synthetic-bytes builders shared by the integration tests, the unit tests
//! and the fuzz targets. Std only, so a fuzz target can include it with
//! `#[path]` without pulling in the test dependencies.

#![allow(dead_code)]

/// A marker segment: the marker code, `Lxxx` (the body length plus the two
/// length bytes), then the body.
pub fn seg(code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = code.to_be_bytes().to_vec();
    out.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// A JP2 box: `LBox` (the payload length plus the eight header bytes), the
/// four-byte type, then the payload.
pub fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}
