//! sRGB output tests: the conversions on hand-computed vectors, the chroma
//! replication against OpenJPEG, and every input the sRGB step refuses, each
//! through the public [`decode_with`].
//!
//! The ICC vectors were computed independently in double precision (numpy)
//! and agree with LittleCMS 2 (Pillow's `ImageCms`, an sRGB output profile)
//! on every value used here.

use super::SIGNATURE;
use super::srgb::{ChromaLayout, Curve, replicate, sycc_sample};
use crate::{ColourOutput, DecodeOptions, Error, Image, decode, decode_with};

/// Three 8-bit components at full resolution, 24x16.
const THREE: &[u8] = include_bytes!("../../tests/fixtures/multicomponent_lossless.j2k");
/// One 8-bit component, 32x24.
const ONE: &[u8] = include_bytes!("../../tests/fixtures/multilayer_lossless.j2k");
/// Three 8-bit components, all sub-sampled 2x2.
const ALL_SUBSAMPLED: &[u8] =
    include_bytes!("../../tests/fixtures/multicomponent_subsampled_lossless.j2k");
/// The sYCC 4:2:0 fixture at an odd origin.
const SYCC_420: &[u8] = include_bytes!("../../tests/fixtures/jp2_sycc_420_odd.jp2");
const SYCC_420_SRGB: &str = include_str!("../../tests/fixtures/jp2_sycc_420_odd.srgb.json");
const SYCC_422: &[u8] = include_bytes!("../../tests/fixtures/jp2_sycc_422_odd.jp2");
const SYCC_422_SRGB: &str = include_str!("../../tests/fixtures/jp2_sycc_422_odd.srgb.json");

fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = (8 + payload.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// The first top-level box of `kind` in a JP2 file: its payload.
fn top_box<'a>(file: &'a [u8], kind: &[u8; 4]) -> &'a [u8] {
    let mut at = SIGNATURE.len();
    while at + 8 <= file.len() {
        let length = u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        if &file[at + 4..at + 8] == kind {
            return &file[at + 8..at + length];
        }
        at += length;
    }
    panic!("no {kind:?} box");
}

/// The SIZ fields `decode` reads first: width, height, component count.
fn geometry(codestream: &[u8]) -> (u32, u32, u16) {
    let siz = &codestream[6..];
    let word = |at: usize| u32::from_be_bytes(siz[at..at + 4].try_into().unwrap());
    let (xsiz, ysiz, x0, y0) = (word(2), word(6), word(10), word(14));
    let csiz = u16::from_be_bytes(siz[34..36].try_into().unwrap());
    (xsiz - x0, ysiz - y0, csiz)
}

/// A JP2 file around `codestream` with `colr` (none if empty) and `children`
/// in `jp2h`.
fn file(codestream: &[u8], colr: &[u8], children: &[Vec<u8>]) -> Vec<u8> {
    let (width, height, components) = geometry(codestream);
    let mut ihdr = height.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&components.to_be_bytes());
    ihdr.extend_from_slice(&[7, 7, 0, 0]);
    let mut jp2h = bx(b"ihdr", &ihdr);
    if !colr.is_empty() {
        jp2h.extend_from_slice(&bx(b"colr", colr));
    }
    jp2h.extend_from_slice(&children.concat());
    let mut out = SIGNATURE.to_vec();
    out.extend_from_slice(&bx(b"ftyp", b"jp2 \0\0\0\0jp2 "));
    out.extend_from_slice(&bx(b"jp2h", &jp2h));
    out.extend_from_slice(&bx(b"jp2c", codestream));
    out
}

fn enumerated(value: u32) -> Vec<u8> {
    let mut out = vec![1, 0, 0];
    out.extend_from_slice(&value.to_be_bytes());
    out
}

fn icc_colr(profile: &[u8]) -> Vec<u8> {
    let mut out = vec![2, 0, 0];
    out.extend_from_slice(profile);
    out
}

fn srgb() -> DecodeOptions {
    DecodeOptions {
        colour: ColourOutput::Srgb,
        ..DecodeOptions::default()
    }
}

fn decode_srgb(bytes: &[u8]) -> crate::Result<Image> {
    decode_with(bytes, srgb())
}

