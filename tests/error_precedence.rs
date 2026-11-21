//! Which error wins when the input has more than one fault.
//!
//! - A codestream and its JP2 wrapper give the same error for the same
//!   options: the reduction is checked before the selection, the channel
//!   plans and the sRGB step on both paths, as OpenJPEG refuses it while it
//!   reads the main header.
//! - `read_header_with` gives the error `decode_with` gives first, on both
//!   pointer widths: a tile over a cap whose tile-part is cut gives the
//!   decode's error for the cut, with `strict` on.

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{
    ColorOutput, DecodeOptions, Error, LimitKind, Region, Window, WorkLimit, decode_with,
    read_header_with,
};
use synth::{bx, seg};

/// A 64 x 64 codestream of one 8-bit component in one tile, with one wavelet
/// level, so a reduction of 2 or more is refused. Its packets are empty.
fn codestream() -> Vec<u8> {
    synth::empty_tiles([64, 64, 0, 0, 64, 64, 0, 0], 1, 4, &[], &[2], 0)
}

/// `codestream` in a JP2 file with a matching `ihdr`, the `colr` box
/// `colr` and the other `jp2h` boxes `boxes`.
fn in_jp2(codestream: &[u8], colr: u32, boxes: &[Vec<u8>]) -> Vec<u8> {
    let mut header = vec![
        synth::ihdr_for(codestream),
        bx(b"colr", &synth::colr_enum(colr)),
    ];
    header.extend_from_slice(boxes);
    synth::jp2_file(&header, &[], codestream, &[])
}

/// A palette of one entry that expands the component to three 8-bit
/// channels.
fn palette() -> Vec<Vec<u8>> {
    vec![
        bx(b"pclr", &[0, 1, 3, 7, 7, 7, 10, 20, 30]),
        bx(b"cmap", &[0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 2]),
    ]
}

/// A `cdef` that names channel 5 of a one-component image: OpenJPEG finds
/// this only once it has decoded, after the reduction.
fn cdef_past_the_components() -> Vec<Vec<u8>> {
    vec![synth::cdef(&[(5, 0, 1)])]
}

/// `result`'s error as its variant, data and message: [`Error`] has no
/// `PartialEq`.
fn keyed<T>(result: Result<T, Error>) -> Result<(), String> {
    result.map(drop).map_err(|error| format!("{error:?}"))
}

/// The reduction that the one level refuses, with `options`.
fn refused_reduction(options: DecodeOptions) -> DecodeOptions {
    options.with_resolution_reduction(3)
}

fn is_reduction_refusal(error: &str) -> bool {
    error.starts_with("InvalidOptions(") && error.contains("resolution reduction 3 discards")
}

/// The options whose own refusal, the selection's, the JP2 path gave before
/// the reduction's.
fn selections() -> Vec<(&'static str, DecodeOptions)> {
    let base = DecodeOptions::default();
    vec![
        (
            "a window outside the image",
            base.clone()
                .with_region(Region::Window(Window::new(100, 100, 200, 200))),
        ),
        (
            "a tile past the grid",
            base.clone().with_region(Region::Tile(9)),
        ),
        (
            "a component past SIZ's",
            base.with_components(Some(vec![4])),
        ),
    ]
}

