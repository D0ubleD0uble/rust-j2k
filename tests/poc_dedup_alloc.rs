//! A `POC` marker whose volumes reach the same packets again: the tile's
//! packets are read once and the repeats skipped, in time and memory that do
//! not grow with the repeats. The file is `pocdup(8, 8, 1, 65535, 31)` from
//! `scripts/gen-work-bound-fixtures.py`: 16 precincts, 65535 LRCP layers of
//! empty packets (1 MB) and 31 identical volumes over all of them.
//! OpenJPEG 2.5.0 decodes it to a flat 128 in 0.1 s and 9 MB.
//!
//! The peak is measured with a counting global allocator, so this file holds a
//! single test: a second one running in parallel would count into the same
//! peak.
//!
//! A global allocator can only be written with `unsafe`. This one is a shim in
//! a test binary that forwards every call to `System` unchanged, which the
//! project conventions allow (`.claude/rules/conventions.md`, "Stay
//! pure-Rust"). The library itself has no `unsafe`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call forwards to `System` unchanged; the counters only
// observe the sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let now = CURRENT.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
        PEAK.fetch_max(now, Ordering::SeqCst);
        // SAFETY: the caller's contract for `alloc` is passed through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Ordering::SeqCst);
        // SAFETY: the caller's contract for `dealloc` is passed through.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const LAYERS: u16 = u16::MAX;
const VOLUMES: usize = 31;

/// A marker segment: the code, the length, the body.
fn seg(code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = code.to_be_bytes().to_vec();
    out.extend_from_slice(&(2 + body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// `pocdup(8, 8, 1, 65535, 31)`: one 8 x 8 tile, no wavelet levels, 2 x 2
/// precincts, LRCP, and `VOLUMES` POC volumes over every layer, resolution
/// and component. Every packet is empty (0x00). With `psot0`, the tile-part's
/// `Psot` is 0 and there is no EOC, as in a JP2 file whose codestream box
/// ends where the packets end.
fn pocdup(psot0: bool) -> Vec<u8> {
    let mut siz = 0u16.to_be_bytes().to_vec();
    for v in [8u32, 8, 0, 0, 8, 8, 0, 0] {
        siz.extend_from_slice(&v.to_be_bytes());
    }
    siz.extend_from_slice(&[0, 1, 7, 1, 1]);
    let mut cod = vec![1, 0];
    cod.extend_from_slice(&LAYERS.to_be_bytes());
    cod.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0x11]);
    // RSpoc, CSpoc, LYEpoc, REpoc, CEpoc, Ppoc.
    let mut volume = vec![0, 0];
    volume.extend_from_slice(&LAYERS.to_be_bytes());
    volume.extend_from_slice(&[1, 1, 0]);
    let data = vec![0u8; 16 * usize::from(LAYERS)];
    let psot = if psot0 { 0 } else { 14 + data.len() as u32 };

    let mut cs = vec![0xFF, 0x4F];
    cs.extend(seg(0xFF51, &siz));
    cs.extend(seg(0xFF52, &cod));
    cs.extend(seg(0xFF5C, &[0x40, 0x40]));
    cs.extend(seg(0xFF5F, &volume.repeat(VOLUMES)));
    cs.extend_from_slice(&[0xFF, 0x90, 0, 10, 0, 0]);
    cs.extend_from_slice(&psot.to_be_bytes());
    cs.extend_from_slice(&[0, 1, 0xFF, 0x93]);
    cs.extend(data);
    if !psot0 {
        cs.extend_from_slice(&[0xFF, 0xD9]);
    }
    cs
}

/// The `Psot = 0` codestream in a JP2 file, in a `jp2c` box that ends where
/// its packets end, then an empty `free` box: `read_header` walks the packets
/// to see that the codestream ends at the box's end.
fn in_jp2() -> Vec<u8> {
    let bx = |kind: &[u8; 4], payload: &[u8]| {
        let mut out = (8 + payload.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    };
    let ihdr = [0, 0, 0, 8, 0, 0, 0, 8, 0, 1, 7, 7, 0, 0];
    let jp2h = [bx(b"ihdr", &ihdr), bx(b"colr", &[1, 0, 0, 0, 0, 0, 17])].concat();
    [
        vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A],
        bx(b"ftyp", b"jp2 \0\0\0\0jp2 "),
        bx(b"jp2h", &jp2h),
        bx(b"jp2c", &pocdup(true)),
        bx(b"free", b""),
    ]
    .concat()
}

/// `pocdup(512, 512, 1, 65535, 2, 4096, RPCL)`: one 512 x 512 tile, no
/// wavelet levels, 2 x 2 precincts (65536 of them), 65535 layers and two
/// RPCL POC volumes over all of them, so the volumes reach 4.3e9 packets, a
/// bitmap of 537 MB. Only the first 4096 packets are in the data (0x00
/// each). Past the bound's 2^24 the decode holds the packets read in a set.
fn pocwide() -> Vec<u8> {
    let mut siz = 0u16.to_be_bytes().to_vec();
    for v in [512u32, 512, 0, 0, 512, 512, 0, 0] {
        siz.extend_from_slice(&v.to_be_bytes());
    }
    siz.extend_from_slice(&[0, 1, 7, 1, 1]);
    let mut cod = vec![1, 2];
    cod.extend_from_slice(&LAYERS.to_be_bytes());
    cod.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0x11]);
    let mut volume = vec![0, 0];
    volume.extend_from_slice(&LAYERS.to_be_bytes());
    volume.extend_from_slice(&[1, 1, 2]);
    let data = vec![0u8; 4096];
    let mut cs = vec![0xFF, 0x4F];
    cs.extend(seg(0xFF51, &siz));
    cs.extend(seg(0xFF52, &cod));
    cs.extend(seg(0xFF5C, &[0x40, 0x40]));
    cs.extend(seg(0xFF5F, &volume.repeat(2)));
    cs.extend_from_slice(&[0xFF, 0x90, 0, 10, 0, 0]);
    cs.extend_from_slice(&(14 + data.len() as u32).to_be_bytes());
    cs.extend_from_slice(&[0, 1, 0xFF, 0x93]);
    cs.extend(data);
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

