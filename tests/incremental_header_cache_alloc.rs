//! `IncrementalDecoder::header` keeps nothing between calls for a file whose
//! tiles' sizes show them within the caps, whatever its main header holds,
//! only the counts of a file whose tiles' sizes do not, and the decoder
//! itself stays small.
//!
//! The file is the worst main header for the parsed form: one tile of
//! 64 x 64 samples and 256 `PPM` markers of about 64 KiB each, 16.8 MB in
//! all, holding about 4.19 million `Nppm` records of no packet headers.
//! Parsed, each record is a `Vec` of 24 bytes, so a copy of the parsed
//! header would be 100 MB, six times the input.
//!
//! While the bytes fed end inside the main header, a call keeps what the
//! segments read so far set, so the next goes on from there. That is measured
//! on main headers of 16384 components, each with segments that override its
//! coding: a `QCC` of one, two or 97 steps, a `COC`, an `RGN`, or a `COC` of
//! precincts, a 97-step `QCC` and an `RGN`. Of these shapes, a one-step `QCC`
//! keeps the most for each of its bytes, about 12; the last keeps the most a
//! component can, 12.2 MB in all. A one-step `QCC` on each of 129 components,
//! whose indexes are one byte, keeps 19.6 bytes for each byte. The tables that
//! hold the overrides grow by doubling: at 16384, a power of two, they have no
//! spare room, and at 129 nearly half is spare. It is also measured on 65000
//! `PPM` segments of one record each. Each header is cut inside a `COM` after
//! its segments, so all of them are read.
//!
//! The bytes are counted with a counting global allocator, so this file
//! holds a single test: a second one running in parallel would count into
//! the same totals. The shim is in `tests/support/alloc.rs`.

#[path = "support/alloc.rs"]
mod alloc;
#[path = "support/synth.rs"]
mod synth;
use alloc::CURRENT;
use std::sync::atomic::Ordering;

use rust_j2k::{DecodeOptions, Header, IncrementalDecoder, read_header_with};

#[global_allocator]
static ALLOCATOR: alloc::PeakAlloc = alloc::PeakAlloc;

fn ppm_file() -> Vec<u8> {
    let siz = synth::siz_body([64, 64, 0, 0, 64, 64, 0, 0], &[[7, 1, 1]]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0)),
        synth::seg(0xFF5C, &synth::qcd_none_body(0, &[8])),
    ]
    .concat();
    for index in 0..=255u8 {
        // `Zppm`, then records of an `Nppm` of 0 and no bytes: 16,382 whole
        // records, as a segment may not end inside an `Nppm` (A.7.4).
        let mut body = vec![index];
        body.resize(1 + 4 * 16_382, 0);
        bytes.extend(synth::seg(0xFF60, &body));
    }
    bytes.extend(synth::tile_part(0, 0, 1, &[], &[]));
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// One tile of 1024 x 1024 samples and no wavelet levels: its size does not
/// show it within the caps, so its counts under the main header are kept.
fn big_tile_file() -> Vec<u8> {
    let siz = synth::siz_body([1024, 1024, 0, 0, 1024, 1024, 0, 0], &[[7, 1, 1]]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0)),
        synth::seg(0xFF5C, &synth::qcd_none_body(0, &[8])),
    ]
    .concat();
    bytes.extend(synth::tile_part(0, 0, 1, &[], &[]));
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// A main header of `components` components, each with the segments
/// `overrides` gives for it, then a `COM` the input ends inside. With the
/// bytes of the segments `overrides` gave.
fn cut_component_header(
    components: u16,
    overrides: impl Fn(u16) -> Vec<Vec<u8>>,
) -> (Vec<u8>, usize) {
    let siz = synth::siz_body(
        [64, 64, 0, 0, 64, 64, 0, 0],
        &vec![[7, 1, 1]; usize::from(components)],
    );
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0)),
        synth::seg(0xFF5C, &synth::qcd_none_body(0, &[8])),
    ]
    .concat();
    let before = bytes.len();
    for component in 0..components {
        bytes.extend(overrides(component).concat());
    }
    let segments = bytes.len() - before;
    bytes.extend(synth::seg(0xFF64, &[0, 1, b'x', b'x']));
    bytes.truncate(bytes.len() - 2);
    (bytes, segments)
}