/// An `XYZ ` tag.
fn xyz(values: [i32; 3]) -> Vec<u8> {
    let mut out = b"XYZ \0\0\0\0".to_vec();
    for v in values {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out
}

/// A `curv` tag with these entries.
fn curv(entries: &[u16]) -> Vec<u8> {
    let mut out = b"curv\0\0\0\0".to_vec();
    out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for e in entries {
        out.extend_from_slice(&e.to_be_bytes());
    }
    out
}

/// A `para` tag of `function` type with these s15.16 parameters.
fn para(function: u16, params: &[f64]) -> Vec<u8> {
    let mut out = b"para\0\0\0\0".to_vec();
    out.extend_from_slice(&function.to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    for p in params {
        out.extend_from_slice(&((p * 65536.0).round() as i32).to_be_bytes());
    }
    out
}

/// An ICC profile of `class`, `space`, `pcs` and `intent` with these tags.
fn profile(
    class: &[u8; 4],
    space: &[u8; 4],
    pcs: &[u8; 4],
    intent: u32,
    tags: &[(&[u8; 4], Vec<u8>)],
) -> Vec<u8> {
    let mut header = vec![0u8; 128];
    header[8..12].copy_from_slice(&0x0220_0000u32.to_be_bytes());
    header[12..16].copy_from_slice(class);
    header[16..20].copy_from_slice(space);
    header[20..24].copy_from_slice(pcs);
    header[36..40].copy_from_slice(b"acsp");
    header[64..68].copy_from_slice(&intent.to_be_bytes());
    let mut table = (tags.len() as u32).to_be_bytes().to_vec();
    let mut data = Vec::new();
    let mut offset = 128 + 4 + 12 * tags.len();
    for (signature, bytes) in tags {
        table.extend_from_slice(*signature);
        table.extend_from_slice(&(offset as u32).to_be_bytes());
        table.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        data.extend_from_slice(bytes);
        offset += bytes.len();
    }
    let mut out = header;
    out.extend_from_slice(&table);
    out.extend_from_slice(&data);
    let size = out.len() as u32;
    out[..4].copy_from_slice(&size.to_be_bytes());
    out
}

/// Gamma 1.8 as the `u8.8` that file5's profile stores (1.80078125).
const GAMMA_18: u16 = 461;

/// file5's wide-gamut primaries (s15.16) with gamma-1.8 curves.
fn wide_gamut_tags() -> Vec<(&'static [u8; 4], Vec<u8>)> {
    vec![
        (b"rXYZ", xyz([52278, 18874, 0])),
        (b"gXYZ", xyz([8860, 46655, 0])),
        (b"bXYZ", xyz([2051, 7, 54061])),
        (b"rTRC", curv(&[GAMMA_18])),
        (b"gTRC", curv(&[GAMMA_18])),
        (b"bTRC", curv(&[GAMMA_18])),
    ]
}

fn wide_gamut() -> Vec<u8> {
    profile(b"scnr", b"RGB ", b"XYZ ", 0, &wide_gamut_tags())
}

fn grey_profile() -> Vec<u8> {
    profile(
        b"scnr",
        b"GRAY",
        b"XYZ ",
        0,
        &[(b"kTRC", curv(&[GAMMA_18]))],
    )
}

/// A `pclr` of `entries` rows over columns of the given `Bi`, each entry `j`
/// of column `i` holding `value(j, i)`, and the `cmap` that maps component 0
/// through every column.
fn palette(bi: &[u8], entries: u16, value: impl Fn(u16, usize) -> u32) -> Vec<Vec<u8>> {
    let mut pclr = entries.to_be_bytes().to_vec();
    pclr.push(bi.len() as u8);
    pclr.extend_from_slice(bi);
    for j in 0..entries {
        for (i, &b) in bi.iter().enumerate() {
            let bytes = usize::from((b & 0x7F) / 8 + 1);
            pclr.extend_from_slice(&value(j, i).to_be_bytes()[4 - bytes..]);
        }
    }
    let mut cmap = Vec::new();
    for i in 0..bi.len() {
        cmap.extend_from_slice(&[0, 0, 1, i as u8]);
    }
    vec![bx(b"pclr", &pclr), bx(b"cmap", &cmap)]
}

#[track_caller]
fn expect_err(result: crate::Result<Image>, want: fn(&Error) -> bool, fragment: &str) {
    match result {
        Err(e) if want(&e) => assert!(
            e.to_string().contains(fragment),
            "expected the error to mention {fragment:?}, got {e}"
        ),
        other => panic!("expected an error mentioning {fragment:?}, got {other:?}"),
    }
}

fn is_unsupported(e: &Error) -> bool {
    matches!(e, Error::Unsupported(_))
}
fn is_file_format(e: &Error) -> bool {
    matches!(e, Error::FileFormat(_))
}

// ---------------------------------------------------------------------------
// sYCC
// ---------------------------------------------------------------------------

/// IEC 61966-2-1 Amd 1 by hand: R = Y + 1.402 Cr, G = Y - 0.344136 Cb -
/// 0.714136 Cr, B = Y + 1.772 Cb, with the chroma offset by 2^(depth-1),
/// rounded and clamped.
#[test]
fn sycc_matches_hand_computed_values() {
    // Neutral chroma is grey.
    assert_eq!(sycc_sample(100, 128, 128, 128, 255), [100, 100, 100]);
    // Cb = +10, Cr = -10: 85.98, 103.7, 117.72.
    assert_eq!(sycc_sample(100, 138, 118, 128, 255), [86, 104, 118]);
    // Clamped at both ends: 250 + 1.402 * 127 and 5 - 1.772 * 128.
    assert_eq!(sycc_sample(250, 128, 255, 128, 255)[0], 255);
    assert_eq!(sycc_sample(5, 0, 128, 128, 255)[2], 0);
    // 16-bit: Cb = +100 gives G = 40000 - 34.4136, B = 40000 + 177.2.
    assert_eq!(
        sycc_sample(40000, 32868, 32768, 32768, 65535),
        [40000, 39966, 40177]
    );
    // Large 16-bit chroma pins each green constant to six places: Cb = +30000
    // gives 30000 - 10324.08 (0.344 would give 19680), Cr = +30000 gives
    // 30000 - 21424.08 (0.714 would give 8580).
    assert_eq!(
        sycc_sample(30000, 62768, 32768, 32768, 65535),
        [30000, 19676, 65535]
    );
    assert_eq!(
        sycc_sample(30000, 32768, 62768, 32768, 65535),
        [65535, 8576, 30000]
    );
    // The IEC constants and rounding, not OpenJPEG's 0.344 and truncation:
    // Cb = Cr = +100 gives G = 200 - 105.8272 = 94.17 (OpenJPEG: 200 - 105 = 95).
    assert_eq!(sycc_sample(200, 228, 228, 128, 255)[1], 94);
}

/// The luma-to-chroma map of a replication, `None` where OpenJPEG uses chroma
/// 0. The chroma width is the one the image geometry gives: the luma width
/// halved, rounded up, plus one for an odd origin of even width.
fn map(width: usize, height: usize, layout: ChromaLayout, odd: (bool, bool)) -> Vec<Option<usize>> {
    let chroma_width = match layout {
        ChromaLayout::Full => width,
        _ => (usize::from(odd.0) + width).div_ceil(2),
    };
    map_with(width, height, chroma_width, layout, odd)
}

fn map_with(
    width: usize,
    height: usize,
    chroma_width: usize,
    layout: ChromaLayout,
    odd: (bool, bool),
) -> Vec<Option<usize>> {
    let mut out = vec![Some(usize::MAX); width * height];
    replicate(
        width,
        height,
        chroma_width,
        layout,
        odd.0,
        odd.1,
        &mut |luma, chroma| {
            out[luma] = chroma;
        },
    );
    assert!(
        !out.contains(&Some(usize::MAX)),
        "every luma sample is visited"
    );
    out
}

/// A 4x4 tile at an odd origin, traced through OpenJPEG 2.5.0's
/// `sycc420_to_rgb` by hand: the first row and the first column of the upper
/// row of the pair take chroma 0, the lower row's first column takes the
/// pair's first chroma, and the last, unpaired row starts with chroma 0 too.
#[test]
fn replication_420_at_an_odd_origin_follows_openjpeg() {
    let n = None;
    let s = Some;
    assert_eq!(
        map(4, 4, ChromaLayout::Both, (true, true)),
        [
            n,
            n,
            n,
            n, //
            n,
            s(0),
            s(0),
            s(1), //
            s(0),
            s(0),
            s(0),
            s(1), //
            n,
            s(2),
            s(2),
            s(3),
        ]
    );
    // An even origin: plain 2x2 replication, a last odd column and row
    // taking the edge chroma.
    assert_eq!(
        map(3, 3, ChromaLayout::Both, (false, false)),
        [s(0), s(0), s(1), s(0), s(0), s(1), s(2), s(2), s(3)]
    );
}

#[test]
fn replication_422_at_an_odd_origin_follows_openjpeg() {
    assert_eq!(
        map(4, 2, ChromaLayout::Horizontal, (true, false)),
        [
            None,
            Some(0),
            Some(0),
            Some(1),
            None,
            Some(2),
            Some(2),
            Some(3)
        ]
    );
    assert_eq!(
        map(3, 1, ChromaLayout::Horizontal, (false, false)),
        [Some(0), Some(0), Some(1)]
    );
    assert_eq!(
        map(2, 2, ChromaLayout::Full, (true, true)),
        [Some(0), Some(1), Some(2), Some(3)]
    );
}

/// OpenJPEG's `comp12w` guard: a last odd column whose pair index equals the
/// chroma width takes chroma 0 instead of reading past the row, and the
/// running chroma cursor does not advance.
#[test]
fn replication_past_the_chroma_width_uses_zero_chroma() {
    assert_eq!(
        map_with(3, 2, 1, ChromaLayout::Horizontal, (false, false)),
        [Some(0), Some(0), None, Some(1), Some(1), None]
    );
    assert_eq!(
        map_with(3, 2, 1, ChromaLayout::Both, (false, false)),
        [Some(0), Some(0), None, Some(0), Some(0), None]
    );
}

/// OpenJPEG 2.5.0's `sycc_to_rgb`: 0.344 and 0.714, each product truncated
/// toward zero. Used only to show the replication matches OpenJPEG exactly.
fn sycc_sample_openjpeg(y: i32, cb: i32, cr: i32) -> [i32; 3] {
    let (cb, cr) = (f64::from(cb - 128), f64::from(cr - 128));
    [
        y + (1.402 * cr) as i32,
        y - (0.344 * cb + 0.714 * cr) as i32,
        y + (1.772 * cb) as i32,
    ]
    .map(|v| v.clamp(0, 255))
}

/// The committed `opj_decompress` output for each sYCC fixture, rebuilt from
/// our as-coded decode with OpenJPEG's own formula through [`replicate`]:
/// every sample must match, so the replication, odd origin included, is
/// OpenJPEG's.
#[test]
fn replication_reproduces_openjpeg_on_the_sycc_fixtures() {
    for (bytes, oracle, layout) in [
        (SYCC_420, SYCC_420_SRGB, ChromaLayout::Both),
        (SYCC_422, SYCC_422_SRGB, ChromaLayout::Horizontal),
    ] {
        let coded = decode(bytes).expect("as coded");
        let oracle: serde_json::Value = serde_json::from_str(oracle).expect("json");
        let [y, cb, cr] = [0, 1, 2].map(|k| &coded.components[k]);
        let mut rebuilt = [
            vec![0; y.samples.len()],
            vec![0; y.samples.len()],
            vec![0; y.samples.len()],
        ];
        replicate(
            y.width as usize,
            y.height as usize,
            cb.width as usize,
            layout,
            true,
            true,
            &mut |luma, chroma| {
                let (b, r) = chroma.map_or((0, 0), |c| (cb.samples[c], cr.samples[c]));
                let rgb = sycc_sample_openjpeg(y.samples[luma], b, r);
                for (plane, v) in rebuilt.iter_mut().zip(rgb) {
                    plane[luma] = v;
                }
            },
        );
        for (k, plane) in rebuilt.iter().enumerate() {
            let want: Vec<i32> = oracle["components"][k]["samples"]
                .as_array()
                .expect("samples")
                .iter()
                .map(|v| v.as_i64().expect("int") as i32)
                .collect();
            assert_eq!(plane, &want, "channel {k}");
        }
    }
}

// ---------------------------------------------------------------------------
// ICC
// ---------------------------------------------------------------------------

#[test]
fn curves_evaluate_as_icc_defines_them() {
    assert_eq!(Curve::Identity.eval(0.3), 0.3);
    assert!((Curve::Gamma(2.0).eval(0.5) - 0.25).abs() < 1e-12);
    let table = Curve::Table(vec![0, 32768, 65535]);
    assert!((table.eval(0.25) - 16384.0 / 65535.0).abs() < 1e-12);
    assert_eq!(table.eval(1.0), 1.0);
    // Type 0 is a plain gamma; type 3 is linear below d (the sRGB form).
    assert!(
        (Curve::Parametric(0, [2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]).eval(0.5) - 0.25).abs() < 1e-12
    );
    let srgb = [
        2.4,
        1.0 / 1.055,
        0.055 / 1.055,
        1.0 / 12.92,
        0.04045,
        0.0,
        0.0,
    ];
    let s = Curve::Parametric(3, srgb);
    assert!((s.eval(0.02) - 0.02 / 12.92).abs() < 1e-12);
    assert!((s.eval(0.5) - 0.214_041_140_5).abs() < 1e-9);
    // Type 4 adds e above d and f below it.
    let p = Curve::Parametric(4, [1.0, 1.0, 0.0, 2.0, 0.5, 0.1, 0.05]);
    assert!((p.eval(0.25) - 0.55).abs() < 1e-12);
    assert!((p.eval(0.75) - 0.85).abs() < 1e-12);
    // Out of range values clamp.
    assert_eq!(
        Curve::Parametric(2, [1.0, 1.0, 0.0, 2.0, 0.0, 0.0, 0.0]).eval(0.5),
        1.0
    );
}

/// A 1-pixel image through file5's gamma-1.8 wide-gamut profile, via a
/// palette so the pixel is exact: (120, 100, 80) -> (160, 114, 95) and
/// (200, 100, 50) -> (255, 80, 46), as numpy and LittleCMS both give.
#[test]
fn an_rgb_profile_matches_hand_computed_values() {
    let pixels = [
        [120u32, 100, 80],
        [200, 100, 50],
        [0, 0, 0],
        [255, 255, 255],
    ];
    let boxes = palette(&[7, 7, 7], 256, |j, i| pixels[usize::from(j) % 4][i]);
    let bytes = file(ONE, &icc_colr(&wide_gamut()), &boxes);
    let coded = decode(&bytes).expect("as coded");
    let image = decode_srgb(&bytes).expect("sRGB");
    let want: [[i32; 3]; 4] = [[160, 114, 95], [255, 80, 46], [0, 0, 0], [255, 255, 255]];
    let mut seen = [false; 4];
    for at in 0..coded.components[0].samples.len() {
        let input: Vec<i32> = (0..3).map(|k| coded.components[k].samples[at]).collect();
        let which = pixels
            .iter()
            .position(|p| p.iter().zip(&input).all(|(&a, &b)| a as i32 == b))
            .expect("a palette colour");
        seen[which] = true;
        let output: Vec<i32> = (0..3).map(|k| image.components[k].samples[at]).collect();
        assert_eq!(output, want[which], "pixel {input:?}");
    }
    assert_eq!(seen, [true; 4], "the image holds every palette colour");
    for c in &image.components {
        assert_eq!((c.bit_depth, c.signed), (8, false), "the depth is kept");
    }
}

/// A grey ramp through a `kTRC` gamma-1.8 profile: the tone curve, then the
/// sRGB curve, as one channel.
#[test]
fn a_grey_profile_matches_hand_computed_values() {
    let ramp = [0u32, 64, 128, 192, 255];
    let boxes = palette(&[7], 256, |j, _| ramp[usize::from(j) % 5]);
    let bytes = file(ONE, &icc_colr(&grey_profile()), &boxes);
    let coded = decode(&bytes).expect("as coded");
    let image = decode_srgb(&bytes).expect("sRGB");
    assert_eq!(image.components.len(), 1, "one grey channel");
    let want = [0, 81, 146, 203, 255];
    for (input, output) in coded.components[0]
        .samples
        .iter()
        .zip(&image.components[0].samples)
    {
        let at = ramp.iter().position(|&r| r as i32 == *input).expect("ramp");
        assert_eq!(*output, want[at], "grey {input}");
    }
}

/// A profile that is the identity in all but the matrix (the sRGB primaries
/// themselves, linear curves) leaves a mid grey where the sRGB curve puts it,
/// and `para` curves are read.
#[test]
fn a_parametric_curve_profile_is_applied() {
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"rXYZ", xyz([28571, 14574, 912])),
        (b"gXYZ", xyz([25253, 46992, 6366])),
        (b"bXYZ", xyz([9367, 3971, 46782])),
        (b"rTRC", para(0, &[1.0])),
        (b"gTRC", para(0, &[1.0])),
        (b"bTRC", para(0, &[1.0])),
    ];
    let icc = profile(b"scnr", b"RGB ", b"XYZ ", 0, &tags);
    let boxes = palette(&[7, 7, 7], 1, |_, _| 51);
    let image = decode_srgb(&file(ONE, &icc_colr(&icc), &boxes)).expect("sRGB");
    // Linear 0.2 encodes to 0.4845 of 255.
    for c in &image.components {
        assert!(
            c.samples.iter().all(|&s| (s - 124).abs() <= 1),
            "{:?}",
            &c.samples[..4]
        );
    }
}

