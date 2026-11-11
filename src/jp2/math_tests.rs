//! `math::pow` and `math::log` against glibc.
//!
//! The default test runs the port on a sweep of inputs and checks digests of
//! its results, recorded from glibc 2.39's FMA variant, on every platform.
//!
//! An `#[ignore]`d test compares the port bit for bit with `f64::powf` and
//! `f64::ln` where those are that variant: x86-64 Linux with glibc, on a CPU
//! with FMA and AVX2. It runs the default sweep and one 100 times larger.
//! It is kept out of the default run so a different host glibc can't fail
//! it. Run it with `cargo test --release --lib math_tests -- --ignored`.

use std::hint::black_box;

use super::math::{log, pow};

/// SplitMix64: a small deterministic generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`, on the 2^-53 grid.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }

    /// Uniform in `[lo, hi)`.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
}

/// 2^k for k in -1074..=1023, exactly.
fn two_to(k: i32) -> f64 {
    if k < -1022 {
        f64::from_bits(1u64 << (k + 1074))
    } else {
        f64::from_bits(((k + 1023) as u64) << 52)
    }
}

/// The special values: zeros, ones, subnormals, the ends of the normal
/// range, values near overflow, infinities, NaNs and negative values.
fn edges() -> Vec<f64> {
    let mut v = vec![
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.5,
        -0.5,
        2.0,
        -2.0,
        3.0,
        -3.0,
        1.0 / 3.0,
        0.1,
        10.0,
        2.2,
        2.4,
        1.0 / 2.4,
        f64::from_bits(1),
        -f64::from_bits(1),
        f64::from_bits(0x000f_ffff_ffff_ffff),
        f64::from_bits(0x0008_0000_0000_0000),
        f64::MIN_POSITIVE,
        -f64::MIN_POSITIVE,
        f64::EPSILON,
        1.0 + f64::EPSILON,
        1.0 - f64::EPSILON / 2.0,
        f64::MAX,
        -f64::MAX,
        f64::MAX / 2.0,
        709.782_712_893_384,
        -745.133_219_101_941_1,
        1024.0,
        -1074.0,
        -1075.0,
        1_023.999_999_999_999_9,
        0.999_999_999_999_999_9,
        1.000_000_000_000_000_2,
        f64::from_bits(0x3fee_0000_0000_0000),
        f64::from_bits(0x3ff1_0900_0000_0000),
        f64::from_bits(0x3fe6_9555_0000_0000),
        f64::from_bits(0x43e0_0000_0000_0000),
        f64::from_bits(0x3be0_0000_0000_0000),
        9_007_199_254_740_993.0,
        9_007_199_254_740_992.0,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        -f64::NAN,
        f64::from_bits(0x7ff0_0000_0000_0001),
    ];
    for k in [
        -1074, -1060, -1023, -1022, -64, -54, -1, 1, 52, 53, 63, 64, 1000, 1023,
    ] {
        v.push(two_to(k));
    }
    v
}

/// The `log` inputs: random bit patterns over the whole positive range, the
/// unit interval lcms uses, the near-1 path, subnormals and the edges.
fn log_inputs(scale: usize, mut each: impl FnMut(f64)) {
    let mut rng = Rng(0x1c0_ffee);
    for x in edges() {
        each(x);
    }
    for _ in 0..1_500_000 * scale {
        each(f64::from_bits(rng.next() & 0x7fff_ffff_ffff_ffff));
    }
    for _ in 0..1_500_000 * scale {
        each(rng.range(0.0, 2.0));
    }
    for _ in 0..1_000_000 * scale {
        each(rng.range(1.0 - 0.0625, 1.0 + 0.065));
    }
    for _ in 0..200_000 * scale {
        each(f64::from_bits(rng.next() & 0x000f_ffff_ffff_ffff));
    }
    // The 4095 gray-ramp nodes `estimate_gamma` evaluates.
    for i in 1..4096 {
        each(f64::from(i) / 4096.0);
    }
    for i in 0..500_000 * scale {
        // Neighbors of 1, a step of one ulp at a time.
        let d = i as u64;
        each(f64::from_bits(0x3ff0_0000_0000_0000 + d));
        each(f64::from_bits(0x3ff0_0000_0000_0000 - d));
    }
}

/// The exponents lcms's curves use: gammas and their inverses.
fn curve_exponents() -> Vec<f64> {
    let mut v = vec![
        1.0 / 3.0,
        1.8,
        2.2,
        2.4,
        2.6,
        1.0 / 1.8,
        1.0 / 2.2,
        1.0 / 2.4,
    ];
    for i in 1..=100 {
        let g = f64::from(i) / 10.0;
        v.push(g);
        v.push(1.0 / g);
    }
    // u8Fixed8 gammas as an ICC `curv` with one entry gives them.
    for i in (26..=2560).step_by(13) {
        v.push(f64::from(i) / 256.0);
    }
    v
}

