//! The decode's tier-1 work bound, and the [`WorkLimit`] options that raise
//! or lift it and the tier-2 one.
//!
//! A code-block's packet header can ask for 88 coding passes in about four
//! bytes, and each pass scans the whole block, so a small file can ask the
//! block decoder for minutes of work. The decode counts that work (passes
//! times samples) before it decodes a tile, and refuses a file past the
//! bound with [`Error::Limit`]. OpenJPEG 2.5.0 has no such bound. See
//! `tests/fixtures/tier1_work_bound/README.md` and `docs/correctness.md`.
//!
//! The committed files are written by
//! `scripts/gen-tier1-work-bound-fixtures.py`; the issue's 76 KB shape is
//! built here, with the same layout.

mod support;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rust_j2k::{
    DecodeOptions, Error, WorkLimit, decode, decode_with, read_header, read_header_with,
};
use support::jp2::sha256_hex;

fn fixture(path: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(path);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Packet header bits, packed as B.10.1 has it: a byte after 0xFF carries
/// only 7 bits.
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    byte: u8,
    used: u32,
    room: u32,
}

impl Bits {
    fn new() -> Self {
        Bits {
            room: 8,
            ..Bits::default()
        }
    }

    fn put(&mut self, bit: u8) {
        self.byte = (self.byte << 1) | bit;
        self.used += 1;
        if self.used == self.room {
            self.out.push(self.byte);
            self.room = if self.byte == 0xFF { 7 } else { 8 };
            self.byte = 0;
            self.used = 0;
        }
    }