/// The segments that override a component's coding, for each component.
type Overrides = Box<dyn Fn(u16) -> Vec<Vec<u8>>>;

/// The `COC`, `QCC` and `RGN` shapes [`cut_component_header`] is measured
/// on, by name.
fn component_shapes() -> Vec<(&'static str, Overrides)> {
    let index = |component: u16| component.to_be_bytes().to_vec();
    // `Sqcc` and one exponent for the one subband of no levels.
    let one_step = move |c: u16| [index(c), vec![1 << 5, 8 << 3]].concat();
    // Scalar derived: one 16-bit step.
    let derived = move |c: u16| [index(c), vec![(1 << 5) | 1, 0x40, 0]].concat();
    // Scalar expounded: a 16-bit step for each of 97 subbands.
    let expounded = move |c: u16| -> Vec<u8> {
        let steps = (0..97).flat_map(|_| (8u16 << 11).to_be_bytes());
        [index(c), vec![(1 << 5) | 2]]
            .into_iter()
            .flatten()
            .chain(steps)
            .collect()
    };
    let coc = move |c: u16, levels: u8| [index(c), vec![0, levels, 0, 0, 0, 1]].concat();
    // `Scoc` of user precincts: a size for each of 33 resolutions.
    let precincts = move |c: u16| [index(c), vec![1, 32, 0, 0, 0, 1], vec![0x77; 33]].concat();
    let rgn = move |c: u16| [index(c), vec![0, 4]].concat();
    vec![
        (
            "one-step QCC",
            Box::new(move |c| vec![synth::seg(0xFF5D, &one_step(c))]),
        ),
        (
            "derived QCC",
            Box::new(move |c| vec![synth::seg(0xFF5D, &derived(c))]),
        ),
        (
            "97-step QCC",
            Box::new(move |c| vec![synth::seg(0xFF5D, &expounded(c))]),
        ),
        (
            "COC",
            Box::new(move |c| vec![synth::seg(0xFF53, &coc(c, 0))]),
        ),
        ("RGN", Box::new(move |c| vec![synth::seg(0xFF5E, &rgn(c))])),
        (
            "COC of precincts, 97-step QCC and RGN",
            Box::new(move |c| {
                vec![
                    synth::seg(0xFF53, &precincts(c)),
                    synth::seg(0xFF5D, &expounded(c)),
                    synth::seg(0xFF5E, &rgn(c)),
                ]
            }),
        ),
    ]
}

/// A main header of 65000 `PPM` segments of one empty record each, cut
/// inside the last.
fn cut_ppm_header() -> Vec<u8> {
    let mut bytes = ppm_file();
    let first = bytes.windows(2).position(|code| code == [0xFF, 0x60]);
    bytes.truncate(first.expect("a PPM"));
    for index in 0..65_000u32 {
        // `Zppm` counts up and wraps; the order is checked once the header
        // is whole.
        bytes.extend(synth::seg(0xFF60, &[index as u8, 0, 0, 0, 0]));
    }
    bytes.truncate(bytes.len() - 2);
    bytes
}

/// The heap `decoder` holds after a call of `header()` that finds the main
/// header cut short, over what it held before.
fn held_while_cut(decoder: &IncrementalDecoder) -> usize {
    let base = CURRENT.load(Ordering::SeqCst);
    assert!(decoder.header().expect("cut short").is_none());
    CURRENT.load(Ordering::SeqCst).saturating_sub(base)
}

/// A decoder whose `header_kept` mutex has been locked once, with nothing fed.
/// Where std's `Mutex` is pthread-based (macOS, for one), it boxes its
/// `pthread_mutex_t` on the first lock (64 bytes on aarch64 macOS), which is
/// not the cache's, so it is taken before the heap is measured.
fn warm_decoder(options: DecodeOptions) -> IncrementalDecoder {
    let decoder = IncrementalDecoder::new(options).unwrap();
    assert!(decoder.header().expect("no bytes, no error").is_none());
    decoder
}

