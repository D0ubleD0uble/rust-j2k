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
  wavelets, because operational GRIB2 such as HRRR uses lossy.

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

Skeleton (Phase 0). Every stage compiles and is stubbed with `todo!()`; each
module's docs cite the ISO section it owns. Phase 1 fills in the GRIB2 decode
path in an order that keeps tests meaningful: codestream parsing, then the MQ
decoder, EBCOT passes, Tier-2 packets, the inverse DWT, and dequantization last.
The phased plan is in [docs/roadmap.md](docs/roadmap.md).

## Correctness

`tests/conformance.rs` is the bar: decode a corpus of real codestreams and
compare against a trusted decoder (OpenJPEG, or eccodes for the GRIB2-sourced
files). Bit-exact for the reversible path, within a stated tolerance for lossy.
Seed the corpus from the GRIB2 §5.40 fixture in the fieldglass repo, plus HRRR
and MRMS samples.

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