/// The heap `run` peaks at beyond what was held before it.
fn peak_of<T>(run: impl FnOnce() -> T) -> (T, usize) {
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let out = run();
    (out, PEAK.load(Ordering::SeqCst) - before)
}

/// 1 M packets, each met by 31 volumes, take under 1 MB of heap beside the
/// 1 MB of tile data, in `decode` and in `read_header`'s walk. A set of the
/// packets read took about 100 MB. The decode is the flat 128 OpenJPEG
/// 2.5.0 decodes.
#[test]
fn repeated_poc_volumes_decode_in_a_small_heap() {
    const LIMIT: usize = 1 << 20;
    let codestream = pocdup(false);
    let file = in_jp2();
    for (what, bytes) in [("codestream", &codestream), ("JP2 file", &file)] {
        let (image, peak) = peak_of(|| rust_j2k::decode(bytes));
        let image = image.unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(image.components.len(), 1, "{what}");
        let samples = &image.components[0].samples;
        assert_eq!(samples.len(), 64, "{what}");
        assert!(samples.iter().all(|&s| s == 128), "{what}: not flat 128");
        eprintln!("{what}: decode peaked at {peak} bytes");
        assert!(peak < LIMIT, "{what}: decode peaked at {peak} bytes");
    }
    let (header, peak) = peak_of(|| rust_j2k::read_header(&file));
    let header = header.unwrap_or_else(|e| panic!("read_header: {e}"));
    assert_eq!((header.width, header.height), (8, 8));
    eprintln!("read_header peaked at {peak} bytes");
    assert!(peak < LIMIT, "read_header peaked at {peak} bytes");

    // Lifting the packet work limit does not lift the bitmap's cap: volumes
    // that reach more packets than the default bound are still held in a
    // set of the packets read, not a 537 MB bitmap: about 15 MB, the
    // geometry of 65536 precincts.
    let wide = pocwide();
    let lifted =
        rust_j2k::DecodeOptions::default().with_packet_work_limit(rust_j2k::WorkLimit::Unlimited);
    for (what, options) in [
        ("default", rust_j2k::DecodeOptions::default()),
        ("lifted", lifted),
    ] {
        let (result, peak) = peak_of(|| rust_j2k::decode_with(&wide, &options));
        eprintln!(
            "pocwide, {what}: decode peaked at {peak} bytes, {:?}",
            result.err()
        );
        assert!(
            peak < 64 * LIMIT,
            "pocwide, {what}: decode peaked at {peak} bytes"
        );
    }
}
