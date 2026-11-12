//! Checking a codestream's tile headers costs `tiles + components`, not
//! `tiles × components`: 4096 tiles over 16384 components are checked with
//! about as many allocations as 4096 tiles over one, where resolving a
//! header per tile made 4096 × 16384 of them.
//!
//! Allocations are counted by a counting global allocator, so this file holds
//! a single test: a second one running in parallel would count into the same
//! total.
//!
//! A global allocator can only be written with `unsafe`. This one is a shim in
//! a test binary that forwards every call to `System` unchanged, which the
//! project conventions allow (`.claude/rules/conventions.md`, "Stay
//! pure-Rust"). The library itself has no `unsafe`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use rust_j2k::{Error, decode};

struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call forwards to `System` unchanged; the counter only
// observes the calls.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: the caller's contract for `alloc` is passed through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract for `dealloc` is passed through.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The tile grid's side: `SIDE^2` = 4096 tiles.
const SIDE: u32 = 64;

/// A marker segment: the code, the length, the body.
fn seg(code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = code.to_be_bytes().to_vec();
    out.extend_from_slice(&(2 + body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// COD for LRCP, one layer, `levels` levels, 4x4 code-blocks, 5/3.
fn cod(levels: u8) -> Vec<u8> {
    seg(0xFF52, &[0, 0, 0, 1, 0, levels, 0, 0, 0, 1])
}

/// `SIDE^2` 1x1 tiles on the image `[1, SIDE + 1)^2` (`tilecomp` in
/// `scripts/gen-tile-header-fixtures.py`): component 0 full size, the other
/// `components - 1` subsampled 255 times both ways. Every tile but the last
/// carries a COD restating the main header's when `tile_cod` is set; the last
/// carries a COD of one level, which the main QCD's one step cannot
/// quantize, so the tile headers are refused, at the last one.
fn tiles(components: u16, tile_cod: bool) -> Vec<u8> {
    let mut siz = 0u16.to_be_bytes().to_vec();
    for v in [SIDE + 1, SIDE + 1, 1, 1, 1, 1, 1, 1] {
        siz.extend_from_slice(&v.to_be_bytes());
    }
    siz.extend_from_slice(&components.to_be_bytes());
    siz.extend_from_slice(&[7, 1, 1]);
    for _ in 1..components {
        siz.extend_from_slice(&[7, 255, 255]);
    }
    let mut cs = vec![0xFF, 0x4F];
    cs.extend(seg(0xFF51, &siz));
    cs.extend(cod(0));
    cs.extend(seg(0xFF5C, &[0x40, 0x40]));
    let count = SIDE * SIDE;
    for index in 0..count {
        let header = if index == count - 1 {
            cod(1)
        } else if tile_cod {
            cod(0)
        } else {
            Vec::new()
        };
        let psot = 12 + header.len() as u32 + 2 + 1;
        cs.extend_from_slice(&[0xFF, 0x90, 0, 10]);
        cs.extend_from_slice(&(index as u16).to_be_bytes());
        cs.extend_from_slice(&psot.to_be_bytes());
        cs.extend_from_slice(&[0, 1]);
        cs.extend(header);
        cs.extend_from_slice(&[0xFF, 0x93, 0x00]);
    }
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

/// The allocations `decode` makes on `bytes`, which must be refused at the
/// last tile's header.
fn allocations(bytes: &[u8]) -> usize {
    let before = ALLOCATIONS.load(Ordering::SeqCst);
    let result = decode(bytes);
    let made = ALLOCATIONS.load(Ordering::SeqCst) - before;
    match result {
        Err(Error::Marker(m)) => assert!(
            m.contains("1 decomposition levels need 4"),
            "refused for another reason: {m}"
        ),
        other => panic!("expected the last tile's COD refused, got {other:?}"),
    }
    made
}

/// 4096 tiles over 16384 components against 4096 tiles over one, with no
/// marker in the tiles and with a COD in every one, which lands on every
/// component. The extra components may cost a few allocations each, once,
/// not once per tile: before, each tile's header was a copy of the main
/// header, 4096 x 16384 = 6.7 x 10^7 allocations.
#[test]
fn tile_headers_cost_tiles_plus_components() {
    for tile_cod in [false, true] {
        let one = allocations(&tiles(1, tile_cod));
        let many = allocations(&tiles(16384, tile_cod));
        let per_component = (many - one) as f64 / 16383.0;
        assert!(
            per_component < 8.0,
            "tile COD {tile_cod}: {one} allocations for one component, {many} for 16384, \
             {per_component:.1} per extra component"
        );
    }
}
