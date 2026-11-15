//! Random windows at reductions 0 to 2 decode to the crop of the whole
//! decode at the same reduction, bit for bit, on 5/3 and on 9/7.
//!
//! A window skips the code-blocks whose wavelet support does not reach it
//! and runs the inverse wavelet over only the samples it needs, so a margin
//! too small anywhere shows here as a sample that differs from the whole
//! decode. The windows are drawn from a fixed seed, so a failure reproduces.

use std::path::PathBuf;

use rust_j2k::{DecodeOptions, Error, Image, Window, decode_with, read_header};

/// A splitmix64 stream: small, and the same on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A value in `0..n`.
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % u64::from(n.max(1))) as u32
    }
}

/// The image area `XOsiz..Xsiz` by `YOsiz..Ysiz` of a raw codestream, read
/// from its SIZ marker, which follows SOC.
fn image_area(bytes: &[u8]) -> (u32, u32, u32, u32) {
    assert_eq!(&bytes[..4], &[0xff, 0x4f, 0xff, 0x51], "SOC then SIZ");
    let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    // Marker, Lsiz and Rsiz, then Xsiz, Ysiz, XOsiz and YOsiz.
    (word(16), word(20), word(8), word(12))
}

/// `ceil(ceil(v / d) / 2^r)`: a reference-grid bound on a component's grid
/// at reduction `r`.
fn at(v: u32, d: u32, r: u8) -> u32 {
    v.div_ceil(d).div_ceil(1 << r)
}

/// The samples of `whole`'s component `c` inside `window`, cut to the image
/// area `image`, at reduction `r`.
fn crop(whole: &Image, c: usize, image: (u32, u32, u32, u32), window: Window, r: u8) -> Vec<i32> {
    let plane = &whole.components[c];
    let (dx, dy) = (u32::from(plane.x_sampling), u32::from(plane.y_sampling));
    let (x0, y0, x1, y1) = (
        window.x0.max(image.0),
        window.y0.max(image.1),
        window.x1.min(image.2),
        window.y1.min(image.3),
    );
    let (ox, oy) = (at(image.0, dx, r), at(image.1, dy, r));
    let mut out = Vec::new();
    for y in at(y0, dy, r)..at(y1, dy, r) {
        for x in at(x0, dx, r)..at(x1, dx, r) {
            out.push(plane.samples[((y - oy) * plane.width + (x - ox)) as usize]);
        }
    }
    out
}

/// A random window on and around the image area `image`: mostly small, so
/// it falls inside a tile and a few of its code-blocks, sometimes wide, and
/// now and then reaching past the image's edges.
fn window(rng: &mut Rng, image: (u32, u32, u32, u32)) -> Window {
    let (x0, y0, x1, y1) = image;
    let mut axis = |lo: u32, hi: u32| {
        let start = (lo + rng.below(hi - lo)).saturating_sub(rng.below(3));
        let reach = [3, 12, 40, hi - lo][rng.below(4) as usize];
        (start, start + 1 + rng.below(reach))
    };
    let (wx0, wx1) = axis(x0, x1);
    let (wy0, wy1) = axis(y0, y1);
    Window::new(wx0, wy0, wx1, wy1)
}

/// Decode `windows` random windows of `bytes` at each reduction from 0 to
/// 2 the codestream allows, and check each against the whole decode. A
/// window the decode refuses is drawn again.
fn check(name: &str, bytes: &[u8], windows: usize, seed: u64) {
    let image = image_area(bytes);
    let header = read_header(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut rng = Rng(seed);
    for r in 0..=header.max_reduction.min(2) {
        let all = DecodeOptions::default().with_resolution_reduction(r);
        let whole = decode_with(bytes, &all).unwrap_or_else(|e| panic!("{name} -r {r}: {e}"));
        let (mut decoded, mut drawn) = (0, 0);
        while decoded < windows {
            drawn += 1;
            assert!(
                drawn <= 20 * windows,
                "{name} -r {r}: {decoded} of {drawn} decoded"
            );
            let window = window(&mut rng, image);
            let what = format!("{name} -r {r} {window:?}");
            let part = match decode_with(bytes, &all.clone().with_window(Some(window))) {
                Ok(part) => part,
                // A window that holds no sample of some component at this
                // reduction is refused; that is `decode_window.rs`'s to test.
                Err(Error::InvalidOptions(_)) => continue,
                Err(e) => panic!("{what}: {e}"),
            };
            decoded += 1;
            assert_eq!(part.components.len(), whole.components.len(), "{what}");
            for (c, component) in part.components.iter().enumerate() {
                let expected = crop(&whole, c, image, window, r);
                assert_eq!(
                    component.samples.len(),
                    expected.len(),
                    "{what}: component {c}"
                );
                if component.samples != expected {
                    let worst = component
                        .samples
                        .iter()
                        .zip(&expected)
                        .map(|(a, b)| (i64::from(*a) - i64::from(*b)).abs())
                        .max()
                        .unwrap_or(0);
                    panic!("{what}: component {c} differs from the whole decode by up to {worst}");
                }
            }
        }
    }
}

fn read(path: &str) -> Option<Vec<u8>> {
    std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)).ok()
}

/// The window fixtures: 3 x 3 tiles with partial edge tiles on 5/3 (with
/// and without the color transform) and 9/7, and the sub-sampled 5/3 and
/// 9/7 codestreams with their image and tiles off the grid's origin.
#[test]
fn random_windows_of_the_window_fixtures_are_crops_of_the_whole_decode() {
    for (seed, name) in ["tiled_53", "tiled_53_nomct", "tiled_97", "sub_53", "sub_97"]
        .into_iter()
        .enumerate()
    {
        let bytes = read(&format!("tests/fixtures/window/{name}.j2k")).expect(name);
        check(name, &bytes, 60, seed as u64);
    }
}

/// Larger codestreams, with many code-blocks to a band: the 2048 x 2048
/// benchmark image (5/3, 256 x 256 tiles), and conformance codestreams on
/// 9/7 (one tile with six levels; 128 x 128 tiles; 37 x 37 tiles of 8 x 8
/// code-blocks) and on 5/3 (sub-sampled components), and two HTJ2K
/// codestreams from OpenJPH, on 9/7 and 5/3. They are left out of the
/// published package, so a missing one is skipped.
#[test]
fn random_windows_of_larger_codestreams_are_crops_of_the_whole_decode() {
    let mut ran = 0;
    for (seed, path) in [
        "tests/fixtures/bench/tiled_2048_gray.j2k",
        "tests/fixtures/conformance/codestreams/p0_04.j2k",
        "tests/fixtures/conformance/codestreams/p1_04.j2k",
        "tests/fixtures/conformance/codestreams/p1_05.j2k",
        "tests/fixtures/conformance/codestreams/p0_05.j2k",
        "tests/fixtures/openjph/files/simple_enc_irv97_64x64.j2c",
        "tests/fixtures/openjph/files/simple_enc_rev53_64x64_yuv.j2c",
    ]
    .into_iter()
    .enumerate()
    {
        let Some(bytes) = read(path) else {
            eprintln!("{path} is not present (packaged crate?); skipping");
            continue;
        };
        check(path, &bytes, 16, 100 + seed as u64);
        ran += 1;
    }
    eprintln!("{ran} of 7 larger codestreams checked");
}
