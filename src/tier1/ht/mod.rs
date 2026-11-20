//! The HTJ2K block coder (Rec. ITU-T T.814 | ISO/IEC 15444-15, clause 7).
//!
//! An HT code-block codes its magnitudes in one cleanup pass, optionally
//! followed by a SigProp and a MagRef pass that refine one more bit-plane.
//!
//! The cleanup segment holds three byte-streams. MagSgn runs forward from the
//! start; MEL runs forward from `Pcup`; VLC runs backward from the end. The
//! block is scanned in 2x2 quads, row-pairs at a time, and each quad's
//! significance, exponent bound and magnitudes come from those three streams
//! (7.3). The vertically causal context flag changes only the SigProp pass
//! (7.4), so the cleanup pass ignores it.
//!
//! The refinement segment holds two more: SigProp runs forward from its
//! start, MagRef backward from its end. Both passes scan the block in
//! four-line stripes, as the Part 1 coder does. SigProp finds the samples
//! that become significant in the next bit-plane and reads their signs
//! (7.4); MagRef reads that bit-plane for the samples the cleanup pass found
//! significant (7.5).
//!
//! The rest of tier-1 reaches this module only through
//! [`BlockCoder`](super::BlockCoder): [`decode`] and [`passes_run`].

#[cfg(test)]
mod tests;
mod vlc_tables;

use super::ebcot::passes::undo_maxshift;
use crate::tier2::CodeBlock;
use crate::{Error, Result};
use vlc_tables::{CXT_VLC_TABLE_0, CXT_VLC_TABLE_1};

/// The most `Mb + s - P` a block can have for the double-scale reconstruction
/// to fit `i32`, with `P` its zero bit-planes (`Mb` when there is no maxshift
/// and `P = 0`). The cleanup pass codes the top `P + 1` planes, so
/// `Mb + s - P` is one more than the cleanup plane's shift: this bounds that
/// shift, not a count of coded planes. The Part 1 coder limits the same
/// quantity, where it is the count of coded planes (`top_coded_plane`).
const MAX_BIT_PLANES: u32 = 30;

/// The largest suffix length `Scup` (7.1.1).
const MAX_SCUP: usize = 4079;

/// One more than the largest cleanup segment length `Lcup` (7.1.1).
const MAX_LCUP: usize = 65535;

/// The most coding passes of one HT set: cleanup, SigProp and MagRef (B.1).
const HT_SET_PASSES: u32 = 3;

#[cold]
fn malformed(what: &str) -> Error {
    Error::codestream(format!("HT code-block {what}"))
}

/// The tile-component's parameters an HT code-block decode needs beyond the
/// block and its subband's `Mb`.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Params {
    /// `SPrgn`, the region-of-interest maxshift `s` (ISO/IEC 15444-1
    /// Annex H); 0 without one.
    pub roi_shift: u8,
    /// The vertically causal context flag, which keeps the SigProp pass from
    /// looking at the next stripe (7.4).
    pub causal: bool,
    /// The bound on the cleanup magnitudes that `CAP` signals for this
    /// subband (A.3.7, 8.7.3), as a bit count: every `mu` is below
    /// `2^bits`. `None` without a bound.
    pub bound_bits: Option<u32>,
}

/// Decode one HT code-block into its coefficients, row-major over the block,
/// at twice their weight like the Part 1 coder.
///
/// `numbps` is the subband's magnitude bit-plane count `Mb`. A maxshift
/// `s` raises the block's count to `Mb + s` (Annex H, H-3), and the decode
/// runs on that count. The cleanup pass codes the top `P + 1` of those
/// magnitude bit-planes, `P` being the block's zero bit-planes (`S_blk`,
/// 7.6), so a significant sample of magnitude `mu` reconstructs to
/// `(2 mu + 1) << (Mb + s - P - 1)`: `mu` at its bit-planes, plus half of
/// the first plane not coded. The SigProp and MagRef passes code one plane
/// more for the samples they reach, which then reconstruct with half of the
/// plane below that. OpenJPEG's `opj_t1_ht_decode_cblk` reconstructs the
/// same way.
///
/// Last, the maxshift is undone (7.6 hands the samples to Annex H) exactly
/// as the Part 1 coder undoes it, by H.1: a magnitude at or above `2^(s + 1)`
/// at double scale is region and comes down `s` planes; one below is
/// background and stays (see `undo_maxshift`).
#[cfg(any(test, feature = "__fuzz"))]
pub(super) fn decode(block: &CodeBlock<'_>, numbps: u32, params: Params) -> Result<Vec<i32>> {
    let mut scratch = Scratch::default();
    decode_into(block, numbps, params, &mut scratch)?;
    Ok(scratch.coeffs)
}

/// The buffers one HT code-block's decode uses, kept from block to block so
/// a subband of many small blocks allocates them once. [`decode_into`]
/// clears each one before a block uses it, and gives the coefficients a new
/// buffer.
#[derive(Debug, Default)]
pub(super) struct Scratch {
    coeffs: Vec<i32>,
    cleanup: CleanupScratch,
    refinement: RefinementScratch,
}

/// The cleanup pass's two rows of quads: the one above, which the context
/// and the exponent predictor read, and the one being decoded.
#[derive(Debug, Default)]
struct CleanupScratch {
    above: Vec<QuadState>,
    row: Vec<QuadState>,
}

/// The refinement passes' buffers.
#[derive(Debug, Default)]
struct RefinementScratch {
    /// [`SIGMA`] and [`REFINED`] for each sample, row-major over the block
    /// with a clear border of one sample around it, so every sample has all
    /// eight neighbors.
    state: Vec<u8>,
    /// The samples of one column-group SigProp makes significant.
    new: Vec<usize>,
}

