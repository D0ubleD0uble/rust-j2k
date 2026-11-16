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

/// The most magnitude bit-planes a code-block can have for the double-scale
/// reconstruction to fit `i32`, the bound OpenJPEG's `opj_t1_ht_decode_cblk`
/// applies to `Mb`. Under a maxshift it applies to the raised count
/// `Mb + s`.
const MAX_BIT_PLANES: u32 = 30;

/// The largest suffix length `Scup` (7.1.1).
const MAX_SCUP: usize = 4079;

/// One more than the largest cleanup segment length `Lcup` (7.1.1).
const MAX_LCUP: usize = 65535;

/// The most coding passes of one HT set: cleanup, SigProp and MagRef (B.1).
const HT_SET_PASSES: u32 = 3;

fn malformed(what: &str) -> Error {
    Error::Codestream(format!("HT code-block {what}"))
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
pub(super) fn decode(block: &CodeBlock<'_>, numbps: u32, params: Params) -> Result<Vec<i32>> {
    let (width, height) = (block.width, block.height);
    let mut coeffs = vec![0i32; width * height];
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
        return Ok(coeffs);
    }
    if planes > MAX_BIT_PLANES {
        return Err(Error::Limit(if params.roi_shift == 0 {
            format!(
                "HT code-block in a subband of {numbps} bit-planes, over the \
                 {MAX_BIT_PLANES}-plane limit"
            )
        } else {
            format!(
                "HT code-block of {planes} bit-planes (a subband of {numbps} and a \
                 maxshift of {}), over the {MAX_BIT_PLANES}-plane limit",
                params.roi_shift
            )
        }));
    }
    let zbp = block.zero_bit_planes;
    // At least one bit-plane must be left to code; OpenJPEG refuses the
    // same blocks.
    if zbp >= planes {
        return Err(malformed(&format!(
            "has {zbp} zero bit-planes in a block of {planes}, leaving none to code"
        )));
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
        // bound.
        max_u: zbp + 2,
        // A bound of 64 bits or more holds of every `u64`.
        bound_bits: params.bound_bits.filter(|&bits| bits < u64::BITS),
        shift,
    }
    .run(&mut coeffs)?;
    let z_blk = z_blk(block, planes);
    if z_blk > 1 {
        let refinement = block.segments.get(1).map(|s| s.bytes()).unwrap_or_default();
        Refinement {
            width,
            height,
            causal: params.causal,
            shift,
        }
        .run(&refinement, z_blk == HT_SET_PASSES, &mut coeffs)?;
    }
    if params.roi_shift != 0 {
        for c in &mut coeffs {
            let magnitude = undo_maxshift(c.abs(), params.roi_shift);
            *c = if *c < 0 { -magnitude } else { magnitude };
        }
    }
    Ok(coeffs)
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

/// The MagSgn bit-stream: forward from byte 0, least significant bit first,
/// with a stuffing bit after every 0xFF (`importMagSgnBit`, 7.1.2).
struct MagSgnReader {
    pos: usize,
    bits: u32,
    tmp: u32,
    last: u32,
}

impl MagSgnReader {
    fn new() -> Self {
        MagSgnReader {
            pos: 0,
            bits: 0,
            tmp: 0,
            last: 0,
        }
    }

    fn bit(&mut self, seg: &Segment<'_>) -> Result<u32> {
        if self.bits == 0 {
            self.bits = if self.last == 0xFF { 7 } else { 8 };
            if self.pos < seg.pcup {
                self.tmp = u32::from(seg.byte(self.pos));
                if self.tmp & (1 << self.bits) != 0 {
                    return Err(malformed("MagSgn byte after 0xFF has its stuffing bit set"));
                }
            } else if self.pos == seg.pcup {
                // One 0xFF byte the encoder may have dropped is implied.
                self.tmp = 0xFF;
            } else {
                return Err(malformed("MagSgn stream runs past its end"));
            }
            self.last = self.tmp;
            self.pos += 1;
        }
        let bit = self.tmp & 1;
        self.tmp >>= 1;
        self.bits -= 1;
        Ok(bit)
    }

    /// `decodeMagSgnValue` (7.3.8): `m` bits, least significant first.
    fn value(&mut self, seg: &Segment<'_>, m: u32) -> Result<u64> {
        let mut val = 0u64;
        for i in 0..m {
            val |= u64::from(self.bit(seg)?) << i;
        }
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
struct VlcReader {
    /// The next byte to read; below `Pcup` (or below 0) the stream is spent.
    pos: Option<usize>,
    bits: u32,
    tmp: u32,
    last: u32,
}

impl VlcReader {
    fn new(seg: &Segment<'_>) -> Self {
        let lcup = seg.data.len();
        let last = u32::from(seg.byte(lcup - 2));
        let tmp = last >> 4;
        VlcReader {
            pos: lcup.checked_sub(3),
            bits: if tmp & 7 < 7 { 4 } else { 3 },
            tmp,
            last,
        }
    }

    fn bit(&mut self, seg: &Segment<'_>) -> Result<u32> {
        if self.bits == 0 {
            let pos = self
                .pos
                .filter(|&pos| pos >= seg.pcup)
                .ok_or_else(|| malformed("VLC stream runs into the MagSgn bytes"))?;
            self.tmp = u32::from(seg.byte(pos));
            self.bits = if self.last > 0x8F && self.tmp & 0x7F == 0x7F {
                7
            } else {
                8
            };
            self.last = self.tmp;
            self.pos = pos.checked_sub(1);
        }
        let bit = self.tmp & 1;
        self.tmp >>= 1;
        self.bits -= 1;
        Ok(bit)
    }

    /// `n` bits, least significant first.
    fn bits(&mut self, seg: &Segment<'_>, n: u32) -> Result<u32> {
        let mut val = 0;
        for i in 0..n {
            val |= self.bit(seg)? << i;
        }
        Ok(val)
    }

    /// `decodeUPrefix` (7.3.6).
    fn u_prefix(&mut self, seg: &Segment<'_>) -> Result<u32> {
        if self.bit(seg)? == 1 {
            return Ok(1);
        }
        if self.bit(seg)? == 1 {
            return Ok(2);
        }
        Ok(if self.bit(seg)? == 1 { 3 } else { 5 })
    }

    /// `decodeUSuffix` (7.3.6).
    fn u_suffix(&mut self, seg: &Segment<'_>, prefix: u32) -> Result<u32> {
        match prefix {
            0..3 => Ok(0),
            3 => self.bit(seg),
            _ => self.bits(seg, 5),
        }
    }

    /// `decodeUExtension` (7.3.6).
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

/// A CxtVLC table as a lookup: `[c][len][cwd]` holds one more than the index
/// of the row whose `len`-bit codeword is `cwd` in context `c`, or 0.
type VlcLookup = [[[u16; 128]; 8]; 8];

const fn lookup(table: &[[u8; 7]]) -> VlcLookup {
    let mut out = [[[0u16; 128]; 8]; 8];
    let mut i = 0;
    while i < table.len() {
        let [c, _, _, _, _, w, l_w] = table[i];
        out[c as usize][l_w as usize][w as usize] = i as u16 + 1;
        i += 1;
    }
    out
}

static VLC_LOOKUP_0: VlcLookup = lookup(&CXT_VLC_TABLE_0);
static VLC_LOOKUP_1: VlcLookup = lookup(&CXT_VLC_TABLE_1);

/// `decodeCxtVLC` (7.3.5): read bits until they spell a codeword of context
/// `c` in the table of the first row of quads or of a later one.
fn decode_cxt_vlc(
    vlc: &mut VlcReader,
    seg: &Segment<'_>,
    first_row: bool,
    c: u8,
) -> Result<QuadInfo> {
    let (table, lookup): (&[[u8; 7]], &VlcLookup) = if first_row {
        (&CXT_VLC_TABLE_0, &VLC_LOOKUP_0)
    } else {
        (&CXT_VLC_TABLE_1, &VLC_LOOKUP_1)
    };
    let mut cwd = 0usize;
    // Entry 0 of each context is for zero-bit codewords, which no row has.
    for (len, codewords) in lookup[usize::from(c)].iter().enumerate().skip(1) {
        cwd |= (vlc.bit(seg)? as usize) << (len - 1);
        let hit = codewords[cwd];
        if hit != 0 {
            let [_, rho, u_off, e_k, e_1, _, _] = table[usize::from(hit - 1)];
            return Ok(QuadInfo {
                rho,
                u_off: u_off == 1,
                e_k,
                e_1,
            });
        }
    }
    Err(malformed(&format!(
        "VLC bits match no CxtVLC codeword of context {c}"
    )))
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
    /// Decode every quad into `coeffs`, row-major over the block.
    fn run(mut self, coeffs: &mut [i32]) -> Result<()> {
        let qw = self.qw;
        let qh = self.height.div_ceil(2);
        // Significance and magnitude exponent of every sample, by its
        // quad-scan index `n = 4q + j` (7.2): `j` is 0 top-left, 1
        // bottom-left, 2 top-right, 3 bottom-right.
        let mut sigma = vec![false; 4 * qw * qh];
        let mut exps = vec![0u32; 4 * qw * qh];
        for qy in 0..qh {
            let first_row = qy == 0;
            for qx in (0..qw).step_by(2) {
                let q1 = qy * qw + qx;
                // With an odd `QW` the last pair of a row has one quad.
                let quads = [q1, q1 + 1];
                let pair = &quads[..if qx + 1 < qw { 2 } else { 1 }];
                let mut info = [QuadInfo::default(); 2];
                for (slot, &q) in pair.iter().enumerate() {
                    let c = self.context(&sigma, q);
                    info[slot] = self.sig_emb(first_row, c)?;
                    for j in 0..4 {
                        let on = info[slot].rho >> j & 1 == 1;
                        if on && !self.inside(q, j) {
                            return Err(malformed("codes a significant sample outside the block"));
                        }
                        sigma[4 * q + j] = on;
                    }
                }
                let u = self.residuals(first_row, &info[..pair.len()])?;
                for (slot, &q) in pair.iter().enumerate() {
                    let kappa = if first_row {
                        1
                    } else {
                        self.predictor(&exps, q, info[slot].rho)
                    };
                    let bound = kappa + u[slot];
                    if bound > self.max_u {
                        return Err(malformed(&format!(
                            "exponent bound U_q = {bound} exceeds zero bit-planes + 2 = {}",
                            self.max_u
                        )));
                    }
                    self.magnitudes(q, info[slot], bound, &mut exps, coeffs)?;
                }
            }
        }
        Ok(())
    }

    /// Whether sample `j` of quad `q` lies inside the block.
    fn inside(&self, q: usize, j: usize) -> bool {
        let x = 2 * (q % self.qw) + (j >> 1);
        let y = 2 * (q / self.qw) + (j & 1);
        x < self.width && y < self.height
    }

    /// The context `c_q` of quad `q` from its decoded neighbors (7.3.5).
    fn context(&self, sigma: &[bool], q: usize) -> u8 {
        let qw = self.qw;
        let s = |n: usize| u8::from(sigma[n]);
        // The quad to the left, in the same row: its right column.
        let (w, sw) = if !q.is_multiple_of(qw) {
            (s(4 * q - 2), s(4 * q - 1))
        } else {
            (0, 0)
        };
        if q < qw {
            let (f, sf) = if q > 0 {
                (s(4 * q - 4), s(4 * q - 3))
            } else {
                (0, 0)
            };
            (f | sf) + 2 * w + 4 * sw
        } else {
            let up = 4 * (q - qw);
            let n = s(up + 1);
            let ne = s(up + 3);
            let nw = if !q.is_multiple_of(qw) { s(up - 1) } else { 0 };
            let nf = if !(q + 1).is_multiple_of(qw) {
                s(up + 5)
            } else {
                0
            };
            (nw | n) + 2 * (w | sw) + 4 * (ne | nf)
        }
    }

    /// `decodeSigEMB` (7.3.5): an all-zero-context quad first asks MEL
    /// whether it is significant at all.
    fn sig_emb(&mut self, first_row: bool, c: u8) -> Result<QuadInfo> {
        if c == 0 && self.mel.symbol(&self.seg) == 0 {
            return Ok(QuadInfo::default());
        }
        decode_cxt_vlc(&mut self.vlc, &self.seg, first_row, c)
    }

    /// The unsigned residuals `u_q` of a quad-pair (7.3.4, 7.3.6): prefixes,
    /// then suffixes, then extensions, first quad before second.
    fn residuals(&mut self, first_row: bool, info: &[QuadInfo]) -> Result<[u32; 2]> {
        let seg = self.seg;
        let both = info.len() == 2 && info[0].u_off && info[1].u_off;
        let mut prefix = [0u32; 2];
        let mut offset = 0;
        if first_row && both {
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
            for (p, quad) in prefix.iter_mut().zip(info) {
                if quad.u_off {
                    *p = self.vlc.u_prefix(&seg)?;
                }
            }
        }
        let mut suffix = [0u32; 2];
        for (s, &p) in suffix.iter_mut().zip(&prefix) {
            *s = self.vlc.u_suffix(&seg, p)?;
        }
        let mut u = [0u32; 2];
        for ((u, quad), (&p, &s)) in u.iter_mut().zip(info).zip(prefix.iter().zip(&suffix)) {
            if quad.u_off {
                *u = offset + p + s + 4 * self.vlc.u_extension(&seg, s)?;
            }
        }
        Ok(u)
    }

    /// The exponent predictor `kappa_q` of quad `q` below the first row
    /// (7.3.7), from the exponents of the row above.
    fn predictor(&self, exps: &[u32], q: usize, rho: u8) -> u32 {
        if rho.count_ones() < 2 {
            return 1;
        }
        let qw = self.qw;
        let up = 4 * (q - qw);
        let mut e = exps[up + 1].max(exps[up + 3]);
        if !q.is_multiple_of(qw) {
            e = e.max(exps[up - 1]);
        }
        if !(q + 1).is_multiple_of(qw) {
            e = e.max(exps[up + 5]);
        }
        e.saturating_sub(1).max(1)
    }

    /// Read quad `q`'s magnitudes and signs from MagSgn (7.3.8), record their
    /// exponents, and write its samples.
    fn magnitudes(
        &mut self,
        q: usize,
        info: QuadInfo,
        bound: u32,
        exps: &mut [u32],
        coeffs: &mut [i32],
    ) -> Result<()> {
        for j in 0..4 {
            if info.rho >> j & 1 == 0 {
                continue;
            }
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
            exps[4 * q + j] = 64 - (2 * mu - 1).leading_zeros();
            // Within the exponent bound, only a `mu` of `P + 2` bits, which
            // the block cannot code (7.6), overflows `i32`, and only at
            // `Mb = 30`.
            let magnitude = i32::try_from((2 * mu + 1) << self.shift)
                .map_err(|_| malformed("codes a magnitude past its bit-planes"))?;
            let x = 2 * (q % self.qw) + (j >> 1);
            let y = 2 * (q / self.qw) + (j & 1);
            coeffs[y * self.width + x] = if v & 1 == 1 { -magnitude } else { magnitude };
        }
        Ok(())
    }
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
    /// on the cleanup pass's `coeffs`.
    fn run(&self, data: &[u8], magref: bool, coeffs: &mut [i32]) -> Result<()> {
        // `sigma_n` of the cleanup pass: its significant samples are exactly
        // its nonzero ones.
        let sigma: Vec<bool> = coeffs.iter().map(|&c| c != 0).collect();
        self.sigprop(data, &sigma, coeffs)?;
        if magref {
            self.magref(data, &sigma, coeffs);
        }
        Ok(())
    }

    /// The stripe-oriented scan (7.4), by column-group: four-line stripes top
    /// to bottom, and in each, groups of up to four columns left to right.
    /// Each item is a group's rows `y0..y1` and columns `x0..x1`, which the
    /// scan visits column by column, top to bottom.
    fn groups(&self) -> impl Iterator<Item = (usize, usize, usize, usize)> + use<> {
        let (w, h) = (self.width, self.height);
        (0..h).step_by(4).flat_map(move |y0| {
            (0..w)
                .step_by(4)
                .map(move |x0| (y0, (y0 + 4).min(h), x0, (x0 + 4).min(w)))
        })
    }

    /// `decodeSigPropMag` and `decodeSigPropSign` (7.4). A sample the cleanup
    /// pass left insignificant is visited when a neighbor is significant: by
    /// the cleanup pass, or by this pass earlier in the scan. Under the
    /// vertically causal flag the next stripe holds no neighbors. A
    /// column-group's refinement bits come first, then the signs of the
    /// samples they made significant. Such a sample's magnitude is the
    /// refined plane alone, so it reconstructs to `3 << (shift - 1)`.
    fn sigprop(&self, data: &[u8], sigma: &[bool], coeffs: &mut [i32]) -> Result<()> {
        let w = self.width;
        let mut sp = SigPropReader::new(data);
        // `r_n`: the samples this pass made significant.
        let mut r = vec![false; coeffs.len()];
        let magnitude = 3i32 << (self.shift - 1);
        let mut new = Vec::with_capacity(16);
        for (y0, y1, x0, x1) in self.groups() {
            let rows = if self.causal { y1 } else { self.height };
            new.clear();
            for x in x0..x1 {
                for y in y0..y1 {
                    let n = y * w + x;
                    if sigma[n] {
                        continue;
                    }
                    let mut mbr = false;
                    for ny in y.saturating_sub(1)..(y + 2).min(rows) {
                        for nx in x.saturating_sub(1)..(x + 2).min(w) {
                            let m = ny * w + nx;
                            mbr |= sigma[m] || r[m];
                        }
                    }
                    if mbr && sp.bit()? == 1 {
                        r[n] = true;
                        new.push(n);
                    }
                }
            }
            for &n in &new {
                coeffs[n] = if sp.bit()? == 1 {
                    -magnitude
                } else {
                    magnitude
                };
            }
        }
        Ok(())
    }

    /// `decodeMagRefValue` (7.5): one bit for each sample the cleanup pass
    /// found significant, in the stripe-oriented scan. The bit replaces the
    /// half-plane the cleanup pass reconstructed with, and half of the plane
    /// below is added.
    fn magref(&self, data: &[u8], sigma: &[bool], coeffs: &mut [i32]) {
        let w = self.width;
        let mut mr = MagRefReader::new(data);
        let (center, half) = (1i32 << self.shift, 1i32 << (self.shift - 1));
        for (y0, y1, x0, x1) in self.groups() {
            for x in x0..x1 {
                for y in y0..y1 {
                    let n = y * w + x;
                    if !sigma[n] {
                        continue;
                    }
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
