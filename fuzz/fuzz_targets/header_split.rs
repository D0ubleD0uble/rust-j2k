#![no_main]
//! Fuzz target over [`rust_j2k::IncrementalDecoder::header`] fed in pieces.
//!
//! The decoder keeps what its header read found between calls (the walk of
//! the tile-parts, the main header's tile counts, what each whole first
//! tile-part gave) and goes on from there when more bytes come in. Each call
//! must still give what `read_header_with` gives with `strict` off on the
//! bytes fed so far, under the decoder's options: the same header, the same
//! error, and `None` where that read finds the main header incomplete. The
//! other targets never call `header()`, and the committed differential test
//! (`tests/incremental_header.rs`) uses the default limits only.
//!
//! The exception is an error more bytes can replace, which `header()` holds
//! back while the input has not closed (`tests/support/held.rs`: in a JP2
//! file, while its codestream has not closed or a box after it can still be
//! read): it gives the header where that read gives the error, or, in a JP2
//! file whose box of length 0 before the codestream is at fault, `None`. In
//! a JP2 file each also needs a suffix that changes what that read gives
//! ([`replaceable`]), but for the header on a refusal of the sRGB step.
//!
//! Layout: 12 control bytes, then the file.
//!
//! - Byte 0, mod 3: the resolution reduction.
//! - Byte 1: bit 0 is `strict` (the oracle reads with it off, as `header()`
//!   documents), bits 1 and 2 the sample limit (`Default`, `Raised`,
//!   `Unlimited` or `AtMost`, sized by byte 4).
//! - Bytes 2 and 3: the region (the whole image, a tile, or one of two kinds
//!   of window).
//! - Byte 5: bit 0 sRGB output, bit 1 raw channels, bit 2 a component subset,
//!   bit 3 a layer cap.
//! - Bytes 6 to 11: the seed of the stream that picks the sizes of the pieces
//!   and the changes of options between them.
//!
//! The file is fed in pieces of random sizes, about 24 on average, so a
//! tile-part is often cut in the middle. Now and then the region, the
//! reduction or the layer cap changes. After each feed `header()` is checked
//! against the oracle on the prefix fed so far, and on one feed in five a
//! second call is checked too, which must give the same again.
//!
//! Random bytes seldom reach a tile over a cap under the main header, which
//! is what the cache exists for, so seed it from
//! `scripts/gen-header-split-seeds.py` (see `fuzz/README.md`).

use libfuzzer_sys::fuzz_target;

#[path = "../../tests/support/held.rs"]
mod held;
#[path = "../../tests/support/inconsistent.rs"]
mod inconsistent;
use inconsistent::checked;
use rust_j2k::{
    read_header_with, ColorOutput, DecodeOptions, Error, Header, IncrementalDecoder, LimitKind,
    Raise, Region, SampleLimit, Window,
};

/// splitmix64: the same stream on every platform, from a seed in the input.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// A region from two input bytes, and `salt` (the bytes fed or the file's
/// length) for the small window. The windows are laid out for tiles 1040
/// wide and 512 high, which the synthetic seeds use.
fn region(code: u8, arg: u8, salt: usize) -> Region {
    match code % 4 {
        0 => Region::Whole,
        1 => Region::Tile(u32::from(arg % 80)),
        2 => {
            let x = u32::from(arg) * 1040 / 8;
            Region::Window(Window::new(x, 0, x + 1040 * 3, 512))
        }
        _ => {
            let x = (salt as u32 % 7) * 3;
            let size = 1 + u32::from(arg);
            Region::Window(Window::new(x, x, x + size, x + size))
        }
    }
}

