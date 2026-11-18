//! Synthetic-bytes builders shared by the integration tests, the unit tests
//! and the fuzz targets. Std only, so a fuzz target can include it with
//! `#[path]` without pulling in the test dependencies.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// `tests/fixtures`, or a directory under it. It reads the including test
/// crate's `CARGO_MANIFEST_DIR`, so fuzz targets (whose manifest is `fuzz/`)
/// must not call it.
pub fn fixtures(sub: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    if sub.is_empty() { root } else { root.join(sub) }
}

/// The bytes of a file; a failure names the path.
pub fn read_file(path: impl AsRef<Path>) -> Vec<u8> {
    let path = path.as_ref();
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A marker segment: the marker code, `Lxxx` (the body length plus the two
/// length bytes), then the body.
pub fn seg(code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = code.to_be_bytes().to_vec();
    let length = u16::try_from(body.len() + 2).expect("marker segment body too long");
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// A JP2 box: `LBox` (the payload length plus the eight header bytes), the
/// four-byte type, then the payload.
pub fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let length = u32::try_from(8 + payload.len()).expect("box payload too long");
    let mut out = length.to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// The JP2 signature box.
pub const JP2_SIGNATURE: [u8; 12] = [0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];

/// The `ftyp` box of a plain JP2 file: brand `jp2 `, compatible with `jp2 `.
pub fn ftyp_jp2() -> Vec<u8> {
    bx(b"ftyp", b"jp2 \0\0\0\0jp2 ")
}

/// An `ihdr` payload: height, width, component count, then `BPC`, `C`,
/// `UnkC` and `IPR`.
pub fn ihdr_payload(height: u32, width: u32, components: u16, tail: [u8; 4]) -> Vec<u8> {
    let mut out = height.to_be_bytes().to_vec();
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&components.to_be_bytes());
    out.extend_from_slice(&tail);
    out
}

/// A `colr` payload with an enumerated color space.
pub fn colr_enum(enum_cs: u32) -> Vec<u8> {
    let mut out = vec![1, 0, 0];
    out.extend_from_slice(&enum_cs.to_be_bytes());
    out
}

/// A `colr` payload that carries an ICC `profile`.
pub fn colr_icc(profile: &[u8]) -> Vec<u8> {
    let mut out = vec![2, 0, 0];
    out.extend_from_slice(profile);
    out
}

/// A `cdef` box of (`Cn`, `Typ`, `Asoc`) entries.
pub fn cdef(entries: &[(u16, u16, u16)]) -> Vec<u8> {
    let count = u16::try_from(entries.len()).expect("too many cdef entries");
    let mut payload = count.to_be_bytes().to_vec();
    for &(cn, typ, asoc) in entries {
        for v in [cn, typ, asoc] {
            payload.extend_from_slice(&v.to_be_bytes());
        }
    }
    bx(b"cdef", &payload)
}

/// A JP2 file: the signature, `ftyp`, a `jp2h` of `header_boxes`, the
/// `between` boxes, a `jp2c` of `codestream`, then the `trailing` boxes.
pub fn jp2_file(
    header_boxes: &[Vec<u8>],
    between: &[Vec<u8>],
    codestream: &[u8],
    trailing: &[Vec<u8>],
) -> Vec<u8> {
    let mut out = JP2_SIGNATURE.to_vec();
    out.extend(ftyp_jp2());
    out.extend(bx(b"jp2h", &header_boxes.concat()));
    out.extend(between.concat());
    out.extend(bx(b"jp2c", codestream));
    out.extend(trailing.concat());
    out
}

/// A `SIZ` body: `Rsiz` 0, the eight grid fields (`Xsiz`, `Ysiz`, `XOsiz`,
/// `YOsiz`, `XTsiz`, `YTsiz`, `XTOsiz`, `YTOsiz`), `Csiz` and each
/// component's (`Ssiz`, `XRsiz`, `YRsiz`).
pub fn siz_body(grid: [u32; 8], components: &[[u8; 3]]) -> Vec<u8> {
    let mut out = 0u16.to_be_bytes().to_vec();
    for field in grid {
        out.extend_from_slice(&field.to_be_bytes());
    }
    let count = u16::try_from(components.len()).expect("too many components");
    out.extend_from_slice(&count.to_be_bytes());
    out.extend(components.concat());
    out
}

/// A `SOT` marker segment: tile `isot`, the tile-part length `psot` (counted
/// from the marker's first byte), part `tpsot` of `tnsot`.
pub fn sot(isot: u16, psot: u32, tpsot: u8, tnsot: u8) -> Vec<u8> {
    let mut body = isot.to_be_bytes().to_vec();
    body.extend_from_slice(&psot.to_be_bytes());
    body.extend_from_slice(&[tpsot, tnsot]);
    seg(0xFF90, &body)
}

/// A tile-part: `SOT` for tile `isot`, part `tpsot` of `tnsot`, then `extra`
/// (marker segments of its header), `SOD` and `data`. `Psot` covers it all.
pub fn tile_part(isot: u16, tpsot: u8, tnsot: u8, extra: &[u8], data: &[u8]) -> Vec<u8> {
    let psot = u32::try_from(12 + extra.len() + 2 + data.len()).expect("tile-part too long");
    let mut out = sot(isot, psot, tpsot, tnsot);
    out.extend_from_slice(extra);
    out.extend_from_slice(&[0xFF, 0x93]);
    out.extend_from_slice(data);
    out
}

/// A `COD` body: LRCP, no color transform, the 5/3 wavelet, no precinct
/// sizes, `layers` layers, `levels` decomposition levels, code-blocks of
/// `4 << block` samples square (`block` is the stored exponent minus 2) and
/// code-block style `style`.
pub fn cod_body(layers: u16, levels: u8, block: u8, style: u8) -> Vec<u8> {
    let mut out = vec![0, 0];
    out.extend_from_slice(&layers.to_be_bytes());
    out.extend_from_slice(&[0, levels, block, block, style, 1]);
    out
}

/// A `QCD` body with no quantization: `guard` guard bits, then one exponent
/// per subband.
pub fn qcd_none_body(guard: u8, exponents: &[u8]) -> Vec<u8> {
    let mut out = vec![guard << 5];
    out.extend(exponents.iter().map(|e| e << 3));
    out
}

/// Packet header bits, packed as B.10.1 has it: a byte after 0xFF carries
/// only 7 bits, and the last byte is padded with 0 bits (and followed by a
/// 0x00 if it is 0xFF).
#[derive(Default)]
pub struct Bits {
    out: Vec<u8>,
    byte: u8,
    used: u32,
    room: u32,
}

impl Bits {
    pub fn new() -> Self {
        Bits {
            room: 8,
            ..Bits::default()
        }
    }

    pub fn put(&mut self, bit: u8) {
        debug_assert!(bit <= 1, "a bit is 0 or 1");
        self.byte = (self.byte << 1) | bit;
        self.used += 1;
        if self.used == self.room {
            self.out.push(self.byte);
            self.room = if self.byte == 0xFF { 7 } else { 8 };
            self.byte = 0;
            self.used = 0;
        }
    }

    /// The low `width` bits of `value`, most significant first.
    pub fn put_int(&mut self, value: u32, width: u32) {
        for shift in (0..width).rev() {
            self.put(((value >> shift) & 1) as u8);
        }
    }

    pub fn finish(mut self) -> Vec<u8> {
        if self.used > 0 {
            self.out.push(self.byte << (self.room - self.used));
        } else if self.out.last() == Some(&0xFF) {
            self.out.push(0);
        }
        self.out
    }
}

/// A tag tree over a row of leaves (B.10.2), encoded as OpenJPEG's
/// `opj_tgt_encode` does.
pub struct TagTree {
    /// `(parent, value, low, known)` per node, the leaves first.
    nodes: Vec<(Option<usize>, u32, u32, bool)>,
}

impl TagTree {
    pub fn new(leaves: &[u32]) -> Self {
        let mut nodes: Vec<(Option<usize>, u32, u32, bool)> =
            leaves.iter().map(|&v| (None, v, 0, false)).collect();
        let (mut start, mut width) = (0, leaves.len());
        while width > 1 {
            let parents = width.div_ceil(2);
            let first = nodes.len();
            for p in 0..parents {
                let value = (2 * p..(2 * p + 2).min(width))
                    .map(|c| nodes[start + c].1)
                    .min()
                    .unwrap();
                nodes.push((None, value, 0, false));
                for c in 2 * p..(2 * p + 2).min(width) {
                    nodes[start + c].0 = Some(first + p);
                }
            }
            (start, width) = (first, parents);
        }
        TagTree { nodes }
    }

    pub fn encode(&mut self, bits: &mut Bits, leaf: usize, threshold: u32) {
        let mut path = vec![leaf];
        while let Some(parent) = self.nodes[*path.last().unwrap()].0 {
            path.push(parent);
        }
        let mut low = 0;
        for &at in path.iter().rev() {
            let node = &mut self.nodes[at];
            low = low.max(node.2);
            while low < threshold {
                if low >= node.1 {
                    if !node.3 {
                        bits.put(1);
                        node.3 = true;
                    }
                    break;
                }
                bits.put(0);
                low += 1;
            }
            node.2 = low;
        }
    }
}

/// A codestream of 256 components in a row of `head + 1` tiles of one
/// sample each, with no wavelet levels, in LRCP order. The main `COD` has
/// `layers` layers. Each of the first `head` tiles has a tile `COD` of one
/// layer and its 256 packets empty (`00`). The last tile uses the main
/// `COD` and has `zeros` bytes of `00`, which read as empty packets. The
/// walk of the last tile is large beside the samples of the image, so an
/// incremental decoder fed it in pieces may walk it again on each refine.
pub fn repeat_walk_tiles(head: u16, layers: u16, zeros: usize) -> Vec<u8> {
    const COMPONENTS: usize = 256;
    let tiles = u32::from(head) + 1;
    let mut cs = vec![0xFF, 0x4F];
    cs.extend(seg(
        0xFF51,
        &siz_body([tiles, 1, 0, 0, 1, 1, 0, 0], &[[7, 1, 1]; COMPONENTS]),
    ));
    cs.extend(seg(0xFF52, &cod_body(layers, 0, 0, 0)));
    cs.extend(seg(0xFF5C, &qcd_none_body(2, &[8])));
    let one_layer = seg(0xFF52, &cod_body(1, 0, 0, 0));
    for index in 0..head {
        cs.extend(tile_part(index, 0, 1, &one_layer, &[0; COMPONENTS]));
    }
    cs.extend(tile_part(head, 0, 1, &[], &vec![0; zeros]));
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

/// A codestream of one 32 x 32 tile of 20 components with `levels` wavelet
/// levels, code-blocks of 32 x 32 and 65535 LRCP layers, in `parts`
/// tile-parts whose data is all `00` (empty packets). Tile-part 0 has a
/// `POC` of one volume over every packet. Each later tile-part adds a `POC`
/// of one volume over one packet, and starts at twice the offset of the one
/// before it, from 256; the last holds `last` bytes of data. Each `POC` a
/// tile-part adds changes the tile's order of packets, so an incremental
/// decoder starts the tile's walk again.
pub fn restarted_poc_tile(levels: u8, parts: u8, last: usize) -> Vec<u8> {
    const COMPONENTS: u8 = 20;
    let mut cs = vec![0xFF, 0x4F];
    cs.extend(seg(
        0xFF51,
        &siz_body(
            [32, 32, 0, 0, 32, 32, 0, 0],
            &[[7, 1, 1]; COMPONENTS as usize],
        ),
    ));
    cs.extend(seg(0xFF52, &cod_body(65535, levels, 3, 0)));
    let exponents = vec![8; 3 * usize::from(levels) + 1];
    cs.extend(seg(0xFF5C, &qcd_none_body(2, &exponents)));
    // `RSpoc`, `CSpoc`, `LYEpoc`, `REpoc`, `CEpoc`, `Ppoc` (LRCP).
    let full = seg(0xFF5F, &[0, 0, 0xFF, 0xFF, levels + 1, COMPONENTS, 0]);
    let one = seg(0xFF5F, &[0, 0, 0, 1, 1, 1, 0]);
    for part in 0..parts {
        let extra = if part == 0 { &full } else { &one };
        // The header of a tile-part: `SOT`, its `POC` and `SOD`.
        let header = 12 + extra.len() + 2;
        let data = if part + 1 == parts {
            last
        } else {
            (256usize << part).saturating_sub(cs.len() + header)
        };
        cs.extend(tile_part(0, part, parts, extra, &vec![0; data]));
    }
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

// `ihdr_for` and `wrap_jp2_boxes` must stay the last items in this file:
// `tests/fuzz_seed_outcomes.rs` pins a hash of everything from here on.

/// An `ihdr` box matching a codestream's `SIZ`: the image area and `Csiz`,
/// with 8-bit unsigned samples (`BPC` 7), `C` 7, `UnkC` 0 and `IPR` 0. The
/// codestream must start with `SOC`, `SIZ`.
pub fn ihdr_for(codestream: &[u8]) -> Vec<u8> {
    let be32 = |at: usize| u32::from_be_bytes(codestream[at..at + 4].try_into().unwrap());
    let (xsiz, ysiz, x0, y0) = (be32(8), be32(12), be32(16), be32(20));
    let mut payload = (ysiz - y0).to_be_bytes().to_vec();
    payload.extend_from_slice(&(xsiz - x0).to_be_bytes());
    payload.extend_from_slice(&codestream[40..42]); // Csiz
    payload.extend_from_slice(&[7, 7, 0, 0]); // BPC, C, UnkC, IPR
    bx(b"ihdr", &payload)
}

/// The file the `jp2_boxes` fuzz target builds from `variant`, `body` and
/// the codestream the variant picks: bit 2 adds the signature and `ftyp`,
/// bit 3 (with it) a `jp2h` of a matching `ihdr` and `body`, and bits 0-1
/// pick the `jp2c` form (32-bit `LBox`, 64-bit `XLBox`, `LBox` 0, or a
/// `body` after it).
pub fn wrap_jp2_boxes(variant: u8, codestream: &[u8], body: &[u8]) -> Vec<u8> {
    let mut file = Vec::new();
    if variant & 0x04 != 0 {
        file.extend_from_slice(&JP2_SIGNATURE);
        file.extend_from_slice(&ftyp_jp2());
        if variant & 0x08 != 0 {
            let mut children = ihdr_for(codestream);
            children.extend_from_slice(body);
            file.extend_from_slice(&bx(b"jp2h", &children));
        } else {
            file.extend_from_slice(body);
        }
    } else {
        file.extend_from_slice(body);
    }
    match variant & 0x03 {
        0 => file.extend_from_slice(&bx(b"jp2c", codestream)),
        1 => {
            file.extend_from_slice(&1u32.to_be_bytes());
            file.extend_from_slice(b"jp2c");
            file.extend_from_slice(&(16 + codestream.len() as u64).to_be_bytes());
            file.extend_from_slice(codestream);
        }
        2 => {
            file.extend_from_slice(&0u32.to_be_bytes());
            file.extend_from_slice(b"jp2c");
            file.extend_from_slice(codestream);
        }
        _ => {
            file.extend_from_slice(&bx(b"jp2c", codestream));
            file.extend_from_slice(body);
        }
    }
    file
}
