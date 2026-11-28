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
//! [`a_whole_box_at_fault_is_final`]. So does a fault that no byte after it
//! can change, in a box of length 0 or a refusal of the sRGB step where no
//! box after the codestream can still be read: [`final_shapes`]. Once none
//! can, a fault of the tile-parts is final too, as it is in a codestream:
//! the later [`final_shapes`] and the `header_split` inputs
//! ([`the_header_split_inputs_are_final_once_the_fault_is_in`]).

#[path = "support/inconsistent.rs"]
mod inconsistent;
#[path = "support/pieces.rs"]
mod pieces;
#[path = "support/replaceable.rs"]
mod replaceable;
#[path = "support/skip.rs"]
mod skip;
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
    let mut bytes = main_header(&vec![[1, 1]; components]);
    bytes.extend(synth::tile_part(0, 0, 1, &[], &vec![0; components]));
    bytes.extend([0xFF, 0xD9]);
    bytes
}

/// The main header of [`codestream`], of a component for each `XRsiz`,
/// `YRsiz` pair of `sampling`.
fn main_header(sampling: &[[u8; 2]]) -> Vec<u8> {
    let components: Vec<[u8; 3]> = sampling.iter().map(|&[x, y]| [7, x, y]).collect();
    let siz = synth::siz_body([4, 4, 0, 0, 4, 4, 0, 0], &components);
    let mut bytes = vec![0xFF, 0x4F];
    bytes.extend(synth::seg(0xFF51, &siz));
    bytes.extend(synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0)));
    bytes.extend(synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])));
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

/// The real file of two shapes, which the published package leaves out.
const REAL: &str = "jp2_real/byte_tlm_plt.jp2";

/// `true`, and says so, for a shape built on [`REAL`] where the package leaves
/// it out; the shape then has no bytes.
#[track_caller]
fn skip_real(bytes: &[u8]) -> bool {
    bytes.is_empty() && skip::excluded_path(REAL)
}

