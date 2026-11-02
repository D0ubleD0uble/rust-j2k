//! `pow` and `log` for the ICC transform, the same on every platform with a
//! correctly rounded `fma`.
//!
//! LittleCMS calls the C library's `pow` and `log`, and the reference output
//! comes from glibc 2.39 on x86-64. A platform's own `f64::powf` and `f64::ln`
//! can differ from glibc in the last bit, which moves an output level, so
//! these are a port of glibc's code instead.
//!
//! glibc 2.39's `pow` and `log` (`sysdeps/ieee754/dbl-64/e_pow.c` and
//! `e_log.c`) are Szabolcs Nagy's code from Arm's optimized-routines. On
//! x86-64, glibc builds them twice and picks one at load time: a variant
//! compiled with `-mfma -mavx2` on CPUs with FMA and AVX2, and a plain SSE2
//! one otherwise (`sysdeps/x86_64/fpu/multiarch`). The reference machine
//! runs the FMA variant. In it, the code takes its `__FP_FAST_FMA` branches,
//! and GCC fuses some multiply-adds of the C source into single `fma`
//! instructions. This port follows the machine code of that variant in
//! Ubuntu's `libm.so.6` (2.39-0ubuntu8.7): each `mul_add` below is a fused
//! multiply-add there, and every other operation is a plain IEEE 754 one.
//! `f64::mul_add` rounds once wherever the platform's `fma` is correctly
//! rounded, so the results are the same there. MinGW's (`*-pc-windows-gnu`)
//! may round twice (rust-lang/rust#140515).
//!
//! The SSE2 variant differs from the FMA one in the last bit on fewer than one
//! input in a thousand, so glibc on an x86-64 CPU without FMA can disagree with
//! these functions.
//!
//! Only round-to-nearest is supported, and errno and the floating-point
//! exception flags are not modelled. A NaN result is a NaN, but its sign and
//! payload are not specified.
//!
//! Ported from `math/pow.c`, `math/pow_common.h`, `math/log.c` and
//! `math/math_err.c` of Arm's optimized-routines
//! (github.com/ARM-software/optimized-routines, commit
//! `2ee4b0d45680a9880412c1d186224b08652c35f6`), which is
//! `MIT OR Apache-2.0 WITH LLVM-exception`. It is used here under the MIT
//! licence, from the `LICENSE` file at that commit:
//!
//! > MIT License
//! >
//! > Copyright (c) 1999-2022, Arm Limited.
//! >
//! > Permission is hereby granted, free of charge, to any person obtaining a
//! > copy of this software and associated documentation files (the
//! > "Software"), to deal in the Software without restriction, including
//! > without limitation the rights to use, copy, modify, merge, publish,
//! > distribute, sublicense, and/or sell copies of the Software, and to
//! > permit persons to whom the Software is furnished to do so, subject to
//! > the following conditions:
//! >
//! > The above copyright notice and this permission notice shall be included
//! > in all copies or substantial portions of the Software.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
//! > OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
//! > MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN
//! > NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
//! > DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
//! > OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE
//! > USE OR OTHER DEALINGS IN THE SOFTWARE.
//!
//! The source files carry "Copyright (c) 2018-2026, Arm Limited" and
//! "SPDX-License-Identifier: MIT OR Apache-2.0 WITH LLVM-exception".

use super::math_data::{
    EXP_POLY, EXP_TAB, LOG_POLY, LOG_POLY1, LOG_TAB, POW_LOG_POLY, POW_LOG_TAB,
};

/// `ln2hi`, 0x1.62e42fefa3800p-1 (`__log_data` and `__pow_log_data`).
const LN2HI: f64 = f64::from_bits(0x3fe6_2e42_fefa_3800);
/// `ln2lo`, 0x1.ef35793c76730p-45.
const LN2LO: f64 = f64::from_bits(0x3d2e_f357_93c7_6730);
/// `__exp_data.invln2N`, 0x1.71547652b82fep0 * 128.
const INV_LN2_N: f64 = f64::from_bits(0x4067_1547_652b_82fe);
/// `__exp_data.negln2hiN`, -0x1.62e42fefa0000p-8.
const NEG_LN2_HI_N: f64 = f64::from_bits(0xbf76_2e42_fefa_0000);
/// `__exp_data.negln2loN`, -0x1.cf79abc9e3b3ap-47.
const NEG_LN2_LO_N: f64 = f64::from_bits(0xbd0c_f79a_bc9e_3b3a);
/// `__exp_data.shift`, 0x1.8p52.
const SHIFT: f64 = f64::from_bits(0x4338_0000_0000_0000);