// ---------------------------------------------------------------------------
// What passes through, and what is refused
// ---------------------------------------------------------------------------

/// sRGB and greyscale files come out as coded.
#[test]
fn srgb_and_greyscale_pass_through() {
    for (codestream, colr) in [(THREE, enumerated(16)), (ONE, enumerated(17))] {
        let bytes = file(codestream, &colr, &[]);
        assert_eq!(
            decode_srgb(&bytes).expect("sRGB").components,
            decode(&bytes).expect("as coded").components
        );
    }
}

/// An opacity channel is left alone; the colour channels are converted.
#[test]
fn a_non_colour_channel_is_untouched() {
    // cdef: channels 0, 1 are colours 1, 2 ... make channel 2 opacity and
    // add no third colour: an RGB profile then has too few colour channels.
    let cdef = bx(
        b"cdef",
        &[0, 3, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 2, 0, 2, 0, 1, 0, 0],
    );
    expect_err(
        decode_srgb(&file(THREE, &icc_colr(&wide_gamut()), &[cdef])),
        is_unsupported,
        "needs colour channels 1, 2 and 3; the file has 2",
    );
    // Grey plus opacity through a grey profile: the opacity is kept as coded.
    let boxes = palette(&[7, 7], 256, |j, i| {
        if i == 0 {
            u32::from(j)
        } else {
            255 - u32::from(j)
        }
    });
    let cdef = bx(b"cdef", &[0, 2, 0, 0, 0, 0, 0, 1, 0, 1, 0, 1, 0, 0]);
    let bytes = file(
        ONE,
        &icc_colr(&grey_profile()),
        &[boxes[0].clone(), boxes[1].clone(), cdef],
    );
    let coded = decode(&bytes).expect("as coded");
    let image = decode_srgb(&bytes).expect("sRGB");
    assert_eq!(
        image.components[1], coded.components[1],
        "opacity untouched"
    );
    assert_ne!(image.components[0], coded.components[0], "grey converted");
}