/// `real` then `after`, or no bytes where `real` is left out of the package.
fn with_real(real: &[u8], after: &[u8]) -> Vec<u8> {
    if real.is_empty() {
        Vec::new()
    } else {
        [real, after].concat()
    }
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
    // Left out of the package: the two shapes built on it have no bytes there,
    // and the tests that use them skip them with `skip_real`.
    let real = if skip::is_excluded_path(REAL) {
        Vec::new()
    } else {
        synth::read_file(synth::fixtures(REAL))
    };
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
            with_real(&real, &gray()),
            DecodeOptions::default(),
        ),
        (
            "byte_tlm_plt.jp2, then a colr of length 0",
            with_real(&real, &open_box(b"colr", &synth::colr_enum(17))),
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
        (
            "a colr after a codestream with FF30 in its main header",
            tile_part_file(&reserved_codestream(0), &gray()),
            DecodeOptions::default(),
        ),
        (
            "a jp2h of length 0 with its ihdr last",
            [
                &synth::JP2_SIGNATURE[..],
                &synth::ftyp_jp2(),
                &open_box(b"jp2h", &[gray(), synth::ihdr_for(&one)].concat()),
            ]
            .concat(),
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
        if skip_real(&bytes) {
            continue;
        }
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
    a_cut_colr_after_a_reserved_marker_waits: 10,
    a_jp2h_of_length_0_with_its_ihdr_last_waits: 11,
}

/// The checks of [`waits!`] on shape `index` of [`shapes`].
fn check_waits(index: usize) {
    let shapes = shapes();
    assert_eq!(shapes.len(), 12, "a shape with no test");
    let (name, bytes, options) = shapes.into_iter().nth(index).expect("a shape");
    if skip_real(&bytes) {
        return;
    }
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
/// the codestream, or a box cut where no byte still to come can fault its
/// length, is final at once: `refine` returns its error and `header`
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
            "a colr too short after a codestream with FF30 in its main header",
            tile_part_file(&reserved_codestream(0), &synth::bx(b"colr", &[1, 0])),
        ),
        (
            "a colr too short in the jp2h",
            synth::jp2_file(&[ihdr.clone(), synth::bx(b"colr", &[1, 0])], &[], &one, &[]),
        ),
        // A fault a later child could lift in a `jp2h` of length 0.
        (
            "a jp2h with no ihdr",
            synth::jp2_file(&[gray()], &[], &one, &[]),
        ),
        (
            "a child that runs past the end of the jp2h",
            synth::jp2_file(
                &[ihdr.clone(), vec![0, 0, 0, 40, b'x', b'x', b'x', b'x']],
                &[],
                &one,
                &[],
            ),
        ),
        (
            "a child of length 4 in the jp2h",
            synth::jp2_file(
                &[ihdr, vec![0, 0, 0, 4, b'x', b'x', b'x', b'x']],
                &[],
                &one,
                &[],
            ),
        ),
        (
            "a box after the signature whose XLBox is past 2^56 so far",
            [&synth::JP2_SIGNATURE[..], &[0, 0, 0, 1], b"xxxx", &[1]].concat(),
        ),
        // A codestream box's `XLBox` is never at fault (#718).
        (
            "a jp2c after the signature whose XLBox is cut",
            [&synth::JP2_SIGNATURE[..], &[0, 0, 0, 1], b"jp2c", &[0]].concat(),
        ),
        (
            "a jp2c with no jp2h whose XLBox is cut",
            [
                &synth::JP2_SIGNATURE[..],
                &synth::ftyp_jp2(),
                &[0, 0, 0, 1],
                b"jp2c",
                &[0],
            ]
            .concat(),
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

/// A `pclr` payload of one 8-bit column and one entry.
const PCLR: [u8; 5] = [0, 1, 1, 7, 0];

/// A `cmap` payload that maps component 0 through palette column 0.
const CMAP: [u8; 4] = [0, 0, 1, 0];

/// A `cdef` payload that makes component 0 the gray channel.
const CDEF: [u8; 8] = [0, 1, 0, 0, 0, 0, 0, 1];

/// The 3-component file declared CMYK of [`shapes`], which the sRGB step
/// refuses, with its one tile-part's `Psot` set to `psot` where given, and
/// `trailing` after the codestream.
fn cmyk_file(psot: Option<u32>, trailing: &[u8]) -> Vec<u8> {
    let mut three = codestream(3);
    if let Some(psot) = psot {
        let sot = three
            .windows(2)
            .position(|w| w == [0xFF, 0x90])
            .expect("a SOT");
        three[sot + 6..sot + 10].copy_from_slice(&psot.to_be_bytes());
    }
    let mut bytes = synth::jp2_file(
        &[
            synth::ihdr_for(&three),
            synth::bx(b"colr", &synth::colr_enum(12)),
        ],
        &[],
        &three,
        &[],
    );
    bytes.extend(trailing);
    bytes
}

/// A shape of [`final_shapes`]: its name, its bytes, the options it is
/// decoded under, and how many bytes come after those that settle its
/// error.
type FinalShape = (&'static str, Vec<u8>, DecodeOptions, usize);

/// Boxes of length 0 after the codestream at fault for their type and the
/// boxes before them alone (a signature box, an `ftyp`, a second `pclr`,
/// `cmap` or `cdef`, a `cmap` with no `pclr`), or for a whole child of a
/// `jp2h` (a `cmap` with no `pclr`, a `colr` too short), each in a gray
/// file, with how many bytes of it settle the fault were the box read.
///
/// Each is held all the same: OpenJPEG skips a box of length 0 there that
/// runs 2^32 bytes or more, so enough bytes after it take the fault away
/// ([`a_box_of_length_0_after_the_codestream_past_2_32_bytes_is_skipped`]).
fn held_after_shapes() -> Vec<FinalShape> {
    let one = codestream(1);
    let ihdr = synth::ihdr_for(&one);
    let pclr = synth::bx(b"pclr", &PCLR);
    let cmap = synth::bx(b"cmap", &CMAP);
    let after = |boxes: &[&[u8]]| gray_file(&boxes.concat());
    let default = DecodeOptions::default;
    vec![
        (
            "a signature box of length 0 after the codestream",
            after(&[&open_box(b"jP  ", &[0x0D, 0x0A, 0x87, 0x0A])]),
            default(),
            4,
        ),
        (
            "an ftyp of length 0 after the codestream",
            after(&[&open_box(b"ftyp", b"jp2 \0\0\0\0jp2 ")]),
            default(),
            12,
        ),
        (
            "a second pclr of length 0 after the codestream",
            after(&[&pclr, &open_box(b"pclr", &PCLR)]),
            default(),
            PCLR.len(),
        ),
        (
            "a second cmap of length 0 after the codestream",
            after(&[&pclr, &cmap, &open_box(b"cmap", &CMAP)]),
            default(),
            CMAP.len(),
        ),
        (
            "a second cdef of length 0 after the codestream",
            after(&[&synth::bx(b"cdef", &CDEF), &open_box(b"cdef", &CDEF)]),
            default(),
            CDEF.len(),
        ),
        (
            "a cmap of length 0 with no pclr after the codestream",
            after(&[&open_box(b"cmap", &CMAP)]),
            default(),
            CMAP.len(),
        ),
        (
            "a jp2h of length 0 after the codestream with a cmap and no pclr",
            after(&[&open_box(b"jp2h", &[ihdr.clone(), cmap, gray()].concat())]),
            default(),
            gray().len(),
        ),
        (
            "a jp2h of length 0 after the codestream with a colr too short",
            after(&[&open_box(
                b"jp2h",
                &[ihdr, synth::bx(b"colr", &[1, 0]), gray()].concat(),
            )]),
            default(),
            gray().len(),
        ),
    ]
}

/// Each of [`held_after_shapes`], whole: `refine` waits on `Held` with the
/// error `decode_with` gives, and `header` gives the header. With more
/// bytes, it stays held with the same error.
#[test]
fn a_fault_of_a_box_of_length_0_after_the_codestream_waits() {
    for (name, bytes, options, _) in held_after_shapes() {
        let lenient = options.clone().with_strict(false);
        let whole = error_of(&decode_with(&bytes, &lenient)).expect("an error");
        assert!(whole.starts_with("FileFormat"), "{name}: {whole}");
        for strict in [false, true] {
            let mut decoder =
                IncrementalDecoder::new(options.clone().with_strict(strict)).expect("valid");
            for piece in [&bytes[..], &[0; 64]] {
                decoder.feed(piece);
                assert!(matches!(decoder.header(), Ok(Some(_))), "{name}: header");
                match decoder.refine() {
                    Ok(Refined::Waiting(Waiting::Held { error, .. })) => {
                        assert_eq!(format!("{error:?}"), whole, "{name}");
                    }
                    // Too few bytes since the last decode for another.
                    Ok(Refined::Waiting(Waiting::MoreBytes { .. })) if piece.len() == 64 => {}
                    other => panic!("{name}, strict {strict}: {other:?}"),
                }
            }
        }
    }
}

/// Why [`held_after_shapes`] wait: with 2^32 bytes or more after the
/// codestream, OpenJPEG skips a box of length 0 there unread, and the file
/// decodes. So does the `header_split` input of #722, an `ftyp` of length
/// 0 there ([`an_ftyp_of_length_0_after_the_codestream_is_held_on_every_prefix`]),
/// as OpenJPEG 2.5.0 does on it ("Cannot handle box sizes higher than
/// 2^32", then the image). The zeros are allocated lazily and only the
/// start is written, so the test needs little memory.
#[test]
#[cfg(target_pointer_width = "64")]
fn a_box_of_length_0_after_the_codestream_past_2_32_bytes_is_skipped() {
    let seed = synth::read_file(synth::fixtures(
        "incremental/header_split_ftyp_lbox0_after_codestream.seed",
    ));
    let mut shapes = held_after_shapes();
    shapes.push((
        "the header_split input of #722",
        seed[12..].to_vec(),
        DecodeOptions::default(),
        0,
    ));
    for (name, bytes, options, _) in &shapes {
        let lenient = options.clone().with_strict(false);
        assert!(decode_with(bytes, &lenient).is_err(), "{name}: whole");
        let mut padded = vec![0u8; bytes.len() + (1 << 32) + 16];
        padded[..bytes.len()].copy_from_slice(bytes);
        assert!(decode_with(&padded, &lenient).is_ok(), "{name}");
    }
}

/// Shapes whose error no byte after the first `len - after` can change:
///
/// - a `jp2h` of length 0 before the codestream whose whole `ihdr` is at
///   fault, or whose child has a length of 4 or 0 or an `XLBox` of 8:
///   settled by that child (its header, for a length), with a `colr` after
///   it;
/// - a refusal of the sRGB step where OpenJPEG reads no box after the
///   codestream: settled by the `Psot` of 0 of its last tile-part, by the
///   header of a `jp2c` the walk there stops at, or by the fourth byte of
///   an `XLBox` of 2^32 there. Each has a `colr` too short after it, which
///   would be a fault if it were read;
/// - a whole `ftyp` after the codestream, then an `xml ` of length 0:
///   settled by the `ftyp`'s last byte. The box of length 0 after it can
///   still grow, but the `ftyp` before it is whole and refused for its type
///   alone, so its fault is final.
fn final_shapes() -> Vec<FinalShape> {
    let short_colr = synth::bx(b"colr", &[1, 0]);
    let default = DecodeOptions::default;
    // After the `Psot` of the one tile-part: `TPsot`, `TNsot`, `SOD`, 3
    // bytes of packets and `EOC`.
    let past_psot = 2 + 2 + 3 + 2;
    vec![
        (
            "a jp2h of length 0 whose ihdr is at fault",
            [
                &synth::JP2_SIGNATURE[..],
                &synth::ftyp_jp2(),
                &open_box(
                    b"jp2h",
                    &[
                        synth::bx(b"ihdr", &synth::ihdr_payload(0, 4, 1, [7, 7, 0, 0])),
                        gray(),
                    ]
                    .concat(),
                ),
            ]
            .concat(),
            default(),
            gray().len(),
        ),
        (
            "an sRGB refusal of a last tile-part with Psot 0",
            cmyk_file(Some(0), &short_colr),
            srgb(),
            past_psot + short_colr.len(),
        ),
        (
            "an sRGB refusal, then a jp2c after the codestream",
            cmyk_file(
                None,
                &[synth::bx(b"jp2c", &[]), short_colr.clone()].concat(),
            ),
            srgb(),
            short_colr.len(),
        ),
        (
            "an sRGB refusal, then an XLBox of 2^32 after the codestream",
            cmyk_file(
                None,
                &[
                    &[0, 0, 0, 1][..],
                    b"xxxx",
                    &[0, 0, 0, 1, 0, 0, 0, 0],
                    &short_colr,
                ]
                .concat(),
            ),
            srgb(),
            4 + short_colr.len(),
        ),
        (
            "a jp2h of length 0 with a child of length 4",
            open_jp2h(&[0, 0, 0, 4, b'x', b'x', b'x', b'x']),
            default(),
            gray().len(),
        ),
        (
            "a jp2h of length 0 with a child of length 0",
            open_jp2h(&[0, 0, 0, 0, b'x', b'x', b'x', b'x']),
            default(),
            gray().len(),
        ),
        (
            "a jp2h of length 0 with a child whose XLBox is 8",
            open_jp2h(&[&[0, 0, 0, 1][..], b"xxxx", &8u64.to_be_bytes()].concat()),
            default(),
            gray().len(),
        ),
        (
            "a fault in a last tile-part with Psot 0",
            tile_part_file(&psot_0_fault(), &short_colr),
            default(),
            3 + short_colr.len(),
        ),
        (
            "a tile-part chain broken by 0xFE90",
            tile_part_file(&broken_chain(), &short_colr),
            default(),
            short_colr.len(),
        ),
        (
            "an sYCC file in a cut jp2c, its chain broken by 0xFF0A",
            cut_sycc_file(),
            srgb(),
            4,
        ),
        (
            "a fault in a tile-part header, then a jp2c after the codestream",
            tile_part_file(
                &coc_fault(),
                &[synth::bx(b"jp2c", &[]), short_colr.clone()].concat(),
            ),
            default(),
            short_colr.len(),
        ),
        (
            "a fault in a tile-part header, then an XLBox of 2^32",
            tile_part_file(
                &coc_fault(),
                &[
                    &[0, 0, 0, 1][..],
                    b"xxxx",
                    &[0, 0, 0, 1, 0, 0, 0, 0],
                    &short_colr,
                ]
                .concat(),
            ),
            default(),
            4 + short_colr.len(),
        ),
        (
            "a whole ftyp after the codestream, then an xml of length 0",
            gray_file(&[synth::ftyp_jp2(), open_box(b"xml ", b"<a/>")].concat()),
            default(),
            12,
        ),
    ]
}

/// A gray JP2 file of `codestream`, with `trailing` after its `jp2c`.
fn tile_part_file(codestream: &[u8], trailing: &[u8]) -> Vec<u8> {
    let ihdr = synth::ihdr_for(codestream);
    let mut bytes = synth::jp2_file(&[ihdr, gray()], &[], codestream, &[]);
    bytes.extend(trailing);
    bytes
}

/// A fault in a tile-part header, then a whole `colr` with an explicit
/// length that passes: `refine` holds the fault, since a box after the
/// `colr` can still be at fault. With a `colr` too short after it,
/// `decode_with` gives that box's error instead.
#[test]
fn a_fault_before_a_whole_colr_that_passes_is_held() {
    let colr = synth::bx(b"colr", &synth::colr_enum(17));
    let bytes = tile_part_file(&coc_fault(), &colr);
    let whole = error_of(&decode_with(&bytes, &DecodeOptions::default())).expect("an error");
    assert!(whole.starts_with("Marker"), "{whole}");
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).expect("valid");
    decoder.feed(&bytes);
    match decoder.refine() {
        Ok(Refined::Waiting(Waiting::Held { error, .. })) => {
            assert_eq!(format!("{error:?}"), whole);
        }
        other => panic!("{other:?}"),
    }
    let longer = [bytes, synth::bx(b"colr", &[1, 0])].concat();
    let error = error_of(&decode_with(&longer, &DecodeOptions::default())).expect("an error");
    assert!(
        error.starts_with("FileFormat") && error.contains("a `colr` box holds 2 bytes"),
        "{error}"
    );
}

/// [`codestream`] of one component with the reserved marker `FF 30`, which
/// has no segment, at the end of its main header, then a `COM` of `comment`
/// zero bytes when `comment` is not 0.
fn reserved_codestream(comment: usize) -> Vec<u8> {
    let mut bytes = main_header(&[[1, 1]]);
    bytes.extend([0xFF, 0x30]);
    if comment > 0 {
        bytes.extend(synth::seg(0xFF64, &vec![0; comment]));
    }
    bytes.extend(synth::tile_part(0, 0, 1, &[], &[0]));
    bytes.extend([0xFF, 0xD9]);
    bytes
}

/// `FF 30` in the main header has no segment, so the walk to the end of the
/// codestream steps over its two bytes alone, as the codestream parser
/// does, and reads the boxes after the codestream. Followed by `SOT`, a
/// walk that read a length there would step past the end of the file; by a
/// `COM` of 65531 bytes, it would land inside the comment. Either way the
/// boxes after the codestream went unread. OpenJPEG 2.5 refuses both files
/// with a `colr` too short after the codestream ("Bad COLR header box (bad
/// size)"), with and without `-allow-partial`, and decodes them without it.
#[test]
fn a_reserved_marker_in_the_main_header_leaves_the_boxes_after_the_codestream_read() {
    let short_colr = synth::bx(b"colr", &[1, 0]);
    let plain = codestream(1);
    for comment in [0, 65531] {
        let reserved = reserved_codestream(comment);
        for strict in [false, true] {
            let options = DecodeOptions::default().with_strict(strict);
            let at = format!("a COM of {comment} bytes, strict {strict}");
            let refused = decode_with(&tile_part_file(&reserved, &short_colr), &options);
            assert!(
                matches!(refused, Err(Error::FileFormat(_))),
                "{at}: {refused:?}"
            );
            assert_eq!(
                error_of(&refused),
                error_of(&decode_with(&tile_part_file(&plain, &short_colr), &options)),
                "{at}"
            );
            let image = decode_with(&tile_part_file(&reserved, &[]), &options)
                .unwrap_or_else(|error| panic!("{at}: {error:?}"));
            let expected = decode_with(&tile_part_file(&plain, &[]), &options).expect("decodes");
            assert_eq!(image.components, expected.components, "{at}");
        }
    }
}

/// A gray codestream whose one tile-part has `Psot = 0` and `00 00` where
/// its header needs a marker, as in the `header_split` input
/// `hs_crash3.bin` of [`HS_CRASH3`]. Then `SOD` and a byte.
fn psot_0_fault() -> Vec<u8> {
    let mut bytes = main_header(&[[1, 1]]);
    bytes.extend(synth::sot(0, 0, 0, 1));
    bytes.extend([0, 0, 0xFF, 0x93, 0]);
    bytes
}

/// [`codestream`] of one component with `FE 90` in place of its `EOC`, as
/// in `hs_crash4_fe90.bin` ([`HS_CRASH4`]).
fn broken_chain() -> Vec<u8> {
    let mut bytes = codestream(1);
    bytes.truncate(bytes.len() - 2);
    bytes.extend([0xFE, 0x90]);
    bytes
}

/// A gray codestream whose one tile-part's header has a `COC` for
/// component 5, which the image does not have, then an `EOC`.
fn coc_fault() -> Vec<u8> {
    let mut bytes = main_header(&[[1, 1]]);
    let coc = synth::seg(0xFF53, &[5, 0, 0, 0, 0, 0, 0]);
    bytes.extend(synth::tile_part(0, 0, 1, &coc, &[0]));
    bytes.extend([0xFF, 0xD9]);
    bytes
}

/// As `hs_crash5_sycc.bin` ([`HS_CRASH5`]): an sYCC file of 4:2:2 chroma
/// whose one tile-part is followed by `FF 0A` and 4 more bytes, in a `jp2c`
/// that claims 16 bytes more than the file holds.
fn cut_sycc_file() -> Vec<u8> {
    let mut codestream = main_header(&[[1, 1], [2, 1], [2, 1]]);
    codestream.extend(synth::tile_part(0, 0, 1, &[], &[0, 0, 0]));
    codestream.extend([0xFF, 0x0A, 0x20, 0x00, 0x01, 0x00]);
    let mut bytes = synth::jp2_file(
        &[
            synth::ihdr_for(&codestream),
            synth::bx(b"colr", &synth::colr_enum(18)),
        ],
        &[],
        &[],
        &[],
    );
    // The `jp2c` box `jp2_file` closed is empty: claim the codestream and
    // 16 bytes more.
    bytes.truncate(bytes.len() - 8);
    let length = u32::try_from(8 + codestream.len() + 16).unwrap();
    bytes.extend(length.to_be_bytes());
    bytes.extend(b"jp2c");
    bytes.extend(codestream);
    bytes
}

/// A JP2 file whose `jp2h` has length 0 and holds a matching `ihdr`,
/// `child`, then a `colr`.
fn open_jp2h(child: &[u8]) -> Vec<u8> {
    let ihdr = synth::ihdr_for(&codestream(1));
    [
        &synth::JP2_SIGNATURE[..],
        &synth::ftyp_jp2(),
        &open_box(b"jp2h", &[&ihdr[..], child, &gray()].concat()),
    ]
    .concat()
}

/// Bytes appended to a file of [`final_shapes`]: none changes its error.
/// Boxes of each kind the walk after the codestream reads, at fault or
/// not, of length 0 and with an `XLBox`, and a second `jp2c`.
fn suffixes() -> Vec<Vec<u8>> {
    vec![
        vec![0],
        vec![0; 64],
        b"jp2 ".to_vec(),
        gray(),
        synth::bx(b"colr", &[1, 0]),
        synth::bx(b"pclr", &PCLR),
        synth::bx(b"cmap", &CMAP),
        synth::bx(b"cdef", &CDEF),
        synth::bx(b"bpcc", &[7, 7]),
        synth::bx(b"jp2h", &[]),
        synth::bx(b"xml ", b"<a/>"),
        synth::JP2_SIGNATURE.to_vec(),
        synth::ftyp_jp2(),
        open_box(b"colr", &[1, 0]),
        open_box(b"jp2h", &[]),
        [&[0, 0, 0, 1][..], b"colr", &18u64.to_be_bytes(), &[1, 0]].concat(),
        synth::bx(b"jp2c", &codestream(1)),
        [&[0xFF, 0xD9][..], &synth::bx(b"colr", &[1, 0])].concat(),
    ]
}

/// Fed each prefix, a decoder under `strict` on and off gives the error of
/// a shape of [`final_shapes`] at once: from the prefix that settles it on,
/// `refine` returns the error `decode_with` gives on the whole file, and
/// `header` the one `read_header_with` gives on it. Before, neither gives
/// an error, and `refine` waits on `Held` with the error of the prefix, or
/// gives what `decode_with` gives on it. The whole file and the file with
/// any of [`suffixes`] appended give the same errors, and `finish` is
/// `decode_with` on the prefix.
macro_rules! final_at_once {
    ($($test:ident: $index:expr,)*) => {$(
        #[test]
        fn $test() {
            check_final($index);
        }
    )*};
}

final_at_once! {
    a_whole_ihdr_at_fault_in_a_jp2h_of_length_0_is_final: 0,
    an_srgb_refusal_with_psot_0_is_final: 1,
    an_srgb_refusal_before_a_second_jp2c_is_final: 2,
    an_srgb_refusal_before_an_xlbox_of_2_32_is_final: 3,
    a_child_of_length_4_in_a_jp2h_of_length_0_is_final: 4,
    a_child_of_length_0_in_a_jp2h_of_length_0_is_final: 5,
    a_child_with_xlbox_8_in_a_jp2h_of_length_0_is_final: 6,
    a_fault_in_a_last_tile_part_with_psot_0_is_final: 7,
    a_tile_part_chain_broken_on_bytes_in_is_final: 8,
    a_chain_broken_in_a_cut_jp2c_is_final: 9,
    a_tile_part_fault_before_a_second_jp2c_is_final: 10,
    a_tile_part_fault_before_an_xlbox_of_2_32_is_final: 11,
    a_whole_ftyp_before_a_box_of_length_0_after_the_codestream_is_final: 12,
}

/// The checks of [`final_at_once!`] on shape `index` of [`final_shapes`].
fn check_final(index: usize) {
    let shapes = final_shapes();
    assert_eq!(shapes.len(), 13, "a shape with no test");
    check_final_shape(shapes.into_iter().nth(index).expect("a shape"));
}

/// The checks of [`final_at_once!`] on one shape.
fn check_final_shape((name, bytes, options, after): FinalShape) {
    let settled = bytes.len() - after;
    let lenient = options.clone().with_strict(false);
    let whole = error_of(&decode_with(&bytes, &lenient)).expect("an error");
    assert!(!whole.starts_with("Incomplete"), "{name}: {whole}");
    // The header read meets a fault of the tile-parts only where it walks
    // them, so it may read the header of a file the decode refuses.
    let whole_header = error_of(&read_header_with(&bytes, &lenient));
    for suffix in suffixes() {
        let longer = [&bytes[..], &suffix].concat();
        let at = format!("{name}, then {suffix:?}");
        assert_eq!(
            error_of(&decode_with(&longer, &lenient)),
            Some(whole.clone()),
            "{at}"
        );
        if whole_header.is_some() {
            assert_eq!(
                error_of(&read_header_with(&longer, &lenient)),
                whole_header,
                "{at}"
            );
        }
    }
    for strict in [false, true] {
        let options = options.clone().with_strict(strict);
        for len in 1..=bytes.len() {
            let prefix = &bytes[..len];
            let at = format!("{name}, strict {strict}, {len} of {} bytes", bytes.len());
            let one_shot = error_of(&decode_with(prefix, &lenient));
            let mut decoder = IncrementalDecoder::new(options.clone()).expect("valid options");
            decoder.feed(prefix);
            let header = decoder.header();
            let refined = decoder.refine();
            if len >= settled {
                assert_eq!(error_of(&header), whole_header, "{at}: header");
                assert!(!matches!(header, Ok(None)), "{at}: header");
                assert_eq!(error_of(&refined), Some(whole.clone()), "{at}: refine");
                assert_eq!(one_shot, Some(whole.clone()), "{at}: decode_with");
            } else {
                assert!(header.is_ok(), "{at}: header {header:?}");
                match refined {
                    Ok(Refined::Waiting(Waiting::Held { error, .. })) => {
                        assert_eq!(Some(format!("{error:?}")), one_shot, "{at}: wait");
                    }
                    Ok(Refined::Waiting(Waiting::HeaderIncomplete { .. })) => {
                        assert!(one_shot.is_some(), "{at}: no header");
                    }
                    Ok(Refined::Image(_)) => assert_eq!(one_shot, None, "{at}: image"),
                    other => panic!("{at}: {other:?}"),
                }
            }
            check_finish(&decoder, prefix, &options, &at);
        }
    }
}

/// An `ftyp` of length 0 with a brand that is not readable, and no
/// compatibility code that is, stays held: the bytes after it are more
/// compatibility codes. A byte more leaves the box out of whole codes, a
/// `jp2 ` makes the file readable, and each gives another error.
#[test]
fn an_unsupported_brand_in_an_ftyp_of_length_0_waits() {
    let bytes = [
        &synth::JP2_SIGNATURE[..],
        &open_box(b"ftyp", b"jpx \0\0\0\0jpx "),
    ]
    .concat();
    let lenient = DecodeOptions::default().with_strict(false);
    let whole = error_of(&decode_with(&bytes, &lenient)).expect("an error");
    assert!(whole.starts_with("Unsupported"), "{whole}");
    for suffix in [&[0u8][..], b"jp2 "] {
        let longer = error_of(&decode_with(&[&bytes[..], suffix].concat(), &lenient));
        assert!(
            longer.as_ref().is_some_and(|e| *e != whole),
            "{suffix:?}: {longer:?}"
        );
    }
    let mut decoder = IncrementalDecoder::new(lenient).expect("valid options");
    decoder.feed(&bytes);
    assert!(matches!(decoder.header(), Ok(None)));
    match decoder.refine() {
        Ok(Refined::Waiting(Waiting::Held { error, .. })) => {
            assert_eq!(format!("{error:?}"), whole);
        }
        other => panic!("{other:?}"),
    }
}

/// `hs_crash3.bin`, found by the `header_split` fuzz target, less its 12
/// control bytes, which are zero but for the seed of the pieces and leave
/// the options at their defaults. A JP2 file of 8400 x 4608 sYCC whose
/// `jp2c` is cut short, with a last tile-part of `Psot = 0` and `00 00`
/// where its header needs a marker.
const HS_CRASH3: &str = "\
    0000000c6a5020200d0a870a00000014667479706a703220000000006a703220\
    0000002d6a703268000000166968647200001200000020d00003070700000000\
    000f636f6c7201000000000012000000766a703263ff4fff51002f0000000020\
    d0000020000000000000000e00000020d0000010000000000000000000000307\
    0101070201070201ff52000c00000001000000000001ff5c000640404040ff90\
    000a0000000000000000000000120001ff93000000010200020000";

/// `hs_crash4_fe90.bin`, the same way: a tile-part followed by `FE 90`.
const HS_CRASH4: &str = "\
    0000000c6a5020200d0a870a00000014667479706a703220000000006a703220\
    0000002d6a703268000000166968647200001200000020d00003070700000000\
    000f636f6c7201000000000012000000766a703263ff4fff51002f0000000020\
    d0000020000000000000000e00000020d000fe10000000000000000000000307\
    0101070201070201ff52000c00000001000000000001ff5c000640404040ff90\
    000a0000000000120001ff930000fffffe90200a0001000000110001ff930000\
    00ffd9";

/// `hs_crash5_sycc.bin`, the same way: a tile-part followed by `FF 0A`.
const HS_CRASH5: &str = "\
    0000000c6a5020200d0a870a00000014667479706a703220000000006a703220\
    0000002d6a703268000000166968647200001200000020d00003070700000000\
    000f636f6c7201000000000012000000766a703263ff4fff51002f0000000020\
    d0000020000000000000000e00000020d000fe10000000000000000000000307\
    0101070201070201ff52000c00000001000000000001ff5c000640404040ff90\
    000a0000000000120001ff9300000000ff0a90200001000000110001ff930000\
    00ffd9";

fn from_hex(hex: &str) -> Vec<u8> {
    let digits: Vec<u8> = hex.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

/// The `header_split` inputs of the issue, each with the bytes that end at
/// its fault and the fault: no box after the codestream is read, as the
/// last tile-part has `Psot = 0` or the tile-part chain is broken on bytes
/// in, so the fault is final once it is in. Before the fix `header` gave
/// the header there, where `read_header_with` refuses.
///
/// Each prefix is fed to a new decoder. Before the fault `header` gives
/// no error. From it on `header` gives the error `read_header_with` gives
/// on the whole file, and `refine` the one `decode_with` gives, the same
/// as on the prefix, on the `jp2c` filled to its length and on any of
/// [`suffixes`] appended to either. `finish` is `decode_with` on the
/// prefix, `strict` on and off. The image is 8400 x 4608, so the calls
/// that would decode tiles are made only from the fault on, where the
/// parse of the tile-parts refuses first.
#[test]
fn the_header_split_inputs_are_final_once_the_fault_is_in() {
    // Each with the bytes that end at its fault, and what follows them.
    for (name, hex, at_fault, message) in [
        (
            "hs_crash3.bin",
            HS_CRASH3,
            &[0xFF, 0x90, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0][..],
            "found 0x0000",
        ),
        (
            "hs_crash4_fe90.bin",
            HS_CRASH4,
            &[0xFE, 0x90],
            "found 0xFE90",
        ),
        (
            "hs_crash5_sycc.bin",
            HS_CRASH5,
            &[0xFF, 0x0A],
            "found 0xFF0A",
        ),
    ] {
        let bytes = from_hex(hex);
        let fault = bytes
            .windows(at_fault.len())
            .position(|window| window == at_fault)
            .expect("the fault")
            + at_fault.len();
        let lenient = DecodeOptions::default().with_strict(false);
        let whole = error_of(&decode_with(&bytes, &lenient)).expect("an error");
        assert!(whole.contains(message), "{name}: {whole}");
        let whole_header = error_of(&read_header_with(&bytes, &lenient));
        assert_eq!(whole_header.as_ref(), Some(&whole), "{name}");
        // The `jp2c` box at 77 claims 118 bytes: all but 8 of `hs_crash3`'s
        // are in, and all of each other's.
        let filled = {
            let mut filled = bytes.clone();
            assert_eq!(&filled[77..85], &[0, 0, 0, 118, b'j', b'p', b'2', b'c']);
            assert!(filled.len() <= 77 + 118, "{name}");
            filled.resize(77 + 118, 0);
            filled
        };
        for longer in
            std::iter::once(filled.clone()).chain(suffixes().into_iter().flat_map(|suffix| {
                [
                    [&bytes[..], &suffix].concat(),
                    [&filled[..], &suffix].concat(),
                ]
            }))
        {
            let at = format!("{name}, {} bytes", longer.len());
            assert_eq!(
                error_of(&decode_with(&longer, &lenient)),
                Some(whole.clone()),
                "{at}"
            );
            assert_eq!(
                error_of(&read_header_with(&longer, &lenient)),
                Some(whole.clone()),
                "{at}"
            );
        }
        for strict in [false, true] {
            let options = DecodeOptions::default().with_strict(strict);
            for len in 1..=bytes.len() {
                let prefix = &bytes[..len];
                let at = format!("{name}, strict {strict}, {len} of {} bytes", bytes.len());
                let mut decoder = IncrementalDecoder::new(options.clone()).expect("valid options");
                decoder.feed(prefix);
                let header = decoder.header();
                if len < fault {
                    assert!(header.is_ok(), "{at}: header {header:?}");
                    continue;
                }
                assert_eq!(error_of(&header), Some(whole.clone()), "{at}: header");
                assert_eq!(
                    error_of(&read_header_with(prefix, &lenient)),
                    Some(whole.clone()),
                    "{at}: read_header_with"
                );
                assert_eq!(
                    error_of(&decode_with(prefix, &lenient)),
                    Some(whole.clone()),
                    "{at}: decode_with"
                );
                assert_eq!(
                    error_of(&decoder.refine()),
                    Some(whole.clone()),
                    "{at}: refine"
                );
                check_finish(&decoder, prefix, &options, &at);
            }
        }
    }
}

/// `incremental/header_split_jp2c_after_signature_cut_xlbox.seed`, the
/// `header_split` input of #718: a `jp2c` box with `LBox` 1 where the
/// `ftyp` belongs, cut inside its `XLBox`. Its 12 control bytes pick
/// options that do not bear on the boxes, so the file after them is read
/// under the defaults. A codestream box's `XLBox` is never at fault, so
/// once its type is in, "the `ftyp` box must follow the signature" is
/// final: before the fix `header` gave `None` on the prefixes whose `XLBox`
/// was all 0 so far, where `read_header_with` refuses.
///
/// Each prefix is fed to a new decoder, and fed a byte at a time to one
/// decoder: `header` gives what `read_header_with` gives on it, an
/// incomplete read as `None`, and from the type on that error, the one the
/// file gives with its `XLBox` whole or with more bytes after it.
#[test]
fn a_jp2c_after_the_signature_cut_in_its_xlbox_is_final() {
    let seed = synth::read_file(synth::fixtures(
        "incremental/header_split_jp2c_after_signature_cut_xlbox.seed",
    ));
    let bytes = &seed[12..];
    assert_eq!(&bytes[..12], &synth::JP2_SIGNATURE[..]);
    assert_eq!(&bytes[12..20], b"\0\0\0\x01jp2c");
    let lenient = DecodeOptions::default().with_strict(false);
    let read = |prefix: &[u8]| match read_header_with(prefix, &lenient) {
        Ok(header) => format!("{:?}", Ok::<_, Error>(Some(header))),
        Err(Error::Incomplete(_)) => "Ok(None)".to_string(),
        Err(error) => format!("{:?}", Err::<Option<()>, _>(error)),
    };
    let whole = read(bytes);
    assert!(
        whole.contains("must follow the signature, not `jp2c`"),
        "{whole}"
    );
    for longer in [
        [bytes, &[0; 3]].concat(),
        [bytes, &[0; 3], &gray()].concat(),
        [&bytes[..20], &[0; 8]].concat(),
    ] {
        assert_eq!(read(&longer), whole, "{} bytes", longer.len());
    }
    for strict in [false, true] {
        let options = DecodeOptions::default().with_strict(strict);
        let mut fed = IncrementalDecoder::new(options.clone()).expect("valid options");
        for len in 1..=bytes.len() {
            let prefix = &bytes[..len];
            let at = format!("strict {strict}, {len} bytes");
            let mut decoder = IncrementalDecoder::new(options.clone()).expect("valid options");
            decoder.feed(prefix);
            let expected = read(prefix);
            if len >= 20 {
                assert_eq!(expected, whole, "{at}");
            }
            assert_eq!(format!("{:?}", decoder.header()), expected, "{at}");
            fed.feed(&prefix[len - 1..]);
            assert_eq!(format!("{:?}", fed.header()), expected, "{at}: fed");
        }
    }
}

/// `incremental/header_split_ftyp_lbox0_after_codestream.seed`, the
/// `header_split` input of #722: a gray JP2 file whose codestream ends at
/// its `EOC`, then an `ftyp` box of length 0, which runs to the end of the
/// file. Its 12 control bytes pick options that do not bear on the boxes,
/// so the file after them is read under the defaults.
///
/// `read_header_with` refuses the file for the `ftyp` alone, as OpenJPEG
/// does ("The ftyp box must be the second box in the file"). The fault is
/// held all the same: OpenJPEG skips a box of length 0 after the
/// codestream unread once 2^32 bytes or more follow it, and the file then
/// decodes ([`a_box_of_length_0_after_the_codestream_past_2_32_bytes_is_skipped`]).
/// The `header_split` target built no suffix that long, so it took the
/// held fault for a wrong answer.
///
/// Each prefix is fed to a new decoder, and fed a byte at a time to one
/// decoder, with `strict` on and off. Before the `ftyp` box's header is
/// whole, `header` gives what `read_header_with` gives. From there it gives
/// the header of the file without the `ftyp` box, and a new decoder's
/// `refine` waits on `Held` with the error `decode_with` gives, the one
/// `read_header_with` gives. Every error `refine` gives or waits on is the
/// one `decode_with` gives on the prefix.
#[test]
fn an_ftyp_of_length_0_after_the_codestream_is_held_on_every_prefix() {
    let seed = synth::read_file(synth::fixtures(
        "incremental/header_split_ftyp_lbox0_after_codestream.seed",
    ));
    let bytes = &seed[12..];
    assert_eq!(&bytes[..12], &synth::JP2_SIGNATURE[..]);
    // Where the `ftyp` box after the codestream starts, and where its
    // header ends.
    let start = bytes.len() - 14;
    assert_eq!(&bytes[start - 2..start + 8], b"\xFF\xD9\0\0\0\0ftyp");
    let open = start + 8;
    let lenient = DecodeOptions::default().with_strict(false);
    let read = |prefix: &[u8]| match read_header_with(prefix, &lenient) {
        Ok(header) => format!("{:?}", Ok::<_, Error>(Some(header))),
        Err(Error::Incomplete(_)) => "Ok(None)".to_string(),
        Err(error) => format!("{:?}", Err::<Option<()>, _>(error)),
    };
    let whole = read(bytes);
    assert!(
        whole.contains("the file has an `ftyp` box, which must be the second box"),
        "{whole}"
    );
    let without = read(&bytes[..start]);
    assert!(without.starts_with("Ok(Some("), "{without}");
    let check_refine = |refined: Result<Refined, Error>, prefix: &[u8], at: &str| {
        let one_shot = error_of(&decode_with(prefix, &lenient));
        match refined {
            Err(error) => {
                assert_eq!(Some(format!("{error:?}")), one_shot, "{at}: refine");
                None
            }
            Ok(Refined::Waiting(Waiting::Held { error, .. })) => {
                assert_eq!(Some(format!("{error:?}")), one_shot, "{at}: wait");
                Some(())
            }
            Ok(Refined::Image(_)) => {
                assert_eq!(one_shot, None, "{at}: image");
                None
            }
            Ok(_) => None,
        }
    };
    for strict in [false, true] {
        let options = DecodeOptions::default().with_strict(strict);
        let mut fed = IncrementalDecoder::new(options.clone()).expect("valid options");
        for len in 1..=bytes.len() {
            let prefix = &bytes[..len];
            let at = format!("strict {strict}, {len} of {} bytes", bytes.len());
            let expected = read(prefix);
            let mut decoder = IncrementalDecoder::new(options.clone()).expect("valid options");
            decoder.feed(prefix);
            let header = format!("{:?}", decoder.header());
            let refined = check_refine(decoder.refine(), prefix, &at);
            if len >= open {
                assert_eq!(expected, whole, "{at}");
                assert_eq!(header, without, "{at}: header");
                assert_eq!(
                    refined,
                    Some(()),
                    "{at}: no wait on the error read_header_with gives"
                );
            } else {
                assert_eq!(header, expected, "{at}: header");
            }
            fed.feed(&prefix[len - 1..]);
            let fed_header = format!("{:?}", fed.header());
            if len >= open {
                assert_eq!(fed_header, without, "{at}: fed");
            } else {
                assert_eq!(fed_header, expected, "{at}: fed");
            }
            check_refine(fed.refine(), prefix, &format!("{at}: fed"));
        }
    }
}

/// A refusal of the sRGB step that more bytes lift, held back from the
/// boxes while a box after the codestream can still be read: a `colr` too
/// short there comes first, so `header` gives the header with the channels
/// as coded and `refine` waits, until no box there can be read. The image
/// is 20000 x 10000 of 4:2:2 sYCC: its 4 * 10^8 samples are under the
/// default limit, the 6 * 10^8 of the sRGB output over it.
#[test]
fn an_srgb_refusal_more_bytes_lift_waits_for_a_box_after_the_codestream() {
    let siz = synth::siz_body(
        [20000, 10000, 0, 0, 20000, 10000, 0, 0],
        &[[7, 1, 1], [7, 2, 1], [7, 2, 1]],
    );
    let mut codestream = vec![0xFF, 0x4F];
    codestream.extend(synth::seg(0xFF51, &siz));
    codestream.extend(synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0)));
    codestream.extend(synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])));
    codestream.extend(synth::tile_part(0, 0, 1, &[], &[]));
    codestream.extend([0xFF, 0xD9]);
    let file = |trailing: &[Vec<u8>]| {
        synth::jp2_file(
            &[
                synth::ihdr_for(&codestream),
                synth::bx(b"colr", &synth::colr_enum(18)),
            ],
            &[],
            &codestream,
            trailing,
        )
    };
    let lenient = srgb().with_strict(false);
    let short_colr = synth::bx(b"colr", &[1, 0]);
    let bytes = file(&[]);
    let refused = error_of(&read_header_with(&bytes, &lenient)).expect("an error");
    assert!(refused.contains("sRGB output"), "{refused}");
    assert!(refused.contains("Raised(2)"), "{refused}");
    let longer = file(std::slice::from_ref(&short_colr));
    let whole = error_of(&read_header_with(&longer, &lenient)).expect("an error");
    assert!(whole.starts_with("FileFormat"), "{whole}");
    assert_eq!(error_of(&decode_with(&longer, &lenient)), Some(whole));
    let as_coded = read_header_with(&bytes, &lenient.clone().with_color(ColorOutput::AsCoded));
    let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
    decoder.feed(&bytes);
    assert_eq!(
        format!("{:?}", decoder.header()),
        format!("{:?}", as_coded.map(Some))
    );
    assert!(matches!(
        decoder.refine(),
        Ok(Refined::Waiting(Waiting::SampleLimit { .. }))
    ));
    // Once a `jp2c` after the codestream stops the walk there, the refusal
    // is the decode's, and `header` gives it.
    let settled = file(&[synth::bx(b"jp2c", &[]), short_colr]);
    let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
    decoder.feed(&settled);
    assert_eq!(error_of(&decoder.header()), Some(refused.clone()));
    assert_eq!(
        error_of(&read_header_with(&settled, &lenient)),
        Some(refused)
    );
}

