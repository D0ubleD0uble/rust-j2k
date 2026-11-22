#![no_main]
//! Fuzz target over the public [`rust_j2k::read_header_with`] entry point.
//!
//! Same contract as `decode`: a typed result for every input, never a panic,
//! an allocation sized from an unchecked field, or a hang. Every input is run
//! under a fixed list of option sets (the defaults, `raw_channels`, a
//! resolution reduction of 1, sRGB output, reduction 1 with sRGB, the
//! sample limit lifted, and `strict` off), so a plain `.j2k` or `.jp2` seed
//! is used as it is.
//! `raw_channels` is left out for a raw codestream, where it repeats the
//! defaults. The header is read
//! under every set; the full decode that follows a header that reads runs
//! under one of the sets whose header read, picked by a hash of the input, to
//! keep the target fast. For each set it checks that
//! `read_header_with` and `decode_with` agree, since `decode_with` runs the
//! same header checks before it reads any tile data:
//!
//! - when `read_header_with` fails, `decode_with` fails with the same error,
//!   except where the header refuses a tile over a cap, or a tile whose own
//!   header has fewer levels than the reduction, and the decode finds a
//!   fault or a work bound in a lower tile's data first, which only decoding
//!   can see (`decode_finds_first_in_a_lower_tile`). That excuse holds only
//!   if decoding that tile alone (`Region::Tile`, in the color the file
//!   codes) gives the header's error too, so a header that refuses a tile
//!   the decode accepts is still caught;
//! - when `decode_with` succeeds, `read_header_with` reports the decoded
//!   image's area, each channel's format, and the same JP2 metadata.
//!
//! `decode_with` may still fail where `read_header_with` succeeds: a fault in
//! the tile data is only found by decoding it.
//!
//! Where a tile is refused, the header read finds the decode's error for the
//! tile-parts with a walk of its own that keeps no tile record. Each input up
//! to `MAX_DECODED_INPUT` bytes is also run through
//! `rust_j2k::fuzz::tile_parts_walks_agree`, which checks that walk against
//! the decode's parse directly, with `strict` on and off, on the input and
//! on prefixes of it.
//!
//! A full decode costs far more than the header, so when the header reads and
//! the input or the image is large, the decode is skipped and only
//! `read_header` is exercised. The `decode` target covers large decodes.

use libfuzzer_sys::fuzz_target;

#[path = "../../tests/support/inconsistent.rs"]
mod inconsistent;
use inconsistent::checked;

#[path = "../common/budget.rs"]
mod budget;

/// Inputs above this many bytes are not decoded once their header reads.
const MAX_DECODED_INPUT: usize = 1 << 16;
/// Images above this many samples, over all channels, are not decoded.
const MAX_DECODED_SAMPLES: u64 = 1 << 16;

fuzz_target!(|data: &[u8]| {
    let default = rust_j2k::DecodeOptions::default();
    let srgb = || default.clone().with_color(rust_j2k::ColorOutput::Srgb);
    let mut option_sets = vec![
        default.clone(),
        default.clone().with_resolution_reduction(1),
        srgb(),
        srgb().with_resolution_reduction(1),
        // The sample limit lifted: the header checks the address-space
        // ceiling instead of the default limit.
        default
            .clone()
            .with_sample_limit(rust_j2k::SampleLimit::Unlimited),
        // `strict` off: the decode takes missing data as missing, and the
        // header read admits a tile with no tile-part.
        default.clone().with_strict(false),
    ];
    // `raw_channels` skips the palette and the channel definitions, which a
    // raw codestream does not have, so there it repeats the defaults.
    if !is_raw_codestream(data) {
        option_sets.insert(1, default.clone().with_raw_channels(true));
    }
    // A full decode costs far more than a header read. A header that fails
    // is compared with a decode that stops just as early, so every set gets
    // that check; a header that reads is compared with a full decode under
    // one set only. That set is picked by a hash of the whole input, so that
    // seeds (which share their first and last bytes) still spread over the
    // sets, from among those whose header read, so that the input always
    // gets a full-decode comparison when any set reads.
    let headers: Vec<_> = option_sets
        .iter()
        .map(|options| checked(rust_j2k::read_header_with(data, options)))
        .collect();
    let readable: Vec<usize> = (0..headers.len()).filter(|&i| headers[i].is_ok()).collect();
    let picked =
        (!readable.is_empty()).then(|| readable[(fnv1a(data) % readable.len() as u64) as usize]);
    for (i, (options, header)) in option_sets.iter().zip(headers).enumerate() {
        check(data, options, header, picked == Some(i));
    }
    if data.len() <= MAX_DECODED_INPUT {
        rust_j2k::fuzz::tile_parts_walks_agree(data);
    }
});