/// Every input the sRGB step refuses, and the error it gets. `AsCoded`
/// decodes each of them (checked per row).
#[test]
fn srgb_reject_matrix_maps_each_input_to_its_typed_error() {
    let rgb_profile = |edit: &dyn Fn(&mut Vec<u8>)| {
        let mut icc = wide_gamut();
        edit(&mut icc);
        file(THREE, &icc_colr(&icc), &[])
    };
    let with_tags = |tags: Vec<(&[u8; 4], Vec<u8>)>| {
        file(
            THREE,
            &icc_colr(&profile(b"scnr", b"RGB ", b"XYZ ", 0, &tags)),
            &[],
        )
    };
    let replace_tag = |name: &[u8; 4], data: Vec<u8>| {
        let tags = wide_gamut_tags()
            .into_iter()
            .map(|(n, d)| if n == name { (n, data.clone()) } else { (n, d) })
            .collect();
        with_tags(tags)
    };
    let sycc_jp2c = top_box(SYCC_420, b"jp2c");
    let mut lut_tags = wide_gamut_tags();
    lut_tags.push((b"A2B0", vec![0; 32]));
    let missing_rtrc: Vec<_> = wide_gamut_tags()
        .into_iter()
        .filter(|(n, _)| *n != b"rTRC")
        .collect();

    type Check = fn(&Error) -> bool;
    let rows: Vec<(&str, Vec<u8>, Check, &str)> = vec![
        (
            "enumerated e-sRGB",
            file(THREE, &enumerated(20), &[]),
            is_unsupported,
            "enumerated colour space 20",
        ),
        (
            "no colr",
            file(THREE, &[], &[]),
            is_unsupported,
            "no colour specification",
        ),
        (
            "profile shorter than its header",
            file(THREE, &icc_colr(&[0; 100]), &[]),
            is_file_format,
            "100 bytes, shorter than its 128-byte header",
        ),
        (
            "display class",
            rgb_profile(&|p| p[12..16].copy_from_slice(b"mntr")),
            is_unsupported,
            "class `mntr`",
        ),
        (
            "CMYK space",
            rgb_profile(&|p| p[16..20].copy_from_slice(b"CMYK")),
            is_unsupported,
            "`CMYK` colour space",
        ),
        (
            "Lab connection space",
            rgb_profile(&|p| p[20..24].copy_from_slice(b"Lab ")),
            is_unsupported,
            "`Lab ` connection space",
        ),
        (
            "absolute colorimetric",
            rgb_profile(&|p| p[64..68].copy_from_slice(&3u32.to_be_bytes())),
            is_unsupported,
            "rendering intent 3",
        ),
        (
            "tag table past the end",
            rgb_profile(&|p| p[128..132].copy_from_slice(&1000u32.to_be_bytes())),
            is_file_format,
            "declares 1000 tags, past its end",
        ),
        (
            "LUT-based",
            with_tags(lut_tags),
            is_unsupported,
            "`A2B0` lookup table",
        ),
        (
            "missing rTRC",
            with_tags(missing_rtrc),
            is_unsupported,
            "without a `rTRC` tag",
        ),
        (
            "tag past the end",
            rgb_profile(&|p| {
                let at = 132 + 4;
                p[at..at + 4].copy_from_slice(&5000u32.to_be_bytes())
            }),
            is_file_format,
            "`rXYZ` tag (20 bytes at 5000) runs past",
        ),
        (
            "XYZ tag of another type",
            replace_tag(b"gXYZ", curv(&[1])),
            is_file_format,
            "`gXYZ` tag is not an `XYZ ` value",
        ),
        (
            "curve of another type",
            replace_tag(b"bTRC", xyz([0, 0, 0])),
            is_unsupported,
            "`bTRC` curve of type `XYZ `",
        ),
        (
            "para function 5",
            replace_tag(b"rTRC", para(5, &[1.0])),
            is_unsupported,
            "parametric function type 5",
        ),
        (
            "para cut short",
            replace_tag(b"rTRC", para(4, &[1.0, 1.0])),
            is_file_format,
            "`rTRC` curve is cut short",
        ),
        (
            "curv cut short",
            replace_tag(b"gTRC", {
                let mut c = curv(&[0, 1, 2]);
                c.truncate(14);
                c
            }),
            is_file_format,
            "`gTRC` curve is cut short",
        ),
        (
            "grey profile, three colours",
            file(THREE, &icc_colr(&grey_profile()), &[]),
            is_unsupported,
            "monochrome ICC profile needs one colour channel; the file has 3",
        ),
        (
            "RGB profile, one colour",
            file(ONE, &icc_colr(&wide_gamut()), &[]),
            is_unsupported,
            "needs colour channels 1, 2 and 3; the file has 1",
        ),
        (
            "RGB profile on 12-bit samples",
            file(
                ONE,
                &icc_colr(&wide_gamut()),
                &palette(&[11, 11, 11], 4, |j, _| u32::from(j)),
            ),
            is_unsupported,
            "RGB ICC profile from 12-bit unsigned",
        ),
        (
            "grey profile on 12-bit samples",
            file(
                ONE,
                &icc_colr(&grey_profile()),
                &palette(&[11], 4, |j, _| u32::from(j)),
            ),
            is_unsupported,
            "monochrome ICC profile from 12-bit unsigned",
        ),
        (
            "palette columns of mixed depth (sRGB)",
            file(
                ONE,
                &enumerated(16),
                &palette(&[7, 11, 7], 4, |j, _| u32::from(j)),
            ),
            is_unsupported,
            "different formats (8-bit unsigned and 12-bit unsigned)",
        ),
        (
            "palette columns of mixed sign (sYCC)",
            file(
                ONE,
                &enumerated(18),
                &palette(&[7, 0x87, 7], 4, |j, _| u32::from(j)),
            ),
            is_unsupported,
            "different formats (8-bit unsigned and 8-bit signed)",
        ),
        (
            "signed sYCC",
            file(
                ONE,
                &enumerated(18),
                &palette(&[0x87, 0x87, 0x87], 4, |j, _| u32::from(j)),
            ),
            is_unsupported,
            "signed sYCC",
        ),
        (
            "sRGB-declared 4:2:0",
            file(sycc_jp2c, &enumerated(16), &[]),
            is_unsupported,
            "different sub-sampling (1x1 and 2x2)",
        ),
        (
            "RGB profile on 4:2:0",
            file(sycc_jp2c, &icc_colr(&wide_gamut()), &[]),
            is_unsupported,
            "different sub-sampling (1x1 and 2x2)",
        ),
        (
            "sYCC with sub-sampled luma",
            file(ALL_SUBSAMPLED, &enumerated(18), &[]),
            is_unsupported,
            "sYCC sampled 2x2, 2x2, 2x2",
        ),
        (
            "sYCC with two colour channels",
            file(
                THREE,
                &enumerated(18),
                &[bx(
                    b"cdef",
                    &[0, 3, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 2, 0, 2, 0, 1, 0, 0],
                )],
            ),
            is_unsupported,
            "sYCC needs colour channels 1, 2 and 3",
        ),
    ];
    for (name, bytes, want, fragment) in rows {
        decode(&bytes).unwrap_or_else(|e| panic!("{name}: AsCoded must decode, got {e}"));
        match decode_srgb(&bytes) {
            Err(e) if want(&e) => assert!(
                e.to_string().contains(fragment),
                "{name}: expected {fragment:?}, got {e}"
            ),
            other => panic!("{name}: expected an error mentioning {fragment:?}, got {other:?}"),
        }
    }
}

