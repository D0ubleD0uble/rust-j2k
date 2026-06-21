# rust-j2k

A pure-Rust JPEG 2000 codec, built **GRIB2-decode-first** and aimed, over time,
at the same coverage as [OpenJPEG](https://www.openjpeg.org/) (the ISO/IEC
reference implementation), with no C dependency, so it cross-compiles cleanly to
every target. As of 2026 there is no production-grade pure-Rust JPEG 2000 codec;
this fills that gap.

## Scope

The long-run target is OpenJPEG-level coverage (Part 1, the JP2 file format,
HTJ2K, an encoder, and the later parts). We get there by delivering the GRIB2
decode path first, then widening the same engine outward — see
[docs/roadmap.md](docs/roadmap.md) and [docs/scope.md](docs/scope.md).

The first deliverable decodes exactly what GRIB2 template 5.40 (`grid_jpeg`)
needs, a strict subset of the Part 1 decoder:

- the raw **codestream** (Annex A), not yet the JP2 file format (no boxes);
- a **single component** (one scalar grid), so no color transform yet;
- **integer** samples, signed or unsigned, up to 32 bits;
- both the reversible **5/3** (lossless) and irreversible **9/7** (lossy)
  wavelets (the 9/7 path is graded by re-encoding a real grid with OpenJPEG,
  since no operational GRIB2 producer ships lossy 9/7).

Everything beyond that subset (multi-component and color, JP2 boxes, HTJ2K,
encoding) is later-phase work, not a permanent non-goal.

## Pipeline

```text
bytes -> codestream -> tier-2 -> tier-1 -> dequant -> inverse DWT -> Image
        (markers)     (packets)  (MQ+EBCOT) (quant)    (5/3 | 9/7)
```

Each stage is a module under `src/`. The public surface is one function:

```rust
let image = rust_j2k::decode(&codestream_bytes)?;
```

## Status

Phase 1 complete (v0.1.0). The GRIB2 §5.40 decode path is implemented end to end
— codestream parsing, the MQ decoder, EBCOT passes, Tier-2 packets, the inverse
DWT (5/3 and 9/7), dequantization, and image assembly — and passes the
conformance gate against the OpenJPEG/eccodes oracle: bit-exact for the
reversible 5/3 path, within tolerance for the irreversible 9/7 path. Each
module's docs cite the ISO section it owns.

Later phases widen this same engine toward general Part 1 decode, the JP2 file
format, HTJ2K, and an encoder. The phased plan is in
[docs/roadmap.md](docs/roadmap.md).

## Correctness

`tests/conformance.rs` is the bar: decode a corpus of real codestreams and
compare against a trusted decoder (OpenJPEG, or eccodes for the GRIB2-sourced
files). Bit-exact for the reversible path, within a stated tolerance for lossy.
Seed the corpus from the GRIB2 §5.40 fixtures in the fieldglass repo, plus a
9/7 re-encode for the irreversible path.

## Development environment

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
[docs/development.md](docs/development.md).

## Why this exists

It is the decoder the fieldglass GRIB2 reader needs for template 5.40, kept in
its own crate so the codec is reusable and testable on its own, and so the
fieldglass bundle stays free of a C JPEG 2000 dependency. See that project's
decision record `docs/decisions/0001-grib2-compressed-packing-codecs.md`.

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this crate by you, as defined in the Apache-2.0 license, shall be dual-licensed as above, without any additional terms or conditions.
