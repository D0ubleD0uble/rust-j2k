//! With `strict` off, an error met in the tiles is final only once the input
//! closes.
//!
//! The decode parses every tile-part before it decodes any tile, so a fault
//! in a later tile-part comes before a tile the reduction or a tile cap
//! refuses, and before a damaged packet header. On a prefix that ends before
//! that tile-part, `decode_with` gives the tile's error; on a longer one,
//! the fault. So an [`IncrementalDecoder`] waits on such an error, as
//! [`Waiting::Held`], and its `header` holds a tile refusal back, until
//! the bytes fed reach the `EOC`. A JP2 file never closes, as boxes after
//! the codestream are checked first. Fed in any two pieces or a byte at a
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

/// A JP2 file never closes: boxes after the codestream are checked before
/// the tile-parts, so a malformed one, or a `jp2h` with no `ihdr`, replaces
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