/// Whether `rust_j2k` reads `data` as a raw codestream: a JP2 file opens
/// with the signature box, and anything else goes to the codestream parser.
fn is_raw_codestream(data: &[u8]) -> bool {
    data.starts_with(&[0xFF, 0x4F]) || data.get(4..8) != Some(b"jP  ")
}

/// FNV-1a over `bytes`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn check(
    data: &[u8],
    options: &rust_j2k::DecodeOptions,
    header: rust_j2k::Result<rust_j2k::Header>,
    decode_when_read: bool,
) {
    if let Ok(header) = &header {
        if !decode_when_read {
            return;
        }
        let samples = (u64::from(header.width) * u64::from(header.height))
            .saturating_mul(header.components.len() as u64);
        // The header lists output channels, which a palette makes fewer than
        // the components the decode allocates, so the `SIZ` count (the
        // budget's) bounds the decode too.
        if data.len() > MAX_DECODED_INPUT
            || samples > MAX_DECODED_SAMPLES
            || budget::declared(data) > u128::from(MAX_DECODED_SAMPLES)
        {
            return;
        }
    }
    let decoded = checked(rust_j2k::decode_with(data, options));
    match (&header, &decoded) {
        (Err(h), Err(d)) if decode_finds_first_in_a_lower_tile(data, options, h, d) => {}
        (Err(h), _) => assert_eq!(
            decoded.as_ref().err().map(|e| format!("{e:?}")),
            Some(format!("{h:?}")),
            "the same error"
        ),
        (Ok(header), Ok(image)) => {
            assert_eq!((header.width, header.height), (image.width, image.height));
            assert_eq!(header.components.len(), image.components.len());
            for (info, component) in header.components.iter().zip(&image.components) {
                assert_eq!(
                    (
                        info.bit_depth,
                        info.signed,
                        info.x_sampling,
                        info.y_sampling,
                        info.width,
                        info.height
                    ),
                    (
                        component.bit_depth,
                        component.signed,
                        component.x_sampling,
                        component.y_sampling,
                        component.width,
                        component.height
                    )
                );
            }
            assert_eq!(header.jp2, image.jp2);
        }
        (Ok(_), Err(_)) => {}
    }
}

