//! The Reader Requirements box (`rreq`, ISO/IEC 15444-2 M.11.1).
//!
//! ```text
//! ML    1 byte    the length of every mask in bytes
//! FUAM  ML bytes
//! DCM   ML bytes
//! NSF   2 bytes   then NSF times: SF (2 bytes), SM (ML bytes)
//! NVF   2 bytes   then NVF times: VF (16 bytes), VM (ML bytes)
//! ```
//!
//! OpenJPEG skips `rreq`, so a malformed one never fails a decode there
//! (`opj_decompress` decodes file5 with its `NSF` raised to 65535, or its `ML`
//! to 255, to the same samples). Here the box is read when it holds exactly
//! these fields and skipped when it is cut short or has bytes left over. Every
//! field is read from the payload with a bounds check before it is used, and
//! each entry takes at least two bytes of it, so the lists are bounded by the
//! payload's length and nothing is allocated from a count.

use super::metadata::{ReaderRequirements, StandardFeature, VendorFeature};

/// Read an `rreq` box's contents, or `None` when it is malformed.
pub(crate) fn parse(payload: &[u8]) -> Option<ReaderRequirements> {
    let mut reader = Reader(payload);
    let mask_length = reader.take::<1>()?[0];
    let mask = |reader: &mut Reader<'_>| reader.bytes(usize::from(mask_length));
    let fully_understand = mask(&mut reader)?;
    let display_completely = mask(&mut reader)?;
    let mut standard_features = Vec::new();
    for _ in 0..u16::from_be_bytes(*reader.take::<2>()?) {
        let feature = u16::from_be_bytes(*reader.take::<2>()?);
        standard_features.push(StandardFeature {
            feature,
            mask: mask(&mut reader)?,
        });
    }
    let mut vendor_features = Vec::new();
    for _ in 0..u16::from_be_bytes(*reader.take::<2>()?) {
        let id = *reader.take::<16>()?;
        vendor_features.push(VendorFeature {
            id,
            mask: mask(&mut reader)?,
        });
    }
    reader.0.is_empty().then_some(ReaderRequirements {
        mask_length,
        fully_understand,
        display_completely,
        standard_features,
        vendor_features,
    })
}

/// The unread rest of a payload.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    /// The next `N` bytes, or `None` when fewer are left.
    fn take<const N: usize>(&mut self) -> Option<&'a [u8; N]> {
        let (head, rest) = self.0.split_first_chunk::<N>()?;
        self.0 = rest;
        Some(head)
    }

    /// The next `len` bytes, copied, or `None` when fewer are left.
    fn bytes(&mut self, len: usize) -> Option<Vec<u8>> {
        let (head, rest) = self.0.split_at_checked(len)?;
        self.0 = rest;
        Some(head.to_vec())
    }
}