/// In [`RefinementScratch::state`], a sample the cleanup pass made
/// significant.
const SIGMA: u8 = 1;

/// In [`RefinementScratch::state`], a sample SigProp made significant.
const REFINED: u8 = 2;

/// Empty `buffer` and refill it with `len` default values, keeping its
/// allocation.
fn zeroed<T: Clone + Default>(buffer: &mut Vec<T>, len: usize) {
    buffer.clear();
    buffer.resize(len, T::default());
}

/// [`decode`] into `scratch`, whose coefficients hold the block until its
/// next decode.
pub(super) fn decode_into<'s>(
    block: &CodeBlock<'_>,
    numbps: u32,
    params: Params,
    scratch: &'s mut Scratch,
) -> Result<&'s [i32]> {
    fill(block, numbps, params, scratch)?;
    Ok(&scratch.coeffs)
}

/// The body of [`decode_into`].
fn fill(block: &CodeBlock<'_>, numbps: u32, params: Params, scratch: &mut Scratch) -> Result<()> {
    let (width, height) = (block.width, block.height);
    // The coefficients get a fresh buffer for each block, unlike the other
    // buffers: reusing it measured 1 to 2% slower on HT files of 64x64
    // blocks, likely from where the long-lived buffers fall in memory.
    scratch.coeffs = vec![0i32; width * height];
    let Scratch {
        coeffs,
        cleanup,
        refinement: refine,
    } = scratch;
    // `M'b = Mb + s` (H-3): the zero bit-planes count down from it.
    let planes = numbps + u32::from(params.roi_shift);
    // More passes would be a second HT set, which only a MULTIHT codestream
    // holds (8.3), and the `CAP` parse refuses those. So a count past one set
    // is a damaged one. OpenJPEG refuses it too.
    if block.num_passes > HT_SET_PASSES {
        return Err(malformed(&format!(
            "has {} coding passes, more than the {HT_SET_PASSES} of one HT set",
            block.num_passes
        )));
    }
    if block.num_passes == 0 || planes == 0 {
        return Ok(());
    }
    let zbp = block.zero_bit_planes;
    // At least one bit-plane must be left to code; OpenJPEG refuses the
    // same blocks.
    if zbp >= planes {
        return Err(malformed(&format!(
            "has {zbp} zero bit-planes in a block of {planes}, leaving none to code"
        )));
    }
    // `Mb + s - P` bounds the cleanup plane's shift, as `top_coded_plane`
    // does in the Part 1 coder; `Mb + s` alone does not, since a block with
    // many zero bit-planes keeps the shift small under any maxshift. The
    // per-sample `try_from` in the cleanup pass still refuses any magnitude
    // that does not fit.
    let coded = planes - zbp;
    if coded > MAX_BIT_PLANES {
        return Err(Error::limit_fixed(
            crate::LimitKind::BitPlanes,
            if params.roi_shift == 0 {
                format!(
                    "HT code-block of {coded} bit-planes (a subband of {numbps} and {zbp} \
                 zero bit-planes), over the {MAX_BIT_PLANES}-plane limit"
                )
            } else {
                format!(
                    "HT code-block of {coded} bit-planes (a subband of {numbps}, a \
                 maxshift of {} and {zbp} zero bit-planes), over the \
                 {MAX_BIT_PLANES}-plane limit",
                    params.roi_shift
                )
            },
        ));
    }
    let data = block
        .segments
        .first()
        .map(|s| s.bytes())
        .unwrap_or_default();
    let lcup = data.len();
    if !(2..MAX_LCUP).contains(&lcup) {
        return Err(malformed(&format!(
            "cleanup segment is {lcup} bytes, outside 2..{MAX_LCUP}"
        )));
    }
    let scup = 16 * usize::from(data[lcup - 1]) + usize::from(data[lcup - 2] & 0x0F);
    if !(2..=lcup.min(MAX_SCUP)).contains(&scup) {
        return Err(malformed(&format!(
            "suffix length Scup = {scup} is outside 2..=min(Lcup = {lcup}, {MAX_SCUP})"
        )));
    }
    let seg = Segment {
        data: &data,
        pcup: lcup - scup,
    };
    let shift = planes - zbp - 1;
    Cleanup {
        seg,
        magsgn: MagSgnReader::new(),
        mel: MelDecoder::new(seg.pcup),
        vlc: VlcReader::new(&seg),
        width,
        height,
        qw: width.div_ceil(2),
        // `mu` has at most `P + 1` bits (7.6), so its exponent, the bit
        // length of `2 mu - 1`, is at most `P + 2`. OpenJPEG has the same
        // bound. The cap at `MAX_BIT_PLANES + 2` keeps the shifts below in
        // range when `P` is large under a maxshift; a magnitude that fits
        // `i32` has an exponent of at most 31.
        max_u: (zbp + 2).min(MAX_BIT_PLANES + 2),
        // A bound of 64 bits or more holds of every `u64`.
        bound_bits: params.bound_bits.filter(|&bits| bits < u64::BITS),
        shift,
    }
    .run(coeffs, cleanup)?;
    let z_blk = z_blk(block, planes);
    if z_blk > 1 {
        let refinement = block.segments.get(1).map(|s| s.bytes()).unwrap_or_default();
        Refinement {
            width,
            height,
            causal: params.causal,
            shift,
        }
        .run(&refinement, z_blk == HT_SET_PASSES, coeffs, refine)?;
    }
    if params.roi_shift != 0 {
        for c in coeffs.iter_mut() {
            let magnitude = undo_maxshift(c.abs(), params.roi_shift);
            *c = if *c < 0 { -magnitude } else { magnitude };
        }
    }
    Ok(())
}