/// With a refused reduction and a refused selection, a codestream and its
/// plain JP2 wrapper both give the reduction's refusal, from the header and
/// from the decode.
#[test]
fn a_jp2_file_checks_the_reduction_before_the_selection() {
    let codestream = codestream();
    let jp2 = in_jp2(&codestream, 17, &[]);
    let mut mismatches = Vec::new();
    for (what, options) in selections() {
        let options = refused_reduction(options);
        let from_codestream = keyed(decode_with(&codestream, &options));
        let error = from_codestream.clone().unwrap_err();
        assert!(is_reduction_refusal(&error), "{what}: {error}");
        for (call, result) in [
            (
                "read_header_with",
                keyed(read_header_with(&codestream, &options)),
            ),
            (
                "JP2 read_header_with",
                keyed(read_header_with(&jp2, &options)),
            ),
            ("JP2 decode_with", keyed(decode_with(&jp2, &options))),
        ] {
            if result != from_codestream {
                mismatches.push(format!("{what}, {call}: {result:?}"));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// The channel plans and the sRGB step of a paletted file come after the
/// reduction too: its palette over the sample limit, its color space that
/// the sRGB step does not convert, and a `cdef` that names a channel the
/// image lacks. The codestream gives the reduction's refusal for each.
#[test]
fn a_jp2_file_checks_the_reduction_before_its_channels() {
    let codestream = codestream();
    let base = DecodeOptions::default();
    // At reduction 3 the image is 8 x 8, 64 samples: the codestream is
    // within 64, its three palette channels are not.
    let at_most_64 = base.clone().with_sample_limit(WorkLimit::AtMost(64));
    let cases = [
        // sRGB, a palette, and 64 samples at most.
        (
            "a palette over the limit",
            in_jp2(&codestream, 16, &palette()),
            at_most_64,
        ),
        // CMYK, which the sRGB step refuses on three channels.
        (
            "an sRGB step refused",
            in_jp2(&codestream, 12, &palette()),
            base.clone().with_color(ColorOutput::Srgb),
        ),
        (
            "a cdef past the components",
            in_jp2(&codestream, 17, &cdef_past_the_components()),
            base,
        ),
    ];
    let mut mismatches = Vec::new();
    for (what, jp2, options) in cases {
        // Without the refused reduction each file has its own fault: the
        // reduction is what the test moves ahead of it.
        let own = keyed(decode_with(
            &jp2,
            &options.clone().with_resolution_reduction(1),
        ));
        assert!(own.is_err(), "{what}: {own:?}");
        let options = refused_reduction(options);
        let from_codestream = keyed(decode_with(&codestream, &options));
        let error = from_codestream.clone().unwrap_err();
        assert!(is_reduction_refusal(&error), "{what}: {error}");
        for (call, result) in [
            ("read_header_with", keyed(read_header_with(&jp2, &options))),
            ("decode_with", keyed(decode_with(&jp2, &options))),
        ] {
            if result != from_codestream {
                mismatches.push(format!("{what}, {call}: {result:?}"));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A sample limit refusal comes before the reduction's on both paths, as
/// the sample limit is checked first.
#[test]
fn the_sample_limit_comes_before_the_reduction_on_both_paths() {
    let codestream = codestream();
    let jp2 = in_jp2(&codestream, 17, &[]);
    let options =
        refused_reduction(DecodeOptions::default().with_sample_limit(WorkLimit::AtMost(1)));
    let from_codestream = keyed(decode_with(&codestream, &options));
    let Err(Error::Limit(limit)) = decode_with(&codestream, &options) else {
        panic!("{from_codestream:?}");
    };
    assert_eq!(limit.kind(), LimitKind::Samples);
    for result in [
        keyed(read_header_with(&codestream, &options)),
        keyed(read_header_with(&jp2, &options)),
        keyed(decode_with(&jp2, &options)),
    ] {
        assert_eq!(result, from_codestream);
    }
}

/// The main header and the first `SOT` marker of an 8192 x 8192 image in one
/// tile, of one 8-bit component, five wavelet levels and 4 x 4 code-blocks:
/// 4,194,304 code-blocks, over the default cap of 2^20 on any target and
/// within the cap under `Unlimited` only on a 64-bit target. The `SOT`
/// segment is cut right after its marker.
fn cut_after_sot() -> Vec<u8> {
    let siz = synth::siz_body([8192, 8192, 0, 0, 8192, 8192, 0, 0], &[[7, 1, 1]]);
    let cod = synth::cod_body(1, 5, 0, 0);
    let qcd = synth::qcd_none_body(2, &[8; 16]);
    [
        vec![0xFF, 0x4F],
        seg(0xFF51, &siz),
        seg(0xFF52, &cod),
        seg(0xFF5C, &qcd),
        vec![0xFF, 0x90],
    ]
    .concat()
}

/// The same with the tile-part whole: six empty packets, then `EOC`.
fn whole_tile_part() -> Vec<u8> {
    let mut bytes = cut_after_sot();
    bytes.truncate(bytes.len() - 2);
    bytes.extend(synth::tile_part(0, 0, 1, &[], &[0; 6]));
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

fn under(limit: WorkLimit) -> DecodeOptions {
    DecodeOptions::default().with_sample_limit(limit)
}

/// The cut `SOT` is the decode's first error under either limit, with
/// `strict` on: it parses every tile-part before it counts a tile. Where
/// `read_header_with` refuses the tile, it gives that error too, not the
/// tile cap's.
#[test]
fn a_cut_tile_part_comes_before_the_tile_cap() {
    let bytes = cut_after_sot();
    let cut = Err("Codestream(\"truncated marker segment\")".to_string());
    for limit in [WorkLimit::Default, WorkLimit::Unlimited] {
        assert_eq!(
            keyed(decode_with(&bytes, &under(limit.clone()))),
            cut,
            "{limit:?}"
        );
    }
    assert_eq!(
        keyed(read_header_with(&bytes, &under(WorkLimit::Default))),
        cut
    );
    // Under `Unlimited` the tile is within the caps on a 64-bit target, so
    // the header reads nothing past the first `SOT`, as for any tile within
    // them, and gives no error. On a 32-bit target `Unlimited` is held to
    // what the target addresses, the tile is over the cap, and the header
    // gives the decode's error.
    let unlimited = keyed(read_header_with(&bytes, &under(WorkLimit::Unlimited)));
    if cfg!(target_pointer_width = "64") {
        assert_eq!(unlimited, Ok(()));
    } else {
        assert_eq!(unlimited, cut);
    }
    // With `strict` off, the decode leaves the tile out, and the header
    // agrees.
    for limit in [WorkLimit::Default, WorkLimit::Unlimited] {
        let options = under(limit.clone()).with_strict(false);
        assert_eq!(
            keyed(read_header_with(&bytes, &options)),
            Ok(()),
            "{limit:?}"
        );
        assert_eq!(keyed(decode_with(&bytes, &options)), Ok(()), "{limit:?}");
    }
}

/// With the tile-part whole, the tile cap is what both refuse the tile for:
/// on a 32-bit target as past the address space, as the sample limit that
/// would admit the tile is past what it can address.
#[test]
fn the_tile_cap_refuses_the_tile_when_its_tile_part_is_whole() {
    let bytes = whole_tile_part();
    let options = under(WorkLimit::Default);
    let header = keyed(read_header_with(&bytes, &options));
    assert_eq!(header, keyed(decode_with(&bytes, &options)));
    let Err(Error::Limit(limit)) = read_header_with(&bytes, &options) else {
        panic!("{header:?}");
    };
    let kind = if cfg!(target_pointer_width = "64") {
        LimitKind::TileCodeBlocks
    } else {
        LimitKind::AddressSpace
    };
    assert_eq!(limit.kind(), kind, "{limit}");
}
