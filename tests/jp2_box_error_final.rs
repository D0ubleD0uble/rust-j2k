//! With `strict` off, an error a JP2 file's boxes give on a prefix is final
//! only where more bytes cannot replace it.
//!
//! The input ends inside a box that it cuts short, or inside a box of length
//! 0, which runs to the end of the input. More bytes can complete such a box,
//! or change what it holds, so its fault is not final. And the decode checks
//! the boxes after the codestream before the sRGB step, so a box yet to come
//! there can replace a refusal of that step. An [`IncrementalDecoder`] waits
//! on these: `refine` as [`Waiting::Held`], with the error `decode_with`
//! gives on the bytes fed, and `header` gives the header, or `None` where the
//! box is before the codestream. The shapes:
//!
//! - a whole `colr` after the codestream, on a prefix that ends inside it,
//!   which `decode_with` refuses as cut short ("box `colr` after the
//!   codestream is 15 bytes but only 8 remain");
//! - a 3-component file declared CMYK under sRGB output, which the sRGB step
//!   refuses, then a `colr` of 2 bytes after the codestream, a fault that
//!   comes first once it is in;
//! - a `jp2h` or an `ftyp` of length 0 before the codestream, whose contents
//!   are at fault on a prefix and whole on the file, which then has no
//!   `jp2c`, and each with bytes at fault at its end, so that a prefix with
//!   no `jp2c` comes before a fault of the box;
//! - a box after the signature with `LBox` 1, on a prefix that ends inside
//!   its `XLBox`: judged on its type there ("the `ftyp` box must follow the
//!   signature"), and on the file refused for its `XLBox` of 0;
//! - a `colr` of length 0 after the codestream, too short on a prefix and
//!   whole on the file;
//! - `jp2_real/byte_tlm_plt.jp2` with a `colr` after its codestream, whole
//!   and of length 0.
//!
//! Each prefix is fed to a new decoder, so each `refine` decodes. Where
//! `refine` or `header` gives an error, it is the one `decode_with` or
//! `read_header_with` gives with `strict` off on the whole file, and on the
//! prefix. `finish` is `decode_with` on the prefix under the decoder's
//! options, `strict` on and off.
//!
//! A whole box at fault stays final at once, before or after the codestream:
//! [`a_whole_box_at_fault_is_final`].

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{
    ColorOutput, DecodeOptions, Error, IncrementalDecoder, Refined, Waiting, decode_with,
    read_header_with,
};

/// `result`'s error as its `Debug` text, or `None`: [`Error`] has no
/// `PartialEq`.
fn error_of<T>(result: &Result<T, Error>) -> Option<String> {
    result.as_ref().err().map(|error| format!("{error:?}"))
}

/// A codestream of one 4 x 4 tile of `components` 8-bit components, no
/// levels, and an empty packet each.
fn codestream(components: usize) -> Vec<u8> {
    let siz = synth::siz_body([4, 4, 0, 0, 4, 4, 0, 0], &vec![[7, 1, 1]; components]);
    let mut bytes = vec![0xFF, 0x4F];
    bytes.extend(synth::seg(0xFF51, &siz));
    bytes.extend(synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0)));
    bytes.extend(synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])));
    bytes.extend(synth::tile_part(0, 0, 1, &[], &vec![0; components]));
    bytes.extend([0xFF, 0xD9]);
    bytes
}

/// A box of length 0, which runs to the end of the file.
fn open_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    [&[0, 0, 0, 0][..], kind, payload].concat()
}

fn gray() -> Vec<u8> {
    synth::bx(b"colr", &synth::colr_enum(17))
}

/// A gray JP2 file of one component, with `trailing` after the codestream.
fn gray_file(trailing: &[u8]) -> Vec<u8> {
    let codestream = codestream(1);
    let mut bytes = synth::jp2_file(
        &[synth::ihdr_for(&codestream), gray()],
        &[],
        &codestream,
        &[],
    );
    bytes.extend(trailing);
    bytes
}

fn srgb() -> DecodeOptions {
    DecodeOptions::default().with_color(ColorOutput::Srgb)
}

