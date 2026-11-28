//! Whether more bytes can change what `read_header_with` gives on a JP2
//! prefix: [`replaceable`] tries suffixes that complete the box or
//! tile-part the input ends inside and then put a box at fault after the
//! codestream, and stands in for one too long to build, under the sample
//! limit such a suffix raises the default to. The `header_split`
//! fuzz target allows `header()` to hold an error back only where this
//! finds such a suffix, and `tests/jp2_box_error_final.rs` checks it on the
//! committed inputs.
//!
//! This file has no dependency but `rust_j2k` and `inconsistent.rs`, which
//! the including crate declares as a sibling module, so the fuzz targets
//! include both with `#[path]`, as they do `held.rs`.

#![allow(dead_code)]

use super::inconsistent::checked;
use rust_j2k::{DecodeOptions, SampleLimit, read_header_with};

fn u16_at(bytes: &[u8], at: usize) -> Option<usize> {
    let b = bytes.get(at..at.checked_add(2)?)?;
    Some(usize::from(u16::from_be_bytes([b[0], b[1]])))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<usize> {
    let b = bytes.get(at..at.checked_add(4)?)?;
    usize::try_from(u32::from_be_bytes([b[0], b[1], b[2], b[3]])).ok()
}

/// A `colr` box after the codestream too short for OpenJPEG's reader: a
/// fault the decode checks before the sRGB step and the tile-parts.
const SHORT_COLR: [u8; 10] = [0, 0, 0, 10, b'c', b'o', b'l', b'r', 1, 0];

/// Where the codestream starts in the JP2 file `bytes`: the payload of the
/// first top-level `jp2c` box, past the signature.
fn codestream_start(bytes: &[u8]) -> Option<usize> {
    let mut at = 12usize;
    loop {
        let lbox = u32_at(bytes, at)?;
        let kind = bytes.get(at + 4..at + 8)?;
        let (length, header) = match lbox {
            0 => (bytes.len() - at, 8),
            1 => (u32_at(bytes, at + 12)?, 16),
            n => (n, 8),
        };
        if kind == b"jp2c" {
            return Some(at + header);
        }
        if length < header {
            return None;
        }
        at = at.checked_add(length)?;
    }
}

/// The most zeros [`after_codestream`] pads a box with, and [`completed`]
/// a tile-part.
const MOST_PADDING: usize = 1 << 16;

/// A suffix after which the decode meets a box fault after the codestream.
enum Completion {
    /// The input with that suffix.
    Bytes(Vec<u8>),
    /// The suffix would take more than [`MOST_PADDING`] zeros, so it is not
    /// built: the box or tile-part the input ends inside is completed, then
    /// [`SHORT_COLR`] follows, as for [`Completion::Bytes`]. What it
    /// holds instead is the input with that box or tile-part dropped, then
    /// an `EOC` where it is a tile-part, then [`SHORT_COLR`]. The decode
    /// checks that box before the tile-parts, so an error read there too
    /// comes from before the bytes dropped, and no suffix replaces it.
    Far(Vec<u8>),
    /// The input ends inside a box of length 0 after the codestream, which
    /// OpenJPEG skips unread once 2^32 bytes or more follow its header
    /// ([`after_codestream`]). That suffix is not built: this holds the
    /// input up to that box, which [`replaceable`] reads under the sample
    /// limit the suffix raises the default to ([`grown`]).
    Skipped(Vec<u8>),
}

/// `options` with the sample limit an input 2^32 bytes or more longer has:
/// [`SampleLimit::Unlimited`], unless the limit is a ceiling the caller set
/// ([`SampleLimit::AtMost`]), which no byte raises.
///
/// The default limit grows with the input: it is the larger of 2^29 and 8
/// samples a byte (`samples::default_limit`). A `Raised` factor multiplies
/// it, and both stop at the samples the target can address, which is what
/// `Unlimited` allows. A
/// suffix of 2^32 bytes or more after a skipped box is a suffix of any
/// length past that, and the read of the codestream is the same for each,
/// as OpenJPEG reads nothing of that box: so the longest suffix, under the
/// largest limit, stands in for them all. A larger limit refuses fewer
/// tiles, so where a tile-part fault is given only because a tile is over
/// a cap, it gives way there, as it does on the longer input; a fault that
/// needs no cap to be met stays, and so does a tile past what the target
/// can address.
fn grown(options: &DecodeOptions) -> DecodeOptions {
    match options.sample_limit() {
        SampleLimit::AtMost(_) => options.clone(),
        _ => options.clone().with_sample_limit(SampleLimit::Unlimited),
    }
}

/// `prefix`, a JP2 file whose codestream ends at `at`, with the box after
/// it that the input ends inside made whole, with zeros or, where its
/// length is not in, the rest of [`SHORT_COLR`], then [`SHORT_COLR`]: a box
/// that OpenJPEG's walk after the codestream reads, at fault. A box of
/// length 0 runs to the end of the input, and OpenJPEG skips it unread once
/// 2^32 bytes or more follow its header, so its fault, whatever it is, is
/// one more bytes take away: that suffix is not built
/// ([`Completion::Skipped`]), and the input up to that box stands in for
/// it.
/// `None` where the walk reads no box past the input: it stopped at a
/// `jp2c`, a box of length 1 (an `XLBox`), or a box shorter than its
/// header.
fn after_codestream(prefix: &[u8], mut at: usize) -> Option<Completion> {
    loop {
        let rest = prefix.get(at..)?;
        if rest.len() < 4 {
            return Some(Completion::Bytes(
                [prefix, &SHORT_COLR[rest.len()..]].concat(),
            ));
        }
        let length = u32_at(rest, 0)?;
        if rest.get(4..8) == Some(&b"jp2c"[..]) {
            return None;
        }
        if length == 0 && rest.len() >= 8 {
            return Some(Completion::Skipped(prefix[..at].to_vec()));
        }
        if length < 8 {
            return None;
        }
        if rest.len() < length {
            if length - rest.len() > MOST_PADDING {
                return Some(Completion::Far([&prefix[..at], &SHORT_COLR].concat()));
            }
            let mut bytes = prefix.to_vec();
            bytes.resize(at + length, 0);
            bytes.extend(SHORT_COLR);
            return Some(Completion::Bytes(bytes));
        }
        at += length;
    }
}

/// The fewest bytes of an `SOT` segment and the start of its tile-part:
/// `Lsot` 10, tile 0, and `Psot` 14, a tile-part with no data.
const MIN_SOT: [u8; 10] = [0xFF, 0x90, 0, 10, 0, 0, 0, 0, 0, 14];

/// `prefix`, a JP2 file cut inside or just after its codestream's
/// tile-parts, with zeros to the end of the tile-part the input ends
/// inside, an `EOC`, then [`SHORT_COLR`]: an input on which the decode
/// meets a box fault before the sRGB step and every tile. Where the input
/// ends inside a marker or an `SOT` before the end of its `Psot`, the rest
/// of [`MIN_SOT`] completes it; where it holds the `EOC`, the boxes after
/// it are completed instead ([`after_codestream`]). The tile-part is not
/// padded where it takes more than [`MOST_PADDING`] zeros
/// ([`Completion::Far`]).
fn completed(prefix: &[u8]) -> Option<Completion> {
    let start = codestream_start(prefix)?;
    let codestream = &prefix[start..];
    if !codestream.starts_with(&[0xFF, 0x4F]) {
        return None;
    }
    // The main header, as `held.rs` walks it: the reserved `0xFF30` to
    // `0xFF3F` and `EPH` have no segment.
    let mut at = 2usize;
    loop {
        match u16_at(codestream, at)? {
            0xFF90 => break,
            0xFF30..=0xFF3F | 0xFF92 => at += 2,
            _ => {
                let length = u16_at(codestream, at + 2)?;
                if length < 2 {
                    return None;
                }
                at += 2 + length;
            }
        }
    }
    // Whether the zeros up to `end`, the end of the tile-part the input
    // ends inside, are over [`MOST_PADDING`].
    let far = |end: usize| end.saturating_sub(codestream.len()) > MOST_PADDING;
    // The input up to the tile-part at `part`, then an `EOC` and
    // [`SHORT_COLR`].
    let dropped = |part: usize| {
        Completion::Far([&prefix[..start + part], &[0xFF, 0xD9], &SHORT_COLR].concat())
    };
    // Where the last tile-part walked starts.
    let mut part = at;
    let end = loop {
        let rest = codestream.get(at..).unwrap_or_default();
        if rest.is_empty() {
            break at;
        }
        if rest.starts_with(&[0xFF, 0xD9]) {
            return after_codestream(prefix, start + at + 2);
        }
        let cut = rest.len().min(MIN_SOT.len());
        let sot = [&rest[..cut], &MIN_SOT[cut..]].concat();
        if u16_at(&sot, 0)? != 0xFF90 {
            return None;
        }
        let psot = u32_at(&sot, 6)?;
        if psot < 14 {
            return None;
        }
        if cut < MIN_SOT.len() {
            let end = at.checked_add(psot)?;
            if far(end) {
                return Some(dropped(at));
            }
            let mut bytes = [prefix, &sot[cut..]].concat();
            bytes.resize(start + end, 0);
            bytes.extend([0xFF, 0xD9]);
            bytes.extend(SHORT_COLR);
            return Some(Completion::Bytes(bytes));
        }
        part = at;
        at = at.checked_add(psot)?;
    };
    if far(end) {
        return Some(dropped(part));
    }
    let mut bytes = prefix.to_vec();
    bytes.resize(start + end, 0);
    bytes.extend([0xFF, 0xD9]);
    bytes.extend(SHORT_COLR);
    Some(Completion::Bytes(bytes))
}

/// Whether more bytes can change what `read_header_with` gives on the JP2
/// prefix `prefix`: some suffix tried here gives another answer. A fault of
/// a box held whole, and of the boxes before it, passes none. Where the
/// suffix is too long to build ([`Completion::Far`],
/// [`Completion::Skipped`]), the input it holds stands in for it, the
/// latter under the sample limit that suffix raises the default to.
pub fn replaceable(prefix: &[u8], options: &DecodeOptions) -> bool {
    let lenient = options.clone().with_strict(false);
    let answer = |bytes: &[u8], options: &DecodeOptions| {
        format!("{:?}", checked(read_header_with(bytes, options)))
    };
    let base = answer(prefix, &lenient);
    let mut longer = vec![
        ([prefix, &[0; 64][..]].concat(), lenient.clone()),
        ([prefix, &vec![0; 8192][..]].concat(), lenient.clone()),
        ([prefix, &SHORT_COLR[..]].concat(), lenient.clone()),
    ];
    match completed(prefix) {
        Some(Completion::Bytes(bytes) | Completion::Far(bytes)) => longer.push((bytes, lenient)),
        Some(Completion::Skipped(bytes)) => longer.push((bytes, grown(&lenient))),
        None => {}
    }
    longer
        .iter()
        .any(|(bytes, options)| answer(bytes, options) != base)
}