/// What `header()` must give on `prefix` under `options`, as text (`Header`
/// and `Error` have no `PartialEq`, and their `Debug` text holds every
/// field): `read_header_with` with `strict` off, an incomplete main header
/// as `None`.
fn oracle(prefix: &[u8], options: &DecodeOptions) -> String {
    format!(
        "{:?}",
        match checked(read_header_with(
            prefix,
            &options.clone().with_strict(false)
        )) {
            Ok(header) => Ok(Some(header)),
            Err(Error::Incomplete(_)) => Ok(None),
            Err(error) => Err(error),
        }
    )
}

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
    /// comes from before the bytes dropped, and no suffix replaces it. For
    /// a box of length 0 after the codestream, the suffix is 2^32 bytes,
    /// past which OpenJPEG skips the box unread, and this holds the input
    /// up to that box ([`after_codestream`]).
    Far(Vec<u8>),
}

/// `prefix`, a JP2 file whose codestream ends at `at`, with the box after
/// it that the input ends inside made whole, with zeros or, where its
/// length is not in, the rest of [`SHORT_COLR`], then [`SHORT_COLR`]: a box
/// that OpenJPEG's walk after the codestream reads, at fault. A box of
/// length 0 runs to the end of the input, and OpenJPEG skips it unread once
/// 2^32 bytes or more follow its header, so its fault, whatever it is, is
/// one more bytes take away: that suffix is not built
/// ([`Completion::Far`]), and the input up to that box stands in for it.
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
            return Some(Completion::Far(prefix[..at].to_vec()));
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
/// suffix is too long to build ([`Completion::Far`]), the input it holds
/// stands in for it.
fn replaceable(prefix: &[u8], options: &DecodeOptions) -> bool {
    let lenient = options.clone().with_strict(false);
    let answer = |bytes: &[u8]| format!("{:?}", checked(read_header_with(bytes, &lenient)));
    let base = answer(prefix);
    let mut longer = vec![
        [prefix, &[0; 64][..]].concat(),
        [prefix, &vec![0; 8192][..]].concat(),
        [prefix, &SHORT_COLR[..]].concat(),
    ];
    match completed(prefix) {
        Some(Completion::Bytes(bytes) | Completion::Far(bytes)) => longer.push(bytes),
        None => {}
    }
    longer.iter().any(|bytes| answer(bytes) != base)
}

/// Check `got`, what `header()` gave on `prefix` under `options`, against
/// the [`oracle`], allowing an error held back where more bytes can replace
/// it, while the input has not closed ([`held::closes`]): in a codestream,
/// always; in a JP2 file, where a suffix changes what `read_header_with`
/// gives, or, for a refusal of the sRGB step, as the header with the
/// channels as coded, or for a tile cap that grows with the input, which
/// more bytes lift (`refine` waits on it as `Waiting::SampleLimit`;
/// past the fuzz input size, so no suffix here tries it). A
/// JP2 file's `None` for an error needs such a suffix too: a fault in a
/// box of length 0 before the codestream that more bytes do not change,
/// such as one of a whole child of a `jp2h`, is final.
fn check(got: &Result<Option<Header>, Error>, prefix: &[u8], options: &DecodeOptions, at: &str) {
    let want = oracle(prefix, options);
    if format!("{got:?}") == want {
        return;
    }
    let lenient = options.clone().with_strict(false);
    let refused = matches!(
        read_header_with(prefix, &lenient),
        Err(error) if !matches!(error, Error::Incomplete(_))
    );
    let codestream = prefix.starts_with(&[0xFF, 0x4F]);
    let as_coded = || {
        matches!(options.color(), ColorOutput::Srgb)
            && matches!(got, Ok(Some(_)))
            && format!("{got:?}")
                == format!(
                    "{:?}",
                    checked(read_header_with(
                        prefix,
                        &lenient.clone().with_color(ColorOutput::AsCoded)
                    ))
                    .map(Some)
                )
    };
    // Held as the header, never as `None`, which in a JP2 file is a fault
    // of a box of length 0 before the codestream.
    let liftable = || {
        matches!(got, Ok(Some(_)))
            && matches!(
                read_header_with(prefix, &lenient),
                Err(Error::Limit(limit))
                    if matches!(
                        limit.kind(),
                        LimitKind::Samples | LimitKind::TileCodeBlocks | LimitKind::TilePrecincts
                    ) && matches!(limit.retry_with(), Some(Raise::Raised(_) | Raise::Unlimited))
            )
    };
    let held = refused
        && !held::closes(prefix)
        && (codestream || replaceable(prefix, options) || as_coded() || liftable());
    match got {
        Ok(Some(_)) if held => {}
        Ok(None) if held && !codestream => {}
        _ => panic!("{at}: {got:?}, where read_header_with gives {want}"),
    }
}

