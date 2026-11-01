//! The Resolution superbox (I.5.3.7, `res `) with its Capture Resolution
//! (`resc`) and Default Display Resolution (`resd`) boxes.
//!
//! OpenJPEG 2.5.0 skips `res ` without reading it, so no shape of it fails a
//! decode there. This reader never fails either: it walks the children of
//! every `res ` in the header and keeps the first well-formed `resc` and the
//! first well-formed `resd`. A child of the wrong length is ignored, and a
//! box-layer fault inside `res ` ends that box's walk. Only `jp2h` and `res `
//! are descended into, so there is no nesting to recurse through.

use super::boxes::Boxes;
use super::header::Jp2Header;
use super::metadata::{Ratio, Res, Resolution};

/// The capture and display resolutions the header's `res ` boxes give.
pub(crate) fn read(header: &Jp2Header<'_>) -> Resolution {
    let mut resolution = Resolution::default();
    for payload in header.res() {
        for child in Boxes::children(payload, "the `res ` box") {
            let Ok(child) = child else {
                break;
            };
            let slot = match &child.kind {
                b"resc" => &mut resolution.capture,
                b"resd" => &mut resolution.display,
                _ => continue,
            };
            if slot.is_none() {
                *slot = parse_res(child.payload);
            }
        }
    }
    resolution
}

/// A `resc` or `resd` payload: `VRN`, `VRD`, `HRN`, `HRD` (each `u16`), then
/// `VRE` and `HRE` (each `i8`). `None` unless it is exactly those 10 bytes.
fn parse_res(payload: &[u8]) -> Option<Res> {
    let &[vn0, vn1, vd0, vd1, hn0, hn1, hd0, hd1, ve, he] = payload else {
        return None;
    };
    Some(Res {
        vertical: Ratio {
            numerator: u16::from_be_bytes([vn0, vn1]),
            denominator: u16::from_be_bytes([vd0, vd1]),
            exponent: i8::from_be_bytes([ve]),
        },
        horizontal: Ratio {
            numerator: u16::from_be_bytes([hn0, hn1]),
            denominator: u16::from_be_bytes([hd0, hd1]),
            exponent: i8::from_be_bytes([he]),
        },
    })
}