/// The `header_split` target's [`replaceable::replaceable`] on its
/// committed inputs. It finds a suffix that changes the error of the #722
/// input, an `ftyp` of length 0 after the codestream, through the 2^32
/// bytes past which OpenJPEG skips that box. It finds none for a whole
/// `ftyp` after the codestream followed by an `xml ` of length 0, nor for a
/// `jp2h` of length 0 before the codestream with a child of length 4: both
/// faults are final. A harness that stood in for more than the box of
/// length 0 after the codestream would pass a decoder that held them.
///
/// It finds one for each of [`held_after_shapes`] too, whose faults
/// `header` holds: a harness that stood in for less, such as one that
/// skipped a `jp2h` of length 0 there, would fail a decoder that is right.
#[test]
fn header_split_finds_a_suffix_only_where_one_changes_the_error() {
    for (name, bytes, options, _) in held_after_shapes() {
        let lenient = options.clone().with_strict(false);
        assert!(read_header_with(&bytes, &lenient).is_err(), "{name}: read");
        assert!(replaceable::replaceable(&bytes, &options), "{name}");
    }
    for (name, expected) in [
        ("header_split_ftyp_lbox0_after_codestream", true),
        ("header_split_ftyp_then_xml_lbox0_after_codestream", false),
        ("header_split_jp2h_lbox0_child_of_length_4", false),
        ("header_split_ftyp_then_xml_lbox0_fixture_prefix", false),
        ("header_split_ftyp_cut_after_refused_reduction", false),
    ] {
        let seed = synth::read_file(synth::fixtures(&format!("incremental/{name}.seed")));
        // All-zero option bytes but the piece sizes' seed: the defaults.
        assert_eq!(&seed[..6], &[0; 6], "{name}");
        let bytes = &seed[12..];
        assert!(
            read_header_with(bytes, &DecodeOptions::default()).is_err(),
            "{name}: read"
        );
        assert_eq!(
            replaceable::replaceable(bytes, &DecodeOptions::default()),
            expected,
            "{name}"
        );
    }
}

