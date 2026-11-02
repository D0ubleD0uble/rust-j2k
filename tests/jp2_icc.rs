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
    Association, ChannelType, ColourOutput, ColourSpace, Component, DecodeOptions, Error, Image,
    decode, decode_with,
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
    let mut options = DecodeOptions::default();
    options.colour = ColourOutput::Srgb;
    options
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
/// whose output is neutral comes out as one grey channel, which must then
/// equal each of OpenJPEG's three identical planes.
fn check_planes(image: &Image, want: &[String], depth: u8, what: &str) {
    assert_eq!(want.len(), 3, "{what}: OpenJPEG gives R, G, B");
    let got: Vec<String> = image.components.iter().map(plane_hash).collect();
    match got.len() {
        1 => {
            assert!(
                want.iter().all(|w| *w == got[0]),
                "{what}: one grey channel, but OpenJPEG's planes differ or do not match it"
            );
            assert_eq!(
                image.jp2.as_ref().unwrap().colour,
                ColourSpace::Greyscale,
                "{what}"
            );
        }
        3 => {
            for (k, (g, w)) in got.iter().zip(want).enumerate() {
                assert_eq!(g, w, "{what}: channel {k} differs from OpenJPEG");
            }
            assert_eq!(
                image.jp2.as_ref().unwrap().colour,
                ColourSpace::Srgb,
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
        let image = decode_with(&bytes, srgb()).unwrap_or_else(|e| panic!("{what}: {e}"));
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
        let image = decode_with(&bytes, srgb()).unwrap();
        check_planes(&image, &v.planes, v.bit_depth, &v.file);
    }
}

/// Changing the intent or white point is not a no-op for every file: absolute
/// colorimetric with a white other than D50 scales the colours, so the
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

/// Where the decoder refuses a profile that LittleCMS converts, and why. Both
/// are malformed: LittleCMS reads past the end of the tag, or applies a
/// monochrome profile to the first of three channels' worth of input.
const REFUSED_BY_US: &[(&str, &str)] = &[
    ("bad_xyz_short", "tag is not an `XYZ ` value"),
    (
        "bad_gray_on_rgb",
        "monochrome ICC profile needs one colour channel",
    ),
];

/// Every profile OpenJPEG converts gives the same samples here; every profile
/// LittleCMS refuses gives a typed error, never a panic. The decoder refuses
/// two malformed profiles that LittleCMS reads anyway ([`REFUSED_BY_US`]).
#[test]
fn icc_profiles_match_openjpeg_or_are_refused() {
    let text = String::from_utf8(read("icc/profiles.json")).unwrap();
    let profiles: Profiles = serde_json::from_str(&text).expect("icc/profiles.json");
    let codestreams: std::collections::BTreeMap<&str, Vec<u8>> = profiles
        .codestreams
        .iter()
        .map(|(k, v)| (k.as_str(), read(&v.file)))
        .collect();
    let (mut converted, mut refused) = (0, 0);
    for case in &profiles.cases {
        let what = format!("{} {}", case.codestream, case.name);
        let codestream = &codestreams[case.codestream.as_str()];
        let depth = rust_j2k::read_header(codestream).unwrap().components[0].bit_depth;
        let bytes = wrap(codestream, &hex(&case.profile));
        let result = decode_with(&bytes, srgb());
        let ours_refused = REFUSED_BY_US.iter().find(|(name, _)| *name == case.name);
        match (case.openjpeg.as_str(), result) {
            ("converted", Ok(image)) => {
                assert!(ours_refused.is_none(), "{what}: expected a refusal");
                check_planes(&image, &case.planes, depth, &what);
                converted += 1;
            }
            ("converted", Err(e)) => {
                let (_, fragment) = ours_refused.unwrap_or_else(|| panic!("{what}: {e}"));
                assert!(e.to_string().contains(fragment), "{what}: {e}");
                refused += 1;
            }
            ("unconverted", Err(e)) => {
                assert!(
                    matches!(e, Error::Unsupported(_) | Error::FileFormat(_)),
                    "{what}: {e:?}"
                );
                // A profile LittleCMS will not open (`_cmsReadHeader`) is
                // malformed.
                if case.name.starts_with("hdr_") {
                    assert!(matches!(e, Error::FileFormat(_)), "{what}: {e:?}");
                }
                refused += 1;
            }
            ("unconverted", Ok(_)) => {
                panic!("{what}: LittleCMS refuses this profile, but it converted")
            }
            (other, _) => panic!("{what}: unknown outcome {other}"),
        }
    }
    // 24 malformed, 39 edge-case and 15 header RGB profiles at 8 and 16 bits,
    // 10 grey. LittleCMS refuses 13 of the malformed and 8 of the header ones.
    assert_eq!(profiles.cases.len(), 2 * (24 + 39 + 15) + 10);
    assert_eq!(refused, 2 * (13 + 8 + REFUSED_BY_US.len()));
    assert_eq!(converted, profiles.cases.len() - refused);
}

/// The version in the refusal reads as major.minor.bugfix from the BCD
/// nibbles: byte 0x10 is minor 1, not 16.
#[test]
fn a_version_above_5_0_is_named_in_bcd() {
    let text = String::from_utf8(read("icc/profiles.json")).unwrap();
    let profiles: Profiles = serde_json::from_str(&text).expect("icc/profiles.json");
    let case = profiles
        .cases
        .iter()
        .find(|c| c.name == "hdr_version_5_1" && c.codestream == "rgb8")
        .expect("hdr_version_5_1");
    let codestream = read(&profiles.codestreams["rgb8"].file);
    let err = decode_with(&wrap(&codestream, &hex(&case.profile)), srgb()).unwrap_err();
    match err {
        Error::FileFormat(message) => {
            assert_eq!(message, "the ICC profile has version 5.1.0, above 5.0");
        }
        other => panic!("expected a FileFormat error, got {other:?}"),
    }
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
            (wrap_sycc(codestream), ColourSpace::Sycc),
            (
                wrap(codestream, &file5_profile()),
                ColourSpace::RestrictedIcc(file5_profile()),
            ),
        ] {
            let what = format!("{channels} channels, {declared:?}");
            let what = &what[..what.len().min(60)];
            let coded = decode(&bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
            let image = decode_with(&bytes, srgb()).unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(image.components.len(), channels, "{what}");
            assert_eq!(image.components, coded.components, "{what}");
            let jp2 = image.jp2.as_ref().unwrap();
            assert_eq!(jp2.colour, declared, "{what}");
            assert_eq!(jp2, coded.jp2.as_ref().unwrap(), "{what}");
        }
    }
}

/// A monochrome profile whose output is not neutral (absolute colorimetric
/// with a tinted white point) gives R, G and B in place of the grey channel,
/// and the metadata says so: three colour channels, sRGB.
#[test]
fn a_tinted_grey_profile_gives_three_colour_channels() {
    let v = variants()
        .into_iter()
        .find(|v| v.file == "files/file8.jp2" && v.variant == "intent3_d65")
        .unwrap();
    let mut bytes = read("conformance/jp2/files/file8.jp2");
    let (start, end) = profile_span(&bytes);
    patch(&mut bytes[start..end], v.intent, v.white);
    let image = decode_with(&bytes, srgb()).unwrap();
    assert_eq!(image.components.len(), 3);
    let jp2 = image.jp2.as_ref().unwrap();
    assert_eq!(jp2.colour, ColourSpace::Srgb);
    if !jp2.channels.is_empty() {
        let colours: Vec<_> = jp2
            .channels
            .iter()
            .map(|c| (c.kind, c.association))
            .collect();
        assert_eq!(
            colours,
            [1, 2, 3].map(|n| (ChannelType::Colour, Association::Colour(n)))
        );
    }
}
