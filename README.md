# jp2k-decoder

A pure-Rust JPEG 2000 decoder, scoped to the slice of the standard that GRIB2
uses. No C dependency, so it cross-compiles cleanly to every target.

## Scope

JPEG 2000 (ISO/IEC 15444-1) is a large standard. This crate decodes only what
GRIB2 template 5.40 (`grid_jpeg`) needs, which keeps it small and self-contained:

- the raw **codestream** (Annex A), not the JP2 file format (no boxes);
- a **single component** (one scalar grid), so no color transform;
- **integer** samples, signed or unsigned, up to 32 bits;
- both the reversible **5/3** (lossless) and irreversible **9/7** (lossy)
  wavelets, because operational GRIB2 such as HRRR uses lossy.

Non-goals: encoding, JP2 boxes, and multi-component or color imagery.

## Pipeline

```text
bytes -> codestream -> tier-2 -> tier-1 -> dequant -> inverse DWT -> Image
        (markers)     (packets)  (MQ+EBCOT) (quant)    (5/3 | 9/7)
```

Each stage is a module under `src/`. The public surface is one function:

```rust
let image = jp2k_decoder::decode(&codestream_bytes)?;
```

## Status

Skeleton. Every stage compiles and is stubbed with `todo!()`; each module's
docs cite the ISO section it owns. Build order that keeps tests meaningful:
codestream parsing, then the MQ decoder, EBCOT passes, Tier-2 packets, the
inverse DWT, and dequantization last.

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