/// 0x1p27.
const TWO_27: f64 = 134_217_728.0;
/// 0x1p52.
const TWO_52: f64 = 4_503_599_627_370_496.0;
/// 0x1p1009.
const TWO_1009: f64 = f64::from_bits((1023 + 1009) << 52);
/// 0x1p-1022.
const TWO_M1022: f64 = f64::MIN_POSITIVE;

const SIGN_MASK: u64 = 0x8000_0000_0000_0000;
const ONE_BITS: u64 = 0x3ff0_0000_0000_0000;
const INF_BITS: u64 = 0x7ff0_0000_0000_0000;

/// `N` for every table: `1 << 7`.
const N: u64 = 128;
/// The table index shift, `52 - 7`.
const TABLE_SHIFT: u32 = 45;
/// `SIGN_BIAS`, `0x800 << EXP_TABLE_BITS`.
const SIGN_BIAS: u32 = 0x800 << 7;

/// The top 12 bits of a double (sign and exponent).
fn top12(x: f64) -> u32 {
    (x.to_bits() >> 52) as u32
}

/// `__math_oflow`: overflow to an infinity of the given sign.
fn oflow(sign: u32) -> f64 {
    let y = f64::from_bits((1023 + 769) << 52); // 0x1p769
    (if sign != 0 { -y } else { y }) * y
}

/// `__math_uflow`: underflow to a zero of the given sign.
fn uflow(sign: u32) -> f64 {
    let y = f64::from_bits((1023 - 767) << 52); // 0x1p-767
    (if sign != 0 { -y } else { y }) * y
}

/// `__math_divzero`: an infinity of the given sign.
fn divzero(sign: u32) -> f64 {
    (if sign != 0 { -1.0 } else { 1.0 }) / 0.0
}

/// `__math_invalid`: a NaN. glibc computes `(x - x) / (x - x)`; the sign and
/// payload of a NaN are not specified here.
fn invalid() -> f64 {
    f64::NAN
}

/// `issignaling_inline` for IEEE 754-2008 NaNs.
fn issignaling(x: f64) -> bool {
    let ix = x.to_bits();
    (ix ^ 0x0008_0000_0000_0000).wrapping_mul(2) > 0x7ff8_0000_0000_0000_u64.wrapping_mul(2)
}