/// The two `header_split` seeds of #726 pin what their piece streams do in
/// the target, whose stream ([`pieces::Pieces`]) this includes, so a
/// change to it does not silently stop them testing what they were made
/// for. `header_split_ftyp_then_xml_lbox0_fixture_prefix` changes the
/// reduction to 2, which its one resolution refuses, at its last feed (199
/// of 199), and not before. `header_split_ftyp_cut_after_refused_reduction`
/// changes it to 2 at 97 bytes, before the main header is whole, changes
/// nothing after, and later ends a feed inside the `ftyp` after the
/// codestream, which takes bytes 167 to 186 of the file: at 168 to 186.
#[test]
fn the_header_split_seeds_of_726_keep_their_piece_streams() {
    let feeds = |name: &str| {
        let seed = synth::read_file(synth::fixtures(&format!("incremental/{name}.seed")));
        pieces::Pieces::new(&seed, seed.len() - 12).collect::<Vec<_>>()
    };
    let changes = |feeds: &[pieces::Feed]| {
        feeds
            .iter()
            .filter_map(|feed| feed.change.map(|change| (feed.to, change)))
            .collect::<Vec<_>>()
    };
    let reduction = pieces::Change::Reduction;

    let last = feeds("header_split_ftyp_then_xml_lbox0_fixture_prefix");
    assert_eq!(last.last().map(|feed| feed.to), Some(199));
    assert_eq!(changes(&last).last(), Some(&(199, reduction(2))));
    assert!(
        changes(&last)
            .iter()
            .all(|&(to, change)| !matches!(change, pieces::Change::Reduction(_)) || to == 199)
    );

    let early = feeds("header_split_ftyp_cut_after_refused_reduction");
    assert_eq!(changes(&early), [(97, reduction(2))]);
    assert!(early.iter().any(|feed| (168..187).contains(&feed.to)));
}

