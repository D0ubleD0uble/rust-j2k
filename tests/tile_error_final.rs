//! With `strict` off, an error met in the tiles is final only once the input
//! closes.
//!
//! The decode parses every tile-part before it decodes any tile, so a fault
//! in a later tile-part comes before a tile the reduction or a tile cap
//! refuses, and before a damaged packet header. On a prefix that ends before
//! that tile-part, `decode_with` gives the tile's error; on a longer one,
//! the fault. So an [`IncrementalDecoder`] waits on such an error, as
//! [`Waiting::Held`], and its `header` holds a tile refusal back, until
//! the bytes fed reach the `EOC`. In a JP2 file boxes after the codestream
//! are checked first, so it waits while one can still be read, and from
//! then on as in a codestream. Fed in any two pieces or a byte at a
//! time, each goes from the main header cut short, to images or waits, to
//! at most one error, which is the one the one-shot call gives on the bytes
//! fed.
//!
//! The shapes are built here. In each, tile 0's first tile-part carries the
//! coding or the data the tile fails on, and a later tile-part of the same
//! tile holds the fault:
//!
//! - one tile whose own `COD` gives no levels, under a reduction of 1, and a
//!   later `COD` that differs;
//! - one tile whose own `COD` gives 1x1 precincts, over the precinct cap of
//!   `SampleLimit::AtMost(2^24)`, and a later `COC` for a component the
//!   image does not have;
//! - each again with the refusal under the main header, so that the header
//!   read counts the tile too: two tiles, of which tile 1 is over the
//!   precinct cap under the main header, and one tile whose main `COD` gives
//!   1x1 precincts;
//! - one 4x4 tile whose packet header has an Lblock indicator longer than
//!   any Lblock, and one whose code-block needs 32 bit-planes, each with a
//!   later `COD` that differs.
//!
//! A later tile-part's `POC` can also clear the error, so that the whole
//! file decodes: [`a_poc_in_a_later_tile_part_can_clear_an_error_in_a_tile`]
//! pins that.

#[path = "support/held.rs"]
mod held;
#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{
    DecodeOptions, Error, IncrementalDecoder, Refined, SampleLimit, Waiting, decode_with,
    read_header_with,
};

/// `result`'s error as its `Debug` text, or `None`: [`Error`] has no
/// `PartialEq`.
fn error_of<T>(result: &Result<T, Error>) -> Option<String> {
    result.as_ref().err().map(|error| format!("{error:?}"))
}

const EOC: [u8; 2] = [0xFF, 0xD9];

/// A `COD` segment: no levels, LRCP, one layer, and with `precincts`, 1x1
/// precincts at its one resolution.
fn cod_no_levels(precincts: bool) -> Vec<u8> {
    let mut cod = synth::cod_body(1, 0, 0, 0);
    if precincts {
        cod[0] = 1;
        cod.push(0x00);
    }
    synth::seg(0xFF52, &cod)
}

/// A `COD` segment of one level and no precinct sizes.
fn cod_one_level() -> Vec<u8> {
    synth::seg(0xFF52, &synth::cod_body(1, 1, 0, 0))
}

/// A `COC` for component 5, which a one-component image does not have.
fn coc_past_the_components() -> Vec<u8> {
    synth::seg(0xFF53, &[5, 0, 0, 0, 0, 0, 0])
}

/// A main header of one 8-bit gray component on `grid` with `cod`, and
/// `QCD` exponents of `exponent` for `bands` subbands, under 2 guard bits.
fn main_header_with(grid: [u32; 8], cod: &[u8], bands: usize, exponent: u8) -> Vec<u8> {
    let siz = synth::siz_body(grid, &[[7, 1, 1]]);
    [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        cod.to_vec(),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &vec![exponent; bands])),
    ]
    .concat()
}

/// [`main_header_with`] exponents of 8.
fn main_header(grid: [u32; 8], cod: &[u8], bands: usize) -> Vec<u8> {
    main_header_with(grid, cod, bands, 8)
}

/// One tile of 64 x 64 under a main `COD` of one level. Its first
/// tile-part's `COD` gives no levels, which a reduction of 1 refuses; its
/// second carries `later`.
fn reduction_one_tile(later: &[u8]) -> Vec<u8> {
    let mut bytes = main_header([64, 64, 0, 0, 64, 64, 0, 0], &cod_one_level(), 4);
    bytes.extend(synth::tile_part(0, 0, 2, &cod_no_levels(false), &[0]));
    bytes.extend(synth::tile_part(0, 1, 2, later, &[]));
    bytes.extend(EOC);
    bytes
}

/// Two tiles, 8 and 1000 wide and 1000 tall (`XOsiz` 992), under a main
/// `COD` of one level, 1x1 precincts at resolution 0 and 2x2 at 1, which
/// puts tile 1 over the precinct cap and has the header read count tile 0
/// under a reduction. Tile 0's first tile-part's `COD` gives no levels; its
/// second, after tile 1's tile-part, carries `later`.
fn reduction_row(later: &[u8]) -> Vec<u8> {
    let mut cod = synth::cod_body(1, 1, 0, 0);
    cod[0] = 1;
    cod.extend([0x00, 0x11]);
    let mut bytes = main_header(
        [2000, 1000, 992, 0, 1000, 1000, 0, 0],
        &synth::seg(0xFF52, &cod),
        4,
    );
    bytes.extend(synth::tile_part(0, 0, 2, &cod_no_levels(false), &[0]));
    bytes.extend(synth::tile_part(1, 0, 1, &[], &[]));
    bytes.extend(synth::tile_part(0, 1, 2, later, &[]));
    bytes.extend(EOC);
    bytes
}

/// One tile of 1000 x 1000 under a main `COD` of no levels. Its first
/// tile-part's `COD` gives 1x1 precincts, 10^6 of them, over the precinct
/// cap of `AtMost(2^24)`; its second carries `later`. With `main`, the main
/// `COD` gives the 1x1 precincts instead, and the first tile-part none.
fn at_most_one_tile(main: bool, later: &[u8]) -> Vec<u8> {
    let (cod, own) = if main {
        (cod_no_levels(true), vec![])
    } else {
        (cod_no_levels(false), cod_no_levels(true))
    };
    let mut bytes = main_header([1000, 1000, 0, 0, 1000, 1000, 0, 0], &cod, 1);
    bytes.extend(synth::tile_part(0, 0, 2, &own, &[0]));
    bytes.extend(synth::tile_part(0, 1, 2, later, &[]));
    bytes.extend(EOC);
    bytes
}