/// The natural logarithm, `log` of glibc 2.39's x86-64 FMA variant.
pub(crate) fn log(x: f64) -> f64 {
    let a = |i: usize| f64::from_bits(LOG_POLY[i]);
    let b = |i: usize| f64::from_bits(LOG_POLY1[i]);
    let mut ix = x.to_bits();
    let top = (ix >> 48) as u32;

    // asuint64(1.0 - 0x1p-4) and asuint64(1.0 + 0x1.09p-4).
    const LO: u64 = 0x3fee_0000_0000_0000;
    const HI: u64 = 0x3ff1_0900_0000_0000;
    if ix.wrapping_sub(LO) < HI - LO {
        // Close to 1.0.
        if ix == ONE_BITS {
            return 0.0;
        }
        let r = x - 1.0;
        let r2 = r * r;
        let r3 = r * r2;
        let p0 = r2.mul_add(b(3), r.mul_add(b(2), b(1)));
        let p1 = r2.mul_add(b(6), r.mul_add(b(5), b(4)));
        let p2 = r3.mul_add(b(10), r2.mul_add(b(9), r.mul_add(b(8), b(7))));
        let p = r3.mul_add(r3.mul_add(p2, p1), p0);
        // w = r * 0x1p27; rhi = r + w - w, with both uses of w fused.
        let rhi = (-r).mul_add(TWO_27, r.mul_add(TWO_27, r));
        let rlo = r - rhi;
        // w = rhi * rhi * B[0], fused into hi and lo.
        let rr = rhi * rhi;
        let hi = rr.mul_add(b(0), r);
        let lo = rr.mul_add(b(0), r - hi);
        let lo = (b(0) * rlo).mul_add(rhi + r, lo);
        let y = r3.mul_add(p, lo);
        return y + hi;
    }
    if top.wrapping_sub(0x0010) >= 0x7ff0 - 0x0010 {
        // x < 0x1p-1022 or inf or nan.
        if ix.wrapping_mul(2) == 0 {
            return divzero(1);
        }
        if ix == INF_BITS {
            return x;
        }
        if (top & 0x8000) != 0 || (top & 0x7ff0) == 0x7ff0 {
            return invalid();
        }
        // Subnormal: normalise it.
        ix = (x * TWO_52).to_bits().wrapping_sub(52 << 52);
    }

    // x = 2^k z, with z in [OFF, 2*OFF) and exact. The range is split into N
    // subintervals; the ith contains z and c is near its centre.
    const OFF: u64 = 0x3fe6_0000_0000_0000;
    let tmp = ix.wrapping_sub(OFF);
    let i = ((tmp >> TABLE_SHIFT) % N) as usize;
    let k = ((tmp as i64) >> 52) as i32;
    let iz = ix.wrapping_sub(tmp & (0xfff << 52));
    let invc = f64::from_bits(LOG_TAB[i][0]);
    let logc = f64::from_bits(LOG_TAB[i][1]);
    let z = f64::from_bits(iz);

    // log(x) = log1p(z/c-1) + log(c) + k*Ln2, with r ~= z/c - 1.
    let r = z.mul_add(invc, -1.0);
    let kd = f64::from(k);

    // hi + lo = r + log(c) + k*Ln2.
    let w = kd.mul_add(LN2HI, logc);
    let hi = w + r;
    let lo = kd.mul_add(LN2LO, w - hi + r);

    // log(x) = lo + (log1p(r) - r) + hi.
    let r2 = r * r;
    let q = r2.mul_add(r.mul_add(a(4), a(3)), r.mul_add(a(2), a(1)));
    (r * r2).mul_add(q, r2.mul_add(a(0), lo)) + hi
}

/// Returns 0 if not an integer, 1 if an odd one, 2 if an even one. The
/// argument is the bit pattern of a non-zero finite value.
fn checkint(iy: u64) -> u32 {
    let e = (iy >> 52 & 0x7ff) as i32;
    if e < 0x3ff {
        return 0;
    }
    if e > 0x3ff + 52 {
        return 2;
    }
    let sh = (0x3ff + 52 - e) as u32;
    if iy & ((1u64 << sh) - 1) != 0 {
        return 0;
    }
    if iy & (1u64 << sh) != 0 {
        return 1;
    }
    2
}

/// True for the bit pattern of 0, infinity or NaN.
fn zeroinfnan(i: u64) -> bool {
    i.wrapping_mul(2).wrapping_sub(1) >= INF_BITS.wrapping_mul(2) - 1
}

/// `log_inline` of `pow`: `(y, tail)` with `y + tail = log(x)`, `tail`
/// carrying about 15 more bits. `ix` is the bit pattern of `x`, normalised in
/// the subnormal range using the sign bit for the exponent.
fn pow_log_inline(ix: u64) -> (f64, f64) {
    let a = |i: usize| f64::from_bits(POW_LOG_POLY[i]);
    const OFF: u64 = 0x3fe6_9555_0000_0000;
    let tmp = ix.wrapping_sub(OFF);
    let i = ((tmp >> TABLE_SHIFT) % N) as usize;
    let k = ((tmp as i64) >> 52) as i32;
    let iz = ix.wrapping_sub(tmp & (0xfff << 52));
    let z = f64::from_bits(iz);
    let kd = f64::from(k);

    // log(x) = k*Ln2 + log(c) + log1p(z/c-1).
    let [invc, logc, logctail] = POW_LOG_TAB[i].map(f64::from_bits);

    // 1/c is j/N or j/N/2 with j an integer in [N, 2N) and |z/c - 1| < 1/N,
    // so r = z/c - 1 is exact.
    let r = z.mul_add(invc, -1.0);

    // k*Ln2 + log(c) + r.
    let t1 = kd.mul_add(LN2HI, logc);
    let t2 = t1 + r;
    let lo1 = kd.mul_add(LN2LO, logctail);
    let lo2 = t1 - t2 + r;

    // k*Ln2 + log(c) + r + A[0]*r*r, with A[0] = -0.5.
    let ar = a(0) * r;
    let ar2 = r * ar;
    let ar3 = r * ar2;
    let hi = t2 + ar2;
    let lo3 = ar.mul_add(r, -ar2);
    let lo4 = t2 - hi + ar2;

    // p = log1p(r) - r - A[0]*r*r.
    let q = ar2.mul_add(r.mul_add(a(6), a(5)), r.mul_add(a(4), a(3)));
    let q = ar2.mul_add(q, r.mul_add(a(2), a(1)));
    let lo = ar3.mul_add(q, lo1 + lo2 + lo3 + lo4);
    let y = hi + lo;
    (y, hi - y + lo)
}