/// The `pow` inputs: a grid over the bases and exponents lcms uses, random
/// pairs in and around those ranges, results near overflow and underflow,
/// negative bases, and every pair of edge values.
fn pow_inputs(scale: usize, mut each: impl FnMut(f64, f64)) {
    let mut rng = Rng(0x9e11_c0de);
    let edges = edges();
    for &x in &edges {
        for &y in &edges {
            each(x, y);
        }
    }
    let ys = curve_exponents();
    let steps = 2048 * scale as u32;
    for i in 0..=steps {
        let x = f64::from(i) / f64::from(steps) * 1.25;
        for &y in &ys {
            each(x, y);
        }
    }
    for _ in 0..500_000 * scale {
        let x = rng.range(0.0, 1.25);
        let y = rng.range(0.1, 10.0);
        each(x, y);
        each(x, 1.0 / y);
    }
    for _ in 0..200_000 * scale {
        // A parametric curve's base `a * r + b`, as lcms forms it.
        let (a, b, r) = (rng.range(0.5, 1.5), rng.range(-0.2, 0.2), rng.unit());
        each(a * r + b, rng.range(0.1, 10.0));
    }
    for _ in 0..300_000 * scale {
        // Wide: any positive x, |y| up to 2^70.
        let x = f64::from_bits(rng.next() & 0x7fff_ffff_ffff_ffff);
        let e = (rng.next() % 140) as i32 - 70;
        let y = rng.range(-1.0, 1.0) * two_to(e);
        each(x, y);
    }
    for _ in 0..200_000 * scale {
        // Results near overflow, near the subnormal range and inside it.
        let x = rng.range(0.25, 4.0);
        let t = rng.range(-1080.0, 1030.0);
        // The port's own log keeps the inputs the same on every platform.
        let y = t / (log(x) / std::f64::consts::LN_2);
        each(x, y);
        each(-x, y.round());
    }
    for _ in 0..100_000 * scale {
        // exp's x * N / ln2 within a few ulps of a half-integer, where how it
        // rounds picks k (the fused multiply-add in `exp_inline` matters).
        let x = if rng.next() & 1 == 0 {
            rng.range(0.25, 0.9)
        } else {
            rng.range(1.1, 4.0)
        };
        let m = (rng.next() % 4000) as f64 - 2000.0;
        let y = (m + 0.5) / (128.0 * log(x) / std::f64::consts::LN_2);
        for d in 0..8 {
            each(x, f64::from_bits(y.to_bits() - 4 + d));
        }
    }
    for _ in 0..100_000 * scale {
        // Negative bases with integer and non-integer exponents.
        let x = -rng.range(0.0, 3.0);
        let y = (rng.range(-40.0, 40.0)).round();
        each(x, y);
        each(x, y + 0.5);
    }
    for _ in 0..20_000 * scale {
        // Negative bases with |y| in [2^52, 2^54), where `checkint` reads the
        // lowest mantissa bit (odd) or finds y even. Bases within a few ulps of
        // -1 keep the result finite; the others overflow or underflow to a
        // signed infinity or zero.
        let odd = f64::from_bits(0x4330_0000_0000_0000 | (rng.next() & 0x000f_ffff_ffff_ffff) | 1);
        let even = f64::from_bits(0x4340_0000_0000_0000 | (rng.next() & 0x000f_ffff_ffff_ffff));
        let near = -f64::from_bits(0x3ff0_0000_0000_0000 + (rng.next() % 9));
        let below = -f64::from_bits(0x3ff0_0000_0000_0000 - (rng.next() % 9));
        let far = -rng.range(0.0, 3.0);
        for x in [-1.0, near, below, far] {
            for y in [odd, -odd, odd - 1.0, -(odd - 1.0), even, -even] {
                each(x, y);
            }
        }
    }
}

/// Equal bits, or both NaN (a NaN's sign and payload are not specified).
fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// FNV-1a over the result bits, with every NaN taken as one value.
fn fold(h: &mut u64, v: f64) {
    let bits = if v.is_nan() {
        0x7ff8_0000_0000_0000
    } else {
        v.to_bits()
    };
    for byte in bits.to_le_bytes() {
        *h ^= u64::from(byte);
        *h = h.wrapping_mul(0x0100_0000_01b3);
    }
}

/// Whether `f64::powf` and `f64::ln` are glibc's FMA variant.
#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
fn platform_is_reference() -> bool {
    std::arch::is_x86_feature_detected!("fma") && std::arch::is_x86_feature_detected!("avx2")
}