/// `bits` packed into bytes with a stuffed bit after each `FF`, as a packet
/// header is.
fn pack_bits(bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut bits = bits.iter().copied().peekable();
    while bits.peek().is_some() {
        let width = if out.last() == Some(&0xFF) { 7 } else { 8 };
        let mut byte = 0u8;
        for _ in 0..width {
            byte = byte << 1 | bits.next().unwrap_or(0);
        }
        out.push(byte);
    }
    out
}

/// One 4 x 4 tile of one 4 x 4 code-block and no levels, with QCD
/// exponents of `exponent`. Its first tile-part holds the packet `packet`;
/// its second carries `later`.
fn packet_one_tile(exponent: u8, packet: &[u8], later: &[u8]) -> Vec<u8> {
    let mut bytes = main_header_with([4, 4, 0, 0, 4, 4, 0, 0], &cod_no_levels(false), 1, exponent);
    bytes.extend(synth::tile_part(0, 0, 2, &[], packet));
    bytes.extend(synth::tile_part(0, 1, 2, later, &[]));
    bytes.extend(EOC);
    bytes
}

/// A packet whose header has the code-block nonempty and included, with no
/// zero bit-planes and one pass, then an Lblock indicator of 64 ones, longer
/// than any Lblock (as in `tests/partial_decode.rs`).
fn lblock_packet() -> Vec<u8> {
    let mut bits = vec![1, 1, 1, 1, 1, 0];
    bits.extend(std::iter::repeat_n(1, 64));
    bits.extend([0, 0, 0, 1]);
    let mut data = pack_bits(&bits);
    data.extend_from_slice(&[0; 4]);
    data
}

/// A packet whose code-block is included with no zero bit-planes and one
/// pass of 1 byte: under QCD exponents of 31 and 2 guard bits it needs 32
/// bit-planes, over the 30-plane limit.
fn bit_planes_packet() -> Vec<u8> {
    let mut data = pack_bits(&[1, 1, 1, 0, 0, 0, 0, 1]);
    data.push(0);
    data
}

/// `codestream` in a JP2 file with a gray `colr`, and the boxes `trailing`
/// after it.
fn in_jp2(codestream: &[u8], trailing: &[Vec<u8>]) -> Vec<u8> {
    let ihdr = synth::ihdr_for(codestream);
    let colr = synth::bx(b"colr", &synth::colr_enum(17));
    synth::jp2_file(&[ihdr, colr], &[], codestream, trailing)
}

fn reduced() -> DecodeOptions {
    DecodeOptions::default().with_resolution_reduction(1)
}

fn at_most() -> DecodeOptions {
    DecodeOptions::default().with_sample_limit(SampleLimit::AtMost(1 << 24))
}

/// How far a stream has got: the main header cut short, then images or
/// waits, then an error. A stream may only move forward through these.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Incomplete,
    Ok,
    Failed(String),
}

/// The phases a decoder's `header` and `refine` have reached, checked to
/// move only forward and to end in at most one error.
#[derive(Default)]
struct Track {
    header: Option<Phase>,
    refine: Option<Phase>,
}

fn advance(seen: &mut Option<Phase>, now: Phase, what: &str, context: &str) {
    if let Some(before) = seen {
        match (&*before, &now) {
            (Phase::Failed(was), Phase::Failed(is)) => {
                assert_eq!(was, is, "{context}: {what} gave a second error");
            }
            (Phase::Failed(was), _) => panic!("{context}: {what} gave {now:?} after {was}"),
            _ => assert!(
                *before <= now,
                "{context}: {what} went from {before:?} to {now:?}"
            ),
        }
    }
    *seen = Some(now);
}

impl Track {
    /// Call `header` and `refine` on `decoder`, check each against the
    /// one-shot call on the bytes fed with `strict` off, and move on. A
    /// header or a wait where the one-shot call gives an error is allowed
    /// only while the bytes fed have not closed.
    fn check(&mut self, decoder: &mut IncrementalDecoder, context: &str) {
        let lenient = decoder.options().clone().with_strict(false);
        let fed = decoder.bytes().to_vec();
        let header = match decoder.header() {
            Ok(None) => Phase::Incomplete,
            Ok(Some(_)) => {
                if let Err(error) = read_header_with(&fed, &lenient) {
                    assert!(!held::closes(&fed), "{context}: header for {error:?}");
                }
                Phase::Ok
            }
            Err(error) => {
                let error = format!("{error:?}");
                let one_shot = error_of(&read_header_with(&fed, &lenient));
                assert_eq!(Some(&error), one_shot.as_ref(), "{context}: header");
                Phase::Failed(error)
            }
        };
        advance(&mut self.header, header, "header", context);
        let refine = match decoder.refine() {
            // Not a step: the refine did not decode.
            Ok(Refined::Waiting(Waiting::MoreBytes { .. })) => return,
            Ok(Refined::Waiting(Waiting::HeaderIncomplete { .. })) => Phase::Incomplete,
            Ok(Refined::Waiting(Waiting::Held { error, .. })) => {
                // The error it waits on is the one-shot decode's, on bytes
                // that have not closed.
                assert!(!held::closes(&fed), "{context}: wait on {error:?}");
                let error = format!("{error:?}");
                let one_shot = error_of(&decode_with(&fed, &lenient));
                assert_eq!(Some(&error), one_shot.as_ref(), "{context}: wait");
                Phase::Ok
            }
            Ok(_) => Phase::Ok,
            Err(error) => {
                let error = format!("{error:?}");
                let one_shot = error_of(&decode_with(&fed, &lenient));
                assert_eq!(Some(&error), one_shot.as_ref(), "{context}: refine");
                Phase::Failed(error)
            }
        };
        advance(&mut self.refine, refine, "refine", context);
    }
}