/// The coding passes [`decode`] runs on `block` in a subband of `numbps`
/// magnitude bit-planes, under a maxshift of `roi_shift`. See [`z_blk`].
pub(super) fn passes_run(block: &CodeBlock<'_>, numbps: u32, roi_shift: u8) -> u64 {
    u64::from(z_blk(block, numbps + u32::from(roi_shift)))
}

/// The passes of `block` to decode: `Z_blk` (B.3), but only the cleanup pass
/// where a refinement pass would code below the block's last bit-plane.
///
/// `Z_blk` is the block's passes, except that a refinement segment of zero
/// length leaves the cleanup pass alone. A block whose cleanup pass already
/// reaches the last of its `numbps` planes (`Mb`, or `Mb + s` under a
/// maxshift), `P = numbps - 1`, has no plane left to refine: a refinement
/// pass would give it `Nb = numbps + 1` planes (7.6). OpenJPEG decodes its
/// cleanup pass alone, with a warning, and so does this decoder.
fn z_blk(block: &CodeBlock<'_>, numbps: u32) -> u32 {
    let passes = block.num_passes.min(HT_SET_PASSES);
    let refined = block
        .segments
        .get(1)
        .is_some_and(|s| s.chunks.iter().any(|c| !c.is_empty()));
    let plane_left = numbps >= block.zero_bit_planes.saturating_add(2);
    if passes > 1 && refined && plane_left {
        passes
    } else {
        passes.min(1)
    }
}

/// The cleanup segment `Dcup`, with `Pcup`, the length of its MagSgn part.
#[derive(Clone, Copy)]
struct Segment<'a> {
    data: &'a [u8],
    pcup: usize,
}

impl Segment<'_> {
    /// `modDcup` (7.1.1): the segment with its last byte and the low nibble of
    /// the one before it, which held `Scup`, read as ones.
    fn byte(&self, pos: usize) -> u8 {
        let lcup = self.data.len();
        if pos == lcup - 1 {
            0xFF
        } else if pos == lcup - 2 {
            self.data[pos] | 0x0F
        } else {
            self.data[pos]
        }
    }
}

/// The low `n` bits set, for `n` up to 63.
fn low_bits(n: u32) -> u64 {
    (1u64 << n) - 1
}

/// The fewest bits the buffered readers keep after a refill, unless their
/// stream has ended: 64 less the most one byte adds, less one.
const REFILL_BITS: u32 = 56;

/// The MagSgn bit-stream: forward from byte 0, least significant bit first,
/// with a stuffing bit after every 0xFF (`importMagSgnBit`, 7.1.2).
///
/// It loads whole bytes ahead into `acc` and hands out bits from its low
/// end. A byte it cannot load (a set stuffing bit, or one past the end) ends
/// the loading and is recorded in `end`; reading past the bits loaded before
/// it gives that error, at the same bit a bit-at-a-time reader would.
struct MagSgnReader {
    /// The next byte to load.
    pos: usize,
    /// The last byte loaded, which decides whether the next has 7 bits.
    last: u32,
    /// The bits loaded and not yet read, the next one in bit 0.
    acc: u64,
    /// How many bits of `acc` are loaded.
    count: u32,
    /// Why loading stopped, once it has.
    end: Option<&'static str>,
}

impl MagSgnReader {
    fn new() -> Self {
        MagSgnReader {
            pos: 0,
            last: 0,
            acc: 0,
            count: 0,
            end: None,
        }
    }

    /// Load bytes until `acc` holds more than [`REFILL_BITS`] bits or the
    /// stream ends.
    fn refill(&mut self, seg: &Segment<'_>) {
        while self.count <= REFILL_BITS && self.end.is_none() {
            let bits = if self.last == 0xFF { 7 } else { 8 };
            let byte = if self.pos < seg.pcup {
                // Below `Pcup`, so not one of the two bytes `modDcup` changes.
                let byte = u32::from(seg.data[self.pos]);
                if byte & (1 << bits) != 0 {
                    self.end = Some("MagSgn byte after 0xFF has its stuffing bit set");
                    break;
                }
                byte
            } else if self.pos == seg.pcup {
                // One 0xFF byte the encoder may have dropped is implied.
                0xFF
            } else {
                self.end = Some("MagSgn stream runs past its end");
                break;
            };
            self.acc |= (u64::from(byte) & low_bits(bits)) << self.count;
            self.count += bits;
            self.last = byte;
            self.pos += 1;
        }
    }

    #[cfg(test)]
    fn bit(&mut self, seg: &Segment<'_>) -> Result<u32> {
        // One bit fits `u32`.
        self.value(seg, 1).map(|v| v as u32)
    }

    /// `decodeMagSgnValue` (7.3.8): `m` bits, least significant first, for
    /// `m` up to 32.
    #[inline]
    fn value(&mut self, seg: &Segment<'_>, m: u32) -> Result<u64> {
        debug_assert!(m <= 32);
        if self.count < m {
            self.refill(seg);
            if self.count < m {
                return Err(malformed(
                    self.end.unwrap_or("MagSgn stream runs past its end"),
                ));
            }
        }
        let val = self.acc & low_bits(m);
        self.acc >>= m;
        self.count -= m;
        Ok(val)
    }
}

/// The MEL bit-stream and its run-length decoder (`importMELBit`, 7.1.3, and
/// `decodeMELSym`, 7.3.3).
struct MelDecoder {
    pos: usize,
    bits: u32,
    tmp: u32,
    k: usize,
    run: u32,
    one: bool,
}

