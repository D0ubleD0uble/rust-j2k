//! With `strict` off, a tile refusal is final only once the tile-parts close.
//!
//! The decode parses every tile-part before it counts any tile, so a fault
//! in a later tile-part comes before a tile the reduction or a tile cap
//! refuses. On a prefix that ends before that tile-part, `decode_with` and
//! `read_header_with` give the refusal; on a longer one, the fault. So an
//! [`IncrementalDecoder`] waits on such a refusal, as
//! [`Waiting::TileRefused`], and its `header` holds it back, until the bytes
//! fed reach the `EOC`. Fed in any two pieces or a byte at a time, each goes
//! from the main header cut short, to images or waits, to one error, which
//! is the one the one-shot call gives on the bytes fed.
//!
//! The shapes are built here. In each, tile 0's first tile-part carries the
//! coding the tile is refused under, and a later tile-part of the same tile
//! holds the fault:
//!
//! - one tile whose own `COD` gives no levels, under a reduction of 1, and a
//!   later `COD` that differs;
//! - one tile whose own `COD` gives 1x1 precincts, over the precinct cap of
//!   `SampleLimit::AtMost(2^24)`, and a later `COC` for a component the
//!   image does not have;
//! - each again with the refusal under the main header, so that the header
//!   read counts the tile too: two tiles, of which tile 1 is over the
//!   precinct cap under the main header, and one tile whose main `COD` gives
//!   1x1 precincts.

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
/// `QCD` exponents for `bands` subbands.
fn main_header(grid: [u32; 8], cod: &[u8], bands: usize) -> Vec<u8> {
    let siz = synth::siz_body(grid, &[[7, 1, 1]]);
    [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        cod.to_vec(),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &vec![8; bands])),
    ]
    .concat()
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
/// move only forward and to end in one error.
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
    /// one-shot call on the bytes fed with `strict` off, and move on.
    fn check(&mut self, decoder: &mut IncrementalDecoder, context: &str) {
        let lenient = decoder.options().clone().with_strict(false);
        let fed = decoder.bytes().to_vec();
        let header = match decoder.header() {
            Ok(None) => Phase::Incomplete,
            Ok(Some(_)) => {
                // A header where the one-shot read refuses is a refusal held
                // back while the tile-parts have not closed.
                if let Err(error) = read_header_with(&fed, &lenient) {
                    assert!(held::held(&error, &fed), "{context}: header for {error:?}");
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
            Ok(Refined::Waiting(Waiting::TileRefused { error, .. })) => {
                // The refusal it waits on is the one-shot decode's, on
                // tile-parts that have not closed.
                assert!(held::held(&error, &fed), "{context}: wait on {error:?}");
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
/// and `finish` gives what `decode_with` gives under `options`.
fn check_streams(bytes: &[u8], options: &DecodeOptions, name: &str) {
    let lenient = options.clone().with_strict(false);
    let whole = error_of(&decode_with(bytes, &lenient));
    assert!(whole.is_some(), "{name}: the whole file is refused");
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
        assert_eq!(
            track.refine,
            Some(Phase::Failed(whole.clone().unwrap())),
            "{context}"
        );
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

/// The shapes with their fault, and the refusal each gives on a prefix that
/// ends before the fault, which the decode replaces with the fault once it
/// is in. `counted` is whether the header read refuses the tile too.
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
    ]
}

/// The one-shot calls flip from the refusal to the fault as the later
/// tile-part comes in, which is why the decoder may not take the refusal as
/// final before the `EOC`. This pins the shapes: each refuses its tile on
/// some prefix and gives the fault on the whole file, in the decode and,
/// where the tile is counted under the main header, in the header read.
#[test]
fn the_one_shot_calls_give_the_refusal_then_the_fault() {
    for (name, bytes, options, counted) in faulty() {
        let lenient = options.with_strict(false);
        let refused = |error: &Option<String>| {
            error
                .as_deref()
                .is_some_and(|e| e.starts_with("InvalidOptions") || e.starts_with("Limit"))
        };
        let faulted = |error: &Option<String>| {
            error
                .as_deref()
                .is_some_and(|e| e.starts_with("Codestream") || e.starts_with("Marker"))
        };
        let prefixes = || (0..bytes.len()).map(|len| &bytes[..len]);
        assert!(
            prefixes().any(|p| refused(&error_of(&decode_with(p, &lenient)))),
            "{name}"
        );
        assert!(faulted(&error_of(&decode_with(&bytes, &lenient))), "{name}");
        if counted {
            assert!(
                prefixes().any(|p| refused(&error_of(&read_header_with(p, &lenient)))),
                "{name}"
            );
            assert!(
                faulted(&error_of(&read_header_with(&bytes, &lenient))),
                "{name}"
            );
        }
        // With `strict` on the whole file gives the fault too, and a prefix
        // without the `EOC` is cut short.
        let strict = lenient.clone().with_strict(true);
        assert_eq!(
            error_of(&decode_with(&bytes, &strict)),
            error_of(&decode_with(&bytes, &lenient)),
            "{name}"
        );
    }
}

/// Fed in any two pieces or a byte at a time, under `strict` off and on,
/// each decoder goes from the header cut short to images or waits to one
/// error, the one-shot decode's, and `header` likewise.
#[test]
fn a_tile_refusal_waits_for_a_later_tile_parts_fault() {
    for (name, bytes, options, _) in faulty() {
        check_streams(&bytes, &options.clone().with_strict(false), name);
        check_streams(&bytes, &options.with_strict(true), name);
    }
}

/// The same shapes with the later tile-part mended, which restates the
/// first one's coding or carries none: the refusal is final once the `EOC`
/// is in, so `refine` returns it there and `header` gives it, as the
/// one-shot calls do.
#[test]
fn a_tile_refusal_is_final_at_the_eoc() {
    let mended = [
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
    ];
    for (name, bytes, options) in mended {
        let lenient = options.clone().with_strict(false);
        let refused = error_of(&decode_with(&bytes, &lenient)).expect("a refusal");
        assert!(
            refused.starts_with("InvalidOptions") || refused.starts_with("Limit"),
            "{name}: {refused}"
        );
        check_streams(&bytes, &lenient, name);
        // The whole file: `refine` gives the refusal, and every later call
        // again.
        let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
        decoder.feed(&bytes);
        for _ in 0..2 {
            assert_eq!(error_of(&decoder.refine()), Some(refused.clone()), "{name}");
        }
        // Without its `EOC` the stream may still grow: `refine` waits, and
        // `finish` gives the refusal with `strict` off, and the cut with it
        // on.
        let cut = &bytes[..bytes.len() - 2];
        let mut decoder = IncrementalDecoder::new(lenient.clone()).expect("valid options");
        decoder.feed(cut);
        assert!(
            matches!(
                decoder.refine(),
                Ok(Refined::Waiting(Waiting::TileRefused { .. }))
            ),
            "{name}"
        );
        assert!(decoder.header().is_ok(), "{name}");
        assert_eq!(error_of(&decoder.finish()), Some(refused.clone()), "{name}");
        let mut strict = IncrementalDecoder::new(options.with_strict(true)).expect("valid options");
        strict.feed(cut);
        assert!(
            matches!(
                strict.refine(),
                Ok(Refined::Waiting(Waiting::TileRefused { .. }))
            ),
            "{name}"
        );
        assert!(
            error_of(&strict.finish()).is_some_and(|e| e.starts_with("Codestream")),
            "{name}"
        );
    }
}
