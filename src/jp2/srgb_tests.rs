//! sRGB output tests: the conversions on hand-computed vectors, the chroma
//! replication against OpenJPEG, and every input the sRGB step refuses, each
//! through the public [`decode_with`].
//!
//! The ICC vectors were computed independently in double precision (numpy)
//! and agree with LittleCMS 2 (Pillow's `ImageCms`, an sRGB output profile)
//! on every value used here.

use super::SIGNATURE;
use super::srgb::{ChromaLayout, replicate, sycc_sample};
use crate::{
    ColourOutput, ColourSpace, DecodeOptions, Error, Image, decode, decode_with, read_header,
    read_header_with,
};

/// Three 8-bit components at full resolution, 24x16.
const THREE: &[u8] = include_bytes!("../../tests/fixtures/multicomponent_lossless.j2k");
/// One 8-bit component, 32x24.
const ONE: &[u8] = include_bytes!("../../tests/fixtures/multilayer_lossless.j2k");
/// Two components.
const TWO: &[u8] = include_bytes!("../../tests/fixtures/cblksty_bypass_lossless.j2k");
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
    decode_with(bytes, &srgb())
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

/// A grey ICC profile on more than two channels is refused, whichever channel
/// is the colour one: OpenJPEG would feed the planes to a grey transform as
/// if they were RGB. The refusal is the same from `read_header_with`. One
/// colour channel, or one plus opacity, still converts.
#[test]
fn a_grey_profile_on_more_than_two_channels_is_unsupported() {
    // cdef: channel 0 is colour 1, channels 1 and 2 are opacity.
    let cdef = bx(
        b"cdef",
        &[0, 3, 0, 0, 0, 0, 0, 1, 0, 1, 0, 1, 0, 0, 0, 2, 0, 1, 0, 0],
    );
    let from_palette = palette(&[7, 7, 7], 256, |j, _| u32::from(j));
    let three_columns = [
        from_palette[0].clone(),
        from_palette[1].clone(),
        cdef.clone(),
    ];
    let two_columns = {
        let boxes = palette(&[7, 7], 256, |j, _| u32::from(j));
        let cdef = bx(b"cdef", &[0, 2, 0, 0, 0, 0, 0, 1, 0, 1, 0, 1, 0, 0]);
        [boxes[0].clone(), boxes[1].clone(), cdef]
    };
    // The colour channel last, after two opacity channels.
    let colour_last = bx(
        b"cdef",
        &[0, 3, 0, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 2, 0, 0, 0, 1],
    );
    let refused = [
        file(THREE, &icc_colr(&grey_profile()), &[colour_last]),
        file(THREE, &icc_colr(&grey_profile()), &[cdef]),
        file(ONE, &icc_colr(&grey_profile()), &three_columns),
    ];
    for bytes in &refused {
        expect_err(
            decode_srgb(bytes),
            is_unsupported,
            "monochrome ICC profile on 3 channels",
        );
        match read_header_with(bytes, &srgb()) {
            Err(Error::Unsupported(what)) => {
                assert!(
                    what.contains("monochrome ICC profile on 3 channels"),
                    "{what}"
                )
            }
            other => panic!("read_header_with: expected Unsupported, got {other:?}"),
        }
    }
    // The same shapes at one and two channels convert.
    for (bytes, channels) in [
        (file(ONE, &icc_colr(&grey_profile()), &[]), 1),
        (file(ONE, &icc_colr(&grey_profile()), &two_columns), 2),
    ] {
        let image = decode_srgb(&bytes).expect("sRGB");
        let header = read_header_with(&bytes, &srgb()).expect("header");
        assert_eq!(image.components.len(), channels);
        assert_eq!(header.components.len(), channels);
    }
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
    let sycc_jp2c = top_box(SYCC_420, b"jp2c");
    let sycc_422_jp2c = top_box(SYCC_422, b"jp2c");
    let mut lut_tags = wide_gamut_tags();
    lut_tags.push((b"A2B0", lut16(3, 3, 2, [2, 2])));

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
            "YCbCr space",
            rgb_profile(&|p| p[16..20].copy_from_slice(b"YCbr")),
            is_unsupported,
            "`YCbr` colour space",
        ),
        (
            "Lab connection space",
            rgb_profile(&|p| p[20..24].copy_from_slice(b"Lab ")),
            is_unsupported,
            "`Lab ` connection space",
        ),
        (
            "LUT-based",
            with_tags(lut_tags),
            is_unsupported,
            "`A2B0` lookup table",
        ),
        (
            "grey profile, three colours",
            file(THREE, &icc_colr(&grey_profile()), &[]),
            is_unsupported,
            "monochrome ICC profile on 3 channels",
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
            "4:2:0 or 4:2:2 file, which OpenJPEG converts as sYCC",
        ),
        (
            // OpenJPEG converts 4:2:0 and 4:2:2 as sYCC whatever colour
            // space the file declares, so greyscale here converts there.
            "greyscale-declared 4:2:0 on three channels",
            file(sycc_jp2c, &enumerated(17), &[]),
            is_unsupported,
            "4:2:0 or 4:2:2 file, which OpenJPEG converts as sYCC",
        ),
        (
            "RGB profile on 4:2:0",
            file(sycc_jp2c, &icc_colr(&wide_gamut()), &[]),
            is_unsupported,
            "different sub-sampling (1x1 and 2x2)",
        ),
        (
            "RGB profile on 4:2:2",
            file(sycc_422_jp2c, &icc_colr(&wide_gamut()), &[]),
            is_unsupported,
            "different sub-sampling (1x1 and 2x1)",
        ),
        (
            "greyscale-declared 4:2:2 on three channels",
            file(sycc_422_jp2c, &enumerated(17), &[]),
            is_unsupported,
            "4:2:0 or 4:2:2 file, which OpenJPEG converts as sYCC",
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

/// `opj_decompress` guesses sYCC and converts a 4:2:0 or 4:2:2 image before it
/// reads the ICC profile, so it converts one whatever the profile is, even one
/// LittleCMS refuses (the output equals the sYCC file's). Checked on the
/// fixtures' codestreams with `lcms_intent_4` (an intent LittleCMS does not
/// know), a refused grey profile, a header too short to read and a CMYK
/// profile: all five give the planes of the sYCC original. Here every one is
/// refused, by `decode_with` and by `read_header_with`.
#[test]
fn an_lcms_refused_profile_on_4_2_0_or_4_2_2_is_refused() {
    let intent_4 = |mut icc: Vec<u8>| {
        icc[64..68].copy_from_slice(&4u32.to_be_bytes());
        icc
    };
    let cmyk = {
        let mut icc = wide_gamut();
        icc[16..20].copy_from_slice(b"CMYK");
        icc
    };
    let profiles = [
        ("intent 4", intent_4(wide_gamut())),
        ("grey intent 4", intent_4(grey_profile())),
        ("short header", vec![0; 10]),
        ("CMYK", cmyk),
    ];
    for (layout, jp2, fragment) in [
        ("4:2:0", SYCC_420, "different sub-sampling (1x1 and 2x2)"),
        ("4:2:2", SYCC_422, "different sub-sampling (1x1 and 2x1)"),
    ] {
        let codestream = top_box(jp2, b"jp2c");
        for (name, icc) in &profiles {
            let what = format!("{layout}, {name}");
            let bytes = file(codestream, &icc_colr(icc), &[]);
            decode(&bytes).unwrap_or_else(|e| panic!("{what}: AsCoded must decode, got {e}"));
            expect_err(decode_srgb(&bytes), is_unsupported, fragment);
            match read_header_with(&bytes, &srgb()) {
                Err(Error::Unsupported(e)) => assert!(e.contains(fragment), "{what}: {e}"),
                other => panic!("{what}: read_header_with gave {other:?}"),
            }
        }
    }
}

/// Outside 4:2:0 and 4:2:2 `opj_decompress` does not convert as sYCC, and a
/// profile LittleCMS refuses leaves the samples as coded, so they pass through
/// here too.
#[test]
fn an_lcms_refused_profile_passes_through_outside_the_sycc_layouts() {
    let mut icc = wide_gamut();
    icc[64..68].copy_from_slice(&4u32.to_be_bytes());
    for (name, codestream) in [("full", THREE), ("all 2x2", ALL_SUBSAMPLED)] {
        let bytes = file(codestream, &icc_colr(&icc), &[]);
        let image = decode_srgb(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            image.components,
            decode(&bytes).unwrap().components,
            "{name}"
        );
        let header = read_header_with(&bytes, &srgb()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(header.components.len(), 3, "{name}");
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
            decode_with(&bytes, &options),
            |e| matches!(e, Error::InvalidOptions(_)),
            "raw channels means no colour conversion",
        );
    }
    // Each option alone is fine.
    let file = file(THREE, &enumerated(16), &[]);
    decode_with(&file, &srgb()).expect("sRGB alone");
    decode_with(
        &file,
        &DecodeOptions {
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
    expect_err(
        decode_srgb(THREE),
        is_unsupported,
        "raw codestream of 3 components",
    );
    assert!(matches!(decode_srgb(b"junk"), Err(Error::Codestream(_))));
}

/// A file declared grey on three components is refused only where
/// `opj_decompress` would convert it as sYCC, with or without a `cdef`: luma
/// at 1x1 and both chroma channels at 2x2 or both at 2x1 (the 4:2:0 and
/// 4:2:2 rows of the reject matrix). Otherwise the extra channels are past
/// grey's one colour and pass through as coded, as OpenJPEG writes them: a
/// signed third channel, a third that is the only one sub-sampled
/// (`tests/fixtures/channels/third_subsampled.j2k`), luma sampled 1x2 with
/// chroma 2x1 (`luma_1x2.j2k`), and the layouts OpenJPEG guesses sYCC for
/// but cannot convert (`second_2x1.j2k` and the others below, checked with
/// `opj_decompress`).
#[test]
fn grey_on_three_channels_is_refused_only_where_openjpeg_converts_sycc() {
    let mut signed_third = THREE.to_vec();
    signed_third[42 + 3 * 2] = 0x87;
    let sub_third = include_bytes!("../../tests/fixtures/channels/third_subsampled.j2k");
    let sycc_jp2c = top_box(SYCC_420, b"jp2c");
    let luma_1x2 = include_bytes!("../../tests/fixtures/channels/luma_1x2.j2k");
    let cdef = bx(
        b"cdef",
        &[0, 3, 0, 0, 0, 0, 0, 1, 0, 1, 0, 1, 0, 0, 0, 2, 0, 1, 0, 0],
    );
    let codestreams: [(&str, &[u8]); 9] = [
        ("third signed", &signed_third),
        ("third sub-sampled", sub_third),
        ("luma 1x2", luma_1x2),
        (
            "1x1:2x1:1x1",
            include_bytes!("../../tests/fixtures/channels/second_2x1.j2k"),
        ),
        (
            "1x1:2x2:1x1",
            include_bytes!("../../tests/fixtures/channels/second_2x2.j2k"),
        ),
        (
            "1x1:2x1:2x2",
            include_bytes!("../../tests/fixtures/channels/chroma_2x1_2x2.j2k"),
        ),
        (
            "2x2:2x1:2x1",
            include_bytes!("../../tests/fixtures/channels/luma_2x2_chroma_2x1.j2k"),
        ),
        (
            "1x1:1x2:1x2",
            include_bytes!("../../tests/fixtures/channels/chroma_1x2.j2k"),
        ),
        (
            "2x2:2x2:2x2",
            include_bytes!("../../tests/fixtures/channels/all_2x2.j2k"),
        ),
    ];
    for (what, codestream) in codestreams {
        let bytes = file(codestream, &enumerated(17), &[]);
        let coded = decode(&bytes).unwrap_or_else(|e| panic!("{what}: as coded: {e}"));
        let image = decode_srgb(&bytes).unwrap_or_else(|e| panic!("{what}: sRGB: {e}"));
        assert_eq!(image.components, coded.components, "{what}");
        let header = read_header_with(&bytes, &srgb()).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(header.components.len(), 3, "{what}");
    }
    // The sYCC guess does not depend on a `cdef`: the 4:2:0 file is refused
    // with one that labels only colour 1, as without.
    expect_err(
        decode_srgb(&file(sycc_jp2c, &enumerated(17), &[cdef])),
        is_unsupported,
        "4:2:0 or 4:2:2 file, which OpenJPEG converts as sYCC",
    );
}

/// A `cdef` box of these (channel, type, association) entries.
fn cdef(entries: &[(u16, u16, u16)]) -> Vec<u8> {
    let mut payload = (entries.len() as u16).to_be_bytes().to_vec();
    for &(channel, kind, association) in entries {
        for field in [channel, kind, association] {
            payload.extend_from_slice(&field.to_be_bytes());
        }
    }
    bx(b"cdef", &payload)
}

/// The sYCC guess holds for a file declared sRGB too: a 4:2:0 or 4:2:2 file
/// is refused whatever its `cdef` labels, as without one: only colour 1 (the
/// others opacity, or unspecified), no colour at all, or colours 2 and 3
/// only. `opj_decompress` 2.5.0, with and without `-force-rgb`, writes three
/// full-size planes for each, byte-identical to the file with no `cdef`.
#[test]
fn srgb_4_2_0_or_4_2_2_is_refused_whatever_cdef_labels() {
    const NONE: u16 = 0xFFFF;
    let cdefs = [
        ("colour 1, opacity", cdef(&[(0, 0, 1), (1, 1, 0), (2, 1, 0)])),
        (
            "colour 1, unspecified",
            cdef(&[(0, 0, 1), (1, NONE, NONE), (2, NONE, NONE)]),
        ),
        ("no colour", cdef(&[(0, 1, 0), (1, 1, 0), (2, 1, 0)])),
        ("colours 2 and 3", cdef(&[(0, 1, 0), (1, 0, 2), (2, 0, 3)])),
    ];
    let fragment = "4:2:0 or 4:2:2 file, which OpenJPEG converts as sYCC";
    for (layout, jp2) in [("4:2:0", SYCC_420), ("4:2:2", SYCC_422)] {
        let codestream = top_box(jp2, b"jp2c");
        for (name, cdef) in &cdefs {
            let what = format!("{layout}, {name}");
            let bytes = file(codestream, &enumerated(16), std::slice::from_ref(cdef));
            decode(&bytes).unwrap_or_else(|e| panic!("{what}: AsCoded must decode, got {e}"));
            expect_err(decode_srgb(&bytes), is_unsupported, fragment);
            match read_header_with(&bytes, &srgb()) {
                Err(Error::Unsupported(e)) => assert!(e.contains(fragment), "{what}: {e}"),
                other => panic!("{what}: read_header_with gave {other:?}"),
            }
        }
    }
}

/// A file declared sRGB or greyscale on one or two channels passes through
/// as coded, as for every other colour space: OpenJPEG reads any image of
/// two or fewer components as grey (and grey plus opacity). Two channels of
/// different sign or depth (here from a palette) are not refused, even when
/// a `cdef` labels both as colours (1 and 2) under greyscale;
/// `opj_decompress` writes them as coded (8-bit unsigned and 8-bit signed,
/// 8-bit and 12-bit). The metadata stays as declared.
#[test]
fn srgb_on_one_or_two_channels_passes_through() {
    let two = |space: u32, bi: &[u8], extra: &[Vec<u8>]| {
        let mut boxes = palette(bi, 4, |j, _| u32::from(j));
        boxes.extend_from_slice(extra);
        file(ONE, &enumerated(space), &boxes)
    };
    let colours_1_2 = cdef(&[(0, 0, 1), (1, 0, 2)]);
    let rows = [
        ("one channel", ColourSpace::Srgb, file(ONE, &enumerated(16), &[])),
        (
            "two channels, different sign",
            ColourSpace::Srgb,
            two(16, &[7, 0x87], &[]),
        ),
        (
            "two channels, different depth",
            ColourSpace::Srgb,
            two(16, &[7, 11], &[]),
        ),
        (
            "grey, colours 1 and 2, different sign",
            ColourSpace::Greyscale,
            two(17, &[7, 0x87], std::slice::from_ref(&colours_1_2)),
        ),
        (
            "grey, colours 1 and 2, different depth",
            ColourSpace::Greyscale,
            two(17, &[7, 11], std::slice::from_ref(&colours_1_2)),
        ),
    ];
    for (what, declared, bytes) in rows {
        let coded = decode(&bytes).unwrap_or_else(|e| panic!("{what}: as coded: {e}"));
        let image = decode_srgb(&bytes).unwrap_or_else(|e| panic!("{what}: sRGB: {e}"));
        assert_eq!(image.components, coded.components, "{what}");
        assert_eq!(image.jp2.unwrap().colour, declared, "{what}");
        let header = read_header_with(&bytes, &srgb()).unwrap_or_else(|e| panic!("{what}: {e}"));
        let formats: Vec<_> = header
            .components
            .iter()
            .map(|c| (c.bit_depth, c.signed, c.width, c.height))
            .collect();
        let decoded: Vec<_> = image
            .components
            .iter()
            .map(|c| (c.bit_depth, c.signed, c.width, c.height))
            .collect();
        assert_eq!(formats, decoded, "{what}");
    }
}

/// One or two components in a raw codestream are grey (and grey plus
/// opacity), as OpenJPEG reads them, and pass through.
#[test]
fn a_raw_grey_codestream_passes_through() {
    for codestream in [ONE, TWO] {
        let coded = decode(codestream).expect("as coded");
        let image = decode_srgb(codestream).expect("sRGB");
        assert_eq!(image.components, coded.components);
        assert!(image.jp2.is_none());
    }
}

/// A JP2 file of one or two channels with no `colr`, or an enumerated
/// colour space this step does not convert, passes through as grey (and
/// grey plus opacity), as OpenJPEG reads it, with the metadata as declared.
/// Three channels (here from a palette) are still refused.
#[test]
fn a_grey_jp2_without_a_converted_colour_space_passes_through() {
    for (codestream, colr, declared) in [
        (ONE, vec![], ColourSpace::Unknown),
        (TWO, vec![], ColourSpace::Unknown),
        (ONE, enumerated(12), ColourSpace::Enumerated(12)),
        (TWO, enumerated(20), ColourSpace::Enumerated(20)),
    ] {
        let bytes = file(codestream, &colr, &[]);
        let coded = decode(&bytes).expect("as coded");
        let image = decode_srgb(&bytes).expect("sRGB");
        assert_eq!(image.components, coded.components);
        assert_eq!(image.jp2.unwrap().colour, declared);
    }
    let boxes = palette(&[7, 7, 7], 4, |j, _| u32::from(j));
    expect_err(
        decode_srgb(&file(ONE, &[], &boxes)),
        is_unsupported,
        "no colour specification",
    );
}

/// D65 as an `s15Fixed16` white point.
const D65: [i32; 3] = [62290, 65536, 71369];

/// A three-component file with an RGB profile of intent 3 and these tags.
fn intent_3(tags: Vec<(&[u8; 4], Vec<u8>)>) -> Vec<u8> {
    file(
        THREE,
        &icc_colr(&profile(b"scnr", b"RGB ", b"XYZ ", 3, &tags)),
        &[],
    )
}

/// Absolute colorimetric with a D50 (or no) media white point scales by 1,
/// so it converts exactly as intent 0, as OpenJPEG + lcms give it
/// byte-identically.
#[test]
fn absolute_colorimetric_with_a_d50_white_point_converts_as_relative() {
    let mut with_d50 = wide_gamut_tags();
    with_d50.push((b"wtpt", xyz([63190, 65536, 54061])));
    let intent_0 = decode_srgb(&file(
        THREE,
        &icc_colr(&profile(b"scnr", b"RGB ", b"XYZ ", 0, &with_d50)),
        &[],
    ))
    .expect("intent 0");
    assert_ne!(intent_0.components, decode(THREE).unwrap().components);
    for tags in [with_d50, wide_gamut_tags()] {
        let image = decode_srgb(&intent_3(tags)).expect("intent 3");
        assert_eq!(image.components, intent_0.components);
    }
}

/// Absolute colorimetric with another media white point scales XYZ by it
/// over D50, so it no longer matches intent 0 and stays in range.
#[test]
fn absolute_colorimetric_with_a_d65_white_point_scales_by_it() {
    let mut with_d65 = wide_gamut_tags();
    with_d65.push((b"wtpt", xyz(D65)));
    let intent_0 = with_d65.clone();
    let relative = decode_srgb(&file(
        THREE,
        &icc_colr(&profile(b"scnr", b"RGB ", b"XYZ ", 0, &intent_0)),
        &[],
    ))
    .expect("intent 0");
    let absolute = decode_srgb(&intent_3(with_d65)).expect("intent 3");
    assert_ne!(absolute.components, relative.components);
    for c in &absolute.components {
        assert!(c.samples.iter().all(|&s| (0..=255).contains(&s)));
    }
}

/// Once converted, the metadata says what the samples are: sRGB with no
/// profile, or grey for a monochrome profile. `read_header` and the default
/// decode report what the file declares; a pass-through keeps it too.
#[test]
fn converted_output_reports_its_colour_space() {
    let rgb = file(THREE, &icc_colr(&wide_gamut()), &[]);
    let boxes = palette(&[7], 256, |j, _| u32::from(j));
    let grey = file(ONE, &icc_colr(&grey_profile()), &boxes);
    let sycc = SYCC_420.to_vec();
    for (bytes, want) in [
        (&rgb, ColourSpace::Srgb),
        (&grey, ColourSpace::Greyscale),
        (&sycc, ColourSpace::Srgb),
    ] {
        let declared = read_header(bytes).unwrap().jp2.unwrap().colour;
        assert_ne!(declared, want);
        assert_eq!(decode(bytes).unwrap().jp2.unwrap().colour, declared);
        assert_eq!(decode_srgb(bytes).unwrap().jp2.unwrap().colour, want);
    }
    for (bytes, want) in [
        (file(THREE, &enumerated(16), &[]), ColourSpace::Srgb),
        (file(ONE, &enumerated(17), &[]), ColourSpace::Greyscale),
    ] {
        assert_eq!(decode_srgb(&bytes).unwrap().jp2.unwrap().colour, want);
    }
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

// ---------------------------------------------------------------------------
// Profiles LittleCMS refuses
// ---------------------------------------------------------------------------
//
// OpenJPEG skips the ICC step when LittleCMS will not open the profile or
// build a transform from it, and writes the samples as coded. Each case here
// was run through `opj_decompress` 2.5.0 (LittleCMS 2.14) on the same file:
// the pass-through cases came out byte-identical to the file with its `colr`
// set to enumerated sRGB (or greyscale), and the converted ones matched this
// decoder's output sample for sample.

/// `Srgb` leaves the samples as coded: the channels, samples and metadata of
/// the default decode, with the profile still in `Jp2Metadata::colour`, and
/// `read_header_with` describes exactly that.
#[track_caller]
fn assert_passes_through(bytes: &[u8]) {
    let coded = decode(bytes).expect("as coded");
    let image = decode_srgb(bytes).expect("sRGB passes through");
    assert_eq!(image.components, coded.components, "samples as coded");
    assert_eq!(image.jp2, coded.jp2, "metadata as declared");
    assert!(
        matches!(
            image.jp2.as_ref().unwrap().colour,
            ColourSpace::RestrictedIcc(_)
        ),
        "the profile stays in the metadata"
    );
    let header = read_header_with(bytes, &srgb()).expect("header");
    assert_eq!(header, read_header(bytes).expect("header"));
    assert_eq!(header.jp2, image.jp2, "the header describes the decode");
    assert_eq!(header.components.len(), image.components.len());
    for (info, c) in header.components.iter().zip(&image.components) {
        assert_eq!(
            (info.bit_depth, info.signed, info.width, info.height),
            (c.bit_depth, c.signed, c.width, c.height)
        );
    }
}

/// `Srgb` converts `bytes` to the same samples and metadata as `like`, and
/// `read_header_with` describes the result.
#[track_caller]
fn assert_converts_like(bytes: &[u8], like: &[u8]) {
    let image = decode_srgb(bytes).expect("sRGB");
    let want = decode_srgb(like).expect("sRGB");
    assert_ne!(
        image.components,
        decode(bytes).expect("as coded").components,
        "converted"
    );
    assert_eq!(image.components, want.components);
    assert_eq!(image.jp2, want.jp2);
    let header = read_header_with(bytes, &srgb()).expect("header");
    assert_eq!(header.jp2, image.jp2, "the header describes the decode");
    assert_eq!(header.components.len(), image.components.len());
}

/// The wide-gamut RGB profile on three channels, edited by `edit`.
fn edited(edit: impl Fn(&mut Vec<u8>)) -> Vec<u8> {
    let mut icc = wide_gamut();
    edit(&mut icc);
    file(THREE, &icc_colr(&icc), &[])
}

fn put_u32(at: usize, value: u32) -> impl Fn(&mut Vec<u8>) {
    move |p: &mut Vec<u8>| p[at..at + 4].copy_from_slice(&value.to_be_bytes())
}

/// An RGB profile with these tags on three channels.
fn rgb_with(tags: &[(&[u8; 4], Vec<u8>)], intent: u32) -> Vec<u8> {
    file(
        THREE,
        &icc_colr(&profile(b"scnr", b"RGB ", b"XYZ ", intent, tags)),
        &[],
    )
}

/// The wide-gamut tags with `name` replaced by `data`.
fn replaced(name: &[u8; 4], data: Vec<u8>) -> Vec<(&'static [u8; 4], Vec<u8>)> {
    wide_gamut_tags()
        .into_iter()
        .map(|(n, d)| if n == name { (n, data.clone()) } else { (n, d) })
        .collect()
}

/// A monochrome profile with these tags on one channel.
fn grey_with(tags: &[(&[u8; 4], Vec<u8>)], intent: u32) -> Vec<u8> {
    file(
        ONE,
        &icc_colr(&profile(b"scnr", b"GRAY", b"XYZ ", intent, tags)),
        &[],
    )
}

/// `data` with its type signature replaced.
fn retyped(mut data: Vec<u8>, kind: [u8; 4]) -> Vec<u8> {
    data[..4].copy_from_slice(&kind);
    data
}

const MONACO_CURVE: [u8; 4] = [0x94, 0x78, 0xEE, 0x00];
const CORBIS_XYZ: [u8; 4] = [0x17, 0xA5, 0x05, 0xB8];

#[test]
fn a_profile_shorter_than_its_header_passes_through() {
    assert_passes_through(&file(THREE, &icc_colr(&[0; 100]), &[]));
    assert_passes_through(&file(ONE, &icc_colr(&[0; 100]), &[]));
    // The 128-byte header, but no tag count.
    assert_passes_through(&file(THREE, &icc_colr(&wide_gamut()[..130]), &[]));
}

#[test]
fn a_profile_without_the_acsp_signature_passes_through() {
    assert_passes_through(&edited(|p| p[36..40].copy_from_slice(b"acsP")));
}

#[test]
fn a_profile_version_above_5_0_passes_through() {
    assert_passes_through(&edited(put_u32(8, 0x0510_0000)));
    // 5.0 itself converts.
    assert_converts_like(&edited(put_u32(8, 0x0500_0000)), &edited(|_| {}));
}

#[test]
fn a_profile_of_more_than_100_tags_passes_through() {
    let names: Vec<[u8; 4]> = (0..95u8)
        .map(|i| [b'z', b'0' + i / 10, b'0' + i % 10, b' '])
        .collect();
    let mut tags = wide_gamut_tags();
    for name in &names {
        tags.push((name, vec![0; 4]));
    }
    // 100 tags convert; 101 pass through.
    assert_converts_like(&rgb_with(&tags[..100], 0), &edited(|_| {}));
    assert_passes_through(&rgb_with(&tags, 0));
    // A count above 100 is refused before the table is read.
    assert_passes_through(&edited(put_u32(128, 101)));
}

#[test]
fn a_tag_table_past_the_profile_end_passes_through() {
    assert_passes_through(&edited(put_u32(128, 99)));
}

#[test]
fn a_duplicate_tag_passes_through() {
    let mut tags = wide_gamut_tags();
    tags.push((b"rTRC", curv(&[300])));
    assert_passes_through(&rgb_with(&tags, 0));
}

/// LittleCMS skips an entry with a zero offset or size, or one that ends
/// past the declared size, so the tag is then missing.
#[test]
fn a_tag_entry_littlecms_skips_passes_through() {
    // The first entry is rXYZ: its offset at 136, its size at 140.
    assert_passes_through(&edited(put_u32(136, 5000)));
    assert_passes_through(&edited(put_u32(136, 0)));
    assert_passes_through(&edited(put_u32(140, 0)));
    assert_passes_through(&grey_with(&[(b"kTRC", vec![])], 0));
}

#[test]
fn a_tag_shorter_than_its_type_header_passes_through() {
    assert_passes_through(&rgb_with(&replaced(b"rTRC", b"curv".to_vec()), 0));
}

#[test]
fn a_colorant_tag_of_another_type_passes_through() {
    assert_passes_through(&rgb_with(&replaced(b"gXYZ", curv(&[1])), 0));
}

#[test]
fn a_curve_tag_of_another_type_passes_through() {
    assert_passes_through(&rgb_with(&replaced(b"bTRC", xyz([0, 0, 0])), 0));
    // LittleCMS reads an old Monaco tool's curve type for `rTRC`, `gTRC`
    // and `bTRC` only.
    let monaco = retyped(curv(&[GAMMA_18]), MONACO_CURVE);
    assert_passes_through(&grey_with(&[(b"kTRC", monaco)], 0));
}

#[test]
fn a_parametric_curve_of_an_unknown_function_passes_through() {
    assert_passes_through(&rgb_with(&replaced(b"rTRC", para(5, &[1.0])), 0));
}

/// A value that runs past the end of the profile is a read LittleCMS fails.
#[test]
fn a_tag_value_past_the_profile_end_passes_through() {
    let last = |name: &'static [u8; 4], data: Vec<u8>| {
        let mut tags: Vec<_> = wide_gamut_tags()
            .into_iter()
            .filter(|(n, _)| *n != name)
            .collect();
        tags.push((name, data));
        rgb_with(&tags, 0)
    };
    let mut short = curv(&[0, 30000, 65535]);
    short.truncate(14);
    assert_passes_through(&last(b"bTRC", short));
    assert_passes_through(&last(b"bTRC", para(4, &[1.0, 1.0])));
    let mut short = xyz([2051, 7, 54061]);
    short.truncate(16);
    assert_passes_through(&last(b"bXYZ", short));
}

#[test]
fn a_rendering_intent_littlecms_does_not_know_passes_through() {
    for intent in [4, 9, 16, u32::MAX] {
        assert_passes_through(&edited(put_u32(64, intent)));
        assert_passes_through(&grey_with(&[(b"kTRC", curv(&[GAMMA_18]))], intent));
    }
}

#[test]
fn a_curve_of_more_than_32767_entries_passes_through() {
    let table = |n: u32| {
        let mut out = b"curv\0\0\0\0".to_vec();
        out.extend_from_slice(&n.to_be_bytes());
        out.extend((0..n).flat_map(|i| ((i * 2) as u16).to_be_bytes()));
        out
    };
    assert_passes_through(&rgb_with(&replaced(b"gTRC", table(32768)), 0));
    // 32767 entries convert.
    let image = decode_srgb(&rgb_with(&replaced(b"gTRC", table(32767)), 0)).expect("sRGB");
    assert_eq!(image.jp2.unwrap().colour, ColourSpace::Srgb);
}

/// LittleCMS reads its black-preserving intents (10 to 15) on an RGB or
/// grey profile as perceptual, relative colorimetric or saturation, without
/// the black point compensation it forces for perceptual and saturation, so
/// each converts as relative colorimetric (1). The curve's nonzero black
/// makes perceptual (0) differ.
#[test]
fn black_preserving_intents_convert_as_relative_colorimetric() {
    let black = para(2, &[2.2, 1.0, 0.1, 0.05]);
    let tags: Vec<(&[u8; 4], Vec<u8>)> = wide_gamut_tags()
        .into_iter()
        .map(|(n, d)| {
            if n.ends_with(b"TRC") {
                (n, black.clone())
            } else {
                (n, d)
            }
        })
        .collect();
    let rgb = |intent| rgb_with(&tags, intent);
    let grey = |intent| grey_with(&[(b"kTRC", black.clone())], intent);
    let shapes: [&dyn Fn(u32) -> Vec<u8>; 2] = [&rgb, &grey];
    for bytes in shapes {
        assert_ne!(
            decode_srgb(&bytes(0)).unwrap().components,
            decode_srgb(&bytes(1)).unwrap().components
        );
        for intent in 10..=15 {
            assert_converts_like(&bytes(intent), &bytes(1));
        }
    }
}

/// LittleCMS reads an old Monaco tool's curve type as `curv` and an old
/// Corbis tool's XYZ type as `XYZ `.
#[test]
fn legacy_curve_and_xyz_types_convert() {
    assert_converts_like(
        &rgb_with(&replaced(b"rTRC", retyped(curv(&[300]), MONACO_CURVE)), 0),
        &rgb_with(&replaced(b"rTRC", curv(&[300])), 0),
    );
    let colorant = xyz([52000, 18000, 0]);
    assert_converts_like(
        &rgb_with(&replaced(b"rXYZ", retyped(colorant.clone(), CORBIS_XYZ)), 0),
        &rgb_with(&replaced(b"rXYZ", colorant), 0),
    );
}

/// Absolute colorimetric with a media white point LittleCMS can't read
/// takes D50, as with no `wtpt` at all.
#[test]
fn an_unreadable_white_point_converts_as_d50() {
    let no_white = intent_3(wide_gamut_tags());
    let with_white = |data: Vec<u8>| {
        let mut tags = wide_gamut_tags();
        tags.push((b"wtpt", data));
        intent_3(tags)
    };
    // A readable white point other than D50 changes the output.
    assert_ne!(
        decode_srgb(&with_white(xyz(D65))).unwrap().components,
        decode_srgb(&no_white).unwrap().components
    );
    assert_converts_like(&with_white(curv(&[1])), &no_white);
    assert_converts_like(&with_white(b"XYZ ".to_vec()), &no_white);
    // The last tag, cut short: the read runs past the profile's end.
    let mut short = xyz(D65);
    short.truncate(12);
    assert_converts_like(&with_white(short), &no_white);
}

/// LittleCMS reads a tag's value without checking the tag's size, so a value
/// that runs past its tag takes the bytes after it: here the next tag's.
#[test]
fn a_value_past_its_tag_reads_the_bytes_after_it() {
    // gTRC declares three entries but holds one. bTRC follows, and starts
    // with "curv".
    let mut short = curv(&[1000, 0, 0]);
    short.truncate(14);
    let entries = [1000, u16::from_be_bytes(*b"cu"), u16::from_be_bytes(*b"rv")];
    assert_converts_like(
        &rgb_with(&replaced(b"gTRC", short), 0),
        &rgb_with(&replaced(b"gTRC", curv(&entries)), 0),
    );
    // bXYZ holds two of its three values. rTRC follows.
    let mut short = xyz([2051, 7, 0]);
    short.truncate(16);
    let value = [2051, 7, i32::from_be_bytes(*b"curv")];
    assert_converts_like(
        &rgb_with(&replaced(b"bXYZ", short), 0),
        &rgb_with(&replaced(b"bXYZ", xyz(value)), 0),
    );
}

/// A profile header field at `at` set to `value`.
fn put(at: usize, value: &'static [u8; 4]) -> impl Fn(&mut Vec<u8>) {
    move |p: &mut Vec<u8>| p[at..at + 4].copy_from_slice(value)
}

/// The monochrome profile on one channel, edited by `edit`.
fn grey_edited(edit: impl Fn(&mut Vec<u8>)) -> Vec<u8> {
    let mut icc = grey_profile();
    edit(&mut icc);
    file(ONE, &icc_colr(&icc), &[])
}

/// A `lut16Type` (`mft2`) table as the oracle script builds it: an identity
/// matrix, two-entry ramps (whatever `entries` says), and a CLUT of
/// arbitrary values.
fn lut16(inputs: u8, outputs: u8, grid: u8, entries: [u16; 2]) -> Vec<u8> {
    let mut out = b"mft2\0\0\0\0".to_vec();
    out.extend_from_slice(&[inputs, outputs, grid, 0]);
    for v in [1, 0, 0, 0, 1, 0, 0, 0, 1i32] {
        out.extend_from_slice(&(v << 16).to_be_bytes());
    }
    out.extend(entries.iter().flat_map(|e| e.to_be_bytes()));
    let ramp = [0, 0, 0xff, 0xff];
    for _ in 0..inputs {
        out.extend_from_slice(&ramp);
    }
    for i in 0..u32::from(grid).pow(u32::from(inputs)) {
        for o in 0..u32::from(outputs) {
            out.extend_from_slice(&(((i * 997 + o * 4001) % 65536) as u16).to_be_bytes());
        }
    }
    for _ in 0..outputs {
        out.extend_from_slice(&ramp);
    }
    out
}

/// A `lut8Type` (`mft1`) table, built like [`lut16`].
fn lut8(inputs: u8, outputs: u8, grid: u8) -> Vec<u8> {
    let mut out = b"mft1\0\0\0\0".to_vec();
    out.extend_from_slice(&[inputs, outputs, grid, 0]);
    for v in [1, 0, 0, 0, 1, 0, 0, 0, 1i32] {
        out.extend_from_slice(&(v << 16).to_be_bytes());
    }
    let ramp: Vec<u8> = (0..=255).collect();
    for _ in 0..inputs {
        out.extend_from_slice(&ramp);
    }
    let cells = u32::from(grid).pow(u32::from(inputs)) * u32::from(outputs);
    out.extend((0..cells).map(|i| ((i * 37) % 256) as u8));
    for _ in 0..outputs {
        out.extend_from_slice(&ramp);
    }
    out
}

/// A tag LittleCMS refuses to read as a lookup table.
fn junk_table() -> Vec<u8> {
    let mut out = b"xxxx".to_vec();
    out.resize(44, 0);
    out
}

/// The wide-gamut tags plus these, on three channels.
fn rgb_plus(extra: &[(&'static [u8; 4], Vec<u8>)], intent: u32) -> Vec<u8> {
    let mut tags = wide_gamut_tags();
    tags.extend(extra.iter().cloned());
    rgb_with(&tags, intent)
}

/// LittleCMS opens a profile of any device class it knows, and refuses
/// others (`validDeviceClass`).
#[test]
fn an_unknown_device_class_passes_through() {
    assert_passes_through(&edited(put(12, b"zzzz")));
    assert_passes_through(&grey_edited(put(12, b"zzzz")));
}

/// A display, output or colour space profile, or one of class zero, is a
/// matrix/TRC profile to LittleCMS like an input profile.
#[test]
fn other_device_classes_with_a_matrix_and_curves_convert() {
    for class in [b"mntr", b"prtr", b"spac", &[0u8; 4]] {
        assert_converts_like(&edited(put(12, class)), &edited(|_| {}));
        assert_converts_like(&grey_edited(put(12, class)), &grey_edited(|_| {}));
    }
    // At absolute colorimetric the white point applies, except that
    // LittleCMS takes D50 for a display profile before version 4.
    let mut tags = wide_gamut_tags();
    tags.push((b"wtpt", xyz(D65)));
    let white = |class: &[u8; 4], version: u32| {
        let mut icc = profile(class, b"RGB ", b"XYZ ", 3, &tags);
        icc[8..12].copy_from_slice(&version.to_be_bytes());
        file(THREE, &icc_colr(&icc), &[])
    };
    let no_white = intent_3(wide_gamut_tags());
    assert_ne!(
        decode_srgb(&white(b"scnr", 0x0220_0000))
            .unwrap()
            .components,
        decode_srgb(&no_white).unwrap().components
    );
    for class in [b"prtr", b"spac", &[0u8; 4]] {
        assert_converts_like(&white(class, 0x0220_0000), &white(b"scnr", 0x0220_0000));
    }
    assert_converts_like(&white(b"mntr", 0x0220_0000), &no_white);
    assert_converts_like(&white(b"mntr", 0x0400_0000), &white(b"scnr", 0x0400_0000));
}

/// LittleCMS builds a device link or abstract profile's transform from its
/// lookup table only, and gives a named colour profile no RGB or grey
/// input.
#[test]
fn a_device_link_abstract_or_named_colour_profile_passes_through() {
    for class in [b"link", b"abst", b"nmcl"] {
        assert_passes_through(&edited(put(12, class)));
        assert_passes_through(&grey_edited(put(12, class)));
    }
    // A valid named colour list changes nothing.
    let mut ncl2 = b"ncl2\0\0\0\0".to_vec();
    for word in [0u32, 1, 3] {
        ncl2.extend_from_slice(&word.to_be_bytes());
    }
    ncl2.resize(ncl2.len() + 64, 0);
    let mut name = b"red".to_vec();
    name.resize(32, 0);
    ncl2.extend_from_slice(&name);
    for v in [30000u16, 40000, 20000, 65535, 0, 0] {
        ncl2.extend_from_slice(&v.to_be_bytes());
    }
    let mut icc = profile(b"scnr", b"RGB ", b"XYZ ", 0, &[(b"ncl2", ncl2)]);
    icc[12..16].copy_from_slice(b"nmcl");
    assert_passes_through(&file(THREE, &icc_colr(&icc), &[]));
    // A device link falls back to `D2B0` where an input profile does not.
    let tables = [(b"D2B0", junk_table()), (b"A2B1", lut16(3, 3, 2, [2, 2]))];
    let mut link = profile(b"link", b"RGB ", b"XYZ ", 1, &tables);
    assert_passes_through(&file(THREE, &icc_colr(&link), &[]));
    link[12..16].copy_from_slice(b"scnr");
    expect_err(
        decode_srgb(&file(THREE, &icc_colr(&link), &[])),
        is_unsupported,
        "`A2B1` lookup table",
    );
}

/// A tag the transform needs and the profile lacks is a tag LittleCMS
/// can't read.
#[test]
fn a_missing_tag_passes_through() {
    for name in [b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC", b"bTRC"] {
        let tags: Vec<_> = wide_gamut_tags()
            .into_iter()
            .filter(|(n, _)| n != &name)
            .collect();
        assert_passes_through(&rgb_with(&tags, 0));
    }
    assert_passes_through(&grey_with(&[(b"wtpt", xyz(D65))], 0));
    assert_passes_through(&rgb_with(&[], 0));
    assert_passes_through(&grey_with(&[], 0));
}

/// LittleCMS can't connect a profile to the sRGB profile through a
/// connection space other than XYZ or Lab.
#[test]
fn a_connection_space_littlecms_does_not_know_passes_through() {
    assert_passes_through(&edited(put(20, b"zzzz")));
    assert_passes_through(&edited(put(20, &[0u8; 4])));
    assert_passes_through(&grey_edited(put(20, b"zzzz")));
}

/// OpenJPEG converts an RGB, grey or YCbCr profile and leaves any other as
/// coded.
#[test]
fn a_colour_space_openjpeg_does_not_convert_passes_through() {
    assert_passes_through(&edited(put(16, b"CMYK")));
    assert_passes_through(&edited(put(16, b"zzzz")));
    let mut icc = wide_gamut();
    icc[16..20].copy_from_slice(b"CMYK");
    assert_passes_through(&file(ONE, &icc_colr(&icc), &[]));
}

/// A lookup table LittleCMS picks but can't read, or whose channels don't
/// fit the transform, makes it refuse the profile, matrix and curves or not.
#[test]
fn a_lookup_table_littlecms_refuses_passes_through() {
    let a2b0 = |table: Vec<u8>| rgb_plus(&[(b"A2B0", table)], 0);
    let mut sixteen_inputs = b"mft2\0\0\0\0".to_vec();
    sixteen_inputs.extend_from_slice(&[16, 3, 2, 0]);
    sixteen_inputs.resize(52, 0);
    let mut past_end = lut16(3, 3, 2, [2, 2]);
    past_end.truncate(60);
    let no_channels = |kind: &[u8; 4], len: usize| {
        let mut out = kind.to_vec();
        out.resize(len, 0);
        out
    };
    for table in [
        junk_table(),
        b"mft2".to_vec(),
        curv(&[GAMMA_18]),
        no_channels(b"mft2", 44),
        no_channels(b"mft1", 44),
        no_channels(b"mAB ", 32),
        sixteen_inputs,
        lut16(1, 3, 2, [2, 2]),
        lut16(3, 1, 2, [2, 2]),
        lut16(3, 3, 1, [2, 2]),
        lut16(3, 3, 2, [1, 2]),
        lut16(3, 3, 2, [2, 1]),
        lut16(3, 3, 2, [0x8000, 2]),
        past_end,
        lut8(3, 3, 1),
        lut8(1, 3, 2),
    ] {
        assert_passes_through(&a2b0(table));
    }
    let mut mpet = b"mpet\0\0\0\0".to_vec();
    mpet.extend_from_slice(&[0, 1, 0, 3, 0, 0, 0, 0]);
    assert_passes_through(&rgb_plus(&[(b"D2B0", mpet)], 0));
    assert_passes_through(&rgb_plus(&[(b"D2B0", junk_table())], 0));
    // A grey profile's table takes one input.
    let grey = |table: Vec<u8>| grey_with(&[(b"kTRC", curv(&[GAMMA_18])), (b"A2B0", table)], 0);
    assert_passes_through(&grey(lut16(3, 3, 2, [2, 2])));
    assert_passes_through(&grey(no_channels(b"mft2", 44)));
    // Each intent reads its own table, then `A2B0`: absolute colorimetric
    // reads `A2B1` but `D2B3`, and a black-preserving intent reads its
    // base intent's.
    for (tag, intent) in [
        (b"A2B1", 1),
        (b"A2B1", 3),
        (b"A2B1", 11),
        (b"A2B1", 14),
        (b"A2B2", 2),
        (b"A2B2", 15),
        (b"A2B0", 1),
        (b"D2B3", 3),
        (b"D2B1", 1),
    ] {
        assert_passes_through(&rgb_plus(&[(tag, junk_table())], intent));
    }
    // A readable `A2B0` does not stand in for an unreadable `A2B1`.
    assert_passes_through(&rgb_plus(
        &[(b"A2B0", lut16(3, 3, 2, [2, 2])), (b"A2B1", junk_table())],
        1,
    ));
    // Nor a table for an intent LittleCMS does not know.
    assert_passes_through(&rgb_plus(&[(b"A2B0", lut16(3, 3, 2, [2, 2]))], 4));
}

/// LittleCMS reads only the lookup table of the profile's intent (or
/// `A2B0`), so another intent's table, readable or not, changes nothing.
#[test]
fn a_lookup_table_for_another_intent_is_not_read() {
    for (tag, intent) in [
        (b"A2B1", 0),
        (b"A2B1", 10),
        (b"A2B2", 1),
        (b"A2B2", 3),
        (b"D2B1", 0),
        (b"D2B1", 3),
        (b"D2B3", 1),
    ] {
        assert_converts_like(
            &rgb_plus(&[(tag, junk_table())], intent),
            &rgb_with(&wide_gamut_tags(), intent),
        );
    }
}

/// A lookup table LittleCMS reads is a form this step does not apply.
#[test]
fn a_lookup_table_littlecms_reads_is_unsupported() {
    let mut empty_mab = b"mAB \0\0\0\0".to_vec();
    empty_mab.extend_from_slice(&[3, 3, 0, 0]);
    empty_mab.resize(32, 0);
    let mut empty_mpet = b"mpet\0\0\0\0".to_vec();
    empty_mpet.extend_from_slice(&[0, 3, 0, 3, 0, 0, 0, 0]);
    for (tables, intent, name) in [
        (vec![(b"A2B0", lut16(3, 3, 2, [2, 2]))], 0, "A2B0"),
        (vec![(b"A2B0", lut16(3, 3, 0, [2, 2]))], 0, "A2B0"),
        (vec![(b"A2B0", lut16(3, 3, 2, [0, 0]))], 0, "A2B0"),
        (vec![(b"A2B0", lut8(3, 3, 2))], 0, "A2B0"),
        (vec![(b"A2B0", empty_mab)], 0, "A2B0"),
        (vec![(b"D2B0", empty_mpet)], 0, "D2B0"),
        (vec![(b"A2B1", lut16(3, 3, 2, [2, 2]))], 1, "A2B1"),
        (vec![(b"A2B1", lut16(3, 3, 2, [2, 2]))], 3, "A2B1"),
        (vec![(b"A2B0", lut16(3, 3, 2, [2, 2]))], 3, "A2B0"),
    ] {
        let tables: Vec<(&'static [u8; 4], Vec<u8>)> = tables
            .into_iter()
            .map(|(n, d)| (n as &'static [u8; 4], d))
            .collect();
        expect_err(
            decode_srgb(&rgb_plus(&tables, intent)),
            is_unsupported,
            &format!("`{name}` lookup table"),
        );
    }
    for class in [b"link", b"abst"] {
        let mut icc = profile(
            b"scnr",
            b"RGB ",
            b"XYZ ",
            0,
            &[(b"A2B0", lut16(3, 3, 2, [2, 2]))],
        );
        icc[12..16].copy_from_slice(class);
        expect_err(
            decode_srgb(&file(THREE, &icc_colr(&icc), &[])),
            is_unsupported,
            "`A2B0` lookup table",
        );
    }
    expect_err(
        decode_srgb(&grey_with(
            &[
                (b"kTRC", curv(&[GAMMA_18])),
                (b"A2B0", lut16(1, 3, 2, [2, 2])),
            ],
            0,
        )),
        is_unsupported,
        "`A2B0` lookup table",
    );
}

/// LittleCMS counts only the entries it keeps as duplicates: a skipped
/// first copy leaves the second to be read.
#[test]
fn a_duplicate_whose_first_copy_is_skipped_converts() {
    let gamma = curv(&[300]);
    let mut tags = wide_gamut_tags();
    tags.push((b"rTRC", gamma.clone()));
    // The first rTRC entry is the fourth: its offset at 132 + 3 * 12 + 4.
    let mut icc = profile(b"scnr", b"RGB ", b"XYZ ", 0, &tags);
    icc[172..176].copy_from_slice(&0u32.to_be_bytes());
    assert_converts_like(
        &file(THREE, &icc_colr(&icc), &[]),
        &rgb_with(&replaced(b"rTRC", gamma), 0),
    );
}

/// OpenJPEG asks LittleCMS for a transform before it looks at the samples,
/// so every profile LittleCMS refuses passes through, whatever the
/// channels: this step's own checks on the profile and the channels come
/// after LittleCMS's. Where LittleCMS builds the transform, they stand.
/// `icc/profiles.json` holds the display class at intent 4
/// (`class_mntr_intent_4`) and the grey profile on three channels at
/// intent 4 (`grey_on_rgb_intent_4`), which OpenJPEG leaves as coded; each
/// file below was checked the same way with `opj_decompress`.
#[test]
fn refusals_are_ordered_as_in_openjpeg() {
    let intent = |icc: &mut Vec<u8>, value: u32| icc[64..68].copy_from_slice(&value.to_be_bytes());
    // A profile LittleCMS will not open, of a class LittleCMS refuses.
    assert_passes_through(&edited(|p| {
        p[12..16].copy_from_slice(b"link");
        p[36..40].copy_from_slice(b"acsP");
    }));
    // A display profile with an intent LittleCMS does not know.
    assert_passes_through(&edited(|p| {
        p[12..16].copy_from_slice(b"mntr");
        intent(p, 4);
    }));
    // A Lab connection space or a lookup table with that intent.
    assert_passes_through(&edited(|p| {
        p[20..24].copy_from_slice(b"Lab ");
        intent(p, 4);
    }));
    let mut lut = profile(
        b"scnr",
        b"RGB ",
        b"XYZ ",
        4,
        &[(b"A2B0", lut16(3, 3, 2, [2, 2]))],
    );
    assert_passes_through(&file(THREE, &icc_colr(&lut), &[]));
    intent(&mut lut, 0);
    expect_err(
        decode_srgb(&file(THREE, &icc_colr(&lut), &[])),
        is_unsupported,
        "`A2B0` lookup table",
    );
    // A YCbCr profile: as coded on one channel, and on three, 8 or 16-bit,
    // with an intent LittleCMS does not know or a missing tag. Where
    // LittleCMS would build the transform it is refused.
    let ycbr = |p: &mut Vec<u8>| p[16..20].copy_from_slice(b"YCbr");
    let wide16 = palette(&[15, 15, 15], 4, |j, _| u32::from(j));
    for value in [0, 4] {
        let mut icc = wide_gamut();
        ycbr(&mut icc);
        intent(&mut icc, value);
        assert_passes_through(&file(ONE, &icc_colr(&icc), &[]));
        for (coded, children) in [(THREE, vec![]), (ONE, wide16.clone())] {
            let decoded = decode_srgb(&file(coded, &icc_colr(&icc), &children));
            if value == 0 {
                expect_err(decoded, is_unsupported, "`YCbr` colour space");
            } else {
                assert_passes_through(&file(coded, &icc_colr(&icc), &children));
            }
        }
    }
    let mut missing = wide_gamut();
    ycbr(&mut missing);
    missing[128..132].copy_from_slice(&0u32.to_be_bytes());
    assert_passes_through(&file(THREE, &icc_colr(&missing), &[]));
    // The monochrome profile on three channels, or on one colour channel of
    // three: refused at intent 0, passed through at intent 4.
    let colour_first = bx(
        b"cdef",
        &[0, 3, 0, 0, 0, 0, 0, 1, 0, 1, 0, 1, 0, 0, 0, 2, 0, 1, 0, 0],
    );
    for (children, fragment) in [
        (vec![], "monochrome ICC profile on 3 channels"),
        (vec![colour_first], "monochrome ICC profile on 3 channels"),
    ] {
        let mut grey = grey_profile();
        expect_err(
            decode_srgb(&file(THREE, &icc_colr(&grey), &children)),
            is_unsupported,
            fragment,
        );
        intent(&mut grey, 4);
        assert_passes_through(&file(THREE, &icc_colr(&grey), &children));
    }
    // Palette columns of mixed depth fail OpenJPEG's AFL test, which comes
    // before the transform, so they pass through under an RGB profile
    // whatever the profile is (`pal_mixed_8_12_8` in `opj_decompress`).
    assert_passes_through(&file(
        ONE,
        &icc_colr(&wide_gamut()),
        &palette(&[7, 11, 7], 4, |j, _| u32::from(j)),
    ));
    // Sample formats this step does not apply a profile to: 12-bit RGB, 12
    // and 16-bit grey.
    let shapes: [(&[u8], Vec<u8>, &str); 3] = [
        (&[11, 11, 11], wide_gamut(), "RGB ICC profile from 12-bit"),
        (&[11], grey_profile(), "monochrome ICC profile from 12-bit"),
        (&[15], grey_profile(), "monochrome ICC profile from 16-bit"),
    ];
    for (depths, mut icc, fragment) in shapes {
        let columns = palette(depths, 4, |j, _| u32::from(j));
        expect_err(
            decode_srgb(&file(ONE, &icc_colr(&icc), &columns)),
            is_unsupported,
            fragment,
        );
        intent(&mut icc, 4);
        assert_passes_through(&file(ONE, &icc_colr(&icc), &columns));
        // A missing tag is a refusal of LittleCMS's too.
        intent(&mut icc, 0);
        icc[128..132].copy_from_slice(&0u32.to_be_bytes());
        assert_passes_through(&file(ONE, &icc_colr(&icc), &columns));
    }
}