/// Feed `bytes` to decoders under `options` in every two pieces and a byte
/// at a time, and check each stream (see [`Track`]). Where the whole file
/// is in, `refine` returns the error `decode_with` gives with `strict` off,
/// or, where the file does not close, may wait on it; `finish` gives what
/// `decode_with` gives under `options`.
fn check_streams(bytes: &[u8], options: &DecodeOptions, name: &str) {
    let lenient = options.clone().with_strict(false);
    let whole = error_of(&decode_with(bytes, &lenient));
    assert!(whole.is_some(), "{name}: the whole file is refused");
    let failed = Phase::Failed(whole.clone().unwrap());
    let ended = |phase: &Option<Phase>| {
        phase.as_ref() == Some(&failed) || (!held::closes(bytes) && phase == &Some(Phase::Ok))
    };
    let finished = error_of(&decode_with(bytes, options));
    for split in 1..bytes.len() {
        let context = format!("{name}, split at {split}");
        let mut decoder = IncrementalDecoder::new(options.clone()).expect("valid options");
        let mut track = Track::default();
        decoder.feed(&bytes[..split]);
        track.check(&mut decoder, &context);
        decoder.feed(&bytes[split..]);
        track.check(&mut decoder, &context);
        // A refine the cadence put off: the refine of a decoder fed the
        // whole file at once.
        let mut whole_at_once = IncrementalDecoder::new(options.clone()).expect("valid options");
        whole_at_once.feed(bytes);
        track.check(&mut whole_at_once, &context);
        assert!(ended(&track.refine), "{context}: {:?}", track.refine);
        assert_eq!(error_of(&decoder.finish()), finished, "{context}: finish");
    }
    let context = format!("{name}, a byte at a time");
    let mut decoder = IncrementalDecoder::new(options.clone()).expect("valid options");
    let mut track = Track::default();
    for byte in bytes {
        decoder.feed(std::slice::from_ref(byte));
        track.check(&mut decoder, &context);
    }
    assert_eq!(error_of(&decoder.finish()), finished, "{context}: finish");
}

/// The shapes with their fault, each of which errs in a tile on a prefix
/// that ends before the fault, an error the decode replaces with the fault
/// once it is in. `counted` is whether the header read refuses the tile
/// too.
fn faulty() -> Vec<(&'static str, Vec<u8>, DecodeOptions, bool)> {
    vec![
        (
            "reduction, one tile",
            reduction_one_tile(&cod_one_level()),
            reduced(),
            false,
        ),
        (
            "reduction, two tiles",
            reduction_row(&cod_one_level()),
            reduced(),
            true,
        ),
        (
            "AtMost, the tile's own COD",
            at_most_one_tile(false, &coc_past_the_components()),
            at_most(),
            false,
        ),
        (
            "AtMost, the main COD",
            at_most_one_tile(true, &coc_past_the_components()),
            at_most(),
            true,
        ),
        (
            "Lblock",
            packet_one_tile(8, &lblock_packet(), &cod_one_level()),
            DecodeOptions::default(),
            false,
        ),
        (
            "bit-planes",
            packet_one_tile(31, &bit_planes_packet(), &cod_one_level()),
            DecodeOptions::default(),
            false,
        ),
    ]
}

/// The one-shot calls flip from the tile's error to the fault as the later
/// tile-part comes in, which is why the decoder may not take the first as
/// final before the `EOC`. This pins the shapes: each gives on some prefix
/// an error other than the whole file's, in the decode and, where the tile
/// is counted under the main header, in the header read.
#[test]
fn the_one_shot_calls_give_the_tiles_error_then_the_fault() {
    for (name, bytes, options, counted) in faulty() {
        let lenient = options.with_strict(false);
        let prefixes = || (0..bytes.len()).map(|len| &bytes[..len]);
        let faulted = |error: &Option<String>| {
            error
                .as_deref()
                .is_some_and(|e| e.starts_with("Codestream") || e.starts_with("Marker"))
        };
        let whole = error_of(&decode_with(&bytes, &lenient));
        assert!(faulted(&whole), "{name}: {whole:?}");
        let before = prefixes()
            .filter_map(|p| error_of(&decode_with(p, &lenient)))
            .find(|e| !e.starts_with("Incomplete") && Some(e) != whole.as_ref());
        assert!(before.is_some(), "{name}");
        if name == "Lblock" {
            assert!(before.as_deref().unwrap().contains("Lblock"), "{before:?}");
        }
        if name == "bit-planes" {
            assert!(
                before.as_deref().unwrap().contains("BitPlanes"),
                "{before:?}"
            );
        }
        if counted {
            let whole = error_of(&read_header_with(&bytes, &lenient));
            assert!(faulted(&whole), "{name}: {whole:?}");
            assert!(
                prefixes().any(|p| error_of(&read_header_with(p, &lenient))
                    .is_some_and(|e| !e.starts_with("Incomplete") && Some(&e) != whole.as_ref())),
                "{name}"
            );
        }
        // With `strict` on the whole file gives the fault too.
        let strict = lenient.clone().with_strict(true);
        assert_eq!(error_of(&decode_with(&bytes, &strict)), whole, "{name}");
    }
}

/// Fed in any two pieces or a byte at a time, under `strict` off and on,
/// each decoder goes from the header cut short to images or waits to one
/// error, the one-shot decode's, and `header` likewise.
#[test]
fn an_error_in_a_tile_waits_for_a_later_tile_parts_fault() {
    for (name, bytes, options, _) in faulty() {
        check_streams(&bytes, &options.clone().with_strict(false), name);
        check_streams(&bytes, &options.with_strict(true), name);
    }
}

/// The shapes with the later tile-part mended, which restates the first
/// one's coding or carries none.
fn mended() -> Vec<(&'static str, Vec<u8>, DecodeOptions)> {
    vec![
        (
            "reduction, one tile",
            reduction_one_tile(&cod_no_levels(false)),
            reduced(),
        ),
        ("reduction, two tiles", reduction_row(&[]), reduced()),
        (
            "AtMost, the tile's own COD",
            at_most_one_tile(false, &[]),
            at_most(),
        ),
        (
            "AtMost, the main COD",
            at_most_one_tile(true, &[]),
            at_most(),
        ),
        (
            "Lblock",
            packet_one_tile(8, &lblock_packet(), &[]),
            DecodeOptions::default(),
        ),
        (
            "bit-planes",
            packet_one_tile(31, &bit_planes_packet(), &[]),
            DecodeOptions::default(),
        ),
    ]
}

