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
//! back while the input has not closed (`tests/support/held.rs`; a JP2 file
//! never closes): it gives the header where that read gives the error, or,
//! in a JP2 file whose box of length 0 before the codestream is at fault,
//! `None`.
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
    read_header_with, ColorOutput, DecodeOptions, Error, Header, IncrementalDecoder, Region,
    SampleLimit, Window,
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

/// Where the top-level boxes of the JP2 file `bytes` end, before any
/// `jp2c`, in a box of length 0, which runs to the end of the input: the
/// offset of that box. `header()` gives `None` on every fault there, as
/// more bytes grow the box, though a fault of a part of it held whole is
/// final; that wait is allowed, not required, where the boxes before it
/// leave `read_header_with` incomplete ([`check`]).
fn open_box_at(bytes: &[u8]) -> Option<usize> {
    let mut at = 12usize;
    while let (Some(lbox), Some(kind)) = (u32_at(bytes, at), bytes.get(at + 4..at + 8)) {
        if kind == b"jp2c" {
            return None;
        }
        let length = match lbox {
            0 => return Some(at),
            1 => match u32_at(bytes, at + 12) {
                Some(length) if length >= 16 => length,
                _ => return None,
            },
            n if n >= 8 => n,
            _ => return None,
        };
        at = at.checked_add(length)?;
    }
    None
}

/// Whether `prefix` ends inside a box of length 0 before the codestream
/// ([`open_box_at`]), and the boxes before it, read alone, leave
/// `read_header_with` incomplete: a fault there is not in an earlier box.
fn open_box_waits(prefix: &[u8], lenient: &DecodeOptions) -> bool {
    open_box_at(prefix).is_some_and(|at| {
        matches!(
            checked(read_header_with(&prefix[..at], lenient)),
            Err(Error::Incomplete(_))
        )
    })
}

/// `prefix`, a JP2 file cut inside or just after its codestream's
/// tile-parts, with zeros to the end of the tile-part the input ends
/// inside, an `EOC`, then [`SHORT_COLR`]: an input on which the decode
/// meets a box fault before the sRGB step and every tile.
fn completed(prefix: &[u8]) -> Option<Vec<u8>> {
    let start = codestream_start(prefix)?;
    let codestream = &prefix[start..];
    if !codestream.starts_with(&[0xFF, 0x4F]) {
        return None;
    }
    let mut at = 2usize;
    while u16_at(codestream, at)? != 0xFF90 {
        let length = u16_at(codestream, at + 2)?;
        if length < 2 {
            return None;
        }
        at += 2 + length;
    }
    let end = loop {
        match u16_at(codestream, at) {
            None if at >= codestream.len() => break at,
            Some(0xFF90) => {}
            Some(0xFFD9) => {
                return Some([prefix, &SHORT_COLR[..]].concat());
            }
            _ => return None,
        }
        let psot = u32_at(codestream, at + 6)?;
        if psot < 14 {
            return None;
        }
        at = at.checked_add(psot)?;
    };
    let mut bytes = prefix.to_vec();
    bytes.resize(start + end, 0);
    bytes.extend([0xFF, 0xD9]);
    bytes.extend(SHORT_COLR);
    Some(bytes)
}

/// Whether more bytes can change what `read_header_with` gives on the JP2
/// prefix `prefix`: some suffix tried here gives another answer. A fault of
/// a box held whole, and of the boxes before it, passes none.
fn replaceable(prefix: &[u8], options: &DecodeOptions) -> bool {
    let lenient = options.clone().with_strict(false);
    let answer = |bytes: &[u8]| format!("{:?}", checked(read_header_with(bytes, &lenient)));
    let base = answer(prefix);
    let mut longer = vec![
        [prefix, &[0; 64][..]].concat(),
        [prefix, &vec![0; 8192][..]].concat(),
        [prefix, &SHORT_COLR[..]].concat(),
    ];
    longer.extend(completed(prefix));
    longer.iter().any(|bytes| answer(bytes) != base)
}

/// Check `got`, what `header()` gave on `prefix` under `options`, against
/// the [`oracle`], allowing an error held back where more bytes can replace
/// it: in a codestream, while its tile-parts have not closed; in a JP2
/// file, where a suffix changes what `read_header_with` gives, or, for a
/// refusal of the sRGB step, as the header with the channels as coded,
/// or, inside a box of length 0 before the codestream, as `None`
/// ([`open_box_waits`]).
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
    let held = refused
        && if codestream {
            !held::closes(prefix)
        } else {
            replaceable(prefix, options) || as_coded()
        };
    match got {
        Ok(Some(_)) if held => {}
        Ok(None) if (held || refused && open_box_waits(prefix, &lenient)) && !codestream => {}
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
        // `read_header_with` gives on the whole input.
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