/// Runs the sweep: the port against the platform when the platform is the
/// reference, and the digests of the port's results. Returns the input
/// counts and the digests.
fn sweep(scale: usize, compare: bool) -> ((u64, u64), (u64, u64)) {
    let (mut n_log, mut n_pow) = (0u64, 0u64);
    let (mut h_log, mut h_pow) = (0xcbf2_9ce4_8422_2325u64, 0xcbf2_9ce4_8422_2325u64);
    let (mut bad_log, mut bad_pow) = (0u64, 0u64);
    let mut shown = Vec::new();
    log_inputs(scale, |x| {
        let got = log(x);
        if compare {
            let want = black_box(x).ln();
            if !same(got, want) {
                bad_log += 1;
                if shown.len() < 20 {
                    shown.push(format!(
                        "log({x:e} = {:#x}) = {got:e}, glibc {want:e}",
                        x.to_bits()
                    ));
                }
            }
        }
        fold(&mut h_log, got);
        n_log += 1;
    });
    pow_inputs(scale, |x, y| {
        let got = pow(x, y);
        if compare {
            let want = black_box(x).powf(black_box(y));
            if !same(got, want) {
                bad_pow += 1;
                if shown.len() < 20 {
                    shown.push(format!(
                        "pow({x:e} = {:#x}, {y:e} = {:#x}) = {got:e}, glibc {want:e}",
                        x.to_bits(),
                        y.to_bits()
                    ));
                }
            }
        }
        fold(&mut h_pow, got);
        n_pow += 1;
    });
    assert!(
        bad_log == 0 && bad_pow == 0,
        "differs from glibc on {bad_log} of {n_log} log and {bad_pow} of {n_pow} pow inputs:\n{}",
        shown.join("\n")
    );
    ((n_log, n_pow), (h_log, h_pow))
}

/// The digests of the default sweep's results, recorded from glibc 2.39's
/// FMA variant (`matches_glibc_bit_for_bit` checks that glibc gives them).
const DIGESTS: (u64, u64) = (0xef5a_be59_dd0e_f2ce, 0x0ee2_0b44_6760_73bf);
/// The default sweep's `log` and `pow` input counts.
const COUNTS: (u64, u64) = (5_204_157, 4_209_591);

/// The port's results on the default sweep, by digest. This pins them on
/// every platform without depending on the host's libm.
#[test]
fn matches_the_glibc_digests() {
    let (counts, digests) = sweep(1, false);
    assert_eq!(counts, COUNTS);
    assert_eq!(
        digests, DIGESTS,
        "digests {digests:#x?} for {counts:?} inputs"
    );
}

/// The port against `f64::ln` and `f64::powf` bit for bit, on the default
/// sweep (which also shows the recorded digests are glibc's) and on one 100
/// times larger. It needs glibc 2.39's FMA variant: x86-64 Linux with glibc,
/// on a CPU with FMA and AVX2, and no `GLIBC_TUNABLES` that turns them off.
/// About a minute in release:
/// `cargo test --release --lib math_tests -- --ignored`.
#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
#[test]
#[ignore = "slow, and needs the host's glibc to be 2.39's FMA variant"]
fn matches_glibc_bit_for_bit() {
    assert!(
        platform_is_reference(),
        "needs glibc's FMA variant (FMA and AVX2)"
    );
    let (counts, digests) = sweep(1, true);
    assert_eq!((counts, digests), (COUNTS, DIGESTS));
    let (counts, _) = sweep(100, true);
    eprintln!("compared {} log and {} pow inputs", counts.0, counts.1);
}

#[test]
fn special_values() {
    let inf = f64::INFINITY;
    assert_eq!(log(1.0).to_bits(), 0);
    assert_eq!(log(0.0), -inf);
    assert_eq!(log(-0.0), -inf);
    assert_eq!(log(inf), inf);
    assert!(log(-1.0).is_nan() && log(f64::NAN).is_nan() && log(-inf).is_nan());
    assert_eq!(pow(0.0, 0.0), 1.0);
    assert_eq!(pow(f64::NAN, 0.0), 1.0);
    assert_eq!(pow(1.0, f64::NAN), 1.0);
    assert_eq!(pow(0.0, -1.0), inf);
    assert_eq!(pow(-0.0, -1.0), -inf);
    assert_eq!(pow(-0.0, 3.0).to_bits(), (-0.0f64).to_bits());
    assert_eq!(pow(-2.0, 3.0), -8.0);
    assert_eq!(pow(-2.0, 2.0), 4.0);
    assert!(pow(-8.0, 1.0 / 3.0).is_nan());
    assert_eq!(pow(2.0, 1024.0), inf);
    assert_eq!(pow(2.0, -1074.0), f64::from_bits(1));
    assert_eq!(pow(2.0, -1076.0), 0.0);
    assert_eq!(pow(0.5, inf), 0.0);
    assert_eq!(pow(2.0, -inf), 0.0);
    assert_eq!(pow(-1.0, inf), 1.0);
}
