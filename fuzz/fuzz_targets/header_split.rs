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
//! A change of options that `read_header_with` refuses outright on the
//! bytes so far (a reduction past the image's resolutions, a tile it does
//! not have) stays in force until a later change lifts it, and the checks
//! meanwhile meet that refusal. So the first time the oracle gives
//! `InvalidOptions` where the options the input starts with are not
//! refused, a shadow decoder under those options is fed the bytes so far,
//! then each later piece, and checked after each feed as the decoder is.
//! The refusal is not taken as final: more bytes can bring an error that
//! comes first. After the last feed, a fresh decoder fed the whole input
//! at once under the starting options is checked too, with a second call.
//!
//! Random bytes seldom reach a tile over a cap under the main header, which
//! is what the cache exists for, so seed it from
//! `scripts/gen-header-split-seeds.py` (see `fuzz/README.md`).

use libfuzzer_sys::fuzz_target;

#[path = "../../tests/support/held.rs"]
mod held;
#[path = "../../tests/support/inconsistent.rs"]
mod inconsistent;
#[path = "../../tests/support/pieces.rs"]
mod pieces;
#[path = "../../tests/support/replaceable.rs"]
mod replaceable;
use pieces::{Change, Feed, Pieces};
use replaceable::replaceable;
use inconsistent::checked;
use rust_j2k::{
    read_header_with, ColorOutput, DecodeOptions, Error, Header, IncrementalDecoder, LimitKind,
    Raise, Region, SampleLimit, Window,
};

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

/// Check `got`, what `header()` gave on `prefix` under `options`, against
/// the [`oracle`], allowing an error held back where more bytes can replace
/// it, while the input has not closed ([`held::closes`]): in a codestream,
/// always; in a JP2 file, where a suffix changes what `read_header_with`
/// gives ([`replaceable`], which reads the input up to a box of length 0
/// after the codestream under the sample limit the 2^32 bytes that skip it
/// raise the default to, so a tile-part fault met only because a tile is
/// over a cap gives way there), or, for a refusal of the sRGB step, as the
/// header with the channels as coded, or for a tile cap that grows with the
/// input, which more bytes lift (`refine` waits on it as
/// `Waiting::SampleLimit`; past the fuzz input size, so no suffix here
/// tries it). A JP2 file's `None` for an error needs such a suffix too: a
/// fault in a box of length 0 before the codestream that more bytes do not
/// change, such as one of a whole child of a `jp2h`, is final. Returns what
/// the oracle gives.
fn check(
    got: &Result<Option<Header>, Error>,
    prefix: &[u8],
    options: &DecodeOptions,
    at: &str,
) -> String {
    let want = oracle(prefix, options);
    if format!("{got:?}") == want {
        return want;
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
    want
}

/// Check `decoder`'s `header()` once it holds the first `fed` bytes of
/// `data`, under its options ([`check`]), and, where `again`, a second
/// call, which must give the same. Returns what the oracle gives.
fn check_call(
    decoder: &IncrementalDecoder,
    data: &[u8],
    fed: usize,
    again: bool,
    at: &str,
) -> String {
    let got = checked(decoder.header());
    let want = check(&got, &data[..fed], decoder.options(), at);
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
            "{at}: an error the whole input does not give",
        );
    }
    if again {
        assert_eq!(
            format!("{:?}", checked(decoder.header())),
            format!("{got:?}"),
            "{at}: again"
        );
    }
    want
}

/// Whether the oracle's answer is a refusal of the options.
fn refuses_options(want: &str) -> bool {
    want.starts_with("Err(InvalidOptions(")
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
    let Ok(mut decoder) = checked(IncrementalDecoder::new(options.clone())) else {
        return;
    };
    let mut fed = 0;
    // A decoder under the starting options, started the first time the
    // oracle refuses the decoder's options but not those.
    let mut shadow: Option<IncrementalDecoder> = None;
    // About 24 feeds on average, so the oracle's reads from the start cost
    // about that many passes over the input.
    for Feed { to, change, again } in Pieces::new(c, data.len()) {
        decoder.feed(&data[fed..to]);
        fed = to;
        // A change that is refused leaves the options as they were, and
        // the oracle reads them back from the decoder.
        let _ = match change {
            Some(Change::Region { code, arg }) => {
                checked(decoder.set_region(region(code, arg, fed)))
            }
            Some(Change::Reduction(levels)) => checked(decoder.set_resolution_reduction(levels)),
            Some(Change::Layers(layers)) => checked(decoder.set_max_quality_layers(Some(layers))),
            None => Ok(()),
        };
        let at = format!("at {fed} of {}", data.len());
        let want = check_call(&decoder, data, fed, again, &at);
        match &mut shadow {
            Some(shadow) => {
                shadow.feed(&data[shadow.bytes_fed()..fed]);
                check_call(shadow, data, fed, again, &format!("{at}, shadow"));
            }
            // While the decoder's options stay refused, its checks meet that
            // refusal, so the starting options are checked alongside from
            // here on, where they are not refused too.
            None if refuses_options(&want) && !refuses_options(&oracle(&data[..fed], &options)) => {
                let mut started =
                    checked(IncrementalDecoder::new(options.clone())).expect("accepted above");
                started.feed(&data[..fed]);
                check_call(&started, data, fed, again, &format!("{at}, shadow"));
                shadow = Some(started);
            }
            None => {}
        }
    }
    // A refused change of options can hide what the checks after it test,
    // so the whole input is checked once more under the options it starts
    // with, which the decoder accepted above.
    let mut whole = checked(IncrementalDecoder::new(options)).expect("accepted above");
    whole.feed(data);
    check_call(
        &whole,
        data,
        data.len(),
        true,
        &format!("the whole {} at once", data.len()),
    );
});