/// The shapes, each with the options it is decoded under.
fn shapes() -> Vec<(&'static str, Vec<u8>, DecodeOptions)> {
    let one = codestream(1);
    let ihdr = synth::ihdr_for(&one);
    let three = codestream(3);
    let cmyk = synth::jp2_file(
        &[
            synth::ihdr_for(&three),
            synth::bx(b"colr", &synth::colr_enum(12)),
        ],
        &[],
        &three,
        &[synth::bx(b"colr", &[1, 0])],
    );
    let open_jp2h = [
        &synth::JP2_SIGNATURE[..],
        &synth::ftyp_jp2(),
        &open_box(b"jp2h", &[ihdr, gray()].concat()),
    ]
    .concat();
    let open_ftyp = [
        &synth::JP2_SIGNATURE[..],
        &open_box(b"ftyp", b"jp2 \0\0\0\0jp2 "),
    ]
    .concat();
    let real = synth::read_file(synth::fixtures("jp2_real/byte_tlm_plt.jp2"));
    vec![
        (
            "a colr after the codestream",
            gray_file(&gray()),
            DecodeOptions::default(),
        ),
        ("an sRGB refusal, then a colr too short", cmyk, srgb()),
        (
            "a jp2h of length 0",
            open_jp2h.clone(),
            DecodeOptions::default(),
        ),
        (
            "an ftyp of length 0",
            open_ftyp.clone(),
            DecodeOptions::default(),
        ),
        (
            "a colr of length 0 after the codestream",
            gray_file(&open_box(b"colr", &synth::colr_enum(17))),
            DecodeOptions::default(),
        ),
        (
            "byte_tlm_plt.jp2, then a colr",
            [real.clone(), gray()].concat(),
            DecodeOptions::default(),
        ),
        (
            "byte_tlm_plt.jp2, then a colr of length 0",
            [real, open_box(b"colr", &synth::colr_enum(17))].concat(),
            DecodeOptions::default(),
        ),
        (
            "a jp2h of length 0 that ends at fault",
            [open_jp2h.clone(), vec![0; 3]].concat(),
            DecodeOptions::default(),
        ),
        (
            "an ftyp of length 0 that ends at fault",
            [open_ftyp.clone(), vec![0; 2]].concat(),
            DecodeOptions::default(),
        ),
        (
            "a box with XLBox 0 after the signature",
            [&synth::JP2_SIGNATURE[..], &[0, 0, 0, 1], &[0; 12]].concat(),
            DecodeOptions::default(),
        ),
    ]
}

/// The one-shot calls give, on some prefix, an error that is neither
/// `Incomplete` nor the whole file's: why the decoder may not take it as
/// final. Each also gives an error, with `strict` off, on a prefix that a
/// longer one decodes or reads past, or on which a longer one gives another
/// error.
#[test]
fn the_one_shot_calls_give_an_error_more_bytes_replace() {
    for (name, bytes, options) in shapes() {
        let lenient = options.with_strict(false);
        for (what, call) in [
            (
                "decode_with",
                &(|b: &[u8], o: &DecodeOptions| error_of(&decode_with(b, o)))
                    as &dyn Fn(&[u8], &DecodeOptions) -> Option<String>,
            ),
            ("read_header_with", &|b: &[u8], o: &DecodeOptions| {
                error_of(&read_header_with(b, o))
            }),
        ] {
            let whole = call(&bytes, &lenient);
            let replaced = (1..bytes.len())
                .filter_map(|len| call(&bytes[..len], &lenient))
                .find(|e| !e.starts_with("Incomplete") && Some(e) != whole.as_ref());
            assert!(replaced.is_some(), "{name}: {what}, whole {whole:?}");
        }
    }
}

/// `finish`, as `decode_with` under `options` gives it on `bytes`: a header
/// cut short is a truncation once the input has ended.
fn check_finish(decoder: &IncrementalDecoder, bytes: &[u8], options: &DecodeOptions, at: &str) {
    let finished = decoder.finish();
    match decode_with(bytes, options) {
        Err(Error::Incomplete(why)) => match finished {
            Err(Error::Codestream(m) | Error::FileFormat(m)) => {
                assert!(m.message().contains(why.message()), "{at}: {m}");
            }
            other => panic!("{at}: {other:?}, where the header is cut short"),
        },
        expected => assert_eq!(error_of(&finished), error_of(&expected), "{at}: finish"),
    }
}

/// Fed each prefix, a decoder under `strict` on and off gives an error from
/// `refine` or `header` only where it is the whole file's, waits on
/// `Held` with the one-shot error of the prefix, and finishes as
/// `decode_with` does. Each shape has a prefix where `refine` waits on an
/// error that is not the whole file's.
macro_rules! waits {
    ($($test:ident: $index:expr,)*) => {$(
        #[test]
        fn $test() {
            check_waits($index);
        }
    )*};
}