/// `MEL_E` (Table 2): the run-length exponent of each MEL state.
const MEL_E: [u32; 13] = [0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 4, 5];

impl MelDecoder {
    fn new(pcup: usize) -> Self {
        MelDecoder {
            pos: pcup,
            bits: 0,
            tmp: 0,
            k: 0,
            run: 0,
            one: false,
        }
    }

    /// Most significant bit first, with a stuffing bit after every 0xFF;
    /// past the end of the segment the stream reads as ones.
    fn bit(&mut self, seg: &Segment<'_>) -> u32 {
        if self.bits == 0 {
            self.bits = if self.tmp == 0xFF { 7 } else { 8 };
            if self.pos < seg.data.len() {
                self.tmp = u32::from(seg.byte(self.pos));
                self.pos += 1;
            } else {
                self.tmp = 0xFF;
            }
        }
        self.bits -= 1;
        (self.tmp >> self.bits) & 1
    }

    #[inline]
    fn symbol(&mut self, seg: &Segment<'_>) -> u32 {
        if self.run == 0 && !self.one {
            let mut eval = MEL_E[self.k];
            if self.bit(seg) == 1 {
                self.run = 1 << eval;
                self.k = (self.k + 1).min(12);
            } else {
                self.run = 0;
                while eval > 0 {
                    self.run = 2 * self.run + self.bit(seg);
                    eval -= 1;
                }
                self.k = self.k.saturating_sub(1);
                self.one = true;
            }
        }
        if self.run > 0 {
            self.run -= 1;
            0
        } else {
            self.one = false;
            1
        }
    }
}

/// The VLC bit-stream: backward from byte `Lcup - 2`, least significant bit
/// first, skipping the stuffing bit of a byte whose low seven bits are ones
/// after a byte above 0x8F (`importVLCBit`, 7.1.4).
///
/// Like [`MagSgnReader`] it loads whole bytes ahead into `acc`. Bits past the
/// last byte read as zeros in [`peek`](Self::peek), and
/// [`consume`](Self::consume) refuses them, so a codeword that needs them is
/// an error at the same bit as when read one at a time.
struct VlcReader {
    /// The next byte to load; below `Pcup` (or below 0) the stream is spent.
    pos: Option<usize>,
    /// The last byte loaded, which decides whether the next has 7 bits.
    last: u32,
    /// The bits loaded and not yet read, the next one in bit 0.
    acc: u64,
    /// How many bits of `acc` are loaded.
    count: u32,
}

impl VlcReader {
    fn new(seg: &Segment<'_>) -> Self {
        let lcup = seg.data.len();
        let last = u32::from(seg.byte(lcup - 2));
        let tmp = last >> 4;
        let bits = if tmp & 7 < 7 { 4 } else { 3 };
        VlcReader {
            pos: lcup.checked_sub(3),
            last,
            acc: u64::from(tmp) & low_bits(bits),
            count: bits,
        }
    }

    /// Load bytes until `acc` holds more than [`REFILL_BITS`] bits or the
    /// stream reaches the MagSgn bytes.
    fn refill(&mut self, seg: &Segment<'_>) {
        while self.count <= REFILL_BITS {
            let Some(pos) = self.pos.filter(|&pos| pos >= seg.pcup) else {
                break;
            };
            // At most `Lcup - 3`, so not one of the bytes `modDcup` changes.
            let byte = u32::from(seg.data[pos]);
            let bits = if self.last > 0x8F && byte & 0x7F == 0x7F {
                7
            } else {
                8
            };
            self.acc |= (u64::from(byte) & low_bits(bits)) << self.count;
            self.count += bits;
            self.last = byte;
            self.pos = pos.checked_sub(1);
        }
    }

    /// The next bits, at least `n` of them (up to [`REFILL_BITS`]) loaded
    /// unless the stream ends first; the bits past its end read as zeros.
    #[inline]
    fn peek(&mut self, seg: &Segment<'_>, n: u32) -> u64 {
        if self.count < n {
            self.refill(seg);
        }
        self.acc
    }

    /// Drop `n` bits that [`peek`](Self::peek) showed, refusing any past the
    /// end of the stream.
    #[inline]
    fn consume(&mut self, n: u32) -> Result<()> {
        if n > self.count {
            return Err(malformed("VLC stream runs into the MagSgn bytes"));
        }
        self.acc >>= n;
        self.count -= n;
        Ok(())
    }

    #[inline]
    fn bit(&mut self, seg: &Segment<'_>) -> Result<u32> {
        self.bits(seg, 1)
    }

    /// `n` bits, least significant first, for `n` up to 32.
    #[inline]
    fn bits(&mut self, seg: &Segment<'_>, n: u32) -> Result<u32> {
        debug_assert!(n <= 32);
        let val = self.peek(seg, n) & low_bits(n);
        self.consume(n)?;
        // At most 32 bits.
        Ok(val as u32)
    }

    /// `decodeUPrefix` (7.3.6): the prefix codewords `1`, `01`, `001` and
    /// `000`, first bit lowest.
    #[inline]
    fn u_prefix(&mut self, seg: &Segment<'_>) -> Result<u32> {
        let bits = self.peek(seg, 3);
        let (prefix, len) = if bits & 1 == 1 {
            (1, 1)
        } else if bits & 2 == 2 {
            (2, 2)
        } else if bits & 4 == 4 {
            (3, 3)
        } else {
            (5, 3)
        };
        self.consume(len)?;
        Ok(prefix)
    }