/// Boxes of length 0 after the codestream of a type the decode does not
/// check there, each after an error that comes before them: an sRGB
/// refusal, or a fault in a tile-part header. No box can follow such a box,
/// and its contents never fault, so the error is final once its type is in
/// (#725). Each comes with how many bytes of it settle the error: those
/// after the box's header.
fn unread_after_shapes() -> Vec<FinalShape> {
    let xml = open_box(b"xml ", b"<a/>");
    vec![
        (
            "an sRGB refusal, then an xml of length 0 after the codestream",
            cmyk_file(None, &xml),
            srgb(),
            4,
        ),
        (
            "a fault in a tile-part header, then an xml of length 0",
            tile_part_file(&coc_fault(), &xml),
            DecodeOptions::default(),
            4,
        ),
    ]
}

/// [`final_at_once!`] on [`unread_after_shapes`].
macro_rules! final_at_once_unread {
    ($($test:ident: $index:expr,)*) => {$(
        #[test]
        fn $test() {
            let shapes = unread_after_shapes();
            assert_eq!(shapes.len(), 2, "a shape with no test");
            check_final_shape(shapes.into_iter().nth($index).expect("a shape"));
        }
    )*};
}

final_at_once_unread! {
    an_srgb_refusal_before_an_xml_of_length_0_is_final: 0,
    a_tile_part_fault_before_an_xml_of_length_0_is_final: 1,
}