waits! {
    a_cut_colr_after_the_codestream_waits: 0,
    an_srgb_refusal_waits_for_a_box_after_the_codestream: 1,
    a_jp2h_of_length_0_waits: 2,
    an_ftyp_of_length_0_waits: 3,
    a_colr_of_length_0_after_the_codestream_waits: 4,
    a_cut_colr_after_a_real_codestream_waits: 5,
    a_colr_of_length_0_after_a_real_codestream_waits: 6,
    a_jp2h_of_length_0_that_ends_at_fault_waits: 7,
    an_ftyp_of_length_0_that_ends_at_fault_waits: 8,
    a_box_whose_xlbox_is_cut_waits: 9,
}

/// The checks of [`waits!`] on shape `index` of [`shapes`].
fn check_waits(index: usize) {
    let shapes = shapes();
    assert_eq!(shapes.len(), 10, "a shape with no test");
    let (name, bytes, options) = shapes.into_iter().nth(index).expect("a shape");
    let lenient = options.clone().with_strict(false);
    let whole = error_of(&decode_with(&bytes, &lenient));
    let whole_header = error_of(&read_header_with(&bytes, &lenient));
    for strict in [false, true] {
        let options = options.clone().with_strict(strict);
        let mut held = 0;
        for len in 1..=bytes.len() {
            let prefix = &bytes[..len];
            let at = format!("{name}, strict {strict}, {len} of {} bytes", bytes.len());
            let one_shot = error_of(&decode_with(prefix, &lenient));
            let mut decoder = IncrementalDecoder::new(options.clone()).expect("valid options");
            decoder.feed(prefix);
            match decoder.header() {
                Err(error) => {
                    let error = Some(format!("{error:?}"));
                    assert_eq!(error, whole_header, "{at}: header");
                    assert_eq!(
                        error,
                        error_of(&read_header_with(prefix, &lenient)),
                        "{at}: header"
                    );
                }
                Ok(None) => assert!(
                    error_of(&read_header_with(prefix, &lenient)).is_some(),
                    "{at}: no header where read_header_with reads one"
                ),
                Ok(Some(_)) => {}
            }
            match decoder.refine() {
                Err(error) => {
                    let error = Some(format!("{error:?}"));
                    assert_eq!(error, whole, "{at}: refine");
                    assert_eq!(error, one_shot, "{at}: refine");
                }
                Ok(Refined::Waiting(Waiting::Held { error, .. })) => {
                    let error = Some(format!("{error:?}"));
                    assert_eq!(error, one_shot, "{at}: wait");
                    held += usize::from(error != whole);
                }
                Ok(Refined::Waiting(Waiting::HeaderIncomplete { .. })) => {
                    assert!(
                        one_shot.is_some(),
                        "{at}: no header where the decode gives one"
                    );
                }
                Ok(Refined::Image(_)) => assert_eq!(one_shot, None, "{at}: image"),
                Ok(other) => panic!("{at}: {other:?}"),
            }
            check_finish(&decoder, prefix, &options, &at);
        }
        assert!(held > 0, "{name}, strict {strict}: nothing held");
    }
}

/// A box the input holds whole whose contents are at fault, before or after
/// the codestream, is final at once: `refine` returns its error and `header`
/// too, and more bytes do not change it.
#[test]
fn a_whole_box_at_fault_is_final() {
    let one = codestream(1);
    let ihdr = synth::ihdr_for(&one);
    for (name, bytes) in [
        (
            "a colr too short after the codestream",
            gray_file(&synth::bx(b"colr", &[1, 0])),
        ),
        (
            "a colr too short in the jp2h",
            synth::jp2_file(&[ihdr, synth::bx(b"colr", &[1, 0])], &[], &one, &[]),
        ),
    ] {
        let lenient = DecodeOptions::default().with_strict(false);
        let whole = error_of(&decode_with(&bytes, &lenient)).expect("an error");
        assert!(whole.starts_with("FileFormat"), "{name}: {whole}");
        let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
        decoder.feed(&bytes);
        assert_eq!(error_of(&decoder.refine()), Some(whole.clone()), "{name}");
        assert_eq!(error_of(&decoder.header()), Some(whole.clone()), "{name}");
        decoder.feed(&gray());
        for _ in 0..2 {
            assert_eq!(error_of(&decoder.refine()), Some(whole.clone()), "{name}");
        }
    }
}