/// With the later tile-part mended, the tile's error is final once the
/// `EOC` is in, so `refine` returns it there and `header` gives a refusal,
/// as the one-shot calls do.
#[test]
fn an_error_in_a_tile_is_final_at_the_eoc() {
    for (name, bytes, options) in mended() {
        let lenient = options.clone().with_strict(false);
        let failed = error_of(&decode_with(&bytes, &lenient)).expect("an error");
        check_streams(&bytes, &lenient, name);
        // The whole file: `refine` gives the error, and every later call
        // again.
        let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
        decoder.feed(&bytes);
        for _ in 0..2 {
            assert_eq!(error_of(&decoder.refine()), Some(failed.clone()), "{name}");
        }
        // Without its `EOC` the stream may still grow: `refine` waits, and
        // `finish` gives the error with `strict` off, and the cut with it
        // on.
        let cut = &bytes[..bytes.len() - 2];
        let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
        decoder.feed(cut);
        assert!(
            matches!(decoder.refine(), Ok(Refined::Waiting(Waiting::Held { .. }))),
            "{name}"
        );
        assert!(decoder.header().is_ok(), "{name}");
        assert_eq!(error_of(&decoder.finish()), Some(failed.clone()), "{name}");
        let mut strict = IncrementalDecoder::new(options.with_strict(true)).expect("valid options");
        strict.feed(cut);
        assert!(
            matches!(strict.refine(), Ok(Refined::Waiting(Waiting::Held { .. }))),
            "{name}"
        );
        assert!(
            error_of(&strict.finish()).is_some_and(|e| e.starts_with("Codestream")),
            "{name}"
        );
    }
}

/// A JP2 file whose boxes after the codestream can still be read does not
/// close: they are checked before the tile-parts, so a malformed one, or a `jp2h` with no `ihdr`, replaces
/// the tile's error even after the `EOC`. `refine` waits on the tile's
/// error until such a box is in, whose fault is final, and `finish` reports
/// what the whole file gives.
#[test]
fn a_jp2_file_never_closes() {
    for (name, codestream, options) in [
        (
            "reduction",
            reduction_one_tile(&cod_no_levels(false)),
            reduced(),
        ),
        (
            "Lblock",
            packet_one_tile(8, &lblock_packet(), &[]),
            DecodeOptions::default(),
        ),
        // The header read refuses this tile, so `header` must hold it.
        (
            "AtMost, the main COD",
            at_most_one_tile(true, &[]),
            at_most(),
        ),
    ] {
        let lenient = options.clone().with_strict(false);
        let tiles = error_of(&decode_with(&codestream, &lenient)).expect("an error");
        for (after, trailing, gives) in [
            (
                "a colr too short",
                vec![synth::bx(b"colr", &[1, 0])],
                "FileFormat",
            ),
            ("an empty jp2h", vec![synth::bx(b"jp2h", &[])], "FileFormat"),
            ("nothing", vec![], ""),
        ] {
            let name = format!("{name} in JP2, then {after}");
            let bytes = in_jp2(&codestream, &trailing);
            let whole = error_of(&decode_with(&bytes, &lenient)).expect("an error");
            if gives.is_empty() {
                assert_eq!(whole, tiles, "{name}");
            } else {
                assert!(whole.starts_with(gives), "{name}: {whole}");
            }
            // A box after the codestream that the input ends inside waits
            // too, so the stream can be cut anywhere.
            check_streams(&bytes, &lenient, &name);
            // Fed whole, the refine gives the box's fault, or waits on the
            // tile's error.
            let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
            decoder.feed(&bytes);
            match decoder.refine() {
                Ok(Refined::Waiting(Waiting::Held { error, .. })) if gives.is_empty() => {
                    assert_eq!(format!("{error:?}"), whole, "{name}");
                    assert!(decoder.header().is_ok(), "{name}");
                }
                Err(error) if !gives.is_empty() => {
                    assert_eq!(format!("{error:?}"), whole, "{name}");
                }
                other => panic!("{name}: {other:?}"),
            }
            assert_eq!(error_of(&decoder.finish()), Some(whole), "{name}");
            // Cut before the box after the codestream, the refine waits.
            let cut = in_jp2(&codestream, &[]);
            let mut open = IncrementalDecoder::new(lenient.clone()).expect("valid options");
            open.feed(&cut);
            assert!(
                matches!(open.refine(), Ok(Refined::Waiting(Waiting::Held { .. }))),
                "{name}"
            );
        }
    }
}

/// An error that comes before the tile-parts in the decode stays final at
/// once: a reduction past the main header's levels, a refusal of the
/// image's samples under `AtMost`, and a fault of a tile-part in a
/// codestream, which the parse finds in the order of the input. None of
/// the inputs closes.
#[test]
fn an_error_before_the_tiles_is_final_at_once() {
    let open = |bytes: Vec<u8>| bytes[..bytes.len() - 2].to_vec();
    for (name, bytes, options) in [
        (
            "reduction past the main header",
            open(reduction_one_tile(&cod_no_levels(false))),
            DecodeOptions::default().with_resolution_reduction(2),
        ),
        (
            "the image's samples under AtMost",
            open(reduction_one_tile(&cod_no_levels(false))),
            DecodeOptions::default().with_sample_limit(SampleLimit::AtMost(100)),
        ),
        (
            "a tile-part's fault",
            open(reduction_one_tile(&cod_one_level())),
            DecodeOptions::default(),
        ),
    ] {
        let lenient = options.with_strict(false);
        assert!(!held::closes(&bytes), "{name}");
        let one_shot = error_of(&decode_with(&bytes, &lenient)).expect("an error");
        let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
        decoder.feed(&bytes);
        assert_eq!(
            error_of(&decoder.refine()),
            Some(one_shot.clone()),
            "{name}"
        );
        assert_eq!(
            error_of(&decoder.header()),
            error_of(&read_header_with(&bytes, &lenient)),
            "{name}"
        );
    }
}

/// One 4 x 4 tile of one level, LRCP, whose first tile-part holds an empty
/// resolution-0 packet and then a resolution-1 packet with the Lblock
/// damage of [`lblock_packet`]. Its second tile-part holds only a `POC`
/// that reads resolution 0 alone. The end of the first tile-part, and the
/// file.
fn poc_in_a_later_tile_part() -> (usize, Vec<u8>) {
    let mut bytes = main_header([4, 4, 0, 0, 4, 4, 0, 0], &cod_one_level(), 4);
    let mut packets = vec![0x00];
    packets.extend(lblock_packet());
    bytes.extend(synth::tile_part(0, 0, 2, &[], &packets));
    let cut = bytes.len();
    let poc = synth::seg(0xFF5F, &[0, 0, 0, 1, 1, 1, 0]);
    bytes.extend(synth::tile_part(0, 1, 2, &poc, &[]));
    bytes.extend(EOC);
    (cut, bytes)
}

