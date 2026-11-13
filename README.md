# rust-j2k

[![CI](https://github.com/D0ubleD0uble/rust-j2k/actions/workflows/ci.yml/badge.svg)](https://github.com/D0ubleD0uble/rust-j2k/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/D0ubleD0uble/rust-j2k/branch/main/graph/badge.svg)](https://codecov.io/gh/D0ubleD0uble/rust-j2k)
[![crates.io](https://img.shields.io/crates/v/rust-j2k.svg)](https://crates.io/crates/rust-j2k)
[![docs.rs](https://img.shields.io/docsrs/rust-j2k)](https://docs.rs/rust-j2k)
![MSRV](https://img.shields.io/badge/rustc-1.87+-blue.svg)
![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)

A pure-Rust JPEG 2000 codec, built **GRIB2-decode-first** and aimed, over time,
at the same coverage as [OpenJPEG](https://www.openjpeg.org/) (the ISO/IEC
reference implementation), with no C dependency, so it cross-compiles cleanly to
every target. It decodes raw codestreams and JP2 files, checked against
OpenJPEG and the ISO/IEC 15444-4 conformance suite.

## Install

```sh
cargo add rust-j2k
```

Or add it to `Cargo.toml`:

```toml
[dependencies]
rust-j2k = "0.3"
```

The crate has no runtime dependencies and no C toolchain requirement. Minimum
supported Rust version is **1.87**.

## Usage

The public surface is small: the bytes of a JPEG 2000 codestream or JP2 file in
(detected from the first bytes), a decoded `Image` out — `decode`, or
`decode_with` to set options such as a resolution reduction or sRGB output.
`read_header` reads the image area, the channel formats and a JP2 file's
metadata without decoding any tile data, and `read_header_with` does the same
for a given set of options. An `Image` is the image area on the reference grid
plus one `Component` per output channel, each on its own sample grid: per
codestream component, or per palette column for a palettized JP2 file. For a
JP2 file, `image.jp2` holds what its boxes say (color space, channel types,
palette, resolution, and the `xml `, `uuid`, `jp2i` and `uinf` boxes).

```rust
use rust_j2k::decode;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A JPEG 2000 codestream (Annex A) or JP2 file (Annex I): the bytes of a
    // `.j2k` or `.jp2` file, or the §7 data section of a GRIB2 template 5.40
    // (`grid_jpeg`) message.
    let bytes = std::fs::read("grid.j2k")?;

    let image = decode(&bytes)?;
    println!("image area {}x{}", image.width, image.height);

    let component = image.component(0).expect("at least one component");
    println!(
        "{}x{}, {}-bit {}",
        component.width,
        component.height,
        component.bit_depth,
        if component.signed { "signed" } else { "unsigned" },
    );

    // `samples` is row-major `i32`, already DC-level-shifted and clamped to the
    // declared depth. Index directly or use the bounds-checked accessor. A
    // sub-sampled component is smaller than the image area; `x_sampling` and
    // `y_sampling` map it back onto the reference grid.
    let top_left = component.sample(0, 0).expect("0,0 is in bounds");
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
    Ok(image) => { /* use image.components … */ }
    Err(Error::Unsupported(what)) => eprintln!("outside the decoded subset: {what}"),
    Err(e) => eprintln!("decode failed: {e}"),
}
```

### Damaged input

Decoding is strict. Truncated or inconsistent data is an `Error` and no partial
image comes back: a file cut short is `Error::Codestream` (`Psot overruns the
codestream`), a codestream missing its `EOC` marker is refused, and anything
that is not JPEG 2000, empty input and text included, is `Codestream` too.
JPEG 2000 has no checksum, so damage inside the entropy-coded data is not
always detected; a flipped byte can decode to a wrong image without an error.
What is refused, and where this decoder is stricter than OpenJPEG, is in
[docs/correctness.md](docs/correctness.md#stricter-than-the-oracle-on-non-conforming-streams).

### Limits

A valid file can come back as `Error::Limit`: the decoder caps what a file may
declare (image area, code-block count, precinct count) and how much work it
may ask for. Only the two work bounds can be changed, with
`DecodeOptions::packet_work_limit` (reading the packet headers) and
`DecodeOptions::code_block_work_limit` (decoding the code-blocks), each a
`WorkLimit`. The other caps have no option.

- For a file from a source you trust, use `WorkLimit::Unlimited`. It is the
  simplest choice.
- To keep a ceiling, use `WorkLimit::Raised(n)`. The `Limit` message for a
  work bound names an `n` the file needs at least (a lower bound when the file has
  several tiles).

The `WorkLimit` docs on [docs.rs](https://docs.rs/rust-j2k) have an example
that catches `Error::Limit` and retries.

`samples` are raw integer values. For GRIB2 the caller un-scales them with the
template's reference value and binary/decimal scale factors; this crate stops at
the codestream's integer output and does not interpret the GRIB2 fields.

## What it decodes

| Decoded | Not yet (returns `Error::Unsupported`) |
| --- | --- |
| Raw codestream (Annex A) and JP2 files (Annex I), detected from the first bytes | JPX, JPM and MJ2 files that do not claim `jp2 ` compatibility |
| JP2 palettes (`pclr`, `cmap`), channel definitions (`cdef`), color specifications (`colr`), and the `ihdr`, `ftyp`, `rreq`, `res `, `xml `, `uuid`, `jp2i` and `uinf` boxes, reported on `Image::jp2` | — |
| Opt-in sRGB output (`DecodeOptions::color`): restricted ICC profiles, matching OpenJPEG with LittleCMS sample for sample, and sYCC with the IEC constants, rounded (MSE about 0.4 a channel against OpenJPEG, which truncates; see [the outcomes](docs/correctness.md#outcomes)) | Other color spaces (CMYK, CIELab, e-sYCC, …) and profiles under that option |
| Integer components, unsigned up to 31 bits or signed up to 32 bits, with sub-sampling | Unsigned 32-bit components, and depths past the `i32` container (33–38 bits) |
| Both color transforms: reversible (RCT) and irreversible (ICT) | Part 2 extensions (array MCT, …) |
| Reversible 5/3 (lossless) and irreversible 9/7 (lossy) wavelets | HTJ2K (Part 15) block coding |
| Multiple tiles and tile-parts | — |
| Multiple quality layers; all five progression orders and POC changes | — |
| Precinct partitions; SOP/EPH delimiters; PPM/PPT packed headers | — |
| All six code-block coding styles; region of interest (maxshift) | — |
| Resolution-reduced decode (each level halves the output) | — |

Anything outside the decoded set is rejected cleanly, not half-decoded. Where
this decoder and OpenJPEG differ, and why, is listed in
[docs/correctness.md](docs/correctness.md). The long-run target is
OpenJPEG-level coverage (HTJ2K, an encoder, and the later parts); we get there
by widening this same engine outward. See [docs/roadmap.md](docs/roadmap.md)
and [docs/scope.md](docs/scope.md).

## Pipeline

```text
bytes -> [jp2 boxes] -> codestream -> tier-2 -> tier-1 -> dequant -> inverse DWT -> Image
         (Annex I)       (markers)     (packets)  (MQ+EBCOT) (quant)    (5/3 | 9/7)
```

Each stage is a module under `src/`, and each module's docs cite the ISO section
it owns.

## Status

All 23 entries of the ISO/IEC 15444-4 conformance suite decode within their
class-1 error bounds, gated in CI against committed reference images. The GRIB2
§5.40 (`grid_jpeg`) path — the crate's original target — is gated bit-exact
against the OpenJPEG/eccodes oracle.

The nine ISO/IEC 15444-4 JP2 reader files decode within their reference bounds
too, and a set of real `.jp2` files matches OpenJPEG.

Next come HTJ2K and an encoder, widening this same engine. The plan is in
[docs/roadmap.md](docs/roadmap.md).

## Correctness

Two harnesses set the bar. `tests/conformance_part4.rs` grades the vendored
ISO/IEC 15444-4 conformance corpus against its class-1 error bounds — the
standard's own definition of a compliant decoder. `tests/conformance.rs` decodes
a corpus of real and synthetic codestreams and compares against a trusted
decoder (OpenJPEG, or eccodes for the GRIB2-sourced files): bit-exact for the
reversible path, within a stated tolerance for lossy. The full strategy —
oracle cross-check, conformance suite, golden vectors, fuzzing — is in
[docs/correctness.md](docs/correctness.md).

## Development

`cargo test` is self-contained: it grades a decode against the committed
`expected.json` snapshots and needs **no** external tools. CI runs the same way.

A reference toolchain is only needed to *(re)generate* those snapshots and to run
the extra gates — OpenJPEG and eccodes (oracles), `cargo-deny` (license/advisory
gate), and `cargo-fuzz` (robustness). Install it in one step and regenerate an
oracle with:

```sh
scripts/install-oracle-tools.sh                 # Debian/Ubuntu; prints brew steps on macOS
scripts/gen-oracle.sh tests/fixtures/jpeg2000_regular_latlon.j2k
# rewrites tests/fixtures/jpeg2000_regular_latlon.expected.json
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

### Third-party code

`src/jp2/lcms.rs` is a Rust port of parts of LittleCMS 2.14, which is MIT
licensed. Its copyright and license notice is at the top of that file.

`src/jp2/math.rs` and `src/jp2/math_data.rs` are a Rust port of `pow` and
`log` from Arm's optimized-routines (commit `2ee4b0d`), used under the MIT
license. Its copyright and license notice is at the top of `math.rs`.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual-licensed as above, without any additional terms or conditions.