/// Whether `decode` is an error the decode finds before the tile that the
/// header's refusal `header` names, a tile cap refusal or a reduction past
/// the levels of the tile's own header, in what only decoding finds:
/// the packets or code-blocks of a lower tile, or, with `strict` off, the
/// packet work charged for the tiles read that have no tile-part. The
/// `read_header_with` rustdoc and `docs/resource-bounds.md` (the paragraph
/// on what the decode can refuse "before it gets to that tile") say the two
/// can then differ.
///
/// It holds only when all of these do:
///
/// - `header` is a tile cap refusal of one tile, T, or a reduction past the
///   levels of T's own header, and `decode` is not an error the header
///   itself gives from a tile's own header: a tile cap or sample limit
///   refusal, or a reduction past a tile's levels.
/// - The decode gives `decode` again with T's refusal lifted, so the refusal
///   does not decide it: for a tile cap, by the retry it names (the options
///   as they are where none raises it); for a reduction, at reduction 0. For
///   an `AddressSpace` refusal this is vacuous: it names no retry, so the
///   lifted decode is the decode itself, and the rule rests on the next two
///   checks, the decodes of T alone.
/// - The decode of T alone, under the options as given, gives exactly the
///   header's error, so the header's refusal of T is the decode's too. A
///   header that refuses a tile the decode of that tile accepts is a bug that
///   a fault in another tile's data must not hide.
/// - The decode of T alone, with T's refusal lifted, does not give `decode`,
///   so it lies in the other tiles' data and not in what every decode reads,
///   such as the tile-parts.
fn decode_finds_first_in_a_lower_tile(
    data: &[u8],
    options: &rust_j2k::DecodeOptions,
    header: &rust_j2k::Error,
    decode: &rust_j2k::Error,
) -> bool {
    let Some((tile, lifted)) = refusal_of(options, header) else {
        return false;
    };
    if from_own_header(decode) {
        return false;
    }
    let debug = |result: rust_j2k::Result<rust_j2k::Image>| result.err().map(|e| format!("{e:?}"));
    let expected = Some(format!("{decode:?}"));
    if debug(checked(rust_j2k::decode_with(data, &lifted))) != expected {
        return false;
    }
    // The decode of T alone, under the options as given, refuses T as the
    // header does: the header's refusal is real, and not a bug of its own
    // (such as ignoring a tile-part `COD` that brings T within the caps).
    let refused = debug(checked(rust_j2k::decode_with(
        data,
        &tile_alone(options, tile),
    )));
    if refused != Some(format!("{header:?}")) {
        return false;
    }
    debug(checked(rust_j2k::decode_with(
        data,
        &tile_alone(&lifted, tile),
    ))) != expected
}

/// `options` for the decode of one tile alone, in the color the file codes.
/// A decode of a tile alone refuses sRGB output from sub-sampled sYCC chroma
/// ("an area smaller than the image") before it reads the tile, which hides
/// what these checks look for. The tile caps and a tile's own reduction
/// refusal do not depend on the color output.
fn tile_alone(options: &rust_j2k::DecodeOptions, tile: u32) -> rust_j2k::DecodeOptions {
    options
        .clone()
        .with_color(rust_j2k::ColorOutput::AsCoded)
        .with_region(rust_j2k::Region::Tile(tile))
}

/// The tile that `header` refuses and the options with that refusal
/// lifted: for a tile cap refusal ("tile T holds ..."), the retry it names
/// (the options as they are where none raises it); for a reduction past the
/// levels of T's own header, reduction 0. `None` for any other error.
fn refusal_of(
    options: &rust_j2k::DecodeOptions,
    header: &rust_j2k::Error,
) -> Option<(u32, rust_j2k::DecodeOptions)> {
    use rust_j2k::{Error, LimitKind};
    match header {
        Error::Limit(limit)
            if matches!(
                limit.kind(),
                LimitKind::TilePrecincts | LimitKind::TileCodeBlocks | LimitKind::AddressSpace
            ) =>
        {
            let tile = limit.refused_at()?.tile();
            Some((
                tile,
                options.raised_for(limit).unwrap_or_else(|| options.clone()),
            ))
        }
        Error::InvalidOptions(detail) => {
            let tile = reduced_tile(detail.message())?;
            Some((tile, options.clone().with_resolution_reduction(0)))
        }
        _ => None,
    }
}

/// Whether `decode` is an error the header read gives from a tile's own
/// header too: a tile cap or sample limit refusal, or a reduction past a
/// tile's levels.
fn from_own_header(decode: &rust_j2k::Error) -> bool {
    use rust_j2k::{Error, LimitKind};
    match decode {
        Error::Limit(other) => matches!(
            other.kind(),
            LimitKind::Samples
                | LimitKind::TilePrecincts
                | LimitKind::TileCodeBlocks
                | LimitKind::AddressSpace
        ),
        Error::InvalidOptions(_) => true,
        _ => false,
    }
}

/// The tile a reduction refusal of a tile's own header names: "resolution
/// reduction R discards ... in tile T".
fn reduced_tile(message: &str) -> Option<u32> {
    let (_, tile) = message
        .strip_prefix("resolution reduction ")?
        .rsplit_once(" in tile ")?;
    tile.parse().ok()
}
