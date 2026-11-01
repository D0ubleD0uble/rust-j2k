//! The ICC → sRGB transform LittleCMS builds for OpenJPEG, ported to Rust so
//! that `ColourOutput::Srgb` matches `opj_decompress -force-rgb` sample for
//! sample.
//!
//! OpenJPEG 2.5.0 (`color_apply_icc_profile` in `src/bin/common/color.c`)
//! opens the file's profile, builds sRGB with `cmsCreate_sRGBProfile`, and
//! calls `cmsCreateTransform` with the profile's rendering intent, flags 0,
//! and the formats `TYPE_RGB_8` → `TYPE_RGB_8`, `TYPE_RGB_16` →
//! `TYPE_RGB_16`, or `TYPE_GRAY_8` → `TYPE_RGB_8`. This module builds the
//! same pipeline for the restricted (matrix/TRC and monochrome) profiles and
//! runs the same optimisation lcms picks for it:
//!
//! - the stages: the profile's curves and matrix into encoded XYZ, a
//!   conversion for absolute colorimetric or black point compensation, the
//!   inverse sRGB matrix and the inverse sRGB curves (`cmsio1.c`,
//!   `cmscnvrt.c`, `cmsvirt.c`, `cmswtpnt.c`);
//! - the pre-optimisation that multiplies adjacent matrices, then the first
//!   of these that applies (`cmsopt.c`): joining curves, the 8-bit
//!   matrix-shaper in 1.14 fixed point, prelinearisation curves with a
//!   33-point table for 8-bit RGB, and resampling into a 33-point table;
//!   16-bit RGB on a near-linear profile is not optimised at all
//!   (`cmsxform.c`);
//! - the 16-bit curve tables and interpolation (`cmsgamma.c`, `cmsintrp.c`),
//!   the 8 ↔ 16 bit conversions (`cmspack.c`), and the white-point fix-up.
//!
//! The arithmetic follows the C source step for step, in the same order and
//! at the same precision (`f32` where lcms uses `cmsFloat32Number`, wrapping
//! integer arithmetic for its fixed point), since a single rounding
//! difference moves an output level.
//!
//! Ported from Little CMS 2.14 (tag `lcms2.14` of github.com/mm2/Little-CMS,
//! the version Ubuntu's OpenJPEG 2.5.0-2ubuntu0.5 links). Its licence, from
//! the `COPYING` file at that tag:
//!
//! > Little CMS
//! > Copyright (c) 1998-2020 Marti Maria Saguer
//! >
//! > Permission is hereby granted, free of charge, to any person obtaining
//! > a copy of this software and associated documentation files (the
//! > "Software"), to deal in the Software without restriction, including
//! > without limitation the rights to use, copy, modify, merge, publish,
//! > distribute, sublicense, and/or sell copies of the Software, and to
//! > permit persons to whom the Software is furnished to do so, subject
//! > to the following conditions:
//! >
//! > The above copyright notice and this permission notice shall be
//! > included in all copies or substantial portions of the Software.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
//! > EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
//! > MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
//! > IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
//! > CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
//! > TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
//! > SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

// ---------------------------------------------------------------------------
// Numbers (lcms2_internal.h)

/// `MAX_ENCODEABLE_XYZ`: XYZ is carried as `0..=1` for `0..=1.99997`.
const MAX_ENCODEABLE_XYZ: f64 = 1.0 + 32767.0 / 32768.0;
/// `InpAdj` and `OutpAdj` (`cmsio1.c`).
const INP_ADJ: f64 = 1.0 / MAX_ENCODEABLE_XYZ;
const OUTP_ADJ: f64 = MAX_ENCODEABLE_XYZ;
/// `cmsD50_XYZ()`.
const D50: [f64; 3] = [0.9642, 1.0, 0.8249];
const MATRIX_DET_TOLERANCE: f64 = 0.0001;
/// `PLUS_INF` and `MINUS_INF` in `cmsgamma.c` are `float` literals.
const PLUS_INF: f64 = 1e22_f32 as f64;
const MINUS_INF: f64 = -1e22_f32 as f64;
/// `PRELINEARIZATION_POINTS`.
const PRELIN_POINTS: usize = 4096;
/// `_cmsReasonableGridpointsByColorspace` for one and three channels.
const GRID_POINTS: usize = 33;

/// `_cmsQuickFloor`: the floor of `val` after rounding it to a multiple of
/// 2^-16, by adding 1.5 * 2^36 and reading the low word.
fn quick_floor(val: f64) -> i32 {
    let t = val + 68_719_476_736.0 * 1.5;
    (t.to_bits() as u32 as i32) >> 16
}

/// `_cmsQuickSaturateWord`.
fn saturate_word(d: f64) -> u16 {
    let d = d + 0.5;
    if d <= 0.0 {
        return 0;
    }
    if d >= 65535.0 {
        return 0xffff;
    }
    (quick_floor(d - 32767.0) as u16).wrapping_add(32767)
}

/// `FROM_8_TO_16`.
fn from_8_to_16(v: u8) -> u16 {
    u16::from(v) << 8 | u16::from(v)
}

/// `FROM_16_TO_8`.
fn from_16_to_8(v: u16) -> u8 {
    ((u32::from(v) * 65281 + 8_388_608) >> 24) as u8
}

/// `_cmsToFixedDomain`.
fn to_fixed_domain(a: i32) -> i32 {
    a.wrapping_add(a.wrapping_add(0x7fff) / 0xffff)
}

/// `_cmsQuantizeVal`.
fn quantize(i: f64, samples: usize) -> u16 {
    saturate_word(i * 65535.0 / (samples - 1) as f64)
}

/// `DOUBLE_TO_1FIXED14`. C converts an out-of-range double to `0x80000000`.
fn to_1fixed14(x: f64) -> i32 {
    let v = (x * 16384.0 + 0.5).floor();
    if (-2_147_483_648.0..2_147_483_648.0).contains(&v) {
        v as i32
    } else {
        i32::MIN
    }
}

// ---------------------------------------------------------------------------
// 3x3 matrices (cmsmtrx.c)

pub(crate) type Mat3 = [[f64; 3]; 3];

