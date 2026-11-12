//! ICC to sRGB output through the public API, graded bit-exactly against
//! `opj_decompress -force-rgb` (OpenJPEG 2.5.0 with LittleCMS 2.14).
//!
//! The oracles are SHA-256 hashes of OpenJPEG's output planes, made by
//! `scripts/gen-icc-oracles.py`:
//!
//! - `conformance/jp2/icc_variants.json`: ISO/IEC 15444-4 files 5, 7 and 8 at
//!   rendering intents 0 to 3, and at intent 3 with three other media white
//!   points. The variants are rebuilt here by patching the committed files.
//! - `icc/profiles.json`: edge-case and malformed profiles over small
//!   codestreams, each either converted by OpenJPEG (hashes recorded) or left
//!   unconverted because LittleCMS refused the profile.

mod support;

use std::path::PathBuf;

use rust_j2k::{
    Association, ChannelDefinition, ChannelType, ColorOutput, ColorSpace, Component, DecodeOptions,
    Error, Image, decode, decode_with,
};
use serde::Deserialize;
use support::jp2::sha256_hex;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read(path: &str) -> Vec<u8> {
    let path = fixtures().join(path);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn srgb() -> DecodeOptions {
    DecodeOptions::default().with_color(ColorOutput::Srgb)
}

/// The hash the oracle script records: samples in raster order, each a
/// 4-byte big-endian signed integer.
fn plane_hash(component: &Component) -> String {
    let bytes: Vec<u8> = component
        .samples
        .iter()
        .flat_map(|s| s.to_be_bytes())
        .collect();
    sha256_hex(&bytes)
}

/// Compare a decode with OpenJPEG's three output planes. A monochrome profile
/// whose output is neutral comes out as one gray channel, which must then
/// equal each of OpenJPEG's three identical planes.
fn check_planes(image: &Image, want: &[String], depth: u8, what: &str) {
    assert_eq!(want.len(), 3, "{what}: OpenJPEG gives R, G, B");
    let got: Vec<String> = image.components.iter().map(plane_hash).collect();
    match got.len() {
        1 => {
            assert!(
                want.iter().all(|w| *w == got[0]),
                "{what}: one gray channel, but OpenJPEG's planes differ or do not match it"
            );
            assert_eq!(
                image.jp2.as_ref().unwrap().color,
                ColorSpace::Grayscale,
                "{what}"
            );
        }
        3 => {
            for (k, (g, w)) in got.iter().zip(want).enumerate() {
                assert_eq!(g, w, "{what}: channel {k} differs from OpenJPEG");
            }
            assert_eq!(
                image.jp2.as_ref().unwrap().color,
                ColorSpace::Srgb,
                "{what}"
            );
        }
        n => panic!("{what}: {n} channels"),
    }
    for c in &image.components {
        assert_eq!((c.bit_depth, c.signed), (depth, false), "{what}: depth");
    }
}

// ---------------------------------------------------------------------------
// Conformance files at every intent
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Variants {
    #[expect(dead_code)]
    provenance: serde_json::Value,
    variants: Vec<Variant>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Variant {
    file: String,
    variant: String,
    intent: u32,
    white: Option<[f64; 3]>,
    width: u32,
    height: u32,
    bit_depth: u8,
    planes: Vec<String>,
}

/// Each JP2 box in `data[start..end]` as (type, payload range).
fn boxes(data: &[u8], start: usize, end: usize) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    let mut at = start;
    while at < end {
        let length = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = data[at + 4..at + 8].try_into().unwrap();
        out.push((kind, at + 8, at + length));
        at += length;
    }
    out
}

/// The byte range of the ICC profile in a JP2 file's `colr` box.
fn profile_span(data: &[u8]) -> (usize, usize) {
    for (kind, start, end) in boxes(data, 12, data.len()) {
        if &kind == b"jp2h" {
            for (kind, start, end) in boxes(data, start, end) {
                if &kind == b"colr" && data[start] == 2 {
                    return (start + 3, end);
                }
            }
        }
    }
    panic!("no ICC profile");
}

/// Set the profile's rendering intent and, if given, its `wtpt` XYZ.
fn patch(profile: &mut [u8], intent: u32, white: Option<[f64; 3]>) {
    profile[64..68].copy_from_slice(&intent.to_be_bytes());
    let Some(white) = white else { return };
    let count = u32::from_be_bytes(profile[128..132].try_into().unwrap()) as usize;
    for i in 0..count {
        let entry = 132 + 12 * i;
        if &profile[entry..entry + 4] == b"wtpt" {
            let offset = u32::from_be_bytes(profile[entry + 4..entry + 8].try_into().unwrap());
            let at = offset as usize + 8;
            for (k, v) in white.iter().enumerate() {
                let fixed = (v * 65536.0).round() as i32;
                profile[at + 4 * k..at + 4 * k + 4].copy_from_slice(&fixed.to_be_bytes());
            }
            return;
        }
    }
    panic!("no wtpt tag");
}

fn variants() -> Vec<Variant> {
    let text = String::from_utf8(read("conformance/jp2/icc_variants.json")).unwrap();
    serde_json::from_str::<Variants>(&text)
        .expect("icc_variants.json")
        .variants
}

/// Files 5, 7 and 8 at intents 0 to 3, and at intent 3 with a D65,
/// (1.2, 1.1, 1.0) and (0.9, 0.95, 0.7) media white point, match OpenJPEG
/// sample for sample: 8-bit output for files 5 and 8, 16-bit for file7.
#[test]
fn icc_conformance_variants_match_openjpeg_exactly() {
    let all = variants();
    assert_eq!(all.len(), 21, "three files x seven variants");
    for v in &all {
        let what = format!("{} {}", v.file, v.variant);
        let mut bytes = read(&format!("conformance/jp2/{}", v.file));
        let (start, end) = profile_span(&bytes);
        patch(&mut bytes[start..end], v.intent, v.white);
        let image = decode_with(&bytes, &srgb()).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(
            (image.width, image.height),
            (v.width, v.height),
            "{what}: size"
        );
        check_planes(&image, &v.planes, v.bit_depth, &what);
    }
}

/// The unpatched intent-0 variant is the file itself, so the oracle also
/// covers the conformance files as shipped.
#[test]
fn the_intent_0_variants_are_the_files_as_shipped() {
    for v in variants().iter().filter(|v| v.variant == "intent0") {
        let bytes = read(&format!("conformance/jp2/{}", v.file));
        let (start, _) = profile_span(&bytes);
        assert_eq!(
            u32::from_be_bytes(bytes[start + 64..start + 68].try_into().unwrap()),
            0,
            "{}",
            v.file
        );
        let image = decode_with(&bytes, &srgb()).unwrap();
        check_planes(&image, &v.planes, v.bit_depth, &v.file);
    }
}

/// Changing the intent or white point is not a no-op for every file: absolute
/// colorimetric with a white other than D50 scales the colors, so the
/// oracle would catch an ignored `wtpt`.
#[test]
fn the_white_point_variants_differ_from_intent_3() {
    let all = variants();
    for file in ["files/file5.jp2", "files/file7.jp2", "files/file8.jp2"] {
        let of = |name: &str| {
            &all.iter()
                .find(|v| v.file == file && v.variant == name)
                .unwrap()
                .planes
        };
        for white in ["intent3_d65", "intent3_white_1", "intent3_white_2"] {
            assert_ne!(of(white), of("intent3"), "{file} {white}");
        }
    }
}

// ---------------------------------------------------------------------------
// Edge-case and malformed profiles
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profiles {
    #[expect(dead_code)]
    provenance: serde_json::Value,
    codestreams: std::collections::BTreeMap<String, CodestreamSource>,
    cases: Vec<ProfileCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CodestreamSource {
    file: String,
    #[expect(dead_code)]
    command: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileCase {
    name: String,
    codestream: String,
    profile: String,
    openjpeg: String,
    #[serde(default)]
    planes: Vec<String>,
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// A JP2 file around `codestream` whose `colr` box carries `profile`.
fn wrap(codestream: &[u8], profile: &[u8]) -> Vec<u8> {
    let header = rust_j2k::read_header(codestream).expect("codestream header");
    let mut ihdr = header.height.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&header.width.to_be_bytes());
    ihdr.extend_from_slice(&(header.components.len() as u16).to_be_bytes());
    ihdr.extend_from_slice(&[header.components[0].bit_depth - 1, 7, 0, 0]);
    let mut colr = vec![2, 0, 0];
    colr.extend_from_slice(profile);
    let mut jp2h = bx(b"ihdr", &ihdr);
    jp2h.extend(bx(b"colr", &colr));
    let mut out = b"\x00\x00\x00\x0cjP  \r\n\x87\n".to_vec();
    out.extend(bx(b"ftyp", b"jp2 \0\0\0\0jp2 "));
    out.extend(bx(b"jp2h", &jp2h));
    out.extend(bx(b"jp2c", codestream));
    out
}

/// Where the decoder refuses a profile, and why. OpenJPEG converts each but
/// one. LittleCMS applies a monochrome profile to the first of three channels'
/// worth of input; the others are forms this step does not apply: a lookup
/// table LittleCMS reads, and a matrix/TRC profile with the Lab connection
/// space. Where LittleCMS refuses the profile and this step can tell, the
/// samples pass through as in OpenJPEG.
const REFUSED_BY_US: &[(&str, &str)] = &[
    ("bad_gray_on_rgb", "monochrome ICC profile on 3 channels"),
    ("lut16", "`A2B0` lookup table"),
    ("lut16_grid_0", "`A2B0` lookup table"),
    ("lut8", "`A2B0` lookup table"),
    ("class_link_lut16", "`A2B0` lookup table"),
    ("class_abst_lut16", "`A2B0` lookup table"),
    ("a2b1_lut16_intent_1", "`A2B1` lookup table"),
    ("d2b0_junk_a2b1_lut16_intent_1", "`A2B1` lookup table"),
    ("pcs_lab", "`Lab ` connection space"),
    ("lut_mab_empty", "`A2B0` lookup table"),
    // LittleCMS refuses this one too, on its elements, which this step
    // does not read.
    ("d2b0_mpet_empty", "`D2B0` lookup table"),
    ("gray_lut16", "`A2B0` lookup table"),
    ("gray_pcs_lab", "`Lab ` connection space"),
];

/// Every profile OpenJPEG converts gives the same samples here. Every
/// profile LittleCMS refuses, which OpenJPEG then leaves as coded, gives the
/// samples as coded with the profile still in the metadata, and
/// `read_header_with` says so. The exceptions are [`REFUSED_BY_US`].
#[test]
fn icc_profiles_match_openjpeg() {
    let text = String::from_utf8(read("icc/profiles.json")).unwrap();
    let profiles: Profiles = serde_json::from_str(&text).expect("icc/profiles.json");
    let codestreams: std::collections::BTreeMap<&str, Vec<u8>> = profiles
        .codestreams
        .iter()
        .map(|(k, v)| (k.as_str(), read(&v.file)))
        .collect();
    let (mut converted, mut as_coded, mut refused) = (0, 0, 0);
    for case in &profiles.cases {
        let what = format!("{} {}", case.codestream, case.name);
        let codestream = &codestreams[case.codestream.as_str()];
        let depth = rust_j2k::read_header(codestream).unwrap().components[0].bit_depth;
        let bytes = wrap(codestream, &hex(&case.profile));
        let result = decode_with(&bytes, &srgb());
        let ours_refused = REFUSED_BY_US.iter().find(|(name, _)| *name == case.name);
        match (case.openjpeg.as_str(), result, ours_refused) {
            ("converted", Ok(image), None) => {
                check_planes(&image, &case.planes, depth, &what);
                converted += 1;
            }
            ("unconverted", Ok(image), None) => {
                let coded = decode(&bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
                assert_eq!(image.components, coded.components, "{what}: as coded");
                assert_eq!(image.jp2, coded.jp2, "{what}: metadata as declared");
                assert!(
                    matches!(
                        image.jp2.as_ref().unwrap().color,
                        ColorSpace::RestrictedIcc(_)
                    ),
                    "{what}"
                );
                let header = rust_j2k::read_header_with(&bytes, &srgb())
                    .unwrap_or_else(|e| panic!("{what}: {e}"));
                assert_eq!(
                    header,
                    rust_j2k::read_header(&bytes).unwrap(),
                    "{what}: header"
                );
                as_coded += 1;
            }
            (_, Err(e), Some((_, fragment))) => {
                assert!(matches!(e, Error::Unsupported(_)), "{what}: {e:?}");
                assert!(e.to_string().contains(fragment), "{what}: {e}");
                refused += 1;
            }
            (outcome, result, _) => panic!("{what}: OpenJPEG {outcome}, here {result:?}"),
        }
    }
    // RGB profiles at 8 and 16 bits: 24 malformed, 39 edge-case, 15 header,
    // 25 LittleCMS-reading and 58 class and lookup table ones. Gray: 10
    // edge-case, 4 LittleCMS-reading and 13 class and lookup table ones.
    // OpenJPEG leaves 13 of the malformed, 8 of the header, 11 of the
    // LittleCMS-reading and 37 of the class and lookup table RGB ones as
    // coded, and 3 + 7 gray ones. 11 RGB ones (one of them left as coded by
    // OpenJPEG) and 2 gray ones are
    // refused here.
    assert_eq!(
        profiles.cases.len(),
        2 * (24 + 39 + 15 + 25 + 58) + 10 + 4 + 13
    );
    assert_eq!(refused, 2 * 11 + 2);
    assert_eq!(as_coded, 2 * (13 + 8 + 11 + 37 - 1) + 3 + 7);
    assert_eq!(converted, profiles.cases.len() - refused - as_coded);
}

// ---------------------------------------------------------------------------
// Fewer than three channels
// ---------------------------------------------------------------------------

/// The ISO file5 profile (RGB, gamma 1.8) from the committed conformance file.
fn file5_profile() -> Vec<u8> {
    let bytes = read("conformance/jp2/files/file5.jp2");
    let (start, end) = profile_span(&bytes);
    bytes[start..end].to_vec()
}

/// A JP2 file around `codestream` with a `colr` of enumerated space 18
/// (sYCC).
fn wrap_sycc(codestream: &[u8]) -> Vec<u8> {
    let mut bytes = wrap(codestream, &[]);
    // `wrap` wrote method 2 with no profile: make it method 1, space 18. The
    // colr box grows by four bytes, and with it jp2h.
    let (start, _) = profile_span(&bytes);
    let colr_at = start - 3;
    bytes[colr_at] = 1;
    bytes.splice(start..start, 18u32.to_be_bytes());
    for header in [colr_at - 8, 32] {
        let length = u32::from_be_bytes(bytes[header..header + 4].try_into().unwrap()) + 4;
        bytes[header..header + 4].copy_from_slice(&length.to_be_bytes());
    }
    bytes
}

/// sYCC and an RGB profile on one or two channels pass through as OpenJPEG
/// leaves them: the samples and the metadata are those of the default decode,
/// which describe the file as declared.
#[test]
fn sycc_and_rgb_profiles_on_fewer_than_three_channels_pass_through() {
    let one = read("multilayer_lossless.j2k");
    let two = read("cblksty_bypass_lossless.j2k");
    for (codestream, channels) in [(&one, 1), (&two, 2)] {
        for (bytes, declared) in [
            (wrap_sycc(codestream), ColorSpace::Sycc),
            (
                wrap(codestream, &file5_profile()),
                ColorSpace::RestrictedIcc(file5_profile()),
            ),
        ] {
            let what = format!("{channels} channels, {declared:?}");
            let what = &what[..what.len().min(60)];
            let coded = decode(&bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
            let image = decode_with(&bytes, &srgb()).unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(image.components.len(), channels, "{what}");
            assert_eq!(image.components, coded.components, "{what}");
            let jp2 = image.jp2.as_ref().unwrap();
            assert_eq!(jp2.color, declared, "{what}");
            assert_eq!(jp2, coded.jp2.as_ref().unwrap(), "{what}");
        }
    }
}

/// A monochrome profile whose output is not neutral (absolute colorimetric
/// with a tinted white point) gives R, G and B in place of the gray channel,
/// and the metadata says so: three color channels, sRGB.
#[test]
fn a_tinted_gray_profile_gives_three_color_channels() {
    let v = variants()
        .into_iter()
        .find(|v| v.file == "files/file8.jp2" && v.variant == "intent3_d65")
        .unwrap();
    let mut bytes = read("conformance/jp2/files/file8.jp2");
    let (start, end) = profile_span(&bytes);
    patch(&mut bytes[start..end], v.intent, v.white);
    let image = decode_with(&bytes, &srgb()).unwrap();
    assert_eq!(image.components.len(), 3);
    let jp2 = image.jp2.as_ref().unwrap();
    assert_eq!(jp2.color, ColorSpace::Srgb);
    if !jp2.channels.is_empty() {
        let colors: Vec<_> = jp2
            .channels
            .iter()
            .map(|c| (c.kind, c.association))
            .collect();
        assert_eq!(
            colors,
            [1, 2, 3].map(|n| (ChannelType::Color, Association::Color(n)))
        );
    }
}

/// Two components, a monochrome ICC profile and no `cdef`: OpenJPEG reads the
/// second component as opacity, converts the first, and writes RGBA. The
/// second component is not a color channel (I.5.3.6 gives a channel past the
/// color space's count no meaning), so the sRGB step converts the first and
/// returns the second as coded, as OpenJPEG does.
///
/// The file is `cblksty_bypass_lossless.j2k` (24x16, 8-bit, two components)
/// under the `gray_gamma18` (neutral output) and `gray_white_d65` (tinted
/// output) profiles from `icc/profiles.json`. The hashes are the planes of
/// `opj_decompress -force-rgb` on it (OpenJPEG 2.5.0, LittleCMS 2.14), made
/// as `scripts/gen-icc-oracles.py` makes its own.
#[test]
fn a_gray_profile_on_two_channels_without_cdef_is_gray_plus_opacity() {
    const OPACITY: &str = "db57ea4486e47a9ac7d8efd8e7e1d3e0f47e7595abca5d06f6b1d1febe95d39f";
    let codestream = read("cblksty_bypass_lossless.j2k");
    let text = String::from_utf8(read("icc/profiles.json")).unwrap();
    let profiles: Profiles = serde_json::from_str(&text).expect("profiles.json");
    let cases: [(&str, &[&str]); 2] = [
        (
            "gray_gamma18",
            &["e11e2a6e7b6302d7e67b0617d5df678c2318fe0fffb97f91e684c8fab377ac48"],
        ),
        (
            "gray_white_d65",
            &[
                "7df53cf1559167d5f754b7694f3a8bbc25558f007a54e30d447decd1f4a59175",
                "a9b13cb5073c0b8fe73cef96492fa3c13bf8341789d15554edca8a0c747b7605",
                "d6641bf952c5f0a3b3af487eaeb0a7ed4451a655d673968b868eeed8e1a04c61",
            ],
        ),
    ];
    let coded = decode(&codestream).unwrap();
    assert_eq!(coded.components.len(), 2);
    assert_eq!(plane_hash(&coded.components[1]), OPACITY, "the opacity");
    for (name, colors) in cases {
        let case = profiles.cases.iter().find(|c| c.name == name).unwrap();
        let bytes = wrap(&codestream, &hex(&case.profile));
        let image = decode_with(&bytes, &srgb()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let got: Vec<String> = image.components.iter().map(plane_hash).collect();
        let mut want: Vec<String> = colors.iter().map(|h| h.to_string()).collect();
        want.push(OPACITY.to_string());
        assert_eq!(got, want, "{name}");
        let header = rust_j2k::read_header_with(&bytes, &srgb()).unwrap();
        assert_eq!(header.components.len(), want.len(), "{name}: header");
        let labels: Vec<_> = image
            .jp2
            .as_ref()
            .unwrap()
            .channels
            .iter()
            .map(|c| (c.kind, c.association))
            .collect();
        let mut expect: Vec<_> = (1..=colors.len() as u16)
            .map(|n| (ChannelType::Color, Association::Color(n)))
            .collect();
        expect.push((ChannelType::Unspecified, Association::None));
        assert_eq!(labels, expect, "{name}: labels");
        assert_eq!(header.jp2.unwrap(), image.jp2.unwrap(), "{name}");
    }
}

// ---------------------------------------------------------------------------
// OpenJPEG's AFL test
// ---------------------------------------------------------------------------

/// `Ssiz` for 8-bit signed and for 12-bit unsigned samples.
const SIGNED_8: u8 = 0x87;
const UNSIGNED_12: u8 = 0x0b;

/// `codestream` with component `k`'s `Ssiz` set to `ssiz`.
fn with_ssiz(codestream: &[u8], k: usize, ssiz: u8) -> Vec<u8> {
    let mut out = codestream.to_vec();
    out[42 + 3 * k] = ssiz;
    out
}

/// The codestream of `jp2_cdef_rgba.jp2`: 16x12, four 8-bit components.
fn rgba() -> Vec<u8> {
    let bytes = read("jp2_cdef_rgba.jp2");
    let (_, start, end) = boxes(&bytes, 12, bytes.len())
        .into_iter()
        .find(|(kind, _, _)| kind == b"jp2c")
        .unwrap();
    bytes[start..end].to_vec()
}

/// A `cdef` box of (`Cn`, `Typ`, `Asoc`) entries.
fn cdef(entries: &[(u16, u16, u16)]) -> Vec<u8> {
    let mut payload = (entries.len() as u16).to_be_bytes().to_vec();
    for &(cn, typ, asoc) in entries {
        for v in [cn, typ, asoc] {
            payload.extend_from_slice(&v.to_be_bytes());
        }
    }
    bx(b"cdef", &payload)
}

/// Colors 1, 2, 3 and an opacity channel for the whole image.
const RGBA_CDEF: &[(u16, u16, u16)] = &[(0, 0, 1), (1, 0, 2), (2, 0, 3), (3, 1, 0)];

/// A JP2 file around `codestream` with `profile` in its `colr` and the
/// `children` after it. Components that differ in depth or sign get `ihdr`
/// BPC 255 and a `bpcc` box.
fn wrap_with(codestream: &[u8], profile: &[u8], children: &[Vec<u8>]) -> Vec<u8> {
    let header = rust_j2k::read_header(codestream).expect("codestream header");
    let bpc: Vec<u8> = header
        .components
        .iter()
        .map(|c| (c.bit_depth - 1) | if c.signed { 0x80 } else { 0 })
        .collect();
    let uniform = bpc.iter().all(|&b| b == bpc[0]);
    let mut ihdr = header.height.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&header.width.to_be_bytes());
    ihdr.extend_from_slice(&(bpc.len() as u16).to_be_bytes());
    ihdr.extend_from_slice(&[if uniform { bpc[0] } else { 255 }, 7, 0, 0]);
    let mut jp2h = bx(b"ihdr", &ihdr);
    if !uniform {
        jp2h.extend(bx(b"bpcc", &bpc));
    }
    let mut colr = vec![2, 0, 0];
    colr.extend_from_slice(profile);
    jp2h.extend(bx(b"colr", &colr));
    jp2h.extend(children.concat());
    let mut out = b"\x00\x00\x00\x0cjP  \r\n\x87\n".to_vec();
    out.extend(bx(b"ftyp", b"jp2 \0\0\0\0jp2 "));
    out.extend(bx(b"jp2h", &jp2h));
    out.extend(bx(b"jp2c", codestream));
    out
}

/// A profile from `icc/profiles.json`.
fn named_profile(name: &str) -> Vec<u8> {
    let text = String::from_utf8(read("icc/profiles.json")).unwrap();
    let profiles: Profiles = serde_json::from_str(&text).expect("profiles.json");
    let case = profiles.cases.iter().find(|c| c.name == name).unwrap();
    hex(&case.profile)
}

/// `Srgb` gives OpenJPEG's planes (`want`): as coded, with the profile still
/// in the metadata, or converted to sRGB. `read_header_with` describes the
/// decode.
#[track_caller]
fn check_afl(what: &str, bytes: &[u8], want: &[&str], converted: bool) {
    let image = decode_with(bytes, &srgb()).unwrap_or_else(|e| panic!("{what}: {e}"));
    let got: Vec<String> = image.components.iter().map(plane_hash).collect();
    assert_eq!(got, want, "{what}: OpenJPEG's planes");
    let coded = decode(bytes).unwrap();
    let jp2 = image.jp2.as_ref().unwrap();
    if converted {
        assert_ne!(image.components, coded.components, "{what}: converted");
        assert_eq!(jp2.color, ColorSpace::Srgb, "{what}");
    } else {
        assert_eq!(image.components, coded.components, "{what}: as coded");
        assert_eq!(image.jp2, coded.jp2, "{what}: metadata as declared");
        assert!(
            matches!(jp2.color, ColorSpace::RestrictedIcc(_)),
            "{what}: the profile stays"
        );
    }
    let header =
        rust_j2k::read_header_with(bytes, &srgb()).unwrap_or_else(|e| panic!("{what}: {e}"));
    assert_eq!(header.jp2, image.jp2, "{what}: header");
    assert_eq!(header.components.len(), image.components.len(), "{what}");
    for (info, c) in header.components.iter().zip(&image.components) {
        assert_eq!(
            (info.bit_depth, info.signed, info.width, info.height),
            (c.bit_depth, c.signed, c.width, c.height),
            "{what}: header"
        );
    }
}

/// OpenJPEG's planes for the files below (`opj_decompress -i <file>.jp2 -o
/// out.pgx`). `RGB_AS_CODED` is `jp2_cdef_rgba`'s first three planes.
const RGB_AS_CODED: [&str; 3] = [
    "9440fef292cd0d865bc9d98c84ef8a648e5cc79bc95bf54857f32b3fb207722a",
    "70be7146623f03e79146c2b179778ffa52c2cabfd1674839bd9115c326450c9b",
    "d086834ee4d5cc3b7340601f3ec141c0adae87e3fb4d340ef74955e5feb17946",
];
const ALPHA_SIGNED: &str = "2f2e0062b7ebc44c2b561ae57c532497543f930e791a9d5dfbd9da60d782fa0b";
const ALPHA_12BIT: &str = "75c0a15f6593760f509b49ceadd501069525dde012326f905a88c40c8f6a0fb3";
/// The planes of `channels/fourth_2x1.j2k`, `fourth_1x2.j2k` and
/// `third_subsampled.j2k`: the first two are the same in all three.
const SUB_FIRST_TWO: [&str; 2] = [
    "163863672477216de38d1f8a5eae8c60d7935221feb4ee9f553f7de0e65be449",
    "ecb883240d00e26c9c4fb5bfb5deb157b30ce8c0c00d4094e43b5d9018460fef",
];
const SUB_THIRD_FULL: &str = "ba46435cccec6c13d0c14059632e6fdfa56fdcb2d402923de4be2021b1d5ea82";
const SUB_FOURTH_2X1: &str = "7118c7e749c7ac2aa28c95e8ac7a67ecc5be5751d1c710745acf9cd7f02ccfaf";
const SUB_FOURTH_1X2: &str = "41042b94f4e10308a50bf2a1d64b0b95948767a7c0aec8f229bd631f994a3f20";
const SUB_THIRD_2X2: &str = "491b6467dd9f3ae58e2482dc8ae9689dc252c52dd425c4e08f4a65a8ff3447e8";
/// `multicomponent_lossless.j2k` with its third component signed.
const BLUE_SIGNED: [&str; 3] = [
    "26c630b1696c1ce4fa7163e35ab23b2d17d05e0cd47c0a5b2c7d344c3feb37be",
    "db57ea4486e47a9ac7d8efd8e7e1d3e0f47e7595abca5d06f6b1d1febe95d39f",
    "2b370743c4c9efdfe3a484562f3469dde33d01d7fc0efe40b0eea5d9decf97f4",
];

/// OpenJPEG's AFL test (`color.c:495-519` in 2.5.0): before it builds the
/// transform for an RGB profile, `opj_decompress` compares the first
/// `min(numcomps, 4)` components with the first by sub-sampling, depth and
/// sign, and writes the samples as coded if any differs. `Srgb` does the
/// same, with a `cdef` and without one.
///
/// The hashes are the planes of `opj_decompress -i <file>.jp2 -o out.pgx`
/// (OpenJPEG 2.5.0, LittleCMS 2.14) on each file built here; on every
/// pass-through file `-force-rgb` fails, since nothing was converted.
#[test]
fn an_rgb_profile_passes_through_when_the_first_four_channels_differ() {
    let rgb = file5_profile();
    let with_cdef = [cdef(RGBA_CDEF)];
    let signed = with_ssiz(&rgba(), 3, SIGNED_8);
    let twelve = with_ssiz(&rgba(), 3, UNSIGNED_12);
    let sub_x = read("channels/fourth_2x1.j2k");
    let sub_y = read("channels/fourth_1x2.j2k");
    let blue = with_ssiz(&read("multicomponent_lossless.j2k"), 2, SIGNED_8);
    let third = read("channels/third_subsampled.j2k");
    let rgb_and = |last| [RGB_AS_CODED[0], RGB_AS_CODED[1], RGB_AS_CODED[2], last];
    let sub_and = |last| [SUB_FIRST_TWO[0], SUB_FIRST_TWO[1], SUB_THIRD_FULL, last];
    let cases: Vec<(&str, Vec<u8>, [&str; 4])> = vec![
        (
            "alpha_signed_cdef",
            wrap_with(&signed, &rgb, &with_cdef),
            rgb_and(ALPHA_SIGNED),
        ),
        (
            "alpha_signed",
            wrap_with(&signed, &rgb, &[]),
            rgb_and(ALPHA_SIGNED),
        ),
        (
            "alpha_12bit_cdef",
            wrap_with(&twelve, &rgb, &with_cdef),
            rgb_and(ALPHA_12BIT),
        ),
        (
            "alpha_12bit",
            wrap_with(&twelve, &rgb, &[]),
            rgb_and(ALPHA_12BIT),
        ),
        (
            "alpha_2x1_cdef",
            wrap_with(&sub_x, &rgb, &with_cdef),
            sub_and(SUB_FOURTH_2X1),
        ),
        (
            "alpha_2x1",
            wrap_with(&sub_x, &rgb, &[]),
            sub_and(SUB_FOURTH_2X1),
        ),
        (
            "alpha_1x2_cdef",
            wrap_with(&sub_y, &rgb, &with_cdef),
            sub_and(SUB_FOURTH_1X2),
        ),
        (
            "alpha_1x2",
            wrap_with(&sub_y, &rgb, &[]),
            sub_and(SUB_FOURTH_1X2),
        ),
    ];
    for (what, bytes, want) in cases {
        check_afl(what, &bytes, &want, false);
    }
    // On three channels the test covers the colors themselves.
    check_afl(
        "blue_signed",
        &wrap_with(&blue, &rgb, &[]),
        &BLUE_SIGNED,
        false,
    );
    check_afl(
        "blue_subsampled",
        &wrap_with(&third, &rgb, &[]),
        &[SUB_FIRST_TWO[0], SUB_FIRST_TWO[1], SUB_THIRD_2X2],
        false,
    );
}

/// The AFL test comes before LittleCMS builds the transform, so before every
/// check this decoder makes on the profile and the channels: a lookup-table
/// or Lab profile (refused here, converted by OpenJPEG when the channels
/// match) and a `cdef` of two colors (refused here) pass through when the
/// fourth channel differs, as in `opj_decompress`.
#[test]
fn the_afl_test_comes_before_this_decoders_refusals() {
    let signed = with_ssiz(&rgba(), 3, SIGNED_8);
    let two_colors = [cdef(&[(0, 0, 1), (1, 0, 2), (2, 1, 0), (3, 1, 0)])];
    let signed_planes = [
        RGB_AS_CODED[0],
        RGB_AS_CODED[1],
        RGB_AS_CODED[2],
        ALPHA_SIGNED,
    ];
    let cases: Vec<(&str, Vec<u8>, &[&str])> = vec![
        (
            "order_lut16",
            wrap_with(&signed, &named_profile("lut16"), &[cdef(RGBA_CDEF)]),
            &signed_planes,
        ),
        (
            "order_lab",
            wrap_with(&signed, &named_profile("pcs_lab"), &[cdef(RGBA_CDEF)]),
            &signed_planes,
        ),
        (
            "order_two_colors",
            wrap_with(&signed, &file5_profile(), &two_colors),
            &signed_planes,
        ),
    ];
    for (what, bytes, want) in cases {
        check_afl(what, &bytes, want, false);
    }
    // The same profiles and `cdef` on the unmodified file are refused.
    let rgba = rgba();
    for (bytes, fragment) in [
        (
            wrap_with(&rgba, &named_profile("lut16"), &[cdef(RGBA_CDEF)]),
            "`A2B0` lookup table",
        ),
        (
            wrap_with(&rgba, &named_profile("pcs_lab"), &[cdef(RGBA_CDEF)]),
            "`Lab ` connection space",
        ),
        (
            wrap_with(&rgba, &file5_profile(), &two_colors),
            "needs color channels 1, 2 and 3",
        ),
    ] {
        match decode_with(&bytes, &srgb()) {
            Err(Error::Unsupported(e)) => assert!(e.contains(fragment), "{e}"),
            other => panic!("expected {fragment:?}, got {other:?}"),
        }
    }
}

/// Where the first four channels match, an RGB profile still converts, as
/// in `opj_decompress`: a fifth channel is not compared, and a monochrome or
/// YCbCr profile has no AFL test.
#[test]
fn the_afl_test_covers_only_rgb_profiles_and_four_channels() {
    let rgb = file5_profile();
    let fifth = read("channels/fifth_subsampled.j2k");
    let five_cdef = [cdef(&[
        (0, 0, 1),
        (1, 0, 2),
        (2, 0, 3),
        (3, 1, 0),
        (4, 1, 0),
    ])];
    let gray_alpha = with_ssiz(&read("cblksty_bypass_lossless.j2k"), 1, SIGNED_8);
    let rgba_planes: &[&str] = &[
        "2e04f36cb26f7c503ac424f3cbb48e2286722ef39dd47f0bf1df96c2326e8cc8",
        "51d9b1ebd7d82216021780370e40ef2423bd76c1f402716ee712c8cde32270dc",
        "038369c2180cfe468c41ce30b740f6e7c66d60c12347a7624f80d63e27f77cd5",
        "f4baf2f8962309bd8a60f09e6d1805e0b5a7304204d7f5bb6be6840f1aa4f2d2",
    ];
    let fifth_planes: &[&str] = &[
        "2a3416ab77f30611a65e8b770e36b931e9c8afaee64490db4621567de763ea4a",
        "07baf36ea076eeab7dd3bd9d206f9a1a214aa08d2b5dd60043eec1153f324193",
        "b224e86099ae1c19c22241db832778e0d96e190f915255492ea6064b22617088",
        "b225dd0389766a26347110bf08fa34fab80c3532f2d8a69ac928ee53390ef6ff",
        "b88be758de874868741d0e190f497b1ad9862ee3618060394fb14ef47aadcaa0",
    ];
    let cases: Vec<(&str, Vec<u8>, &[&str])> = vec![
        (
            "rgba_cdef",
            wrap_with(&rgba(), &rgb, &[cdef(RGBA_CDEF)]),
            rgba_planes,
        ),
        ("rgba", wrap_with(&rgba(), &rgb, &[]), rgba_planes),
        (
            "fifth_subsampled_cdef",
            wrap_with(&fifth, &rgb, &five_cdef),
            fifth_planes,
        ),
        (
            "fifth_subsampled",
            wrap_with(&fifth, &rgb, &[]),
            fifth_planes,
        ),
        (
            "gray_alpha_signed",
            wrap_with(&gray_alpha, &named_profile("gray_white_d65"), &[]),
            &[
                "7df53cf1559167d5f754b7694f3a8bbc25558f007a54e30d447decd1f4a59175",
                "a9b13cb5073c0b8fe73cef96492fa3c13bf8341789d15554edca8a0c747b7605",
                "d6641bf952c5f0a3b3af487eaeb0a7ed4451a655d673968b868eeed8e1a04c61",
                "0fb095b5690d1fbaaccc9cb421bbdf7f73cd186779e5fc642942a0e27aeef7a7",
            ],
        ),
    ];
    for (what, bytes, want) in cases {
        check_afl(what, &bytes, want, true);
    }
    // A YCbCr profile is refused here whatever the fourth channel is;
    // OpenJPEG converts it at 16 bits and crashes at 8.
    let mut ycbr = rgb.clone();
    ycbr[16..20].copy_from_slice(b"YCbr");
    let signed = with_ssiz(&rgba(), 3, SIGNED_8);
    match decode_with(&wrap_with(&signed, &ycbr, &[cdef(RGBA_CDEF)]), &srgb()) {
        Err(Error::Unsupported(e)) => assert!(e.contains("`YCbr` color space"), "{e}"),
        other => panic!("expected the YCbCr refusal, got {other:?}"),
    }
}

/// `opj_decompress` guesses sYCC for three components whose second is
/// sub-sampled in x and whose first has equal x and y sampling
/// (`opj_decompress.c:1620-1623`), but `color_sycc_to_rgb` converts only
/// 4:2:0 and 4:2:2 (`color.c:403-436`). Any other layout of that shape stays
/// as coded ("CAN NOT CONVERT"), and the AFL test then applies under an RGB
/// profile: differing channels pass through, matching ones convert. Luma
/// 1x1 with chroma 1x2 is outside the guess and passes through the same way.
///
/// The files are `tests/fixtures/channels/` under the `gamma22` profile; the
/// hashes are the planes of `opj_decompress -i <file>.jp2 -o out.pgx`
/// (OpenJPEG 2.5.0, LittleCMS 2.14).
#[test]
fn the_afl_test_applies_where_openjpeg_cannot_convert_as_sycc() {
    let gamma22 = named_profile("gamma22");
    let chroma_2x1 = "793eaf33d2401874f1374fcb9c202a779b9bb7f6c7161e7282d471fd11f65837";
    let chroma_2x2 = "dbeefc0ddd114a693ba6780d1dabbf6b896fa54f8635c73781d5e1f11e49fd3c";
    let third_2x1 = "8b2d6903d906f83dd922d6155e51a8e1bdff5fd3a849624e7250b21fadcec780";
    let luma = SUB_FIRST_TWO[0];
    let passes: [(&str, [&str; 3]); 5] = [
        ("second_2x1", [luma, chroma_2x1, SUB_THIRD_FULL]),
        ("second_2x2", [luma, chroma_2x2, SUB_THIRD_FULL]),
        ("chroma_2x1_2x2", [luma, chroma_2x1, SUB_THIRD_2X2]),
        (
            "luma_2x2_chroma_2x1",
            [
                "3758e20c68c36e6f709428bd5035be88cbdef35b271b390b87af88f9d2ca8fec",
                chroma_2x1,
                third_2x1,
            ],
        ),
        (
            "chroma_1x2",
            [
                luma,
                "2ed6fe30a075b5df385a56fa3bdf5d86004780afa7e61bb3c5d1ef664302bd8a",
                "0091f5217fa8e0132918d579655b396a92fdd3bc5ffa0422e7f00192c32bea48",
            ],
        ),
    ];
    for (name, want) in passes {
        let bytes = wrap_with(&read(&format!("channels/{name}.j2k")), &gamma22, &[]);
        check_afl(name, &bytes, &want, false);
    }
    check_afl(
        "all_2x2",
        &wrap_with(&read("channels/all_2x2.j2k"), &gamma22, &[]),
        &[
            "8387e5c6cf63767228725b93753e558326450bf7bf261a1a475be7d247174fcb",
            "453b53b1beaad85b3d7d8f1bae1c43a59109921a490b794b0280c7de07c22477",
            "86c6b642679e2be8a9ce5370e1fe49e60b120aa9291d5b182174b4ef69f15ead",
        ],
        true,
    );
}

/// Under `Srgb` the color channels are the ones the `cdef` box labels color
/// 1, 2 and 3, wherever they sit after it. This `cdef` names channel 2 as
/// color 3 and then as opacity, so the output keeps the coded order with
/// color 3 last; the decode converts planes 0, 1 and 3 and leaves the
/// opacity plane as coded. `opj_decompress -force-rgb` converts the first
/// three positions instead, planes 0, 1 and 2 (`docs/correctness.md`).
///
/// The oracle is the same codestream with a plain `cdef` that puts color 3
/// third: OpenJPEG converts planes 0, 1 and 3 there too. The hashes are its
/// planes (`opj_decompress -i <file>.jp2 -o out.pgx`, OpenJPEG 2.5.0,
/// LittleCMS 2.14); the label-based decode gives the same planes, with the
/// last two in the order of its `cdef`.
#[test]
fn srgb_picks_the_color_channels_by_their_cdef_labels() {
    let profile = named_profile("gamma22");
    let plain = wrap_with(
        &rgba(),
        &profile,
        &[cdef(&[(0, 0, 1), (1, 0, 2), (3, 0, 3), (2, 1, 0)])],
    );
    let red = "04ac395e065c497368f588c30ee66ac3f6291583efb49ac2ff2d7049f9b7d12f";
    let green = "f9661fc23e1f3a3c8bd923c2abc2d53dc7f52aac495368995f45578301e827b9";
    let blue = "68c126e6492ec4df1be9d8c4883edc386e675c9b27e0175aeb22353bad9a6135";
    let opacity = "d086834ee4d5cc3b7340601f3ec141c0adae87e3fb4d340ef74955e5feb17946";
    check_afl("plain cdef", &plain, &[red, green, blue, opacity], true);

    let relabeled = wrap_with(
        &rgba(),
        &profile,
        &[cdef(&[
            (0, 0, 1),
            (1, 0, 2),
            (3, 0, 3),
            (2, 0, 3),
            (2, 1, 0),
        ])],
    );
    check_afl(
        "relabeled cdef",
        &relabeled,
        &[red, green, opacity, blue],
        true,
    );
    let image = decode_with(&relabeled, &srgb()).unwrap();
    let color = |n| ChannelDefinition {
        kind: ChannelType::Color,
        association: Association::Color(n),
    };
    let alpha = ChannelDefinition {
        kind: ChannelType::Opacity,
        association: Association::WholeImage,
    };
    assert_eq!(
        image.jp2.unwrap().channels,
        [color(1), color(2), alpha, color(3)]
    );
}