    fn put_int(&mut self, value: u32, width: u32) {
        for shift in (0..width).rev() {
            self.put(((value >> shift) & 1) as u8);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.used > 0 {
            self.out.push(self.byte << (self.room - self.used));
        } else if self.out.last() == Some(&0xFF) {
            self.out.push(0);
        }
        self.out
    }
}

/// `t1bomb` from the generator: one `size` x `size` tile of one 8-bit
/// component, no wavelet levels, 64x64 code-blocks, one layer, each block
/// included with 88 coding passes and the one byte 0x00. QCD gives the band
/// 30 bit-planes and no block has a zero bit-plane, so each runs all 88.
fn t1bomb(size: u32) -> Vec<u8> {
    t1bomb_with_length(size, 1)
}

/// [`t1bomb`] with each code-block's length `length` (0 or 1), and that
/// many bytes of 0x00 for each.
fn t1bomb_with_length(size: u32, length: u32) -> Vec<u8> {
    assert!(length <= 1);
    let blocks = size.div_ceil(64);
    // The tag trees' levels, leaves first: (columns, rows).
    let mut levels = vec![(blocks, blocks)];
    while *levels.last().unwrap() != (1, 1) {
        let (c, r) = *levels.last().unwrap();
        levels.push((c.div_ceil(2), r.div_ceil(2)));
    }
    let mut known = [
        std::collections::HashSet::new(),
        std::collections::HashSet::new(),
    ];
    let mut bits = Bits::new();
    bits.put(1);
    for y in 0..blocks {
        for x in 0..blocks {
            // Inclusion, then the zero bit-planes: every value is 0, so each
            // node not yet known codes a 1.
            for tree in &mut known {
                for level in (0..levels.len() as u32).rev() {
                    if tree.insert((level, x >> level, y >> level)) {
                        bits.put(1);
                    }
                }
            }
            // 88 passes (Table B.4), Lblock stays 3, the length in
            // 3 + floor(log2(88)) = 9 bits.
            bits.put_int(0b1_1111_1111, 9);
            bits.put_int(88 - 37, 7);
            bits.put(0);
            bits.put_int(length, 9);
        }
    }
    let mut data = bits.finish();
    data.resize(data.len() + (blocks * blocks * length) as usize, 0);

    let seg = |code: u16, body: &[u8]| {
        let mut out = code.to_be_bytes().to_vec();
        out.extend_from_slice(&(2 + body.len() as u16).to_be_bytes());
        out.extend_from_slice(body);
        out
    };
    let mut siz = 0u16.to_be_bytes().to_vec();
    for v in [size, size, 0, 0, size, size, 0, 0] {
        siz.extend_from_slice(&v.to_be_bytes());
    }
    siz.extend_from_slice(&[0, 1, 7, 1, 1]);
    let mut cs = vec![0xFF, 0x4F];
    cs.extend(seg(0xFF51, &siz));
    cs.extend(seg(0xFF52, &[0, 0, 0, 1, 0, 0, 4, 4, 0, 1]));
    cs.extend(seg(0xFF5C, &[0x40, 29 << 3]));
    cs.extend_from_slice(&[0xFF, 0x90, 0, 10, 0, 0]);
    cs.extend_from_slice(&(12 + 2 + data.len() as u32).to_be_bytes());
    cs.extend_from_slice(&[0, 1, 0xFF, 0x93]);
    cs.extend(data);
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

fn assert_limit(name: &str, result: rust_j2k::Result<rust_j2k::Image>) {
    match result {
        Err(Error::Limit(m)) => assert!(m.contains("work bound"), "{name}: {m}"),
        Err(e) => panic!("{name}: expected the work bound, got {e}"),
        Ok(_) => panic!("{name}: expected the work bound, got an image"),
    }
}

fn samples_u8(image: &rust_j2k::Image) -> Vec<u8> {
    image.components[0]
        .samples
        .iter()
        .map(|&s| s as u8)
        .collect()
}

/// The generator's file and the shape built here are the same bytes.
#[test]
fn the_committed_bomb_is_the_shape_built_here() {
    assert_eq!(
        t1bomb(1024),
        fixture("tier1_work_bound/t1bomb_1024_64_88.j2k")
    );
}

/// 1024^2 in 64x64 code-blocks of 88 passes asks for 9.2e7 sample-passes
/// in 1.2 KB of tile data, past the bound's floor of 2^26: refused, while
/// the header still reads. Twice the bound admits it, and it decodes to
/// what OpenJPEG 2.5.0 decodes (the hash of `opj_decompress`'s samples).
#[test]
fn raising_the_code_block_limit_admits_a_file_the_default_refuses() {
    let bytes = fixture("tier1_work_bound/t1bomb_1024_64_88.j2k");
    assert_limit("default", decode(&bytes));
    read_header(&bytes).expect("the header does not decode code-blocks");
    let raised = DecodeOptions::default().with_code_block_work_limit(WorkLimit::Raised(2));
    let image = decode_with(&bytes, &raised).expect("within twice the bound");
    assert_eq!(
        sha256_hex(&samples_u8(&image)),
        "a20e3adaff1e3c720b7cb8efe4a3da15a0a41df1b17c1b9cb247c3207882734d"
    );
    // Lifting the bound decodes it the same way.
    let lifted = DecodeOptions::default().with_code_block_work_limit(WorkLimit::Unlimited);
    assert_eq!(decode_with(&bytes, &lifted).unwrap(), image);
    // Raising the other bound does not.
    let other = DecodeOptions::default().with_packet_work_limit(WorkLimit::Unlimited);
    assert_limit("packet limit", decode_with(&bytes, &other));
}

/// The same shape with a length of zero for every code-block: passes and no
/// bytes, which B.10.7 allows and Tier-1 decodes from the `0xFF` fill. The
/// passes cost what they cost with a byte, so the file, smaller by a byte a
/// block, is still past the bound. Twice the bound admits it, and it decodes
/// to what OpenJPEG 2.5.0 decodes.
#[test]
fn passes_with_no_bytes_still_count_against_the_code_block_bound() {
    let bytes = t1bomb_with_length(1024, 0);
    assert_limit("default", decode(&bytes));
    let raised = DecodeOptions::default().with_code_block_work_limit(WorkLimit::Raised(2));
    let image = decode_with(&bytes, &raised).expect("within twice the bound");
    // The hash of `opj_decompress`'s samples for these bytes.
    assert_eq!(
        sha256_hex(&samples_u8(&image)),
        "36ff34972077a9e824cce89d6a7056a0923719b8ae884a5ed7f0ba299303534e"
    );
}

/// The issue's shape: 8192^2 in 64x64 code-blocks of 88 passes, 76 KB. It
/// asks for 5.9e9 sample-passes, about 19 s on a release build, past the
/// bound of 2^15 a byte (2.5e9). It is refused before any code-block is
/// decoded.
#[test]
fn the_issue_bomb_is_refused_fast() {
    let bytes = t1bomb(8192);
    assert_eq!(bytes.len(), 76525);
    let start = Instant::now();
    assert_limit("t1bomb 8192", decode(&bytes));
    // The refusal costs the packet walk alone: well under a second on a
    // release build. Debug builds are slower, so the check is loose.
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "{:?}",
        start.elapsed()
    );
}

/// With the bound lifted, the issue's shape decodes, as OpenJPEG decodes
/// it. About 19 s on a release build, minutes on a debug one, so ignored:
/// run with `cargo test --release --test work_limit -- --ignored`.
#[test]
#[ignore = "slow: about 19 s on a release build"]
fn the_issue_bomb_decodes_with_the_limit_lifted() {
    let bytes = t1bomb(8192);
    let lifted = DecodeOptions::default().with_code_block_work_limit(WorkLimit::Unlimited);
    let image = decode_with(&bytes, &lifted).expect("decodes");
    assert_eq!(image.components[0].samples.len(), 8192 * 8192);
}

/// Encoder output that spends much work per byte: `opj_compress` on a
/// 16-bit image of 1000 with 128 specks of 60000, lossless. Each speck
/// makes its code-blocks run every pass for a few bytes. It decodes at the
/// default bound, sample for sample the image that was coded (OpenJPEG
/// 2.5.0 decodes it the same way).
#[test]
fn encoder_output_of_specks_decodes_at_the_default_bound() {
    let image = decode(&fixture("tier1_work_bound/opj_specks2048_16bit.j2k")).expect("decodes");
    let mut expected = vec![1000; 2048 * 2048];
    for i in 0..128 {
        let (x, y) = (i * 977 % 2048, i * 613 % 2048);
        expected[y * 2048 + x] = 60000;
    }
    assert_eq!(image.components.len(), 1);
    assert!(
        image.components[0].samples == expected,
        "not the coded image"
    );
}

/// The encoder output that spends the most work per byte seen:
/// `opj_compress -n 1` on a flat 16-bit image of 0, 42% of the bound. Each
/// code-block runs about 46 passes for about 14 bytes. It decodes at the
/// default bound to the flat image that was coded (OpenJPEG 2.5.0 decodes
/// it the same way).
#[test]
fn encoder_output_of_a_flat_image_without_wavelet_decodes_at_the_default_bound() {
    let image = decode(&fixture("tier1_work_bound/opj_flat2048_16bit_n1.j2k")).expect("decodes");
    assert_eq!(image.components.len(), 1);
    assert_eq!(image.components[0].samples.len(), 2048 * 2048);
    assert!(image.components[0].samples.iter().all(|&s| s == 0));
}

/// `t2skip_2048x1024_40000` walks 2.0e7 steps for 40 KB, past the tier-2
/// floor of 2^24: refused by default, and decoded to its flat 128 under
/// twice the bound or none.
#[test]
fn raising_the_packet_limit_admits_a_file_the_default_refuses() {
    let bytes = fixture("work_bound/t2skip_2048x1024_40000.j2k");
    assert_limit("default", decode(&bytes));
    for limit in [WorkLimit::Raised(2), WorkLimit::Unlimited] {
        let options = DecodeOptions::default().with_packet_work_limit(limit.clone());
        let image = decode_with(&bytes, &options).unwrap_or_else(|e| panic!("{limit:?}: {e}"));
        assert_eq!(image.components[0].samples.len(), 2048 * 1024);
        assert!(image.components[0].samples.iter().all(|&s| s == 128));
    }
    let other = DecodeOptions::default().with_code_block_work_limit(WorkLimit::Unlimited);
    assert_limit("code-block limit", decode_with(&bytes, &other));
}

/// The walk `read_header_with` makes of a `Psot = 0` tile takes the packet
/// limit too. `jp2_t2skip_512_20000.jp2` walks 5.1e6 steps, past that
/// walk's floor of 2^22, so by default the codestream is taken to run to the
/// end of the file and the decode refuses the `free` box after it as bytes
/// left after the last packet. Under twice the bound the walk finds the
/// packets end at the box's end, and the file decodes.
#[test]
fn the_packet_limit_reaches_the_walk_of_a_psot_zero_tile() {
    let bytes = fixture("work_bound/jp2_t2skip_512_20000.jp2");
    match decode(&bytes) {
        Err(Error::Codestream(_)) => {}
        other => panic!("expected the bytes after the packets refused, got {other:?}"),
    }
    let raised = DecodeOptions::default().with_packet_work_limit(WorkLimit::Raised(2));
    let image = decode_with(&bytes, &raised).expect("ends at the box");
    assert!(image.components[0].samples.iter().all(|&s| s == 128));
    let header = read_header_with(&bytes, &raised).unwrap();
    let default = read_header(&bytes).unwrap();
    assert_eq!(header.jp2.as_ref().unwrap(), image.jp2.as_ref().unwrap());
    assert_eq!(
        (header.width, header.height),
        (default.width, default.height)
    );
}

/// A limit raised by a factor of 0 is an error in the options, whatever the
/// input.
#[test]
fn a_limit_raised_by_zero_is_invalid_options() {
    let bytes = fixture("tier1_work_bound/t1bomb_1024_64_88.j2k");
    for options in [
        DecodeOptions::default().with_code_block_work_limit(WorkLimit::Raised(0)),
        DecodeOptions::default().with_packet_work_limit(WorkLimit::Raised(0)),
    ] {
        assert!(matches!(
            decode_with(&bytes, &options),
            Err(Error::InvalidOptions(_))
        ));
        assert!(matches!(
            read_header_with(&bytes, &options),
            Err(Error::InvalidOptions(_))
        ));
    }
    // `Raised(1)` is the default bound.
    let one = DecodeOptions::default().with_code_block_work_limit(WorkLimit::Raised(1));
    assert_limit("Raised(1)", decode_with(&bytes, &one));
}