/// A `POC` that comes only in a later tile-part changes which packets the
/// decode reads, so it can clear an error in a tile. Each prefix, fed at
/// once, decodes to an image until the damaged packet header is in, then
/// waits on its error through the prefix that ends before the `POC`, and
/// decodes again once the tile-part that holds the `POC` is in, the whole
/// file too. `finish`, with `strict` on, gives what `decode_with` gives.
#[test]
fn a_poc_in_a_later_tile_part_can_clear_an_error_in_a_tile() {
    let (cut, bytes) = poc_in_a_later_tile_part();
    let options = DecodeOptions::default();
    let lenient = options.clone().with_strict(false);
    let mut held_at = None;
    // Where the tile-part with the `POC` ends, before the `EOC`.
    let poc_in = bytes.len() - EOC.len();
    for end in 1..=bytes.len() {
        let mut decoder = IncrementalDecoder::new(options.clone()).expect("valid options");
        decoder.feed(&bytes[..end]);
        let refined = decoder.refine();
        let one_shot = decode_with(&bytes[..end], &lenient);
        match refined {
            Ok(Refined::Waiting(Waiting::Held { error, .. })) => {
                let error = format!("{error:?}");
                assert!(error.contains("Lblock"), "{end}: {error}");
                assert!(end < poc_in, "{end}: waits with the POC in");
                assert_eq!(error_of(&one_shot), Some(error), "{end}");
                held_at.get_or_insert(end);
            }
            Ok(Refined::Image(image)) => {
                assert!(
                    held_at.is_none() || end >= poc_in,
                    "an image at {end}, after the error at {held_at:?}"
                );
                assert_eq!(Some(image), one_shot.ok(), "{end}");
            }
            Ok(Refined::Waiting(_)) => {
                assert!(held_at.is_none(), "{end}: a wait after the error")
            }
            other => panic!("{end}: {other:?}"),
        }
        if end == cut {
            assert!(held_at.is_some(), "no wait on the error at {cut}");
        }
    }
    let mut decoder = IncrementalDecoder::new(options.clone()).expect("valid options");
    decoder.feed(&bytes);
    let finished = error_of(&decoder.finish()).expect("an error");
    assert!(finished.contains("`POC` volumes"), "{finished}");
    assert_eq!(Some(finished), error_of(&decode_with(&bytes, &options)));
}

/// The seeds of the `incremental_decode` fuzz target built from the shapes
/// here, each with its six control bytes where the options are not the
/// defaults (`20`, a reduction of 1). A seed that starts with `SOC` or the
/// JP2 signature is read with all-zero control bytes.
///
/// The target decodes only on a refine due under the doubling rule, so a
/// seed fails a refine that does not wait only where one is due between
/// the tile's error and the fault after it. A comment of [`PADDING`] bytes
/// before the fault (a `COM` in the later tile-part, or a `free` box after
/// the codestream) puts it at more than three times the bytes before the
/// error, where some refine is due whatever the pieces.
fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    let with_control = |control: u8, file: Vec<u8>| [vec![control, 0, 0, 0, 0, 0], file].concat();
    let com = |later: Vec<u8>| {
        let mut body = vec![0, 1];
        body.resize(PADDING, b' ');
        [synth::seg(0xFF64, &body), later].concat()
    };
    vec![
        (
            "tile_error_reduction_then_cod.seed",
            with_control(0x20, reduction_one_tile(&com(cod_one_level()))),
        ),
        (
            "tile_error_lblock_then_coc.seed",
            packet_one_tile(8, &lblock_packet(), &com(coc_past_the_components())),
        ),
        (
            "tile_error_lblock_then_cod.seed",
            packet_one_tile(8, &lblock_packet(), &com(cod_one_level())),
        ),
        (
            "tile_error_lblock_jp2_then_colr.seed",
            in_jp2(
                &packet_one_tile(8, &lblock_packet(), &[]),
                &[
                    synth::bx(b"free", &vec![0; PADDING]),
                    synth::bx(b"colr", &[1, 0]),
                ],
            ),
        ),
        // A tile's error in a JP2 file, final once a box after the
        // codestream stops OpenJPEG's walk there for good
        // ([`walk_stoppers`]): a second `jp2c`, or an `XLBox` of 2^32. The
        // bytes after the stopper are never read, so [`PADDING`] of them
        // puts a due refine past it.
        (
            "tile_error_lblock_jp2_then_jp2c.seed",
            in_jp2(
                &packet_one_tile(8, &lblock_packet(), &[]),
                &[synth::bx(b"jp2c", &vec![0; PADDING])],
            ),
        ),
        (
            "tile_error_lblock_jp2_then_xlbox.seed",
            in_jp2(
                &packet_one_tile(8, &lblock_packet(), &[]),
                &[[
                    &[0, 0, 0, 1][..],
                    b"xml ",
                    &(1u64 << 32).to_be_bytes(),
                    &vec![0; PADDING],
                ]
                .concat()],
            ),
        ),
        // Not a tile's error: a box after the codestream that the input
        // ends inside, which a refine waits on until it is whole
        // (`tests/jp2_box_error_final.rs`). The header's `colr` was read,
        // so OpenJPEG ignores this one's contents, and the file decodes.
        (
            "held_box_cut_colr_jp2.seed",
            in_jp2(
                &packet_one_tile(8, &[0], &[]),
                &[synth::bx(
                    b"colr",
                    &[synth::colr_enum(17), vec![0; PADDING]].concat(),
                )],
            ),
        ),
    ]
}

/// The bytes of the comment [`seeds`] puts before the fault.
const PADDING: usize = 512;

/// The committed seeds under `tests/fixtures/incremental` are the ones
/// [`seeds`] builds. `J2K_BLESS_TILE_ERROR_SEEDS=1` writes them.
#[test]
fn the_committed_seeds_are_the_shapes() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/incremental");
    let bless = std::env::var_os("J2K_BLESS_TILE_ERROR_SEEDS").is_some();
    for (name, bytes) in seeds() {
        let path = dir.join(name);
        if bless {
            std::fs::write(&path, &bytes).expect("write the seed");
        }
        let committed = std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(committed, bytes, "{name}");
    }
}

/// The two tiles of [`reduction_row`], with a tile-part for tile 1 alone:
/// tile 1, of 500,000 precincts under the main header, is over the precinct
/// cap the default sample limit gives these bytes, a refusal more bytes
/// lift. Its one tile-part, then `later`.
fn default_cap_row(later: &[u8]) -> Vec<u8> {
    let mut cod = synth::cod_body(1, 1, 0, 0);
    cod[0] = 1;
    cod.extend([0x00, 0x11]);
    let mut bytes = main_header(
        [2000, 1000, 992, 0, 1000, 1000, 0, 0],
        &synth::seg(0xFF52, &cod),
        4,
    );
    bytes.extend(synth::tile_part(1, 0, 1, &[], &[]));
    bytes.extend(later);
    bytes
}