    /// `decodeUSuffix` (7.3.6).
    #[inline]
    fn u_suffix(&mut self, seg: &Segment<'_>, prefix: u32) -> Result<u32> {
        match prefix {
            0..3 => Ok(0),
            3 => self.bit(seg),
            _ => self.bits(seg, 5),
        }
    }

    /// `decodeUExtension` (7.3.6).
    #[inline]
    fn u_extension(&mut self, seg: &Segment<'_>, suffix: u32) -> Result<u32> {
        if suffix < 28 {
            Ok(0)
        } else {
            self.bits(seg, 4)
        }
    }
}

/// One quad's decoded CxtVLC tuple `(rho, u_off, eps_k, eps_1)`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct QuadInfo {
    rho: u8,
    u_off: bool,
    e_k: u8,
    e_1: u8,
}

/// The longest CxtVLC codeword, in bits.
const MAX_CWD_BITS: u32 = 7;

/// A CxtVLC table as a lookup: `[c][bits]`, for context `c` and the next
/// seven bits of the stream (first bit lowest), holds the codeword those
/// bits start with, packed as `len | rho << 3 | u_off << 7 | e_k << 8 |
/// e_1 << 12`, or 0 when none does. Both tables are complete prefix codes,
/// so no entry is 0 in practice.
type VlcLookup = [[u16; 1 << MAX_CWD_BITS]; 8];

/// Fill each entry with the shortest codeword the bits start with: longer
/// rows go in first and shorter ones over them. That is the codeword a
/// bit-at-a-time search finds first, so the lookup gives the same result
/// whether or not the table is a complete prefix code.
const fn lookup(table: &[[u8; 7]]) -> VlcLookup {
    let mut out = [[0u16; 1 << MAX_CWD_BITS]; 8];
    let mut len = MAX_CWD_BITS as u8;
    while len >= 1 {
        let mut i = 0;
        while i < table.len() {
            let [c, rho, u_off, e_k, e_1, w, l_w] = table[i];
            if l_w == len {
                let packed = len as u16
                    | (rho as u16) << 3
                    | (u_off as u16) << 7
                    | (e_k as u16) << 8
                    | (e_1 as u16) << 12;
                // Every 7-bit string whose low `len` bits are `w`.
                let mut high = 0usize;
                while high < 1 << (MAX_CWD_BITS as usize - len as usize) {
                    out[c as usize][high << len | w as usize] = packed;
                    high += 1;
                }
            }
            i += 1;
        }
        len -= 1;
    }
    out
}

static VLC_LOOKUP_0: VlcLookup = lookup(&CXT_VLC_TABLE_0);
static VLC_LOOKUP_1: VlcLookup = lookup(&CXT_VLC_TABLE_1);

/// `decodeCxtVLC` (7.3.5): the codeword of context `c` the next bits spell,
/// in the table of the first row of quads or of a later one.
#[inline(always)]
fn decode_cxt_vlc(
    vlc: &mut VlcReader,
    seg: &Segment<'_>,
    first_row: bool,
    c: u8,
) -> Result<QuadInfo> {
    let lookup = if first_row {
        &VLC_LOOKUP_0
    } else {
        &VLC_LOOKUP_1
    };
    let bits = vlc.peek(seg, MAX_CWD_BITS) & low_bits(MAX_CWD_BITS);
    // `bits` is below 128.
    let hit = lookup[usize::from(c)][bits as usize];
    let len = u32::from(hit & 7);
    if len == 0 {
        // No codeword matches, as in a bit-at-a-time search, which reads all
        // seven bits first and so runs out on a short stream. Unreachable
        // with complete tables.
        vlc.consume(MAX_CWD_BITS)?;
        return Err(malformed(&format!(
            "VLC bits match no CxtVLC codeword of context {c}"
        )));
    }
    vlc.consume(len)?;
    // Each field is masked to its width before the cast.
    Ok(QuadInfo {
        rho: (hit >> 3 & 0xF) as u8,
        u_off: hit >> 7 & 1 == 1,
        e_k: (hit >> 8 & 0xF) as u8,
        e_1: (hit >> 12 & 0xF) as u8,
    })
}

/// The cleanup pass of one block: its three streams and its geometry.
struct Cleanup<'a> {
    seg: Segment<'a>,
    magsgn: MagSgnReader,
    mel: MelDecoder,
    vlc: VlcReader,
    width: usize,
    height: usize,
    /// Quads per row, `ceil(width / 2)`.
    qw: usize,
    /// The largest exponent bound `U_q` a quad may have.
    max_u: u32,
    /// The `CAP` bound on `mu` as a bit count below 64, if there is one.
    bound_bits: Option<u32>,
    /// The left shift from `2 mu + 1` to the double-scale coefficient.
    shift: u32,
}