/// `raw_channels` means no colour conversion: asking for sRGB too is refused
/// before anything is read, for a JP2 file and a raw codestream alike.
#[test]
fn raw_channels_with_srgb_output_is_invalid() {
    let options = DecodeOptions {
        raw_channels: true,
        colour: ColourOutput::Srgb,
        ..DecodeOptions::default()
    };
    for bytes in [
        file(THREE, &enumerated(16), &[]),
        THREE.to_vec(),
        b"junk".to_vec(),
    ] {
        expect_err(
            decode_with(&bytes, options),
            |e| matches!(e, Error::InvalidOptions(_)),
            "raw channels means no colour conversion",
        );
    }
    // Each option alone is fine.
    let file = file(THREE, &enumerated(16), &[]);
    decode_with(&file, srgb()).expect("sRGB alone");
    decode_with(
        &file,
        DecodeOptions {
            raw_channels: true,
            ..DecodeOptions::default()
        },
    )
    .expect("raw channels alone");
}

/// A raw codestream declares no colour space, so there is nothing to convert
/// from; a malformed one still reports its own fault.
#[test]
fn srgb_output_from_a_raw_codestream_is_unsupported() {
    expect_err(decode_srgb(THREE), is_unsupported, "raw codestream");
    assert!(matches!(decode_srgb(b"junk"), Err(Error::Codestream(_))));
}