/// The types of a box of length 0 after the codestream that leave an
/// error before it final: types the decode does not check there, `ihdr`
/// among them.
const UNREAD: [&[u8; 4]; 9] = [
    b"xml ", b"uuid", b"jp2i", b"res ", b"uinf", b"rreq", b"ihdr", b"free", b"jp2x",
];

/// The types of a box of length 0 after the codestream that the decode
/// checks there, whose contents, or whose absence past 2^32 bytes, can
/// change the error, each with contents that pass in a gray file where the
/// type allows: a `jp2h` with an `ihdr`, a gray `colr`, a `bpcc` for one
/// component, a first `pclr`, a `cdef`. The signature box, the `ftyp` and
/// a `cmap` with no `pclr` are at fault on their type.
fn checked_after() -> [(&'static [u8; 4], Vec<u8>); 8] {
    [
        (b"jP  ", vec![0x0D, 0x0A, 0x87, 0x0A]),
        (b"ftyp", b"jp2 \0\0\0\0jp2 ".to_vec()),
        (b"jp2h", synth::ihdr_for(&codestream(1))),
        (b"colr", synth::colr_enum(17)),
        (b"bpcc", vec![7]),
        (b"pclr", PCLR.to_vec()),
        (b"cmap", CMAP.to_vec()),
        (b"cdef", CDEF.to_vec()),
    ]
}

/// A fault in a tile-part header, then a box of length 0 of each type, fed
/// whole and then with more bytes. After each of [`UNREAD`], holding 2
/// bytes that would be at fault in a `colr`, `refine` gives the fault at
/// once and `header` what `read_header_with` gives, and more bytes change
/// neither. So it is after a `colr` that passes, which passes with any
/// bytes more. After each other of [`checked_after`], `header` gives the
/// header and `refine` waits on `Held`, though the box passes where its
/// type allows: more bytes can bring a fault in it, as they do to a `bpcc`
/// and a `jp2h` here.
#[test]
fn an_error_before_a_box_of_length_0_is_final_only_where_the_box_cannot_turn_at_fault() {
    let lenient = DecodeOptions::default().with_strict(false);
    let unread = UNREAD.iter().map(|kind| (*kind, vec![1, 0], true));
    let checked = checked_after().into_iter();
    let checked = checked.map(|(kind, payload)| (kind, payload, kind == b"colr"));
    for (kind, payload, final_at_once) in unread.chain(checked) {
        let name = String::from_utf8_lossy(&kind[..]).into_owned();
        let bytes = tile_part_file(&coc_fault(), &open_box(kind, &payload));
        let whole = error_of(&decode_with(&bytes, &lenient)).expect("an error");
        let fault = [b"bpcc", b"jp2h"].contains(&kind).then(|| {
            let more = [&bytes[..], &synth::bx(b"colr", &[1, 0])].concat();
            error_of(&decode_with(&more, &lenient)).expect("an error")
        });
        assert!(fault.is_none_or(|fault| fault != whole), "{name}: {whole}");
        let whole_header = error_of(&read_header_with(&bytes, &lenient));
        if final_at_once {
            assert!(whole.contains("COC"), "{name}: {whole}");
        }
        for strict in [false, true] {
            let mut decoder =
                IncrementalDecoder::new(lenient.clone().with_strict(strict)).expect("valid");
            for (fed, piece) in [&bytes[..], &gray(), &[0; 64]].into_iter().enumerate() {
                decoder.feed(piece);
                let at = format!("`{name}`, strict {strict}, then {} bytes", piece.len());
                if final_at_once {
                    assert_eq!(error_of(&decoder.header()), whole_header, "{at}: header");
                    assert_eq!(error_of(&decoder.refine()), Some(whole.clone()), "{at}");
                } else {
                    assert!(matches!(decoder.header(), Ok(Some(_))), "{at}: header");
                    match decoder.refine() {
                        Ok(Refined::Waiting(Waiting::Held { .. })) => {}
                        // Too few bytes since the last decode for another.
                        Ok(Refined::Waiting(Waiting::MoreBytes { .. })) if fed > 0 => {}
                        other => panic!("{at}: {other:?}"),
                    }
                }
            }
        }
    }
}

/// Why [`unread_after_shapes`] are final at any length: with 2^32 bytes or
/// more after the codestream, OpenJPEG skips the box of length 0 there
/// unread, and the decode gives the same error. So it does after a box of
/// length 0 of each of [`UNREAD`]. The zeros are allocated lazily and only
/// the start is written, so the test needs little memory.
#[test]
#[cfg(target_pointer_width = "64")]
fn an_error_before_a_box_of_length_0_not_checked_stays_past_2_32_bytes() {
    let mut shapes = unread_after_shapes();
    for kind in UNREAD {
        shapes.push((
            "a fault in a tile-part header, then a box of length 0",
            tile_part_file(&coc_fault(), &open_box(kind, &[1, 0])),
            DecodeOptions::default(),
            0,
        ));
    }
    for (name, bytes, options, _) in &shapes {
        let lenient = options.clone().with_strict(false);
        let whole = error_of(&decode_with(bytes, &lenient)).expect("an error");
        let mut padded = vec![0u8; bytes.len() + (1 << 32) + 16];
        padded[..bytes.len()].copy_from_slice(bytes);
        assert_eq!(
            error_of(&decode_with(&padded, &lenient)),
            Some(whole),
            "{name}, {bytes:?}"
        );
    }
}