impl Cleanup<'_> {
    /// Decode every quad into `coeffs`, row-major over the block, with
    /// `scratch`'s buffers.
    fn run(mut self, coeffs: &mut [i32], scratch: &mut CleanupScratch) -> Result<()> {
        let qw = self.qw;
        let qh = self.height.div_ceil(2);
        // The quads of the row above and of this one. Every quad of a row is
        // written before the next row reads it, so neither needs clearing.
        let CleanupScratch { above, row } = scratch;
        zeroed(above, qw);
        zeroed(row, qw);
        for qy in 0..qh {
            let first_row = qy == 0;
            // Whether the bottom row of this row's quads lies inside the
            // block; it does not in the last row of an odd height.
            let bottom = 2 * qy + 1 < self.height;
            let mut qx = 0;
            while qx < qw {
                // With an odd `QW` the last pair of a row has one quad. The
                // missing second quad reads as insignificant, with no
                // residual, so it reads no bits.
                let pair = qx + 1 < qw;
                let first = self.quad(first_row, bottom, above, row, qx)?;
                let second = if pair {
                    self.quad(first_row, bottom, above, row, qx + 1)?
                } else {
                    QuadInfo::default()
                };
                let u = self.residuals(first_row, [first, second])?;
                row[qx].exps =
                    self.quad_magnitudes(first_row, above, qx, qy, first, u[0], coeffs)?;
                if pair {
                    row[qx + 1].exps =
                        self.quad_magnitudes(first_row, above, qx + 1, qy, second, u[1], coeffs)?;
                }
                qx += 2;
            }
            std::mem::swap(above, row);
        }
        Ok(())
    }

    /// Decode the significance of the quad at column `qx` (7.3.5), refuse a
    /// significant sample outside the block, and record its `rho` in `row`.
    #[inline(always)]
    fn quad(
        &mut self,
        first_row: bool,
        bottom: bool,
        above: &[QuadState],
        row: &mut [QuadState],
        qx: usize,
    ) -> Result<QuadInfo> {
        let c = if first_row {
            first_row_context(row, qx)
        } else {
            context(above, row, qx)
        };
        let info = self.sig_emb(first_row, c)?;
        // The samples inside the block, as a `rho` mask: the right column is
        // outside in the last quad of an odd width.
        let right = 2 * qx + 1 < self.width;
        let inside =
            1 | u8::from(bottom) << 1 | u8::from(right) << 2 | u8::from(bottom && right) << 3;
        if info.rho & !inside != 0 {
            return Err(malformed("codes a significant sample outside the block"));
        }
        row[qx].rho = info.rho;
        Ok(info)
    }

    /// The exponent bound `U_q = kappa_q + u_q` of the quad at column `qx`
    /// (7.3.7), checked, then its magnitudes; returns their exponents.
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn quad_magnitudes(
        &mut self,
        first_row: bool,
        above: &[QuadState],
        qx: usize,
        qy: usize,
        info: QuadInfo,
        u: u32,
        coeffs: &mut [i32],
    ) -> Result<[u32; 4]> {
        let kappa = if first_row {
            1
        } else {
            predictor(above, qx, info.rho)
        };
        let bound = kappa + u;
        if bound > self.max_u {
            return Err(bound_error(bound, self.max_u));
        }
        self.magnitudes(qx, qy, info, bound, coeffs)
    }

    /// `decodeSigEMB` (7.3.5): an all-zero-context quad first asks MEL
    /// whether it is significant at all.
    #[inline(always)]
    fn sig_emb(&mut self, first_row: bool, c: u8) -> Result<QuadInfo> {
        if c == 0 && self.mel.symbol(&self.seg) == 0 {
            return Ok(QuadInfo::default());
        }
        decode_cxt_vlc(&mut self.vlc, &self.seg, first_row, c)
    }

    /// The unsigned residuals `u_q` of a quad-pair (7.3.4, 7.3.6): prefixes,
    /// then suffixes, then extensions, first quad before second. A lone last
    /// quad comes with a second that has no residual (`u_off` clear).
    #[inline(always)]
    fn residuals(&mut self, first_row: bool, info: [QuadInfo; 2]) -> Result<[u32; 2]> {
        let seg = self.seg;
        let [first, second] = info;
        let mut prefix = [0u32; 2];
        let mut offset = 0;
        if first_row && first.u_off && second.u_off {
            if self.mel.symbol(&seg) == 1 {
                offset = 2;
                prefix = [self.vlc.u_prefix(&seg)?, self.vlc.u_prefix(&seg)?];
            } else {
                prefix[0] = self.vlc.u_prefix(&seg)?;
                prefix[1] = if prefix[0] > 2 {
                    self.vlc.bit(&seg)? + 1
                } else {
                    self.vlc.u_prefix(&seg)?
                };
            }
        } else {
            if first.u_off {
                prefix[0] = self.vlc.u_prefix(&seg)?;
            }
            if second.u_off {
                prefix[1] = self.vlc.u_prefix(&seg)?;
            }
        }
        let suffix = [
            self.vlc.u_suffix(&seg, prefix[0])?,
            self.vlc.u_suffix(&seg, prefix[1])?,
        ];
        let mut u = [0u32; 2];
        if first.u_off {
            u[0] = offset + prefix[0] + suffix[0] + 4 * self.vlc.u_extension(&seg, suffix[0])?;
        }
        if second.u_off {
            u[1] = offset + prefix[1] + suffix[1] + 4 * self.vlc.u_extension(&seg, suffix[1])?;
        }
        Ok(u)
    }

    /// Read the magnitudes and signs of the quad at column `qx` of quad row
    /// `qy` from MagSgn (7.3.8), write its samples, and return their
    /// exponents, by `j` (0 for an insignificant sample).
    #[inline(always)]
    fn magnitudes(
        &mut self,
        qx: usize,
        qy: usize,
        info: QuadInfo,
        bound: u32,
        coeffs: &mut [i32],
    ) -> Result<[u32; 4]> {
        let mut exps = [0u32; 4];
        let top_left = 2 * qy * self.width + 2 * qx;
        // The significant samples, lowest `j` first.
        let mut rho = info.rho;
        while rho != 0 {
            let j = rho.trailing_zeros() as usize;
            rho &= rho - 1;
            let k = u32::from(info.e_k >> j & 1);
            let i = u64::from(info.e_1 >> j & 1);
            let m = bound - k;
            let v = self.magsgn.value(&self.seg, m)? | i << m;
            let mu = (v >> 1) + 1;
            // `CAP` promises every `mu` of the codestream is below its bound
            // (A.3.7).
            if let Some(bits) = self.bound_bits.filter(|&bits| mu >> bits != 0) {
                return Err(malformed(&format!(
                    "cleanup magnitude {mu} is not below 2^{bits}, the bound `CAP` signals"
                )));
            }
            exps[j] = 64 - (2 * mu - 1).leading_zeros();
            // Within the exponent bound, a `mu` with more bits than the block
            // codes (7.6) can still overflow `i32`, shifted up by the
            // cleanup plane.
            let magnitude = i32::try_from((2 * mu + 1) << self.shift)
                .map_err(|_| malformed("codes a magnitude past its bit-planes"))?;
            // `j` is 0 top-left, 1 bottom-left, 2 top-right, 3 bottom-right.
            let n = top_left + (j & 1) * self.width + (j >> 1);
            coeffs[n] = if v & 1 == 1 { -magnitude } else { magnitude };
        }
        Ok(exps)
    }
}

