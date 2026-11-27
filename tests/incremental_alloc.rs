//! An `IncrementalDecoder` fed a file in pieces holds what its rustdoc
//! says: the walks it keeps between calls hold at most twice the image's
//! samples plus twice the walk of the largest tile walked so far, and during
//! a refine it holds one decode's peak plus those walks, plus up to twice
//! the bytes fed (in a `Vec`, so up to twice their length) in both cases.
//!
//! The files have many tiles of one sample each and many components, so a
//! tile's packet walk is large beside the samples it adds to the image, and
//! the walks fill the room the decoder keeps them in. One is in LRCP order;
//! the other has 32 PCRL `POC` volumes, whose order holds a list of the
//! (component, resolution) pairs of each volume.
//!
//! The bytes are counted with a counting global allocator, so this file
//! holds a single test: a second one running in parallel would count into
//! the same totals.
//!
//! A global allocator can only be written with `unsafe`. The shim in
//! `tests/support/alloc.rs` forwards every call to `System` unchanged, which
//! the project conventions allow (`.claude/rules/conventions.md`, "Stay
//! pure-Rust"). The library itself has no `unsafe`.

#[path = "support/alloc.rs"]
mod alloc;
#[path = "support/synth.rs"]
mod synth;
use alloc::{CURRENT, PEAK};
use std::sync::atomic::Ordering;
use synth::seg;

use rust_j2k::{DecodeOptions, IncrementalDecoder, decode_with};

#[global_allocator]
static ALLOCATOR: alloc::PeakAlloc = alloc::PeakAlloc;

/// The components of each file. Past 256, a `POC` volume's component
/// fields take two bytes.
const COMPONENTS: u16 = 512;

/// The tiles of each file.
const TILES: u32 = 128;

/// The size of each piece fed.
const PIECE: usize = 8 << 10;

/// `TILES` tiles of one sample each in a row, of `COMPONENTS` 8-bit
/// components with no wavelet levels and two layers, each packet empty
/// (`00`). LRCP when `volumes` is 0; else PCRL, with a `POC` marker of
/// `volumes` volumes that each cover every layer, resolution and
/// component.
fn file(volumes: u8) -> Vec<u8> {
    let siz = synth::siz_body(
        [TILES, 1, 0, 0, 1, 1, 0, 0],
        &vec![[7, 1, 1]; usize::from(COMPONENTS)],
    );
    let progression = if volumes > 0 { 3 } else { 0 };
    let mut cs = vec![0xFF, 0x4F];
    cs.extend(seg(0xFF51, &siz));
    cs.extend(seg(0xFF52, &[0, progression, 0, 2, 0, 0, 4, 4, 0, 1]));
    cs.extend(seg(0xFF5C, &[0x40, 0x40]));
    if volumes > 0 {
        let mut body = Vec::new();
        for _ in 0..volumes {
            // RSpoc, CSpoc, LYEpoc, REpoc, CEpoc, Ppoc.
            body.push(0);
            body.extend_from_slice(&0u16.to_be_bytes());
            body.extend_from_slice(&2u16.to_be_bytes());
            body.push(1);
            body.extend_from_slice(&COMPONENTS.to_be_bytes());
            body.push(3);
        }
        cs.extend(seg(0xFF5F, &body));
    }
    let packets = 2 * u32::from(COMPONENTS);
    for index in 0..TILES {
        cs.extend_from_slice(&[0xFF, 0x90, 0, 10]);
        cs.extend_from_slice(&u16::try_from(index).unwrap().to_be_bytes());
        cs.extend_from_slice(&(14 + packets).to_be_bytes());
        cs.extend_from_slice(&[0, 1, 0xFF, 0x93]);
        cs.extend(std::iter::repeat_n(0u8, packets as usize));
    }
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

/// Start counting a new peak from the bytes held now, which it returns.
fn reset() -> usize {
    let now = CURRENT.load(Ordering::SeqCst);
    PEAK.store(now, Ordering::SeqCst);
    now
}

/// What feeding `bytes` in pieces holds, in bytes: the most one decode of a
/// prefix a refine decoded holds at its peak, the most a refine holds at
/// its peak, and the most the decoder holds between calls.
struct Held {
    one_shot: usize,
    refine: usize,
    between: usize,
}

fn feed(bytes: &[u8]) -> Held {
    let lenient = DecodeOptions::default().with_strict(false);
    let base = reset();
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    let mut held = Held {
        one_shot: 0,
        refine: 0,
        between: 0,
    };
    let mut images = 0;
    for piece in bytes.chunks(PIECE) {
        decoder.feed(piece);
        reset();
        let refined = decoder.refine().unwrap().image();
        held.refine = held.refine.max(PEAK.load(Ordering::SeqCst) - base);
        if let Some(image) = refined {
            images += 1;
            drop(image);
            let prefix = &bytes[..decoder.bytes_fed()];
            let before = reset();
            drop(decode_with(prefix, &lenient).unwrap());
            held.one_shot = held.one_shot.max(PEAK.load(Ordering::SeqCst) - before);
        }
        held.between = held.between.max(CURRENT.load(Ordering::SeqCst) - base);
    }
    assert!(images > 3, "{images} images");
    held
}

#[test]
fn a_decoder_fed_in_pieces_holds_what_its_docs_say() {
    for (volumes, name) in [(0, "LRCP"), (32, "32 PCRL `POC` volumes")] {
        let bytes = file(volumes);
        let fed = 2 * bytes.len();
        let held = feed(&bytes);
        let Held {
            one_shot,
            refine,
            between,
        } = held;
        let report = format!(
            "{name}: one decode {one_shot}, a refine {refine}, between calls {between}, \
             {} bytes fed",
            bytes.len()
        );
        println!("{report}");
        assert!(between <= 2 * one_shot + fed, "{report}");
        assert!(refine <= 3 * one_shot + fed, "{report}");
        // The walks fill their room, or the bounds above would hold for
        // a decoder that keeps nothing.
        assert!(between >= one_shot + fed, "{report}");
    }
}