/// A tile refused by a cap more bytes lift is held back by `header` until
/// the tile-parts close, as a refusal they do not lift is: a later fault
/// replaces it in the one-shot calls. Here the fault is two zero bytes
/// where an `SOT` or the `EOC` belongs (the 86-byte input of issue #649).
/// `refine` waits on the refusal as [`Waiting::SampleLimit`].
#[test]
fn a_tile_cap_refusal_more_bytes_lift_waits_for_a_later_fault() {
    let bytes = default_cap_row(&[0, 0]);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hex,
        "ff4fff5100290000000007d0000003e8000003e000000000000003e8000003e8000000000000000000\
         01070101ff52000e010000010001000000010011ff5c00074040404040ff90000a00010000000e0001\
         ff930000"
    );
    let lenient = DecodeOptions::default().with_strict(false);
    let whole = error_of(&read_header_with(&bytes, &lenient)).expect("an error");
    assert!(whole.starts_with("Codestream"), "{whole}");
    assert_eq!(
        error_of(&decode_with(&bytes, &lenient)),
        Some(whole.clone())
    );
    // Before the zeros, the one-shot calls refuse the tile, `header` gives
    // the header, and `refine` waits on the same refusal.
    let cut = &bytes[..bytes.len() - 2];
    let refused = error_of(&read_header_with(cut, &lenient)).expect("an error");
    assert!(refused.contains("TilePrecincts"), "{refused}");
    let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
    decoder.feed(cut);
    assert!(
        matches!(decoder.header(), Ok(Some(_))),
        "{:?}",
        decoder.header()
    );
    match decoder.refine() {
        Ok(Refined::Waiting(Waiting::SampleLimit { limit, .. })) => {
            assert_eq!(format!("{:?}", Error::Limit(limit)), refused);
        }
        other => panic!("{other:?}"),
    }
    // On every prefix `header` gives no error the whole file does not.
    for end in 0..=bytes.len() {
        let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
        decoder.feed(&bytes[..end]);
        if let Err(error) = decoder.header() {
            assert_eq!(format!("{error:?}"), whole, "at {end}");
        }
    }
    check_streams(&bytes, &lenient, "default cap");
    check_streams(&bytes, &DecodeOptions::default(), "default cap, strict");
}

/// In a JP2 file whose boxes after the codestream can still be read,
/// `header` holds the refusal back until `finish`: such a box is checked
/// first, so a short `colr`
/// there replaces it even after the `EOC`.
#[test]
fn a_tile_cap_refusal_more_bytes_lift_waits_in_a_jp2_file() {
    let codestream = default_cap_row(&EOC);
    let bytes = in_jp2(&codestream, &[synth::bx(b"colr", &[1, 0])]);
    let lenient = DecodeOptions::default().with_strict(false);
    let whole = error_of(&read_header_with(&bytes, &lenient)).expect("an error");
    assert!(whole.starts_with("FileFormat"), "{whole}");
    let cut = in_jp2(&codestream, &[]);
    let refused = error_of(&read_header_with(&cut, &lenient)).expect("an error");
    assert!(refused.contains("TilePrecincts"), "{refused}");
    let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
    decoder.feed(&cut);
    assert!(matches!(decoder.header(), Ok(Some(_))));
    assert!(matches!(
        decoder.refine(),
        Ok(Refined::Waiting(Waiting::SampleLimit { .. }))
    ));
    assert_eq!(error_of(&decoder.finish()), Some(refused));
    check_streams(
        &bytes,
        &lenient,
        "default cap in JP2, then a colr too short",
    );
}

/// With an `EOC` in place of the zeros the tile-parts close, so `header`
/// gives the refusal, as the one-shot read does, and `refine` still waits
/// on it as [`Waiting::SampleLimit`].
#[test]
fn a_tile_cap_refusal_more_bytes_lift_is_given_at_the_eoc() {
    let bytes = default_cap_row(&EOC);
    let lenient = DecodeOptions::default().with_strict(false);
    let refused = error_of(&read_header_with(&bytes, &lenient)).expect("an error");
    assert!(refused.contains("TilePrecincts"), "{refused}");
    let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
    decoder.feed(&bytes);
    assert_eq!(error_of(&decoder.header()), Some(refused.clone()));
    assert!(
        matches!(
            decoder.refine(),
            Ok(Refined::Waiting(Waiting::SampleLimit { .. }))
        ),
        "a refusal more bytes lift"
    );
    assert_eq!(error_of(&decoder.finish()), Some(refused));
}

/// A box after the codestream that stops OpenJPEG's walk there for good: the
/// header of a second `jp2c`, or a box whose `XLBox` is 2^32. Nothing after
/// it is read, so no byte appended to the file adds a box the decode checks
/// before the tile-parts. With each, how many of its bytes come after the
/// one that settles it: the walk stops at the `jp2c` type, and at the
/// nonzero high half of the `XLBox`.
fn walk_stoppers() -> Vec<(&'static str, Vec<u8>, usize)> {
    vec![
        ("a second jp2c", synth::bx(b"jp2c", &[]), 0),
        (
            "an XLBox of 2^32",
            [&[0, 0, 0, 1][..], b"xml ", &(1u64 << 32).to_be_bytes()].concat(),
            4,
        ),
    ]
}

/// Bytes appended to a JP2 file whose tile-parts' error no later byte can
/// change: boxes of each kind the walk after the codestream reads, at fault
/// or not, of length 0 and with an `XLBox`, and a second `jp2c`.
fn jp2_suffixes() -> Vec<Vec<u8>> {
    let open = |kind: &[u8; 4], payload: &[u8]| [&[0, 0, 0, 0][..], kind, payload].concat();
    vec![
        vec![0; 64],
        synth::bx(b"colr", &[1, 0]),
        synth::bx(b"colr", &synth::colr_enum(16)),
        synth::bx(b"jp2h", &[]),
        synth::JP2_SIGNATURE.to_vec(),
        synth::ftyp_jp2(),
        synth::bx(b"bpcc", &[7, 7]),
        synth::bx(b"pclr", &[0, 1, 1, 7, 0]),
        synth::bx(b"cmap", &[0, 0, 1, 0]),
        synth::bx(b"cdef", &[0, 1, 0, 0, 0, 0]),
        synth::bx(b"xml ", b"<a/>"),
        open(b"colr", &[1, 0]),
        open(b"jp2h", &[]),
        [&[0, 0, 0, 1][..], b"colr", &18u64.to_be_bytes(), &[1, 0]].concat(),
        synth::bx(b"jp2c", &[0xFF, 0x4F]),
        [&EOC[..], &synth::bx(b"colr", &[1, 0])].concat(),
    ]
}