/// The error for a quad whose exponent bound `U_q` is over `max_u`.
#[cold]
fn bound_error(bound: u32, max_u: u32) -> Error {
    // `max_u` is `P + 2`, at most the cap; the cap's message also holds when
    // `P` is exactly 30 and nothing was capped.
    malformed(&if max_u == MAX_BIT_PLANES + 2 {
        format!(
            "exponent bound U_q = {bound} exceeds {max_u}, the most a block can code (zero \
             bit-planes + 2, at most {max_u})"
        )
    } else {
        format!("exponent bound U_q = {bound} exceeds zero bit-planes + 2 = {max_u}")
    })
}

/// What the cleanup pass keeps of one quad for the quads after it: its
/// significance pattern `rho` (bit `j` for sample `j`, 7.2) and the
/// magnitude exponent of each sample.
#[derive(Debug, Clone, Copy, Default)]
struct QuadState {
    rho: u8,
    exps: [u32; 4],
}

/// Whether sample `j` of `quad` is significant, as 0 or 1.
fn sig(quad: QuadState, j: u32) -> u8 {
    quad.rho >> j & 1
}

/// The context `c_q` of the quad at column `qx` of the first row, from the
/// quad to its left in `row` (7.3.5).
fn first_row_context(row: &[QuadState], qx: usize) -> u8 {
    let Some(left) = qx.checked_sub(1).map(|x| row[x]) else {
        return 0;
    };
    (sig(left, 0) | sig(left, 1)) + 2 * sig(left, 2) + 4 * sig(left, 3)
}

/// The context `c_q` of the quad at column `qx` below the first row, from
/// the quad to its left in `row` and the three above it in `above` (7.3.5).
fn context(above: &[QuadState], row: &[QuadState], qx: usize) -> u8 {
    let (w, sw, nw) = match qx.checked_sub(1) {
        Some(x) => (sig(row[x], 2), sig(row[x], 3), sig(above[x], 3)),
        None => (0, 0, 0),
    };
    let up = above[qx];
    let nf = above.get(qx + 1).map_or(0, |&q| sig(q, 1));
    (nw | sig(up, 1)) + 2 * (w | sw) + 4 * (sig(up, 3) | nf)
}

/// The exponent predictor `kappa_q` of the quad at column `qx` below the
/// first row, with significance pattern `rho` (7.3.7): from the exponents of
/// the bottom samples of the quads above it in `above`.
fn predictor(above: &[QuadState], qx: usize, rho: u8) -> u32 {
    if rho.count_ones() < 2 {
        return 1;
    }
    let up = above[qx].exps;
    let mut e = up[1].max(up[3]);
    if let Some(x) = qx.checked_sub(1) {
        e = e.max(above[x].exps[3]);
    }
    if let Some(q) = above.get(qx + 1) {
        e = e.max(q.exps[1]);
    }
    e.saturating_sub(1).max(1)
}

/// The SigProp bit-stream: forward from byte 0 of the refinement segment,
/// least significant bit first, with a stuffing bit after every 0xFF; past
/// the end it reads as zeros (`importSigPropBit`, 7.1.5).
struct SigPropReader<'a> {
    data: &'a [u8],
    pos: usize,
    bits: u32,
    tmp: u32,
    last: u32,
}

impl<'a> SigPropReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        SigPropReader {
            data,
            pos: 0,
            bits: 0,
            tmp: 0,
            last: 0,
        }
    }

    fn bit(&mut self) -> Result<u32> {
        if self.bits == 0 {
            self.bits = if self.last == 0xFF { 7 } else { 8 };
            self.tmp = match self.data.get(self.pos) {
                Some(&byte) => {
                    self.pos += 1;
                    u32::from(byte)
                }
                None => 0,
            };
            if self.tmp & (1 << self.bits) != 0 {
                return Err(malformed(
                    "SigProp byte after 0xFF has its stuffing bit set",
                ));
            }
            self.last = self.tmp;
        }
        let bit = self.tmp & 1;
        self.tmp >>= 1;
        self.bits -= 1;
        Ok(bit)
    }
}

/// The MagRef bit-stream: backward from the last byte of the refinement
/// segment, least significant bit first, skipping the stuffing bit of a byte
/// whose low seven bits are ones after a byte above 0x8F (the first byte
/// read counts as following one); before the start it reads as zeros
/// (`importMagRefBit`, 7.1.6).
struct MagRefReader<'a> {
    data: &'a [u8],
    /// The bytes not yet read; the next one is `data[left - 1]`.
    left: usize,
    bits: u32,
    tmp: u32,
    last: u32,
}

