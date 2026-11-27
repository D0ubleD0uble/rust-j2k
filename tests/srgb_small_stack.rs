//! sRGB output on a thread with a small stack.
//!
//! Building a matrix/shaper transform used to put a 98 KB table on the
//! stack before moving it to the heap, which aborts the process on a small
//! stack (a 64 KiB runtime, wasm). Every 8-bit ICC profile case that
//! OpenJPEG converts must decode on such a thread, with the same samples as
//! on a normal one.

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

/// The decoder needs about 25 KiB here; 64 KiB leaves headroom and is the
/// smallest stack the issue names.
const STACK: usize = 64 * 1024;

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
}
