# rust-j2k

[![CI](https://github.com/D0ubleD0uble/rust-j2k/actions/workflows/ci.yml/badge.svg)](https://github.com/D0ubleD0uble/rust-j2k/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/rust-j2k.svg)](https://crates.io/crates/rust-j2k)
[![docs.rs](https://img.shields.io/docsrs/rust-j2k)](https://docs.rs/rust-j2k)
![MSRV](https://img.shields.io/badge/rustc-1.87+-blue.svg)
![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)

A pure-Rust JPEG 2000 codec, built **GRIB2-decode-first** and aimed, over time,
at the same coverage as [OpenJPEG](https://www.openjpeg.org/) (the ISO/IEC
reference implementation), with no C dependency, so it cross-compiles cleanly to
every target. As of 2026 there is no production-grade pure-Rust JPEG 2000 codec;
this fills that gap.

## Install

```sh
cargo add rust-j2k
```

Or add it to `Cargo.toml`:

```toml
[dependencies]
rust-j2k = "0.1"
```

The crate has no runtime dependencies and no C toolchain requirement. Minimum
supported Rust version is **1.87**.

## Usage

The public surface is one function: bytes of a raw JPEG 2000 codestream in, a
decoded single-component `Image` out.

```rust
use rust_j2k::decode;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A raw JPEG 2000 codestream (Annex A): the bytes of a `.j2k` file, or the
    // §7 data section of a GRIB2 template 5.40 (`grid_jpeg`) message.
    let bytes = std::fs::read("grid.j2k")?;

    let image = decode(&bytes)?;

    println!(
        "{}x{}, {}-bit {}",
        image.width,
        image.height,
        image.bit_depth,
        if image.signed { "signed" } else { "unsigned" },
    );

    // `samples` is row-major `i32`, already DC-level-shifted and clamped to the
    // declared depth. Index directly or use the bounds-checked accessor.
    let top_left = image.sample(0, 0).expect("0,0 is in bounds");
    println!("first sample = {top_left}");

    Ok(())
}
```

Decoding never panics on malformed input; every failure is a typed `Error`. The
`Unsupported` variant specifically marks valid JPEG 2000 that falls outside the
currently decoded subset (see below), so you can tell "broken file" from "not
yet supported":

```rust
use rust_j2k::{decode, Error};

match decode(&bytes) {
    Ok(image) => { /* use image.samples … */ }
    Err(Error::Unsupported(what)) => eprintln!("outside the decoded subset: {what}"),
    Err(e) => eprintln!("decode failed: {e}"),
}
```

`samples` are raw integer values. For GRIB2 the caller un-scales them with the
template's reference value and binary/decimal scale factors; this crate stops at
the codestream's integer output and does not interpret the GRIB2 fields.

## What it decodes

This release decodes the JPEG 2000 codestream subset that GRIB2 template 5.40
(`grid_jpeg`) produces:

| Decoded | Not yet (returns `Error::Unsupported`) |
| --- | --- |
| Raw codestream (Annex A) | JP2 file format / boxes |
| One integer component, signed or unsigned, up to 32 bits | Multiple components, color transform (RCT/ICT) |
| Reversible 5/3 (lossless) and irreversible 9/7 (lossy) wavelets | — |
| Single tile, single quality layer, LRCP progression | Multiple tiles/layers, other progressions, POC |
| No precinct subdivision, no ROI | Precincts, region of interest |

Anything outside the subset is rejected cleanly, not half-decoded. The long-run
target is OpenJPEG-level coverage (full Part 1, the JP2 file format, HTJ2K, an
encoder, and the later parts); we get there by widening this same engine
outward. See [docs/roadmap.md](docs/roadmap.md) and [docs/scope.md](docs/scope.md).

## Pipeline

```text
bytes -> codestream -> tier-2 -> tier-1 -> dequant -> inverse DWT -> Image
        (markers)     (packets)  (MQ+EBCOT) (quant)    (5/3 | 9/7)
```

Each stage is a module under `src/`, and each module's docs cite the ISO section
it owns.

## Status

Phase 1 complete (v0.1.0). The GRIB2 §5.40 decode path is implemented end to end
— codestream parsing, the MQ decoder, EBCOT passes, Tier-2 packets, the inverse
DWT (5/3 and 9/7), dequantization, and image assembly — and passes the
conformance gate against the OpenJPEG/eccodes oracle: bit-exact for the
reversible 5/3 path, within tolerance for the irreversible 9/7 path.

Later phases widen this same engine toward general Part 1 decode, the JP2 file
format, HTJ2K, and an encoder. The phased plan is in
[docs/roadmap.md](docs/roadmap.md).

## Correctness

`tests/conformance.rs` is the bar: decode a corpus of real codestreams and
compare against a trusted decoder (OpenJPEG, or eccodes for the GRIB2-sourced
files). Bit-exact for the reversible path, within a stated tolerance for lossy.
The full strategy — oracle cross-check, conformance suite, golden vectors,
fuzzing — is in [docs/correctness.md](docs/correctness.md).

## Development

`cargo test` is self-contained: it grades a decode against the committed
`expected.json` snapshots and needs **no** external tools. CI runs the same way.

A reference toolchain is only needed to *(re)generate* those snapshots and to run
the extra gates — OpenJPEG and eccodes (oracles), `cargo-deny` (license/advisory
gate), and `cargo-fuzz` (robustness). Install it in one step and regenerate an
oracle with:

```sh
scripts/install-oracle-tools.sh                 # Debian/Ubuntu; prints brew steps on macOS
scripts/gen-oracle.sh tests/fixtures/sample.j2k # writes sample.expected.json
```

These tools are for oracle generation only; the test suite and CI never use them.
Versions, the macOS path, and the GRIB2 sample-mapping note are in
[docs/development.md](docs/development.md). See [CONTRIBUTING.md](CONTRIBUTING.md)
to get started and [docs/](docs/) for the design docs.

## Contributing

Contributions are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md) for the
workflow, the pure-Rust / no-C rule, and the quality gates every PR must pass.

## Security

This crate decodes untrusted binary input. To report a vulnerability, see
[SECURITY.md](SECURITY.md). It also covers the robustness contract (no panics on
malformed input) and the fuzzing that backs it.

## Why this exists

It is the decoder the fieldglass GRIB2 reader needs for template 5.40, kept in
its own crate so the codec is reusable and testable on its own, and so the
fieldglass bundle stays free of a C JPEG 2000 dependency.

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual-licensed as above, without any additional terms or conditions.