/// sYCC chroma replication that would grow the output past the decode guard
/// is refused before any tile is read. The 4:2:0 fixture's SIZ is widened to
/// 8192x4096: as coded that is 1.5 x 2^25 samples, under the 2^26 guard, but
/// sRGB output is 3 x 2^25.
#[test]
fn chroma_replication_past_the_sample_budget_is_a_limit() {
    let mut codestream = top_box(SYCC_420, b"jp2c").to_vec();
    // SIZ: FF51 Lsiz Rsiz Xsiz Ysiz XOsiz YOsiz XTsiz YTsiz ...
    assert_eq!(&codestream[2..4], &[0xFF, 0x51]);
    let put =
        |cs: &mut Vec<u8>, at: usize, v: u32| cs[at..at + 4].copy_from_slice(&v.to_be_bytes());
    put(&mut codestream, 8, 5 + 8192); // Xsiz
    put(&mut codestream, 12, 3 + 4096); // Ysiz
    put(&mut codestream, 24, 5 + 8192); // XTsiz
    put(&mut codestream, 28, 3 + 4096); // YTsiz
    let bytes = file(&codestream, &enumerated(18), &[]);
    expect_err(
        decode_srgb(&bytes),
        |e| matches!(e, Error::Limit(_)),
        "chroma replication grows the output",
    );
    // As coded, the same file is under the guard and fails only later, on
    // its (now too short) tile data.
    assert!(!matches!(decode(&bytes), Err(Error::Limit(_)) | Ok(_)));
}
