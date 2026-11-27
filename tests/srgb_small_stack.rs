//! sRGB output on a thread with a small stack.
//!
//! The matrix/shaper transform's 98 KB table is allocated on the heap
//! directly, so it is never built on the stack. This test decodes every
//! 8-bit ICC profile case that OpenJPEG converts on a thread with a 96 KiB
//! stack, and checks it gives the same samples as on a normal thread.
//!
//! The test catches a table built on the stack only at opt-level 0. At the
//! test profile's opt-level 1 the compiler moves such a table to the heap,
//! so the test passes either way. To check for the regression, run:
//!
//! ```sh
//! CARGO_PROFILE_TEST_OPT_LEVEL=0 cargo test --test srgb_small_stack
//! ```

mod support;
use support::srgb;
use support::synth::{self, bx};

use rust_j2k::decode_with;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Profiles {
    codestreams: BTreeMap<String, Stream>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Stream {
    file: String,
}

#[derive(Deserialize)]
struct Case {
    codestream: String,
    profile: String,
    openjpeg: String,
}

/// At opt-level 0 the decoder needs 68 to 72 KiB here, and over 160 KiB
/// with the table built on the stack.
const STACK: usize = 96 * 1024;

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

/// A JP2 file around the 8-bit, three-component `codestream` with `profile`
/// as its ICC `colr` box.
fn wrap(codestream: &[u8], profile: &[u8]) -> Vec<u8> {
    let header = rust_j2k::read_header(codestream).expect("codestream header");
    let ihdr = synth::ihdr_payload(header.height, header.width, 3, [7, 7, 0, 0]);
    let boxes = [bx(b"ihdr", &ihdr), bx(b"colr", &synth::colr_icc(profile))];
    synth::jp2_file(&boxes, &[], codestream, &[])
}

#[test]
#[cfg_attr(target_os = "wasi", ignore = "WASI cannot spawn threads")]
fn icc_conversion_decodes_on_a_small_stack() {
    let root = support::synth::fixtures("");
    let text = std::fs::read_to_string(root.join("icc/profiles.json")).unwrap();
    let profiles: Profiles = serde_json::from_str(&text).unwrap();
    let streams: BTreeMap<String, Vec<u8>> = profiles
        .codestreams
        .iter()
        .map(|(k, v)| (k.clone(), std::fs::read(root.join(&v.file)).unwrap()))
        .collect();
    let files: Vec<Vec<u8>> = profiles
        .cases
        .iter()
        .filter(|c| c.openjpeg == "converted" && c.codestream == "rgb8")
        .map(|c| wrap(&streams[&c.codestream], &hex(&c.profile)))
        .collect();
    assert!(!files.is_empty());
    let want: Vec<_> = files
        .iter()
        .map(|b| {
            decode_with(b, &srgb())
                .map(|i| i.components)
                .map_err(|e| e.to_string())
        })
        .collect();
    let got = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            files
                .iter()
                .map(|b| {
                    decode_with(b, &srgb())
                        .map(|i| i.components)
                        .map_err(|e| e.to_string())
                })
                .collect::<Vec<_>>()
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(got, want);
    // Equal errors on both sides would pass vacuously. 62 of the cases build
    // a matrix/shaper transform and convert.
    let ok = got.iter().filter(|r| r.is_ok()).count();
    assert!(ok >= 60, "only {ok} cases converted");
}