fuzz_target!(|input: &[u8]| {
    if input.len() < 12 {
        return;
    }
    let (c, data) = input.split_at(12);
    let mut options = DecodeOptions::default()
        .with_resolution_reduction(c[0] % 3)
        .with_strict(c[1] & 1 == 1)
        .with_region(region(c[2], c[3], data.len()));
    options = options.with_sample_limit(match c[1] >> 1 & 3 {
        0 => SampleLimit::Default,
        1 => SampleLimit::Raised(1 + u32::from(c[4] % 8)),
        2 => SampleLimit::Unlimited,
        _ => SampleLimit::AtMost(1 + (u64::from(c[4]) << 16)),
    });
    if c[5] & 1 == 1 {
        options = options.with_color(ColorOutput::Srgb);
    }
    if c[5] & 2 == 2 {
        options = options.with_raw_channels(true);
    }
    if c[5] & 4 == 4 {
        options = options.with_components(Some(vec![0]));
    }
    if c[5] & 8 == 8 {
        options = options.with_max_quality_layers(Some(1));
    }
    // Options the decoder refuses at the start leave nothing to check.
    let Ok(mut decoder) = checked(IncrementalDecoder::new(options)) else {
        return;
    };
    let mut rng = Rng(u64::from_le_bytes([
        c[6], c[7], c[8], c[9], c[10], c[11], 0, 0,
    ]));
    // About 24 feeds on average, so the oracle's reads from the start cost
    // about that many passes over the input.
    let most = (data.len() / 24).max(1);
    let mut fed = 0;
    while fed < data.len() {
        let piece = 1 + (rng.next() as usize) % (2 * most);
        let to = (fed + piece).min(data.len());
        decoder.feed(&data[fed..to]);
        fed = to;
        let r = rng.next();
        if r % 8 == 0 {
            let arg = (r >> 8) as u8;
            // A change that is refused leaves the options as they were, and
            // the oracle reads them back from the decoder.
            let _ = match (r >> 16) % 3 {
                0 => checked(decoder.set_region(region((r >> 24) as u8, arg, fed))),
                1 => checked(decoder.set_resolution_reduction(arg % 3)),
                _ => checked(decoder.set_max_quality_layers(Some(1 + u16::from(arg % 3)))),
            };
        }
        let got = checked(decoder.header());
        check(
            &got,
            &data[..fed],
            decoder.options(),
            &format!("at {fed} of {}", data.len()),
        );
        // An error is final: more bytes do not change it, so it is what
        // `read_header_with` gives on the whole input. A tile-cap refusal
        // more bytes lift, given once the input closes, can still be lifted
        // by bytes after the `EOC` (about 128 MiB for the tile of #649),
        // which is past the fuzz input size limit.
        if let Err(error) = &got {
            let whole = checked(read_header_with(
                data,
                &decoder.options().clone().with_strict(false),
            ));
            assert_eq!(
                format!("{:?}", Err::<Header, _>(error)),
                format!("{:?}", whole.as_ref()),
                "at {fed} of {}: an error the whole input does not give",
                data.len()
            );
        }
        if r % 5 == 0 {
            assert_eq!(
                format!("{:?}", checked(decoder.header())),
                format!("{got:?}"),
                "again at {fed}"
            );
        }
    }
});