/// The heap `decoder` holds after each of 3 calls of `header()`, over what
/// it held before the first, with each header checked against `want`.
fn held_after_calls(decoder: &IncrementalDecoder, want: &Header) -> [usize; 3] {
    let base = CURRENT.load(Ordering::SeqCst);
    let mut helds = [0; 3];
    for (call, slot) in helds.iter_mut().enumerate() {
        let got = decoder.header().expect("a header").expect("a header");
        assert_eq!(&got, want, "call {call}");
        // The header returned is heap too.
        drop(got);
        *slot = CURRENT.load(Ordering::SeqCst).saturating_sub(base);
    }
    helds
}

#[test]
fn header_holds_what_the_files_tiles_need() {
    // The decoder was 448 bytes before the cache, and 896 with it inline.
    let size = size_of::<IncrementalDecoder>();
    assert!(size <= 512, "{size} bytes");
    let lenient = DecodeOptions::default().with_strict(false);

    // Tiles whose sizes show them within the caps: nothing is kept, though
    // the main header is 16.8 MB of `PPM` data.
    let bytes = ppm_file();
    assert!(bytes.len() > 16_000_000, "{} bytes", bytes.len());
    let want = read_header_with(&bytes, &lenient).expect("a header");
    let mut decoder = warm_decoder(lenient.clone());
    decoder.feed(&bytes);
    let none = held_after_calls(&decoder, &want);
    drop(decoder);

    // A tile that is not: its counts are kept, a few hundred bytes.
    let bytes = big_tile_file();
    let want = read_header_with(&bytes, &lenient).expect("a header");
    let mut decoder = warm_decoder(lenient);
    decoder.feed(&bytes);
    let counts = held_after_calls(&decoder, &want);
    drop(decoder);

    // Main headers cut short: what their segments set is kept.
    let held_for = |bytes: &[u8]| {
        let mut decoder = warm_decoder(DecodeOptions::default().with_strict(false));
        decoder.feed(bytes);
        held_while_cut(&decoder)
    };
    let mut cut = Vec::new();
    for (name, overrides) in component_shapes() {
        let (bytes, segments) = cut_component_header(16384, overrides);
        cut.push((name, bytes.len(), segments, held_for(&bytes)));
    }
    // A one-step `QCC` on each of 129 components, with a one-byte index.
    let (bytes, narrow_segments) = cut_component_header(129, |c| {
        vec![synth::seg(0xFF5D, &[c as u8, 1 << 5, 8 << 3])]
    });
    let narrow = held_for(&bytes);
    let ppm = held_for(&cut_ppm_header());

    // Printed after the measures: the captured output is heap too.
    println!(
        "size {size}; held {none:?} (PPM file), {counts:?} (big tile), \
         (name, header bytes, segment bytes, held) {cut:?}, \
         {narrow} for {narrow_segments} (129 one-step QCC), {ppm} (65000 PPM)"
    );
    assert_eq!(none, [0; 3]);
    for held in counts {
        assert!((1..=4096).contains(&held), "{held} bytes held");
    }
    // Up to about 12 bytes for each byte of these segments, whose component
    // indexes are two bytes, at least 2 for what is kept to count at all, and
    // about 24 bytes a `PPM` segment. The 13x bound covers only these
    // 16384-component shapes, whose tables have no spare capacity.
    for (name, _, segments, held) in cut {
        assert!(
            (2 * segments..=13 * segments).contains(&held),
            "{name}: {held} bytes held for {segments}"
        );
        // The most a component can hold, 16384 times: about 12 MB.
        assert!(held <= 13_000_000, "{name}: {held} bytes held");
    }
    // With a one-byte index and the tables' spare capacity, a one-step `QCC`
    // keeps about 20 bytes for each byte, the figure the docs give (19.6
    // measured; 17.0 at 257 components and 16.8 at 4097).
    assert!(
        (2 * narrow_segments..=20 * narrow_segments).contains(&narrow),
        "{narrow} bytes held for {narrow_segments}"
    );
    assert!(
        (16 * 65_000..=32 * 65_000).contains(&ppm),
        "{ppm} bytes held"
    );
}