const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// `_cmsMAT3per`: `a * b`.
fn mat_per(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut r = [[0.0; 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    r
}

/// `_cmsMAT3eval`.
fn mat_eval(a: &Mat3, v: [f64; 3]) -> [f64; 3] {
    a.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}

/// `_cmsMAT3inverse`.
fn mat_inverse(a: &Mat3) -> Option<Mat3> {
    let c0 = a[1][1] * a[2][2] - a[1][2] * a[2][1];
    let c1 = -a[1][0] * a[2][2] + a[1][2] * a[2][0];
    let c2 = a[1][0] * a[2][1] - a[1][1] * a[2][0];
    let det = a[0][0] * c0 + a[0][1] * c1 + a[0][2] * c2;
    if det.abs() < MATRIX_DET_TOLERANCE {
        return None;
    }
    Some([
        [
            c0 / det,
            (a[0][2] * a[2][1] - a[0][1] * a[2][2]) / det,
            (a[0][1] * a[1][2] - a[0][2] * a[1][1]) / det,
        ],
        [
            c1 / det,
            (a[0][0] * a[2][2] - a[0][2] * a[2][0]) / det,
            (a[0][2] * a[1][0] - a[0][0] * a[1][2]) / det,
        ],
        [
            c2 / det,
            (a[0][1] * a[2][0] - a[0][0] * a[2][1]) / det,
            (a[0][0] * a[1][1] - a[0][1] * a[1][0]) / det,
        ],
    ])
}

/// `_cmsMAT3isIdentity`: every entry within 1/65535 of the identity.
fn is_identity(a: &Mat3) -> bool {
    (0..3).all(|i| (0..3).all(|j| (IDENTITY[i][j] - a[i][j]).abs() < 1.0 / 65535.0))
}

/// `isFloatMatrixIdentity` (`cmsopt.c`): within `0.00001f`.
fn is_float_identity(a: &Mat3) -> bool {
    let tolerance = f64::from(0.00001_f32);
    (0..3).all(|i| (0..3).all(|j| (IDENTITY[i][j] - a[i][j]).abs() < tolerance))
}

// ---------------------------------------------------------------------------
// The sRGB output profile (cmsvirt.c, cmswtpnt.c)

/// `_cmsAdaptationMatrix` with the Bradford cone matrix.
fn adaptation(from: [f64; 3], to: [f64; 3]) -> Option<Mat3> {
    const BRADFORD: Mat3 = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let chad_inv = mat_inverse(&BRADFORD)?;
    let source = mat_eval(&BRADFORD, from);
    let dest = mat_eval(&BRADFORD, to);
    let cone = [
        [dest[0] / source[0], 0.0, 0.0],
        [0.0, dest[1] / source[1], 0.0],
        [0.0, 0.0, dest[2] / source[2]],
    ];
    let tmp = mat_per(&cone, &BRADFORD);
    Some(mat_per(&chad_inv, &tmp))
}

/// `cmsxyY2XYZ`.
fn xyy_to_xyz([x, y, big_y]: [f64; 3]) -> [f64; 3] {
    [(x / y) * big_y, big_y, ((1.0 - x - y) / y) * big_y]
}

/// The colorant matrix of `cmsCreate_sRGBProfile` (rows X, Y, Z; columns
/// R, G, B): `_cmsBuildRGB2XYZtransferMatrix` for D65 = (0.3127, 0.3290) and
/// the Rec. 709 primaries, adapted to D50. The profile is built in memory,
/// so lcms reads these doubles back unrounded.
fn srgb_colorants() -> Mat3 {
    let (xn, yn) = (0.3127, 0.3290);
    let [(xr, yr), (xg, yg), (xb, yb)] = [(0.64, 0.33), (0.30, 0.60), (0.15, 0.06)];
    let primaries = [
        [xr, xg, xb],
        [yr, yg, yb],
        [1.0 - xr - yr, 1.0 - xg - yg, 1.0 - xb - yb],
    ];
    let inverse = mat_inverse(&primaries).expect("the Rec. 709 primaries are independent");
    let coef = mat_eval(&inverse, [xn / yn, 1.0, (1.0 - xn - yn) / yn]);
    let r = [
        [coef[0] * xr, coef[1] * xg, coef[2] * xb],
        [coef[0] * yr, coef[1] * yg, coef[2] * yb],
        [
            coef[0] * (1.0 - xr - yr),
            coef[1] * (1.0 - xg - yg),
            coef[2] * (1.0 - xb - yb),
        ],
    ];
    let bradford = adaptation(xyy_to_xyz([xn, yn, 1.0]), D50).expect("D65 adapts to D50");
    mat_per(&bradford, &r)
}

/// `Build_sRGBGamma`'s parameters (type 4).
fn srgb_params() -> [f64; 10] {
    let mut p = [0.0; 10];
    p[..5].copy_from_slice(&[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045]);
    p
}

// ---------------------------------------------------------------------------
// Tone curves (cmsgamma.c)

/// A `cmsToneCurve` as these profiles produce it: one parametric segment
/// over the whole line with its 16-bit table, or a 16-bit table alone.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToneCurve {
    parametric: Option<(i32, [f64; 10])>,
    table: Vec<u16>,
}

impl ToneCurve {
    /// `cmsBuildParametricToneCurve` (one segment, `-1e22..=1e22`) for the
    /// lcms type (ICC function type + 1; negative for an inverse).
    pub(crate) fn parametric(kind: i32, params: [f64; 10]) -> Self {
        // `EntriesByGamma` for type 1, else 4096.
        let entries = if kind == 1 && (params[0] - 1.0).abs() < 0.001 {
            2
        } else {
            4096
        };
        let mut curve = ToneCurve {
            parametric: Some((kind, params)),
            table: Vec::new(),
        };
        curve.table = (0..entries)
            .map(|i| {
                saturate_word(curve.segmented(f64::from(i) / f64::from(entries - 1)) * 65535.0)
            })
            .collect();
        curve
    }

    /// `cmsBuildTabulatedToneCurve16`. `table` has at least two entries.
    pub(crate) fn tabulated(table: Vec<u16>) -> Self {
        ToneCurve {
            parametric: None,
            table,
        }
    }

    /// `EvalSegmentedFn` for the one segment.
    fn segmented(&self, r: f64) -> f64 {
        let Some((kind, params)) = &self.parametric else {
            return MINUS_INF;
        };
        if !(r > MINUS_INF && r <= PLUS_INF) {
            return MINUS_INF;
        }
        let out = eval_parametric(*kind, params, r);
        // `isinf(Out)` is true for either sign.
        if out.is_infinite() { PLUS_INF } else { out }
    }

    /// `cmsEvalToneCurveFloat`.
    fn eval_float(&self, v: f32) -> f32 {
        if self.parametric.is_none() {
            let input = saturate_word(f64::from(v) * 65535.0);
            return (f64::from(self.eval16(input)) / 65535.0) as f32;
        }
        self.segmented(f64::from(v)) as f32
    }

    /// `cmsEvalToneCurve16`: `LinLerp1D` on the table.
    fn eval16(&self, v: u16) -> u16 {
        let domain = (self.table.len() - 1) as i32;
        if v == 0xffff || domain == 0 {
            return self.table[domain as usize];
        }
        let val3 = to_fixed_domain(domain.wrapping_mul(i32::from(v)));
        let cell = (val3 >> 16) as usize;
        let rest = val3 & 0xffff;
        linear_interp(
            rest,
            i32::from(self.table[cell]),
            i32::from(self.table[cell + 1]),
        )
    }

    /// `cmsIsToneCurveDescending`.
    fn descending(&self) -> bool {
        self.table[0] > self.table[self.table.len() - 1]
    }

    /// `cmsIsToneCurveLinear`: within 15 of the identity everywhere.
    fn is_linear(&self) -> bool {
        let n = self.table.len();
        self.table
            .iter()
            .enumerate()
            .all(|(i, &v)| (i32::from(v) - i32::from(quantize(i as f64, n))).abs() <= 0x0f)
    }

    /// `cmsIsToneCurveMonotonic`, with lcms's ripple of 2.
    fn is_monotonic(&self) -> bool {
        let t = &self.table;
        if t.len() < 2 {
            return true;
        }
        if self.descending() {
            let mut last = i32::from(t[0]);
            for &v in &t[1..] {
                if i32::from(v) - last > 2 {
                    return false;
                }
                last = i32::from(v);
            }
        } else {
            let mut last = i32::from(t[t.len() - 1]);
            for &v in t[..t.len() - 1].iter().rev() {
                if i32::from(v) - last > 2 {
                    return false;
                }
                last = i32::from(v);
            }
        }
        true
    }

    /// `IsDegenerated` (`cmsopt.c`): more than 5% of the table at 0 or at
    /// 0xffff.
    fn is_degenerated(&self) -> bool {
        let n = self.table.len();
        let zeros = self.table.iter().filter(|&&v| v == 0).count();
        let poles = self.table.iter().filter(|&&v| v == 0xffff).count();
        if zeros == 1 && poles == 1 {
            return false;
        }
        zeros > n / 20 || poles > n / 20
    }

    /// `cmsReverseToneCurveEx`: analytic for a parametric curve, else the
    /// table inverted by linear interpolation into `samples` entries.
    fn reverse(&self, samples: usize) -> ToneCurve {
        if let Some((kind, params)) = self.parametric
            && kind > 0
        {
            return ToneCurve::parametric(-kind, params);
        }
        let n = self.table.len();
        let ascending = !self.descending();
        let mut out = vec![0u16; samples];
        let (mut a, mut b) = (0.0, 0.0);
        for (i, slot) in out.iter_mut().enumerate() {
            let y = i as f64 * 65535.0 / (samples - 1) as f64;
            if let Some(j) = self.interval(y) {
                let x1 = f64::from(self.table[j]);
                let x2 = f64::from(self.table[j + 1]);
                let y1 = (j as f64 * 65535.0) / (n - 1) as f64;
                let y2 = ((j + 1) as f64 * 65535.0) / (n - 1) as f64;
                if x1 == x2 {
                    *slot = saturate_word(if ascending { y2 } else { y1 });
                    continue;
                }
                a = (y2 - y1) / (x2 - x1);
                b = y2 - a * x2;
            }
            *slot = saturate_word(a * y + b);
        }
        ToneCurve::tabulated(out)
    }

    /// `GetInterval`: the table segment containing `y`.
    fn interval(&self, y: f64) -> Option<usize> {
        let t = &self.table;
        let domain = t.len() - 1;
        if domain < 1 {
            return None;
        }
        let within = |i: usize| {
            let (y0, y1) = (f64::from(t[i]), f64::from(t[i + 1]));
            if y0 <= y1 {
                y >= y0 && y <= y1
            } else {
                y >= y1 && y <= y0
            }
        };
        if t[0] < t[domain] {
            (0..domain).rev().find(|&i| within(i))
        } else {
            (0..domain).find(|&i| within(i))
        }
    }
}

/// `DefaultEvalParametricFn` for the types a restricted profile reads (1 to
/// 5) and the inverse sRGB curve (-4).
fn eval_parametric(kind: i32, p: &[f64; 10], r: f64) -> f64 {
    match kind {
        1 => {
            if r < 0.0 {
                if (p[0] - 1.0).abs() < MATRIX_DET_TOLERANCE {
                    r
                } else {
                    0.0
                }
            } else {
                r.powf(p[0])
            }
        }
        2 => {
            if p[1].abs() < MATRIX_DET_TOLERANCE {
                return 0.0;
            }
            let disc = -p[2] / p[1];
            if r >= disc {
                let e = p[1] * r + p[2];
                if e > 0.0 { e.powf(p[0]) } else { 0.0 }
            } else {
                0.0
            }
        }
        3 => {
            if p[1].abs() < MATRIX_DET_TOLERANCE {
                return 0.0;
            }
            let mut disc = -p[2] / p[1];
            if disc < 0.0 {
                disc = 0.0;
            }
            if r >= disc {
                let e = p[1] * r + p[2];
                if e > 0.0 { e.powf(p[0]) + p[3] } else { 0.0 }
            } else {
                p[3]
            }
        }
        4 => {
            if r >= p[4] {
                let e = p[1] * r + p[2];
                if e > 0.0 { e.powf(p[0]) } else { 0.0 }
            } else {
                r * p[3]
            }
        }
        -4 => {
            let e = p[1] * p[4] + p[2];
            let disc = if e < 0.0 { 0.0 } else { e.powf(p[0]) };
            if r >= disc {
                if p[0].abs() < MATRIX_DET_TOLERANCE || p[1].abs() < MATRIX_DET_TOLERANCE {
                    0.0
                } else {
                    (r.powf(1.0 / p[0]) - p[2]) / p[1]
                }
            } else if p[3].abs() < MATRIX_DET_TOLERANCE {
                0.0
            } else {
                r / p[3]
            }
        }
        5 => {
            if r >= p[4] {
                let e = p[1] * r + p[2];
                if e > 0.0 { e.powf(p[0]) + p[5] } else { p[5] }
            } else {
                r * p[3] + p[6]
            }
        }
        _ => 0.0,
    }
}

/// `LinearInterp` (`cmsintrp.c`), in lcms's unsigned wrap-around.
fn linear_interp(a: i32, l: i32, h: i32) -> u16 {
    let dif = (h.wrapping_sub(l) as u32)
        .wrapping_mul(a as u32)
        .wrapping_add(0x8000);
    ((dif >> 16).wrapping_add(l as u32)) as u16
}

// ---------------------------------------------------------------------------
// Pipelines (cmslut.c)

/// One pipeline stage.
#[derive(Debug, Clone, PartialEq)]
enum Stage {
    Curves(Vec<ToneCurve>),
    /// `rows` outputs from `cols` inputs, row-major, with an optional offset.
    Matrix {
        rows: usize,
        cols: usize,
        m: Vec<f64>,
        offset: Option<[f64; 3]>,
    },
}

impl Stage {
    fn matrix(m: &Mat3, offset: Option<[f64; 3]>) -> Stage {
        Stage::Matrix {
            rows: 3,
            cols: 3,
            m: m.iter().flatten().copied().collect(),
            offset,
        }
    }

    /// The stage's `EvalPtr` in floating point.
    fn eval(&self, input: &[f32; 3], out: &mut [f32; 3]) {
        match self {
            Stage::Curves(curves) => {
                for (i, curve) in curves.iter().enumerate() {
                    out[i] = curve.eval_float(input[i]);
                }
            }
            Stage::Matrix {
                rows,
                cols,
                m,
                offset,
            } => {
                for i in 0..*rows {
                    let mut tmp = 0.0_f64;
                    for j in 0..*cols {
                        tmp += f64::from(input[j]) * m[i * cols + j];
                    }
                    if let Some(off) = offset {
                        tmp += off[i];
                    }
                    out[i] = tmp as f32;
                }
            }
        }
    }
}

/// `cmsPipelineEvalFloat`.
fn eval_float(stages: &[Stage], input: [f32; 3]) -> [f32; 3] {
    let mut value = input;
    for stage in stages {
        let mut next = [0.0_f32; 3];
        stage.eval(&value, &mut next);
        value = next;
    }
    value
}

/// `PreOptimize` for these pipelines: `_MultiplyMatrix` until it changes
/// nothing. Adjacent 3x3 matrices without offsets become their product,
/// dropped when it is the identity; a matrix pair it cannot join ends the
/// pass with "no change", as lcms returns FALSE there.
fn pre_optimize(stages: &mut Vec<Stage>) {
    while multiply_matrix(stages) {}
}

/// `_MultiplyMatrix`.
fn multiply_matrix(stages: &mut Vec<Stage>) -> bool {
    let mut any = false;
    let mut i = 0;
    while i + 1 < stages.len() {
        let (
            Stage::Matrix {
                rows: r1,
                cols: c1,
                m: m1,
                offset: o1,
            },
            Stage::Matrix {
                rows: r2,
                cols: c2,
                m: m2,
                offset: o2,
            },
        ) = (&stages[i], &stages[i + 1])
        else {
            i += 1;
            continue;
        };
        if o1.is_some() || o2.is_some() || (*r1, *c1, *r2, *c2) != (3, 3, 3, 3) {
            return false;
        }
        let res = mat_per(&to_mat3(m2), &to_mat3(m1));
        stages.drain(i..i + 2);
        if !is_float_identity(&res) {
            stages.insert(i, Stage::matrix(&res, None));
        }
        any = true;
    }
    any
}

fn to_mat3(m: &[f64]) -> Mat3 {
    [[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]]
}

// ---------------------------------------------------------------------------
// 16-bit tables (cmslut.c, cmsintrp.c)

/// A 16-bit CLUT stage with `GRID_POINTS` nodes per input.
#[derive(Debug, Clone, PartialEq)]
struct Clut {
    inputs: usize,
    table: Vec<u16>,
}

impl Clut {
    /// `cmsStageSampleCLut16bit` with `XFormSampler16` over `stages`: every
    /// node through the pipeline in floating point.
    fn sample(stages: &[Stage], inputs: usize) -> Clut {
        let n = GRID_POINTS;
        let total = n.pow(inputs as u32);
        let mut table = Vec::with_capacity(total * 3);
        for i in 0..total {
            let mut rest = i;
            let mut node = [0u16; 3];
            for t in (0..inputs).rev() {
                node[t] = quantize((rest % n) as f64, n);
                rest /= n;
            }
            let mut input = [0.0_f32; 3];
            for t in 0..inputs {
                input[t] = (f64::from(node[t]) / 65535.0) as f32;
            }
            let out = eval_float(stages, input);
            table.extend(out.iter().map(|&v| saturate_word(f64::from(v) * 65535.0)));
        }
        Clut { inputs, table }
    }

    /// `opta`, nearest input last: `opta[k]` steps input `inputs - 1 - k`.
    fn opta(&self, k: usize) -> usize {
        3 * GRID_POINTS.pow(k as u32)
    }

    /// The CLUT's `Lerp16`: `Eval1Input` or `TetrahedralInterp16`.
    fn eval16(&self, input: &[u16; 3]) -> [u16; 3] {
        if self.inputs == 1 {
            self.eval1(input[0])
        } else {
            self.tetrahedral(input)
        }
    }

    fn eval1(&self, v: u16) -> [u16; 3] {
        let domain = (GRID_POINTS - 1) as i32;
        let t = &self.table;
        let mut out = [0u16; 3];
        if v == 0xffff {
            let y0 = domain as usize * 3;
            out.copy_from_slice(&t[y0..y0 + 3]);
            return out;
        }
        let fk = to_fixed_domain(i32::from(v) * domain);
        let k0 = (fk >> 16) as usize;
        let rk = fk & 0xffff;
        let k1 = k0 + 1;
        for (c, o) in out.iter_mut().enumerate() {
            *o = linear_interp(rk, i32::from(t[k0 * 3 + c]), i32::from(t[k1 * 3 + c]));
        }
        out
    }

    fn tetrahedral(&self, input: &[u16; 3]) -> [u16; 3] {
        let domain = (GRID_POINTS - 1) as i32;
        let t = &self.table;
        let f = input.map(|v| to_fixed_domain(i32::from(v) * domain));
        let [rx, ry, rz] = f.map(|v| v & 0xffff);
        let (opta2, opta1, opta0) = (self.opta(2), self.opta(1), self.opta(0));
        let base = opta2 * (f[0] >> 16) as usize
            + opta1 * (f[1] >> 16) as usize
            + opta0 * (f[2] >> 16) as usize;
        let step = |v: u16, opta: usize| if v == 0xffff { 0 } else { opta };
        let (mut x1, mut y1, mut z1) = (
            step(input[0], opta2),
            step(input[1], opta1),
            step(input[2], opta0),
        );
        // The six tetrahedra, as `TetrahedralInterp16` accumulates the
        // offsets and then the differences.
        #[derive(Clone, Copy)]
        enum Diff {
            A,
            B,
            C,
            D,
            E,
            F,
        }
        let case = if rx >= ry {
            if ry >= rz {
                y1 += x1;
                z1 += y1;
                Diff::A
            } else if rz >= rx {
                x1 += z1;
                y1 += x1;
                Diff::B
            } else {
                z1 += x1;
                y1 += z1;
                Diff::C
            }
        } else if rx >= rz {
            x1 += y1;
            z1 += x1;
            Diff::D
        } else if ry >= rz {
            z1 += y1;
            x1 += z1;
            Diff::E
        } else {
            y1 += z1;
            x1 += y1;
            Diff::F
        };
        let mut out = [0u16; 3];
        for (k, o) in out.iter_mut().enumerate() {
            let at = base + k;
            let c0 = i32::from(t[at]);
            let mut c1 = i32::from(t[at + x1]);
            let mut c2 = i32::from(t[at + y1]);
            let mut c3 = i32::from(t[at + z1]);
            match case {
                Diff::A => {
                    c3 -= c2;
                    c2 -= c1;
                    c1 -= c0;
                }
                Diff::B => {
                    c2 -= c1;
                    c1 -= c3;
                    c3 -= c0;
                }
                Diff::C => {
                    c2 -= c3;
                    c3 -= c1;
                    c1 -= c0;
                }
                Diff::D => {
                    c3 -= c1;
                    c1 -= c2;
                    c2 -= c0;
                }
                Diff::E => {
                    c1 -= c3;
                    c3 -= c2;
                    c2 -= c0;
                }
                Diff::F => {
                    c1 -= c2;
                    c2 -= c3;
                    c3 -= c0;
                }
            }
            let rest = c1
                .wrapping_mul(rx)
                .wrapping_add(c2.wrapping_mul(ry))
                .wrapping_add(c3.wrapping_mul(rz))
                .wrapping_add(0x8001);
            *o = (c0 as u16 as i32).wrapping_add(rest.wrapping_add(rest >> 16) >> 16) as u16;
        }
        out
    }

    /// `PatchLUT` at white (every input 0xffff, which lands on the last node).
    fn patch_white(&mut self) {
        let last = GRID_POINTS - 1;
        let index: usize = (0..self.inputs).map(|k| self.opta(k) * last).sum();
        self.table[index..index + 3].fill(0xffff);
    }
}

/// `WhitesAreEqual`.
fn whites_equal(white: u16, obtained: &[u16; 3]) -> bool {
    for &o in obtained {
        if (i32::from(white) - i32::from(o)).abs() > 0xf000 {
            return true;
        }
        if white != o {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Optimised evaluators (cmsopt.c)

/// `MatShaper8Data`.
#[derive(Debug, Clone, PartialEq)]
struct MatShaper {
    shaper1: Box<[[i32; 256]; 3]>,
    mat: [[i32; 3]; 3],
    off: [i32; 3],
    shaper2: Box<[[u16; 16385]; 3]>,
}

impl MatShaper {
    fn new(curve1: &[ToneCurve], mat: &Mat3, off: Option<[f64; 3]>, curve2: &[ToneCurve]) -> Self {
        let mut shaper1 = Box::new([[0i32; 256]; 3]);
        for (table, curve) in shaper1.iter_mut().zip(curve1) {
            // `FillFirstShaper`.
            for (i, slot) in table.iter_mut().enumerate() {
                let r = (i as f64 / 255.0) as f32;
                let y = curve.eval_float(r);
                *slot = if f64::from(y) < 131072.0 {
                    to_1fixed14(f64::from(y))
                } else {
                    0x7fff_ffff
                };
            }
        }
        let mut shaper2 = Box::new([[0u16; 16385]; 3]);
        for (table, curve) in shaper2.iter_mut().zip(curve2) {
            // `FillSecondShaper` for 8-bit output.
            for (i, slot) in table.iter_mut().enumerate() {
                let r = (i as f64 / 16384.0) as f32;
                // NaN stays NaN, as in C.
                let val = curve.eval_float(r).clamp(0.0, 1.0);
                let w = saturate_word(f64::from(val) * 65535.0);
                *slot = from_8_to_16(from_16_to_8(w));
            }
        }
        MatShaper {
            shaper1,
            mat: mat.map(|row| row.map(to_1fixed14)),
            off: off.map_or([0; 3], |o| o.map(to_1fixed14)),
            shaper2,
        }
    }

    /// `MatShaperEval16`.
    fn eval16(&self, input: &[u16; 3]) -> [u16; 3] {
        let [r, g, b] = [0, 1, 2].map(|k| self.shaper1[k][usize::from(input[k] & 0xff)]);
        let mut out = [0u16; 3];
        for (k, o) in out.iter_mut().enumerate() {
            let m = self.mat[k];
            let l = m[0]
                .wrapping_mul(r)
                .wrapping_add(m[1].wrapping_mul(g))
                .wrapping_add(m[2].wrapping_mul(b))
                .wrapping_add(self.off[k])
                .wrapping_add(0x2000)
                >> 14;
            let i = l.clamp(0, 16384) as usize;
            *o = self.shaper2[k][i];
        }
        out
    }
}

/// `Prelin8Data`: prelinearisation curves folded into node indices and
/// offsets for 8-bit input.
#[derive(Debug, Clone, PartialEq)]
struct Prelin8 {
    nodes: [[i32; 256]; 3],
    rests: [[i32; 256]; 3],
    clut: Clut,
}

impl Prelin8 {
    /// `PrelinOpt8alloc`.
    fn new(curves: &[ToneCurve], clut: Clut) -> Self {
        let domain = (GRID_POINTS - 1) as i32;
        let mut nodes = [[0i32; 256]; 3];
        let mut rests = [[0i32; 256]; 3];
        for k in 0..3 {
            let opta = clut.opta(2 - k) as i32;
            for i in 0..256 {
                let input = curves[k].eval16(from_8_to_16(i as u8));
                let v = to_fixed_domain(i32::from(input) * domain);
                nodes[k][i] = opta * (v >> 16);
                rests[k][i] = v & 0xffff;
            }
        }
        Prelin8 { nodes, rests, clut }
    }

    /// `PrelinEval8`.
    fn eval16(&self, input: &[u16; 3]) -> [u16; 3] {
        let idx = input.map(|v| usize::from(v >> 8));
        let [x0, y0, z0] = [0, 1, 2].map(|k| self.nodes[k][idx[k]]);
        let [rx, ry, rz] = [0, 1, 2].map(|k| self.rests[k][idx[k]]);
        let step = |r: i32, k: usize| if r == 0 { 0 } else { self.clut.opta(k) as i32 };
        let x1 = x0 + step(rx, 2);
        let y1 = y0 + step(ry, 1);
        let z1 = z0 + step(rz, 0);
        let t = &self.clut.table;
        let mut out = [0u16; 3];
        for (k, o) in out.iter_mut().enumerate() {
            let dens = |i: i32, j: i32, l: i32| i32::from(t[(i + j + l) as usize + k]);
            let c0 = dens(x0, y0, z0);
            let (c1, c2, c3);
            if rx >= ry && ry >= rz {
                c1 = dens(x1, y0, z0) - c0;
                c2 = dens(x1, y1, z0) - dens(x1, y0, z0);
                c3 = dens(x1, y1, z1) - dens(x1, y1, z0);
            } else if rx >= rz && rz >= ry {
                c1 = dens(x1, y0, z0) - c0;
                c2 = dens(x1, y1, z1) - dens(x1, y0, z1);
                c3 = dens(x1, y0, z1) - dens(x1, y0, z0);
            } else if rz >= rx && rx >= ry {
                c1 = dens(x1, y0, z1) - dens(x0, y0, z1);
                c2 = dens(x1, y1, z1) - dens(x1, y0, z1);
                c3 = dens(x0, y0, z1) - c0;
            } else if ry >= rx && rx >= rz {
                c1 = dens(x1, y1, z0) - dens(x0, y1, z0);
                c2 = dens(x0, y1, z0) - c0;
                c3 = dens(x1, y1, z1) - dens(x1, y1, z0);
            } else if ry >= rz && rz >= rx {
                c1 = dens(x1, y1, z1) - dens(x0, y1, z1);
                c2 = dens(x0, y1, z0) - c0;
                c3 = dens(x0, y1, z1) - dens(x0, y1, z0);
            } else if rz >= ry && ry >= rx {
                c1 = dens(x1, y1, z1) - dens(x0, y1, z1);
                c2 = dens(x0, y1, z1) - dens(x0, y0, z1);
                c3 = dens(x0, y0, z1) - c0;
            } else {
                (c1, c2, c3) = (0, 0, 0);
            }
            let rest = c1
                .wrapping_mul(rx)
                .wrapping_add(c2.wrapping_mul(ry))
                .wrapping_add(c3.wrapping_mul(rz))
                .wrapping_add(0x8001);
            *o = c0.wrapping_add(rest.wrapping_add(rest >> 16) >> 16) as u16;
        }
        out
    }
}

/// What a transform evaluates per pixel, in lcms's 16-bit domain.
#[derive(Debug, Clone, PartialEq)]
enum Eval {
    /// `FastIdentity16`.
    Identity,
    /// `FastEvaluateCurves8` or `FastEvaluateCurves16`: joined curves
    /// tabulated at 256 or 65536 inputs.
    Curves {
        eight: bool,
        tables: Vec<Vec<u16>>,
    },
    MatShaper(Box<MatShaper>),
    Prelin8(Box<Prelin8>),
    Clut(Clut),
    /// `_LUTeval16`: no optimisation, the stages in floating point.
    Float(Vec<Stage>),
}

impl Eval {
    fn eval16(&self, input: &[u16; 3]) -> [u16; 3] {
        match self {
            Eval::Identity => *input,
            Eval::Curves { eight, tables } => {
                let mut out = *input;
                for (k, table) in tables.iter().enumerate() {
                    let at = if *eight { input[k] >> 8 } else { input[k] };
                    out[k] = table[usize::from(at)];
                }
                out
            }
            Eval::MatShaper(m) => m.eval16(input),
            Eval::Prelin8(p) => p.eval16(input),
            Eval::Clut(c) => c.eval16(input),
            Eval::Float(stages) => {
                let n = input_channels(stages);
                let mut value = [0.0_f32; 3];
                for k in 0..n {
                    value[k] = f32::from(input[k]) / 65535.0_f32;
                }
                eval_float(stages, value).map(|v| saturate_word(f64::from(v) * 65535.0))
            }
        }
    }
}

fn input_channels(stages: &[Stage]) -> usize {
    match stages.first() {
        Some(Stage::Curves(c)) => c.len(),
        Some(Stage::Matrix { cols, .. }) => *cols,
        None => 3,
    }
}

/// `OptimizeByJoiningCurves`, when every stage is a curve set.
fn join_curves(stages: &[Stage], eight: bool) -> Option<Eval> {
    if !stages.iter().all(|s| matches!(s, Stage::Curves(_))) {
        return None;
    }
    let n = input_channels(stages);
    let mut tables = vec![vec![0u16; PRELIN_POINTS]; n];
    for i in 0..PRELIN_POINTS {
        let v = (i as f64 / (PRELIN_POINTS - 1) as f64) as f32;
        let out = eval_float(stages, [v; 3]);
        for (table, value) in tables.iter_mut().zip(out) {
            table[i] = saturate_word(f64::from(value) * 65535.0);
        }
    }
    let curves: Vec<ToneCurve> = tables.into_iter().map(ToneCurve::tabulated).collect();
    if curves.iter().all(ToneCurve::is_linear) {
        return Some(Eval::Identity);
    }
    let tables = curves
        .iter()
        .map(|c| {
            if eight {
                (0..=255u8).map(|j| c.eval16(from_8_to_16(j))).collect()
            } else {
                (0..=u16::MAX).map(|j| c.eval16(j)).collect()
            }
        })
        .collect();
    Some(Eval::Curves { eight, tables })
}

/// `OptimizeMatrixShaper`: 8-bit RGB through curves, one matrix (or two
/// that multiply), curves.
fn matrix_shaper(stages: &[Stage]) -> Option<Eval> {
    let (curve1, res, offset, curve2) = match stages {
        [
            Stage::Curves(c1),
            Stage::Matrix {
                rows: 3,
                cols: 3,
                m: m1,
                offset: None,
            },
            Stage::Matrix {
                rows: 3,
                cols: 3,
                m: m2,
                offset: o2,
            },
            Stage::Curves(c2),
        ] => (c1, mat_per(&to_mat3(m2), &to_mat3(m1)), *o2, c2),
        [
            Stage::Curves(c1),
            Stage::Matrix {
                rows: 3,
                cols: 3,
                m,
                offset,
            },
            Stage::Curves(c2),
        ] => (c1, to_mat3(m), *offset, c2),
        _ => return None,
    };
    if c_len(curve1) != 3 || c_len(curve2) != 3 {
        return None;
    }
    if is_identity(&res) && offset.is_none() {
        let joined = [Stage::Curves(curve1.clone()), Stage::Curves(curve2.clone())];
        return join_curves(&joined, true);
    }
    Some(Eval::MatShaper(Box::new(MatShaper::new(
        curve1, &res, offset, curve2,
    ))))
}

fn c_len(c: &[ToneCurve]) -> usize {
    c.len()
}

/// `SlopeLimiting`: straight lines over the first and last 2%.
fn slope_limit(curve: &mut ToneCurve) {
    let n = curve.table.len();
    let at_begin = (n as f64 * 0.02 + 0.5).floor() as usize;
    let at_end = n - at_begin - 1;
    let (begin_val, end_val) = if curve.descending() {
        (65535.0, 0.0)
    } else {
        (0.0, 65535.0)
    };
    let val = f64::from(curve.table[at_begin]);
    let slope = (val - begin_val) / at_begin as f64;
    let beta = val - slope * at_begin as f64;
    for i in 0..at_begin {
        curve.table[i] = saturate_word(i as f64 * slope + beta);
    }
    let val = f64::from(curve.table[at_end]);
    let slope = (end_val - val) / at_begin as f64;
    let beta = val - slope * at_end as f64;
    for i in at_end..n {
        curve.table[i] = saturate_word(i as f64 * slope + beta);
    }
}

/// `OptimizeByComputingLinearization` for 8-bit RGB: per-channel curves from
/// a grey ramp, then a table sampled through their inverses.
fn linearization(stages: &[Stage], intent: u32) -> Option<Eval> {
    if let Some(Stage::Curves(last)) = stages.last()
        && last.iter().any(ToneCurve::is_degenerated)
    {
        return None;
    }
    let mut trans = vec![vec![0u16; PRELIN_POINTS]; 3];
    for i in 0..PRELIN_POINTS {
        let v = (i as f64 / (PRELIN_POINTS - 1) as f64) as f32;
        let out = eval_float(stages, [v; 3]);
        for (t, value) in trans.iter_mut().zip(out) {
            t[i] = saturate_word(f64::from(value) * 65535.0);
        }
    }
    let mut trans: Vec<ToneCurve> = trans.into_iter().map(ToneCurve::tabulated).collect();
    for t in &mut trans {
        slope_limit(t);
    }
    if trans
        .iter()
        .any(|t| !t.is_monotonic() || t.is_degenerated())
    {
        return None;
    }
    let reverse: Vec<ToneCurve> = trans.iter().map(|t| t.reverse(PRELIN_POINTS)).collect();
    let mut plus = Vec::with_capacity(stages.len() + 1);
    plus.push(Stage::Curves(reverse));
    plus.extend_from_slice(stages);
    let clut = Clut::sample(&plus, 3);
    let mut prelin = Prelin8::new(&trans, clut);
    if intent != 3 {
        // `FixWhiteMisalignment` over prelinearisation curves and a table.
        let obtained = prelin.eval16(&[0xffff; 3]);
        if !whites_equal(0xffff, &obtained) {
            let white_in = trans.iter().map(|t| t.eval16(0xffff));
            let last = (GRID_POINTS - 1) as f64;
            if white_in
                .map(|w| f64::from(w) * last / 65535.0)
                .all(|p| p - p.floor() == 0.0)
            {
                let at: usize = trans
                    .iter()
                    .enumerate()
                    .map(|(k, t)| {
                        let p = f64::from(t.eval16(0xffff)) * last / 65535.0;
                        prelin.clut.opta(2 - k) * p.floor() as usize
                    })
                    .sum();
                prelin.clut.table[at..at + 3].fill(0xffff);
            }
        }
    }
    Some(Eval::Prelin8(Box::new(prelin)))
}

/// `OptimizeByResampling`: the pipeline sampled into a 33-point table.
fn resampling(stages: &[Stage], intent: u32) -> Eval {
    let inputs = input_channels(stages);
    let mut clut = Clut::sample(stages, inputs);
    if intent != 3 {
        let obtained = clut.eval16(&[0xffff; 3]);
        if !whites_equal(0xffff, &obtained) {
            clut.patch_white();
        }
    }
    Eval::Clut(clut)
}

// ---------------------------------------------------------------------------
// The transform (cmsxform.c, cmscnvrt.c, cmsio1.c)

/// The input samples' format, as OpenJPEG picks the lcms format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    /// `TYPE_RGB_8` to `TYPE_RGB_8`.
    Rgb8,
    /// `TYPE_RGB_16` to `TYPE_RGB_16`.
    Rgb16,
    /// `TYPE_GRAY_8` to `TYPE_RGB_8`.
    Grey8,
}

/// What the transform needs from a restricted input profile.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InputProfile {
    /// The rendering intent from the header, 0 to 3.
    pub(crate) intent: u32,
    /// The media white point (`wtpt`), or `None` for D50.
    pub(crate) white: Option<[f64; 3]>,
    /// `rTRC`, `gTRC`, `bTRC`, or `kTRC` alone.
    pub(crate) curves: Vec<ToneCurve>,
    /// The colorants `rXYZ`, `gXYZ`, `bXYZ` as rows X, Y, Z; unused for
    /// grey.
    pub(crate) colorants: Mat3,
}

/// A built transform.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Transform {
    format: Format,
    eval: Eval,
}

impl Transform {
    /// `cmsCreateTransform(input, format, sRGB, format, intent, 0)`.
    pub(crate) fn new(profile: &InputProfile, format: Format) -> Self {
        let intent = profile.intent;
        let mut stages = Vec::new();
        // `_cmsReadInputLUT` for a matrix/TRC or monochrome profile.
        if format == Format::Grey8 {
            stages.push(Stage::Curves(profile.curves[..1].to_vec()));
            stages.push(Stage::Matrix {
                rows: 3,
                cols: 1,
                m: D50.iter().map(|&d| INP_ADJ * d).collect(),
                offset: None,
            });
        } else {
            stages.push(Stage::Curves(profile.curves.clone()));
            let m = profile.colorants.map(|row| row.map(|v| v * INP_ADJ));
            stages.push(Stage::matrix(&m, None));
        }
        // `ComputeConversion` and `AddConversion`.
        if intent == 3 {
            let white = profile.white.unwrap_or(D50);
            let m = [
                [white[0] / D50[0], 0.0, 0.0],
                [0.0, white[1] / D50[1], 0.0],
                [0.0, 0.0, white[2] / D50[2]],
            ];
            if !is_empty_layer(&m, &[0.0; 3]) {
                stages.push(Stage::matrix(&m, Some([0.0; 3])));
            }
        } else if intent == 0 || intent == 2 {
            // The sRGB profile is ICC v4, which makes lcms apply black point
            // compensation under perceptual and saturation. Its black is XYZ
            // 0; the input's is what its curves and matrix give device 0.
            let black_in = black_point(eval_float(&stages, [0.0; 3]));
            let black_out = black_point([0.0; 3]);
            if black_in != black_out {
                let (m, off) = black_point_compensation(black_in, black_out);
                let off = off.map(|o| o / MAX_ENCODEABLE_XYZ);
                if !is_empty_layer(&m, &off) {
                    stages.push(Stage::matrix(&m, Some(off)));
                }
            }
        }
        // `BuildRGBOutputMatrixShaper` for the built-in sRGB profile.
        let inverse = mat_inverse(&srgb_colorants()).expect("the sRGB colorants invert");
        stages.push(Stage::matrix(
            &inverse.map(|row| row.map(|v| v * OUTP_ADJ)),
            None,
        ));
        let srgb = ToneCurve::parametric(4, srgb_params()).reverse(4096);
        stages.push(Stage::Curves(vec![srgb; 3]));

        let noopt = format == Format::Rgb16 && {
            let gamma = detect_gamma(profile);
            gamma > 0.0 && gamma < 1.6
        };
        pre_optimize(&mut stages);
        let eval = if noopt {
            Eval::Float(stages)
        } else {
            let eight = format != Format::Rgb16;
            join_curves(&stages, eight)
                .or_else(|| {
                    (format == Format::Rgb8)
                        .then(|| matrix_shaper(&stages))
                        .flatten()
                })
                .or_else(|| {
                    (format == Format::Rgb8)
                        .then(|| linearization(&stages, intent))
                        .flatten()
                })
                .unwrap_or_else(|| resampling(&stages, intent))
        };
        Transform { format, eval }
    }

    /// One 8-bit RGB pixel (`Unroll3Bytes`, the evaluator, `Pack3Bytes`).
    pub(crate) fn rgb8(&self, rgb: [u8; 3]) -> [u8; 3] {
        debug_assert_eq!(self.format, Format::Rgb8);
        self.eval.eval16(&rgb.map(from_8_to_16)).map(from_16_to_8)
    }

    /// One 16-bit RGB pixel.
    pub(crate) fn rgb16(&self, rgb: [u16; 3]) -> [u16; 3] {
        debug_assert_eq!(self.format, Format::Rgb16);
        self.eval.eval16(&rgb)
    }

    /// One 8-bit grey sample to 8-bit RGB (`Unroll1Byte` copies it to all
    /// three inputs).
    pub(crate) fn grey8(&self, grey: u8) -> [u8; 3] {
        debug_assert_eq!(self.format, Format::Grey8);
        self.eval.eval16(&[from_8_to_16(grey); 3]).map(from_16_to_8)
    }
}

/// `IsEmptyLayer`: the conversion is within 0.002 of nothing.
fn is_empty_layer(m: &Mat3, off: &[f64; 3]) -> bool {
    let mut diff = 0.0;
    for i in 0..3 {
        for j in 0..3 {
            diff += (m[i][j] - IDENTITY[i][j]).abs();
        }
    }
    for o in off {
        diff += o.abs();
    }
    diff < 0.002
}

/// `BlackPointAsDarkerColorant`: the black point of a profile whose
/// device black reaches the PCS as `encoded` (lcms's 0..1 XYZ encoding), by
/// way of a `TYPE_Lab_DBL` transform: L* only, clipped to 0..=50, with a* and
/// b* forced to 0.
fn black_point(encoded: [f32; 3]) -> [f64; 3] {
    // `EvaluateXYZ2Lab` and `PackLabDoubleFromFloat`.
    let y = f64::from(encoded[1]) * MAX_ENCODEABLE_XYZ;
    let l = 116.0 * lab_f(y / D50[1]) - 16.0;
    let l = (f64::from((l / 100.0) as f32) * 100.0).clamp(0.0, 50.0);
    // `cmsLab2XYZ` with a* = b* = 0.
    let fy = (l + 16.0) / 116.0;
    let fx = fy + 0.002 * 0.0;
    let fz = fy - 0.005 * 0.0;
    [
        lab_f_inv(fx) * D50[0],
        lab_f_inv(fy) * D50[1],
        lab_f_inv(fz) * D50[2],
    ]
}

/// `f` of `cmsXYZ2Lab`.
fn lab_f(t: f64) -> f64 {
    let limit = (24.0 / 116.0) * (24.0 / 116.0) * (24.0 / 116.0);
    if t <= limit {
        (841.0 / 108.0) * t + (16.0 / 116.0)
    } else {
        t.powf(1.0 / 3.0)
    }
}

/// `f_1` of `cmsLab2XYZ`.
fn lab_f_inv(t: f64) -> f64 {
    let limit = 24.0 / 116.0;
    if t <= limit {
        (108.0 / 841.0) * (t - (16.0 / 116.0))
    } else {
        t * t * t
    }
}

/// `ComputeBlackPointCompensation`: the scale and offset that take the input
/// black to the output black and keep D50.
fn black_point_compensation(black_in: [f64; 3], black_out: [f64; 3]) -> (Mat3, [f64; 3]) {
    let mut m = [[0.0; 3]; 3];
    let mut off = [0.0; 3];
    for k in 0..3 {
        let t = black_in[k] - D50[k];
        m[k][k] = (black_out[k] - D50[k]) / t;
        off[k] = -D50[k] * (black_out[k] - black_in[k]) / t;
    }
    (m, off)
}

/// `cmsDetectRGBProfileGamma(profile, 0.1)`: lcms leaves a 16-bit transform
/// unoptimised when the profile's grey ramp fits a gamma below 1.6. The ramp
/// is the profile's Y for R = G = B, as the `TYPE_XYZ_DBL` transform lcms
/// runs gives it.
fn detect_gamma(profile: &InputProfile) -> f64 {
    let row = profile.colorants[1].map(|v| v * INP_ADJ);
    let ys: Vec<f32> = (0..=255u8)
        .map(|i| {
            let v = (f64::from(from_8_to_16(i)) / 65535.0) as f32;
            let lin: Vec<f32> = profile.curves.iter().map(|c| c.eval_float(v)).collect();
            let mut tmp = 0.0_f64;
            for (l, m) in lin.iter().zip(row) {
                tmp += f64::from(*l) * m;
            }
            (f64::from(tmp as f32) * MAX_ENCODEABLE_XYZ) as f32
        })
        .collect();
    estimate_gamma(&ys, 0.1)
}

/// `cmsEstimateGamma` on `cmsBuildTabulatedToneCurveFloat(ys)`.
fn estimate_gamma(ys: &[f32], precision: f64) -> f64 {
    const NODES: usize = 4097;
    let domain = (ys.len() - 1) as f32;
    // The middle segment's `LinLerp1Dfloat`.
    let eval = |x: f32| -> f32 {
        let v = if x < 1.0e-9 {
            0.0
        } else if x > 1.0 {
            1.0
        } else {
            x
        };
        if v == 1.0 {
            return ys[ys.len() - 1];
        }
        let v = v * domain;
        let (c0, c1) = (v.floor() as usize, v.ceil() as usize);
        let rest = v - c0 as f32;
        ys[c0] + (ys[c1] - ys[c0]) * rest
    };
    let (mut sum, mut sum2, mut n) = (0.0_f64, 0.0_f64, 0.0_f64);
    for i in 1..NODES - 1 {
        let x = i as f64 / (NODES - 1) as f64;
        let y = f64::from(eval(((x as f32) as f64 / 1.0) as f32));
        if y > 0.0 && y < 1.0 && x > 0.07 {
            let gamma = y.ln() / x.ln();
            sum += gamma;
            sum2 += gamma * gamma;
            n += 1.0;
        }
    }
    if n <= 1.0 {
        return -1.0;
    }
    let std = ((n * sum2 - sum * sum) / (n * (n - 1.0))).sqrt();
    if std > precision {
        return -1.0;
    }
    sum / n
}