/// Check that `bytes`, whose error under `options` with `strict` off is
/// final, give that error with every [`jp2_suffixes`] appended, in the
/// decode and, where it refuses, the header read.
fn check_suffixes(bytes: &[u8], options: &DecodeOptions, name: &str) {
    let lenient = options.clone().with_strict(false);
    let whole = error_of(&decode_with(bytes, &lenient)).expect("an error");
    let header = error_of(&read_header_with(bytes, &lenient));
    for suffix in jp2_suffixes() {
        let longer = [bytes, &suffix].concat();
        let at = format!("{name}, then {suffix:02x?}");
        assert_eq!(
            error_of(&decode_with(&longer, &lenient)),
            Some(whole.clone()),
            "{at}"
        );
        if header.is_some() {
            assert_eq!(
                error_of(&read_header_with(&longer, &lenient)),
                header,
                "{at}"
            );
        }
    }
}

/// Once a box after the codestream stops the walk there for good
/// ([`walk_stoppers`]), no box that more bytes bring is read, so a JP2
/// file's tile-parts are held as a codestream's are: an error in a tile is
/// final at the `EOC`, and `refine` returns it once that box is in, as
/// `header` does a tile the header read refuses. A box of length 0 after
/// the codestream is read whatever follows it, while under 2^32 bytes do,
/// so the error stays held.
#[test]
fn a_jp2_file_settles_once_no_box_after_the_codestream_can_be_read() {
    for (name, codestream, options) in mended() {
        let lenient = options.clone().with_strict(false);
        let tiles = error_of(&decode_with(&codestream, &lenient)).expect("an error");
        for (stopper, after, past) in walk_stoppers() {
            let name = format!("{name} in JP2, then {stopper}");
            let bytes = in_jp2(&codestream, std::slice::from_ref(&after));
            assert_eq!(
                error_of(&decode_with(&bytes, &lenient)),
                Some(tiles.clone()),
                "{name}"
            );
            // The oracle the fuzz targets use finds the same from the bytes.
            assert!(held::closes(&bytes), "{name}");
            check_streams(&bytes, &lenient, &name);
            check_suffixes(&bytes, &options, &name);
            let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
            decoder.feed(&bytes);
            assert_eq!(error_of(&decoder.refine()), Some(tiles.clone()), "{name}");
            assert_eq!(
                error_of(&decoder.header()),
                error_of(&read_header_with(&bytes, &lenient)),
                "{name}"
            );
            // Cut before the byte that settles it, the walk may still read
            // a box.
            let mut cut = IncrementalDecoder::new(lenient.clone()).expect("valid options");
            let short = &bytes[..bytes.len() - past - 1];
            cut.feed(short);
            assert!(!held::closes(short), "{name}: cut");
            assert!(
                matches!(cut.refine(), Ok(Refined::Waiting(Waiting::Held { .. }))),
                "{name}: cut"
            );
            assert!(matches!(cut.header(), Ok(Some(_))), "{name}: cut");
        }
        let name = format!("{name} in JP2, then a box of length 0");
        let open = [&[0, 0, 0, 0][..], b"xml ", b"<a/>"].concat();
        let bytes = in_jp2(&codestream, &[open]);
        assert!(!held::closes(&bytes), "{name}");
        let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
        decoder.feed(&bytes);
        match decoder.refine() {
            Ok(Refined::Waiting(Waiting::Held { error, .. })) => {
                assert_eq!(format!("{error:?}"), tiles, "{name}");
            }
            other => panic!("{name}: {other:?}"),
        }
        assert!(matches!(decoder.header(), Ok(Some(_))), "{name}");
    }
}

/// `held::closes`, the oracle the fuzz targets use, finds from the bytes
/// what the decoder does: a refine on the whole input gives the tile's
/// error once the input closes, and waits on it before. The shapes: a
/// reserved marker with no segment (`0xFF30`) in the main header, which
/// the walk steps over on its two bytes (ISO/IEC 15444-1 A.1.3); and a
/// `jp2c` box of length 0, as OpenJPH writes one. Each is checked as a raw
/// codestream, and in a JP2 file then a second `jp2c`, whole and cut one
/// byte short of that box's type.
#[test]
fn the_oracle_agrees_with_the_decoder_on_when_a_file_closes() {
    let plain = packet_one_tile(8, &lblock_packet(), &[]);
    let sot = plain
        .windows(2)
        .position(|w| w == [0xFF, 0x90])
        .expect("an SOT");
    let reserved = [&plain[..sot], &[0xFF, 0x30], &plain[sot..]].concat();
    let stopper = synth::bx(b"jp2c", &[]);
    let lbox_0 = |codestream: &[u8]| {
        let header = in_jp2(codestream, &[]);
        let jp2c = header.len() - codestream.len() - 8;
        [
            &header[..jp2c],
            &[0, 0, 0, 0],
            b"jp2c",
            codestream,
            &stopper,
        ]
        .concat()
    };
    let mut cases = Vec::new();
    for (name, codestream) in [("plain", &plain), ("0xFF30", &reserved)] {
        cases.push((format!("{name}, a codestream"), codestream.clone(), true));
        let file = in_jp2(codestream, std::slice::from_ref(&stopper));
        cases.push((format!("{name}, JP2"), file.clone(), true));
        cases.push((
            format!("{name}, JP2 cut"),
            file[..file.len() - 1].to_vec(),
            false,
        ));
        let jph = lbox_0(codestream);
        cases.push((format!("{name}, jp2c of length 0"), jph.clone(), true));
        cases.push((
            format!("{name}, jp2c of length 0 cut"),
            jph[..jph.len() - 1].to_vec(),
            false,
        ));
    }
    let lenient = DecodeOptions::default().with_strict(false);
    for (name, bytes, closed) in cases {
        let one_shot = error_of(&decode_with(&bytes, &lenient)).expect("an error");
        assert!(one_shot.contains("Lblock"), "{name}: {one_shot}");
        assert_eq!(held::closes(&bytes), closed, "{name}");
        let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
        decoder.feed(&bytes);
        match decoder.refine() {
            Ok(Refined::Waiting(Waiting::Held { error, .. })) if !closed => {
                assert_eq!(format!("{error:?}"), one_shot, "{name}");
            }
            refined => assert_eq!(error_of(&refined), Some(one_shot), "{name}"),
        }
    }
}

