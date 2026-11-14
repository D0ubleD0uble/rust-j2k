# rust-j2k

[![CI](https://github.com/D0ubleD0uble/rust-j2k/actions/workflows/ci.yml/badge.svg)](https://github.com/D0ubleD0uble/rust-j2k/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/D0ubleD0uble/rust-j2k/branch/main/graph/badge.svg)](https://codecov.io/gh/D0ubleD0uble/rust-j2k)
[![crates.io](https://img.shields.io/crates/v/rust-j2k.svg)](https://crates.io/crates/rust-j2k)
[![docs.rs](https://img.shields.io/docsrs/rust-j2k)](https://docs.rs/rust-j2k)
![MSRV](https://img.shields.io/badge/rustc-1.87+-blue.svg)
![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)

A pure-Rust JPEG 2000 decoder. It reads raw codestreams and JP2 files, and its
output is checked against [OpenJPEG](https://www.openjpeg.org/) (an ISO/IEC
reference implementation) and the ISO/IEC 15444-4 conformance suite. It has no
C dependency, so cross-compiling it needs no C toolchain. It was built first to
decode the JPEG 2000 data in GRIB2 weather files.

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

Pass the bytes of a JPEG 2000 codestream or JP2 file to `decode`, and you get
back an `Image`. The format is detected from the first bytes. `decode_with`
takes options, such as a resolution reduction or sRGB output. `read_header`
reads the image size, the channel formats and a JP2 file's metadata without
decoding any tile data, and `read_header_with` describes the image that
`decode_with` would return for the same options.

An `Image` gives the size of the image area and one `Component` per output
channel. The size is measured on the reference grid, the coordinate space that
all the channels share (the reduced grid, under a resolution reduction). Each
channel has its own sample grid, which is smaller than the image area when the
channel is sub-sampled. There is one channel per codestream component, or one
per palette column for a palettized JP2 file. For a JP2 file, `image.jp2` holds
what its boxes say: color space, channel types, palette, resolution, and the
`xml `, `uuid`, `jp2i` and `uinf` boxes.

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

`samples` are raw integer values. For GRIB2 the caller un-scales them with the
template's reference value and binary/decimal scale factors; this crate stops at
the codestream's integer output and does not interpret the GRIB2 fields.

Decoding never panics on malformed input; every failure is a typed `Error`. The
`Unsupported` variant marks valid JPEG 2000 that uses a feature this decoder
does not read (see [What it decodes](#what-it-decodes)), as opposed to a file
that is malformed. Damage can also look unsupported; see
[Damaged input](#damaged-input).

```rust
use rust_j2k::{decode, Error};

// `bytes` as in the example above.

match decode(&bytes) {
    Ok(image) => { /* use image.components … */ }
    Err(Error::Unsupported(what)) => eprintln!("outside the decoded subset: {what}"),
    Err(e) => eprintln!("decode failed: {e}"),
}
```

### Damaged input

Decoding is strict by default. Truncated or inconsistent data is an `Error` and
no partial image comes back. A raw codestream cut short is `Error::Codestream`
(`Psot overruns the codestream`), a JP2 file cut short is `Codestream` or
`FileFormat` depending on where, and a raw codestream missing its `EOC` marker
is refused (a JP2 file whose last tile-part has `Psot = 0` decodes without
one, as OpenJPEG reads it). Anything that is not JPEG 2000, empty input and
text included, is `Codestream` too.

Damage can show up as any variant: a flipped header bit can read as an absurd
image size (`Limit`) or as an unknown marker or feature (`Unsupported`), so
don't retry a `Limit` with `Unlimited` on that evidence alone. JPEG 2000 has
no checksum, so damage inside the entropy-coded data is not always detected; a
flipped byte can decode to a wrong image without an error. What is refused,
and where this decoder is stricter than OpenJPEG, is in
[docs/correctness.md](docs/correctness.md#stricter-than-the-oracle-on-non-conforming-streams).

To decode what a cut codestream holds, as `opj_decompress -allow-partial`
does, turn `strict` off. Only missing data and bytes left after a tile's last
packet are relaxed; other damage is still an error. The prefix must hold the
whole main header, up to the first `SOT` marker. `Image::report` says what
was missing, and its `complete_reduction` is the finest level the data
already gives in full, so a codestream can be decoded as it streams in:

```rust
use rust_j2k::{DecodeOptions, decode_with};

// `prefix`: the bytes of the codestream that have arrived so far.
fn show_missing(prefix: &[u8]) -> Result<(), rust_j2k::Error> {
    let options = DecodeOptions::default().with_strict(false);
    let image = decode_with(prefix, &options)?;
    if !image.report.is_complete() {
        for entry in &image.report.entries {
            eprintln!("{entry}");
        }
    }
    Ok(())
}
```

### Limits

A valid file can come back as `Error::Limit`: the decoder caps what a file may
declare and how much work it may ask for. Three limits can be changed, each a
`WorkLimit`:

- `DecodeOptions::sample_limit`, the image's size: 2^28 samples (1 GiB as
  `i32`) by default, counted after any resolution reduction, over every output
  channel, a palette's and sRGB output's included.
- `DecodeOptions::packet_work_limit`, the work of reading the packet headers.
- `DecodeOptions::code_block_work_limit`, the work of decoding the
  code-blocks.

The other guards have no option; the list is on
[`Error::Limit`](https://docs.rs/rust-j2k/latest/rust_j2k/enum.Error.html#variant.Limit).

- For a file from a source you trust, set all three to `WorkLimit::Unlimited`.
  With the sample limit lifted, the decode allocates whatever the header
  declares, and the process aborts if that allocation fails.
- To keep a ceiling, use `WorkLimit::Raised(n)`. The `Limit` message suggests
  an `n`; the `WorkLimit` docs say how far to trust it. For the packet bound
  it may not be enough, so double `n` on a retry.

The `WorkLimit` docs on [docs.rs](https://docs.rs/rust-j2k) have an example
that catches `Error::Limit` and retries.

## What it decodes

| Decoded | Not decoded (returns `Error::Unsupported`) |
| --- | --- |
| Raw codestream (Annex A) and JP2 files (Annex I), detected from the first bytes | JPX, JPM and MJ2 files that do not claim `jp2 ` compatibility |
| JP2 palettes (`pclr`, `cmap`), channel definitions (`cdef`), color specifications (`colr`), and the `ihdr`, `ftyp`, `rreq`, `res `, `xml `, `uuid`, `jp2i` and `uinf` boxes, reported on `Image::jp2` | — |
| Opt-in sRGB output (`DecodeOptions::color`): restricted ICC profiles, matching OpenJPEG with LittleCMS sample for sample, and sYCC with the IEC constants, rounded (MSE about 0.38 to 0.49 a channel against OpenJPEG, which truncates; see [the outcomes](docs/correctness.md#outcomes)) | Other color spaces (CMYK, CIELab, e-sYCC, …) and ICC profiles applied through a lookup table or Lab, under that option ([every case](docs/correctness.md#outcomes)) |
| Integer components, unsigned up to 31 bits or signed up to 32 bits, with sub-sampling | Unsigned 32-bit components, and depths past the `i32` container (33–38 bits) |
| Both color transforms: reversible (RCT) and irreversible (ICT) | Part 2 extensions (array MCT, …) |
| Reversible 5/3 (lossless) and irreversible 9/7 (lossy) wavelets | HTJ2K (Part 15) block coding |
| Multiple tiles and tile-parts | — |
| Multiple quality layers; all five progression orders and POC changes | — |
| Precinct partitions; SOP/EPH delimiters; PPM/PPT packed headers | — |
| All six code-block coding styles; region of interest (maxshift) | — |
| Resolution-reduced decode (each level about halves the output) | — |
| Partial decode of a cut codestream (`strict` off), with a report of what is missing | — |

Anything outside the decoded set is refused with an error rather than
half-decoded. Partial decode relaxes only missing data and leftover bytes; a
damaged packet header or code-block is still an error. Where this decoder and
OpenJPEG differ, and why, is listed in
[docs/correctness.md](docs/correctness.md). [docs/scope.md](docs/scope.md) maps
every feature, and [docs/roadmap.md](docs/roadmap.md) has the plan for HTJ2K, an
encoder and the later parts.

## Pipeline

```text
bytes -> [jp2 boxes] -> codestream -> tier-2 -> tier-1 -> dequant -> inverse DWT -> inverse MCT -> [palette, cdef, sRGB] -> Image
         (Annex I)       (markers)     (packets)  (MQ+EBCOT) (quant)    (5/3 | 9/7)    (RCT | ICT)    (JP2 only)
```

Each stage is a module under `src/`, and each module's docs cite the ISO section
it owns.

## Status

All 23 entries of the ISO/IEC 15444-4 conformance suite decode within their
class-1 error bounds, gated in CI against committed reference images. The nine
ISO/IEC 15444-4 JP2 reader files decode within their reference bounds too, and
a set of real `.jp2` files matches OpenJPEG. The GRIB2 §5.40 (`grid_jpeg`)
path, the crate's original target, is gated bit-exact against OpenJPEG on
codestreams taken from GRIB2 files that eccodes wrote.

## Correctness

Three harnesses set the bar. `tests/conformance_part4.rs` grades the vendored
ISO/IEC 15444-4 conformance corpus against its class-1 error bounds, the
standard's own definition of a compliant decoder. `tests/conformance.rs` decodes
a corpus of real and synthetic codestreams and compares against OpenJPEG:
bit-exact for the reversible path, within a stated tolerance for lossy.
`tests/conformance_jp2.rs` grades the nine Part 4 JP2 reader files against
their reference images. The full strategy
(oracle cross-check, conformance suite, golden vectors, fuzzing) is in
[docs/correctness.md](docs/correctness.md).

## Development

`cargo test` is self-contained: it grades a decode against the committed
`expected.json` snapshots and needs **no** external tools. CI runs the same way.

Other tools are needed only to regenerate those snapshots and to run the extra
gates: OpenJPEG (the oracle), eccodes (to inspect GRIB2 sources), `cargo-deny`
(the license and advisory gate) and `cargo-fuzz` (robustness). One script
installs them, and another regenerates an oracle:

```sh
scripts/install-oracle-tools.sh                 # Debian/Ubuntu; prints brew steps on macOS
scripts/gen-oracle.sh tests/fixtures/jpeg2000_regular_latlon.j2k
# rewrites tests/fixtures/jpeg2000_regular_latlon.expected.json
```

OpenJPEG and eccodes are only for making fixtures; the test suite and CI never
run them. Versions, the macOS path, and the GRIB2 sample-mapping note are in
[docs/development.md](docs/development.md), and the design docs are in
[docs/](docs/).

## Contributing

Contributions are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md) for the
workflow, the pure-Rust, no-C rule, and the quality gates every PR must pass.

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