impl<'a> MagRefReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        MagRefReader {
            data,
            left: data.len(),
            bits: 0,
            tmp: 0,
            last: 0xFF,
        }
    }

    fn bit(&mut self) -> u32 {
        if self.bits == 0 {
            self.tmp = match self.left.checked_sub(1) {
                Some(pos) => {
                    self.left = pos;
                    u32::from(self.data[pos])
                }
                None => 0,
            };
            self.bits = if self.last > 0x8F && self.tmp & 0x7F == 0x7F {
                7
            } else {
                8
            };
            self.last = self.tmp;
        }
        let bit = self.tmp & 1;
        self.tmp >>= 1;
        self.bits -= 1;
        bit
    }
}

/// The SigProp and MagRef passes of one block, which code the plane below
/// the cleanup pass's last (7.4, 7.5).
struct Refinement {
    width: usize,
    height: usize,
    causal: bool,
    /// The cleanup pass's shift from `2 mu + 1`, at least 1: the plane
    /// refined is bit `shift` of the double-scale magnitude.
    shift: u32,
}

impl Refinement {
    /// Run the SigProp pass over `data`, then the MagRef pass if `magref`,
    /// on the cleanup pass's `coeffs`, with `scratch`'s buffers.
    fn run(
        &self,
        data: &[u8],
        magref: bool,
        coeffs: &mut [i32],
        scratch: &mut RefinementScratch,
    ) -> Result<()> {
        let RefinementScratch { state, new } = scratch;
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 {
            return Ok(());
        }
        // A border of one sample on each side, always clear, stands in for the
        // neighbors outside the block.
        zeroed(state, (w + 2) * (h + 2));
        // `sigma_n` of the cleanup pass: its significant samples are exactly
        // its nonzero ones. `r_n`, the samples SigProp makes significant,
        // starts clear.
        for (y, line) in coeffs.chunks_exact(w).enumerate() {
            let start = (y + 1) * (w + 2) + 1;
            for (s, &c) in state[start..start + w].iter_mut().zip(line) {
                *s = u8::from(c != 0) * SIGMA;
            }
        }
        self.sigprop(data, state, new, coeffs)?;
        if magref {
            self.magref(data, state, coeffs);
        }
        Ok(())
    }

    /// `decodeSigPropMag` and `decodeSigPropSign` (7.4). A sample the cleanup
    /// pass left insignificant is visited when a neighbor is significant: by
    /// the cleanup pass, or by this pass earlier in the scan. Under the
    /// vertically causal flag the next stripe holds no neighbors. A
    /// column-group's refinement bits come first, then the signs of the
    /// samples they made significant. Such a sample's magnitude is the
    /// refined plane alone, so it reconstructs to `3 << (shift - 1)`.
    ///
    /// The scan is stripe-oriented: four-line stripes top to bottom, and in
    /// each, groups of up to four columns left to right, each visited column
    /// by column, top to bottom.
    ///
    /// `state` holds [`SIGMA`] and [`REFINED`] for each sample, with its
    /// border; `new` is room for a column-group's new samples.
    fn sigprop(
        &self,
        data: &[u8],
        state: &mut [u8],
        new: &mut Vec<usize>,
        coeffs: &mut [i32],
    ) -> Result<()> {
        let (w, h) = (self.width, self.height);
        let pw = w + 2;
        let mut sp = SigPropReader::new(data);
        let magnitude = 3i32 << (self.shift - 1);
        for y0 in (0..h).step_by(4) {
            let y1 = (y0 + 4).min(h);
            for x0 in (0..w).step_by(4) {
                new.clear();
                for x in x0..(x0 + 4).min(w) {
                    for y in y0..y1 {
                        let p = (y + 1) * pw + x + 1;
                        if state[p] & SIGMA != 0 {
                            continue;
                        }
                        // The three neighbors above and the two beside; the
                        // sample itself is clear, as it is visited once.
                        let mut mbr = state[p - pw - 1]
                            | state[p - pw]
                            | state[p - pw + 1]
                            | state[p - 1]
                            | state[p + 1];
                        // The three below, unless the causal flag hides the
                        // next stripe. Past the block's last row is border.
                        if !(self.causal && y + 1 == y1) {
                            mbr |= state[p + pw - 1] | state[p + pw] | state[p + pw + 1];
                        }
                        if mbr != 0 && sp.bit()? == 1 {
                            state[p] |= REFINED;
                            new.push(y * w + x);
                        }
                    }
                }
                for &n in new.iter() {
                    coeffs[n] = if sp.bit()? == 1 {
                        -magnitude
                    } else {
                        magnitude
                    };
                }
            }
        }
        Ok(())
    }

    /// `decodeMagRefValue` (7.5): one bit for each sample the cleanup pass
    /// found significant, in the stripe-oriented scan. The bit replaces the
    /// half-plane the cleanup pass reconstructed with, and half of the plane
    /// below is added.
    fn magref(&self, data: &[u8], state: &[u8], coeffs: &mut [i32]) {
        let (w, h) = (self.width, self.height);
        let pw = w + 2;
        let mut mr = MagRefReader::new(data);
        let (center, half) = (1i32 << self.shift, 1i32 << (self.shift - 1));
        for y0 in (0..h).step_by(4) {
            let y1 = (y0 + 4).min(h);
            for x0 in (0..w).step_by(4) {
                for x in x0..(x0 + 4).min(w) {
                    for y in y0..y1 {
                        if state[(y + 1) * pw + x + 1] & SIGMA == 0 {
                            continue;
                        }
                        let n = y * w + x;
                        let c = coeffs[n];
                        let mut m = c.abs();
                        if mr.bit() == 0 {
                            m ^= center;
                        }
                        m |= half;
                        coeffs[n] = if c < 0 { -m } else { m };
                    }
                }
            }
        }
    }
}