/// `codestream` with the `Psot` of its last tile-part set to 0 and its
/// `EOC` dropped, so that tile-part runs to the end of the input.
fn open_last_part(codestream: &[u8]) -> Vec<u8> {
    let be16 = |at: usize| u16::from_be_bytes([codestream[at], codestream[at + 1]]);
    let mut at = 2;
    while be16(at) != 0xFF90 {
        at += 2 + usize::from(be16(at + 2));
    }
    loop {
        let psot = u32::from_be_bytes(codestream[at + 6..at + 10].try_into().unwrap());
        let next = at + psot as usize;
        if be16(next) == 0xFFD9 {
            let mut open = codestream[..next].to_vec();
            open[at + 6..at + 10].fill(0);
            return open;
        }
        at = next;
    }
}

/// OpenJPEG reads no box after a codestream whose last tile-part has
/// `Psot = 0`, as it runs to the end of the file, so the tile-parts of such
/// a JP2 file are held as a codestream's are. An error in a tile waits, as
/// the tile-parts never close, until a fault of the parse of the last
/// tile-part comes in, which is final; `header` holds a tile the header
/// read refuses until then.
#[test]
fn a_fault_in_a_last_tile_part_with_psot_0_is_final_in_a_jp2_file() {
    for (name, codestream, options, _) in faulty() {
        let lenient = options.clone().with_strict(false);
        let bytes = in_jp2(&open_last_part(&codestream), &[]);
        let name = format!("{name}, Psot 0, in JP2");
        let whole = error_of(&decode_with(&bytes, &lenient)).expect("an error");
        assert!(
            whole.starts_with("Codestream") || whole.starts_with("Marker"),
            "{name}: {whole}"
        );
        check_streams(&bytes, &lenient, &name);
        check_streams(&bytes, &options.clone().with_strict(true), &name);
        check_suffixes(&bytes, &options, &name);
        let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
        decoder.feed(&bytes);
        assert_eq!(error_of(&decoder.refine()), Some(whole), "{name}");
    }
}

/// `codestream` cut at the end of its first tile-part, with no `EOC`.
fn first_part(codestream: &[u8]) -> Vec<u8> {
    let sot = codestream
        .windows(2)
        .position(|w| w == [0xFF, 0x90])
        .expect("an SOT");
    let psot = u32::from_be_bytes(codestream[sot + 6..sot + 10].try_into().unwrap());
    codestream[..sot + psot as usize].to_vec()
}

/// A JP2 file whose codestream stops at the end of a tile-part, then one
/// byte that is not `0xFF` where the next `SOT` or the `EOC` belongs. No
/// byte after it makes that a marker, so OpenJPEG's walk never reaches the
/// boxes after the codestream, and the decode reads the `jp2c` box's
/// payload, which a box of explicit length holds whole whatever follows.
/// The tile's error (`decode_with` with `strict` off) is then final, and
/// `refine` and `header` give it at once. The byte is past the box, or the
/// last of its payload. In a `jp2c` box of length 0, or one the input ends
/// inside, the byte is the last of a payload that more bytes extend, and
/// they bring a fault of the
/// tile-parts after it, so there the error stays held; so it does where
/// the byte is `0xFF`, which more bytes can make a marker.
fn check_lone_byte(name: &str, codestream: &[u8], options: &DecodeOptions) {
    let lenient = options.clone().with_strict(false);
    let part = first_part(codestream);
    for byte in [0x00, 0x41] {
        let past = [in_jp2(&part, &[]), vec![byte]].concat();
        let inside = in_jp2(&[&part[..], &[byte]].concat(), &[]);
        for (place, bytes) in [("past the box", past), ("inside the box", inside)] {
            let name = format!("{name}, {byte:#04x} {place}");
            let whole = error_of(&decode_with(&bytes, &lenient)).expect("an error");
            let header = error_of(&read_header_with(&bytes, &lenient));
            let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
            decoder.feed(&bytes);
            assert_eq!(error_of(&decoder.header()), header, "{name}: header");
            assert_eq!(error_of(&decoder.refine()), Some(whole), "{name}: refine");
            check_suffixes(&bytes, options, &name);
            check_streams(&bytes, &lenient, &name);
        }
    }
    // Held: the byte is the last of a `jp2c` box of length 0 or one the
    // input ends inside, or it is `0xFF`.
    let header = in_jp2(&part, &[]);
    let jp2c = header.len() - part.len() - 8;
    let open = [&header[..jp2c], &[0, 0, 0, 0], b"jp2c", &part, &[0x00]].concat();
    let mut cut = in_jp2(&[&part[..], &[0x00, 0x00]].concat(), &[]);
    cut.pop();
    let ff = [in_jp2(&part, &[]), vec![0xFF]].concat();
    for (place, bytes) in [
        ("0x00 in a jp2c of length 0", open),
        ("0x00 in a jp2c the input ends inside", cut),
        ("0xff", ff),
    ] {
        let name = format!("{name}, {place}");
        let whole = error_of(&decode_with(&bytes, &lenient)).expect("an error");
        let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
        decoder.feed(&bytes);
        match decoder.refine() {
            Ok(Refined::Waiting(Waiting::Held { error, .. })) => {
                assert_eq!(format!("{error:?}"), whole, "{name}");
            }
            other => panic!("{name}: {other:?}"),
        }
        assert!(matches!(decoder.header(), Ok(Some(_))), "{name}");
        // And more bytes do replace the error: after `0xFF` an `EOC`,
        // then a `colr` box at fault; after the `0x00`, a fault of the
        // tile-part chain.
        let longer = [&bytes[..], &[0xD9], &synth::bx(b"colr", &[1, 0])].concat();
        assert_ne!(
            error_of(&decode_with(&longer, &lenient)),
            Some(whole),
            "{name}"
        );
    }
}

/// The tile's error of [`check_lone_byte`]: the packet header's Lblock
/// indicator, which the decode meets.
#[test]
fn a_lone_byte_where_a_marker_belongs_settles_a_jp2_file_for_the_decode() {
    check_lone_byte(
        "Lblock",
        &packet_one_tile(8, &lblock_packet(), &[]),
        &DecodeOptions::default(),
    );
}

/// The tile's error of [`check_lone_byte`]: a tile refused by the
/// reduction, which the header read meets too.
#[test]
fn a_lone_byte_where_a_marker_belongs_settles_a_jp2_file_for_the_header() {
    check_lone_byte("reduction, two tiles", &reduction_row(&[]), &reduced());
}
