//! An `IncrementalDecoder` fed a short file that declares a large image
//! holds, between calls, what its rustdoc says: the packet walks it keeps
//! hold at most the larger of 1 MiB and 32 bytes per byte fed, beside the
//! bytes fed (in a `Vec`, so up to twice their length).
//!
//! The file is 1228 bytes: one component of 2048 x 2048 samples in 64
//! tiles of 256 x 256, 3 wavelet levels, one layer and code-blocks of
//! 4 x 4 samples, every packet empty (`00`). Each tile's walk holds the
//! state of its 4096 code-blocks, about 10 MB for all of them, which the
//! room of twice the image's 16 MB would keep.
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

/// The image's width and height.
const SIZE: u32 = 2048;

/// The tiles' width and height.
const TILE: u32 = 256;

/// The wavelet levels: each tile has `LEVELS + 1` packets.
const LEVELS: u8 = 3;

/// The walks the decoder may keep with `fed` bytes fed, as its rustdoc
/// states it.
fn room(fed: usize) -> usize {
    (1 << 20).max(32 * fed)
}

fn file() -> Vec<u8> {
    let siz = synth::siz_body([SIZE, SIZE, 0, 0, TILE, TILE, 0, 0], &[[7, 1, 1]]);
    let mut cs = vec![0xFF, 0x4F];
    cs.extend(seg(0xFF51, &siz));
    // Code-blocks of 4 x 4 samples.
    cs.extend(seg(0xFF52, &synth::cod_body(1, LEVELS, 0, 0)));
    cs.extend(seg(
        0xFF5C,
        &synth::qcd_none_body(2, &vec![8; 3 * usize::from(LEVELS) + 1]),
    ));
    let tiles = (SIZE / TILE) * (SIZE / TILE);
    for index in 0..tiles {
        let index = u16::try_from(index).unwrap();
        cs.extend(synth::tile_part(
            index,
            0,
            1,
            &[],
            &vec![0; usize::from(LEVELS) + 1],
        ));
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

/// Feed `bytes` in `pieces` pieces, refining after each: each image is the
/// strict-off decode of the bytes fed, and the most the decoder holds
/// between calls, which it returns, is within the room plus the bytes fed.
fn held(bytes: &[u8], pieces: usize) -> usize {
    let lenient = DecodeOptions::default().with_strict(false);
    let base = reset();
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    let mut most = 0;
    let mut images = 0;
    for piece in bytes.chunks(bytes.len().div_ceil(pieces)) {
        decoder.feed(piece);
        let refined = decoder.refine().unwrap();
        let fed = decoder.bytes_fed();
        if let Some(image) = refined {
            images += 1;
            assert_eq!(image, decode_with(&bytes[..fed], &lenient).unwrap());
        }
        let between = CURRENT.load(Ordering::SeqCst) - base;
        most = most.max(between);
        assert!(
            between <= room(fed) + 2 * fed,
            "{pieces} pieces: {between} bytes held with {fed} fed"
        );
    }
    assert!(images > 0, "{pieces} pieces");
    most
}

#[test]
fn a_short_file_of_a_large_image_holds_little_between_calls() {
    let bytes = file();
    assert_eq!(bytes.len(), 1228);
    for pieces in [1, 4] {
        let most = held(&bytes, pieces);
        println!("{pieces} pieces: at most {most} bytes held between calls");
    }
}