/// `specialcase` of `exp_inline`: `scale * (1 + tmp)` where it may overflow
/// or underflow. `sbits` is the bit pattern of `scale` with an exponent that
/// may have overflowed into the sign bit; `ki as i32` is the reduction's `k`.
fn exp_specialcase(tmp: f64, mut sbits: u64, ki: u64) -> f64 {
    if ki & 0x8000_0000 == 0 {
        // k > 0: the exponent of scale may have overflowed by up to 460.
        sbits = sbits.wrapping_sub(1009 << 52);
        let scale = f64::from_bits(sbits);
        return TWO_1009 * scale.mul_add(tmp, scale);
    }
    // k < 0: take care in the subnormal range. sbits is the signed scale.
    sbits = sbits.wrapping_add(1022 << 52);
    let scale = f64::from_bits(sbits);
    // `scale * tmp` has a second use below, so glibc does not fuse it here.
    let st = scale * tmp;
    let mut y = scale + st;
    if y.abs() < 1.0 {
        // Round y to the right precision before scaling it into the subnormal
        // range, to avoid double rounding.
        let one = if y < 0.0 { -1.0 } else { 1.0 };
        let lo = scale - y + st;
        let hi = one + y;
        let lo = one - hi + y + lo;
        y = (hi + lo) - one;
        // Fix the sign of 0.
        if y == 0.0 {
            y = f64::from_bits(sbits & SIGN_MASK);
        }
    }
    TWO_M1022 * y
}

/// `exp_inline`: `sign * exp(x + xtail)` with `|xtail| < 2^-8/N` and
/// `|xtail| <= |x|`. `sign_bias` is `SIGN_BIAS` or 0, for -1 or 1.
fn exp_inline(x: f64, xtail: f64, sign_bias: u32) -> f64 {
    let c = |i: usize| f64::from_bits(EXP_POLY[i]);
    // top12(0x1p-54) and top12(512.0), top12(1024.0).
    const TOP_TINY: u32 = 0x3c9;
    const TOP_512: u32 = 0x408;
    const TOP_1024: u32 = 0x409;
    let mut abstop = top12(x) & 0x7ff;
    if abstop.wrapping_sub(TOP_TINY) >= TOP_512 - TOP_TINY {
        if abstop.wrapping_sub(TOP_TINY) >= 0x8000_0000 {
            // Tiny x (0 is a common input).
            let one = 1.0 + x;
            return if sign_bias != 0 { -one } else { one };
        }
        if abstop >= TOP_1024 {
            // inf and nan are already handled.
            return if x.to_bits() >> 63 != 0 {
                uflow(sign_bias)
            } else {
                oflow(sign_bias)
            };
        }
        // Large x is special-cased below.
        abstop = 0;
    }

    // exp(x) = 2^(k/N) * exp(r), with exp(r) in [2^(-1/2N), 2^(1/2N)] and
    // x = ln2/N*k + r, k an integer and r in [-ln2/2N, ln2/2N].
    let kd = x.mul_add(INV_LN2_N, SHIFT);
    let ki = kd.to_bits();
    let kd = kd - SHIFT;
    let r = kd.mul_add(NEG_LN2_LO_N, kd.mul_add(NEG_LN2_HI_N, x));
    let r = r + xtail;
    // 2^(k/N) ~= scale * (1 + tail).
    let idx = (2 * (ki % N)) as usize;
    let top = ki.wrapping_add(u64::from(sign_bias)) << (52 - 7);
    let tail = f64::from_bits(EXP_TAB[idx]);
    // A valid scale only when -1023*N < k < 1024*N.
    let sbits = EXP_TAB[idx + 1].wrapping_add(top);
    // exp(x) = 2^(k/N) * exp(r) ~= scale + scale * (tail + exp(r) - 1).
    let r2 = r * r;
    let tmp = r2.mul_add(r.mul_add(c(1), c(0)), tail + r);
    let tmp = (r2 * r2).mul_add(r.mul_add(c(3), c(2)), tmp);
    if abstop == 0 {
        return exp_specialcase(tmp, sbits, ki);
    }
    let scale = f64::from_bits(sbits);
    scale.mul_add(tmp, scale)
}

