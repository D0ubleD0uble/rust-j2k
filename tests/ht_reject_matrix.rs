//! The reject matrix for HT code-blocks, through the public `decode`.
//!
//! Every way a decoder can refuse an HT code-block that tier-2 or the HT
//! coder checks, each as a one-block codestream with the one fault. The
//! unit tests beside the code (`src/tier1/ht/tests.rs`, `src/tier2/`) pin the
//! same refusals on the parts; this matrix pins that each one reaches a
//! caller as the typed error the crate commits to: `Codestream` for a block
//! that breaks the format, `Limit` for a block past a resource guard. A row
//! that stops refusing fails here first.
//!
//! The vectors are those the unit tests found by a seeded search over short
//! segments, each the first to fail for its reason. Each is checked against
//! the unit test of the same name, and a control row proves the builder
//! writes a block that decodes. The refusals of the main header (`CAP`, `CPF`,
//! MIXED) are in the matrix of `src/codestream/tests.rs`.

use rust_j2k::{Error, decode};

/// Packet header bits, packed as B.10.1 has it: a byte after 0xFF carries
/// only 7 bits.
struct Bits {
    out: Vec<u8>,
    byte: u8,
    used: u32,
    room: u32,
}

impl Bits {
    fn new() -> Self {
        Bits {
            out: Vec::new(),
            byte: 0,
            used: 0,
            room: 8,
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

/// One HT code-block, alone in the image.
struct Block<'a> {
    width: u32,
    height: u32,
    /// The subband's magnitude bit-planes `Mb`.
    mb: u32,
    zero_bit_planes: u32,
    /// Coding passes: 1 (cleanup), 2 or 3.
    passes: u32,
    cleanup: &'a [u8],
    refinement: &'a [u8],
    /// A maxshift, which also signals `RGN` in `CAP`.
    roi_shift: Option<u8>,
}

impl Default for Block<'_> {
    fn default() -> Self {
        Block {
            width: 1,
            height: 1,
            mb: 8,
            zero_bit_planes: 0,
            passes: 1,
            cleanup: &[],
            refinement: &[],
            roi_shift: None,
        }
    }
}

fn bit_length(value: usize) -> u32 {
    usize::BITS - value.leading_zeros()
}

/// A codestream of one tile of one 8-bit component, no wavelet levels, whose
/// one code-block is `block`, in one layer.
fn ht_file(block: &Block<'_>) -> Vec<u8> {
    let mut bits = Bits::new();
    bits.put(1); // a non-empty packet
    bits.put(1); // included in layer 0
    // The zero bit-planes, coded as a tag tree of one node: that many 0s.
    for _ in 0..block.zero_bit_planes {
        bits.put(0);
    }
    bits.put(1);
    // The passes (Table B.4).
    match block.passes {
        1 => bits.put(0),
        2 => bits.put_int(0b10, 2),
        _ => bits.put_int(0b1100, 4),
    }
    // Lblock grows until both lengths fit: the cleanup segment holds one pass,
    // the refinement segment the others.
    let refined = block.passes - 1;
    let mut increments = 0;
    while 3 + increments < bit_length(block.cleanup.len())
        || (refined > 0 && 3 + increments + refined.ilog2() < bit_length(block.refinement.len()))
    {
        increments += 1;
    }
    for _ in 0..increments {
        bits.put(1);
    }
    bits.put(0);
    bits.put_int(block.cleanup.len() as u32, 3 + increments);
    if refined > 0 {
        bits.put_int(
            block.refinement.len() as u32,
            3 + increments + refined.ilog2(),
        );
    }
    let mut data = bits.finish();
    data.extend_from_slice(block.cleanup);
    data.extend_from_slice(block.refinement);

    let seg = |code: u16, body: &[u8]| {
        let mut out = code.to_be_bytes().to_vec();
        out.extend_from_slice(&(2 + body.len() as u16).to_be_bytes());
        out.extend_from_slice(body);
        out
    };
    let mut siz = 0u16.to_be_bytes().to_vec();
    for v in [
        block.width,
        block.height,
        0,
        0,
        block.width,
        block.height,
        0,
        0,
    ] {
        siz.extend_from_slice(&v.to_be_bytes());
    }
    siz.extend_from_slice(&[0, 1, 7, 1, 1]);
    let mut cs = vec![0xFF, 0x4F];
    cs.extend(seg(0xFF51, &siz));
    // CAP: Part 15 only, with the `RGN` bit (12) set under a maxshift.
    let ccap: u16 = if block.roi_shift.is_some() {
        0x1004
    } else {
        0x0004
    };
    cs.extend(seg(0xFF50, &[0, 2, 0, 0, (ccap >> 8) as u8, ccap as u8]));
    // COD: block style HT (0x40), 64x64 code-blocks, no wavelet level.
    cs.extend(seg(0xFF52, &[0, 0, 0, 1, 0, 0, 4, 4, 0x40, 1]));
    // QCD: two guard bits and the exponent that gives `Mb`.
    cs.extend(seg(
        0xFF5C,
        &[0x40, (block.mb.saturating_sub(1).min(31) as u8) << 3],
    ));
    if let Some(shift) = block.roi_shift {
        cs.extend(seg(0xFF5E, &[0, 0, shift]));
    }
    cs.extend_from_slice(&[0xFF, 0x90, 0, 10, 0, 0]);
    cs.extend_from_slice(&(12 + 2 + data.len() as u32).to_be_bytes());
    cs.extend_from_slice(&[0, 1, 0xFF, 0x93]);
    cs.extend_from_slice(&data);
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

/// What a row must produce.
#[derive(Clone, Copy, Debug)]
enum Want {
    Decodes,
    /// A `Codestream` error whose message holds this text.
    Codestream(&'static str),
    /// A `Limit` error whose message holds this text.
    Limit(&'static str),
}

/// The cleanup segment of the one-sample block that decodes to 384 at `Mb`
/// 8, and of the 1x2 block whose top sample is significant.
const ONE_SAMPLE: [u8; 2] = [0x62, 0x00];

#[test]
fn every_ht_block_refusal_reaches_the_caller_as_its_typed_error() {
    // A cleanup segment of 65535 bytes: MagSgn zeros, then the one-sample
    // MEL and VLC bytes. Past Lcup < 65535 (7.1.1).
    let mut long_cleanup = vec![0u8; 65533];
    long_cleanup.extend(ONE_SAMPLE);
    let long_refinement = vec![0u8; 2047];

    let rows: Vec<(&str, Block<'_>, Want)> = vec![
        (
            "control: a one-sample cleanup pass",
            Block {
                cleanup: &ONE_SAMPLE,
                ..Block::default()
            },
            Want::Decodes,
        ),
        (
            "control: all three passes",
            Block {
                width: 1,
                height: 2,
                passes: 3,
                cleanup: &[0x00, 0x62, 0x00],
                refinement: &[0x01, 0x01],
                ..Block::default()
            },
            Want::Decodes,
        ),
        // Tier-2: the segment length limits.
        (
            "cleanup segment of 65535 bytes",
            Block {
                cleanup: &long_cleanup,
                ..Block::default()
            },
            Want::Codestream("HT cleanup segment is 65535 bytes"),
        ),
        (
            "refinement segment of 2047 bytes",
            Block {
                width: 1,
                height: 2,
                passes: 2,
                cleanup: &[0x00, 0x62, 0x00],
                refinement: &long_refinement,
                ..Block::default()
            },
            Want::Codestream("HT refinement segment is 2047 bytes"),
        ),
        // Tier-1: the cleanup segment's framing.
        (
            "cleanup segment of one byte (refused in tier-2)",
            Block {
                cleanup: &[0x02],
                ..Block::default()
            },
            Want::Codestream("first cleanup segment is 1 byte long"),
        ),
        (
            "Scup over Lcup",
            Block {
                cleanup: &[0x00, 0x04, 0x00],
                ..Block::default()
            },
            Want::Codestream("Scup = 4"),
        ),
        (
            "Scup under 2",
            Block {
                cleanup: &[0x00, 0x01, 0x00],
                ..Block::default()
            },
            Want::Codestream("Scup = 1"),
        ),
        // Tier-1: the block's bit-planes.
        (
            "no bit-plane left after the zero bit-planes",
            Block {
                zero_bit_planes: 8,
                cleanup: &[0x01, 0x62, 0x00],
                ..Block::default()
            },
            Want::Codestream("leaving none to code"),
        ),
        (
            "31 bit-planes in the subband",
            Block {
                mb: 31,
                cleanup: &[0x01, 0x62, 0x00],
                ..Block::default()
            },
            Want::Limit("31 bit-planes"),
        ),
        (
            "a maxshift past the 30-plane limit",
            Block {
                mb: 28,
                roi_shift: Some(3),
                cleanup: &[0x01, 0x62, 0x00],
                ..Block::default()
            },
            Want::Limit("maxshift of 3"),
        ),
        // Tier-1: the cleanup pass's streams.
        (
            "significance outside the block",
            Block {
                width: 3,
                height: 3,
                zero_bit_planes: 1,
                cleanup: &[0x59, 0xCC, 0xE4, 0x00],
                ..Block::default()
            },
            Want::Codestream("outside the block"),
        ),
        (
            "VLC stream running into the MagSgn bytes",
            Block {
                width: 3,
                height: 1,
                zero_bit_planes: 1,
                cleanup: &[0xD6, 0xB3, 0x72, 0x00],
                ..Block::default()
            },
            Want::Codestream("VLC stream runs into"),
        ),
        (
            "MagSgn byte after 0xFF with its stuffing bit set",
            Block {
                width: 4,
                height: 2,
                zero_bit_planes: 1,
                cleanup: &[0xFF, 0xDF, 0x39, 0x6F, 0xD4, 0xA5, 0x00],
                ..Block::default()
            },
            Want::Codestream("stuffing bit set"),
        ),
        (
            "MagSgn stream past its end",
            Block {
                width: 4,
                height: 4,
                mb: 30,
                zero_bit_planes: 2,
                cleanup: &[0x6D, 0x49, 0x64, 0x00],
                ..Block::default()
            },
            Want::Codestream("MagSgn stream runs past"),
        ),
        (
            "exponent bound over the coded bit-planes",
            Block {
                width: 4,
                height: 2,
                zero_bit_planes: 2,
                cleanup: &[0x0B, 0x85, 0x54, 0x91, 0xE1, 0x56, 0x00],
                ..Block::default()
            },
            Want::Codestream("U_q = 21 exceeds"),
        ),
        (
            "exponent bound one over zero bit-planes + 2",
            Block {
                width: 4,
                height: 2,
                mb: 30,
                zero_bit_planes: 1,
                cleanup: &[0xB5, 0x0B, 0xB2, 0xB4, 0x00],
                ..Block::default()
            },
            Want::Codestream("U_q = 4 exceeds zero bit-planes + 2 = 3"),
        ),
        (
            "residual extension past the exponent bound",
            Block {
                mb: 30,
                zero_bit_planes: 11,
                cleanup: &[0x15, 0x2C, 0x7E, 0x07, 0xF5, 0x00],
                ..Block::default()
            },
            Want::Codestream("U_q = 62 exceeds"),
        ),
        (
            "a magnitude past the coded bit-planes",
            Block {
                width: 4,
                height: 2,
                mb: 30,
                cleanup: &[0x0F, 0x32, 0xC4, 0x00],
                ..Block::default()
            },
            Want::Codestream("magnitude past its bit-planes"),
        ),
        // Tier-1: the refinement passes.
        (
            "SigProp byte after 0xFF with its stuffing bit set",
            Block {
                width: 4,
                height: 2,
                passes: 2,
                cleanup: &[0x52, 0x70, 0x63, 0x00],
                refinement: &[0xFF, 0x80],
                ..Block::default()
            },
            Want::Codestream("stuffing bit"),
        ),
    ];

    let mut wrong = Vec::new();
    for (name, block, want) in &rows {
        let result = decode(&ht_file(block));
        let ok = match (want, &result) {
            (Want::Decodes, Ok(_)) => true,
            (Want::Codestream(text), Err(Error::Codestream(m))) => m.contains(text),
            (Want::Limit(text), Err(Error::Limit(m))) => m.contains(text),
            _ => false,
        };
        if !ok {
            let got = match result {
                Ok(_) => "an image".to_string(),
                Err(e) => format!("{e:?}"),
            };
            wrong.push(format!("  {name}: wanted {want:?}, got {got}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} rows refused wrongly:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}