/// `x` to the power `y`, `pow` of glibc 2.39's x86-64 FMA variant.
pub(crate) fn pow(x: f64, y: f64) -> f64 {
    let mut sign_bias = 0;
    let mut ix = x.to_bits();
    let iy = y.to_bits();
    let mut topx = top12(x);
    let topy = top12(y);
    if topx.wrapping_sub(0x001) >= 0x7ff - 0x001
        || (topy & 0x7ff).wrapping_sub(0x3be) >= 0x43e - 0x3be
    {
        // If |y| > 1075 * ln2 * 2^53 ~= 0x1.749p62 then pow(x,y) = inf/0, and
        // if |y| < 2^-54 / 1075 ~= 0x1.e7b6p-65 then pow(x,y) = +-1.
        // Special cases: (x < 0x1p-126 or inf or nan) or
        // (|y| < 0x1p-65 or |y| >= 0x1p63 or nan).
        if zeroinfnan(iy) {
            if iy.wrapping_mul(2) == 0 {
                return if issignaling(x) { x + y } else { 1.0 };
            }
            if ix == ONE_BITS {
                return if issignaling(y) { x + y } else { 1.0 };
            }
            if ix.wrapping_mul(2) > INF_BITS.wrapping_mul(2)
                || iy.wrapping_mul(2) > INF_BITS.wrapping_mul(2)
            {
                return x + y;
            }
            if ix.wrapping_mul(2) == ONE_BITS.wrapping_mul(2) {
                return 1.0;
            }
            if (ix.wrapping_mul(2) < ONE_BITS.wrapping_mul(2)) == (iy >> 63 == 0) {
                // |x| < 1 && y == inf, or |x| > 1 && y == -inf.
                return 0.0;
            }
            return y * y;
        }
        if zeroinfnan(ix) {
            let mut x2 = x * x;
            if ix >> 63 != 0 && checkint(iy) == 1 {
                x2 = -x2;
                sign_bias = 1;
            }
            if ix.wrapping_mul(2) == 0 && iy >> 63 != 0 {
                return divzero(sign_bias);
            }
            return if iy >> 63 != 0 { 1.0 / x2 } else { x2 };
        }
        // Here x and y are non-zero and finite.
        if ix >> 63 != 0 {
            // Finite x < 0.
            let yint = checkint(iy);
            if yint == 0 {
                return invalid();
            }
            if yint == 1 {
                sign_bias = SIGN_BIAS;
            }
            ix &= !SIGN_MASK;
            topx &= 0x7ff;
        }
        if (topy & 0x7ff).wrapping_sub(0x3be) >= 0x43e - 0x3be {
            // sign_bias == 0 here because y is not odd.
            if ix == ONE_BITS {
                return 1.0;
            }
            if (topy & 0x7ff) < 0x3be {
                // |y| < 2^-65, x^y ~= 1 + y*log(x).
                return if ix > ONE_BITS { 1.0 + y } else { 1.0 - y };
            }
            return if (ix > ONE_BITS) == (topy < 0x800) {
                oflow(0)
            } else {
                uflow(0)
            };
        }
        if topx == 0 {
            // Normalise subnormal x so the exponent becomes negative.
            ix = (x * TWO_52).to_bits() & !SIGN_MASK;
            ix = ix.wrapping_sub(52 << 52);
        }
    }

    let (hi, lo) = pow_log_inline(ix);
    let ehi = y * hi;
    let elo = y.mul_add(lo, hi.mul_add(y, -ehi));
    exp_inline(ehi, elo, sign_bias)
}
