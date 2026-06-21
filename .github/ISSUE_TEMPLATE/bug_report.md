---
name: Bug report
about: A codestream that fails to decode, a wrong sample value, or a panic
title: "[bug] "
labels: bug
---

## What happened

<!-- One sentence: what did you expect, and what did you see instead? -->

## Codestream involved

<!--
The single most useful thing for a decoder bug is the input itself. If you can
share the codestream (a `.j2k` file, or the extracted GRIB2 §7 data section),
please attach it or link to a public source. Otherwise, please describe it:
-->

- Wavelet: <!-- 5/3 reversible (lossless) / 9/7 irreversible (lossy) / unknown -->
- Source: <!-- e.g. eccodes grid_jpeg, OpenJPEG re-encode, a GRIB2 product -->
- Geometry / depth if known: <!-- e.g. 512x256, 16-bit unsigned -->
- Approximate size:

## Error or wrong output

<!--
- For a decode failure: the `Error` value (e.g. `Unsupported("...")`,
  `Codestream("...")`).
- For a wrong value: the expected vs. actual sample(s) and how you obtained the
  expected value (e.g. opj_decompress / eccodes output).
- For a panic: the full backtrace (run with RUST_BACKTRACE=1).
-->

## Steps to reproduce

```rust
let bytes = std::fs::read("…")?;
let image = rust_j2k::decode(&bytes)?;
```

## Environment

- rust-j2k version:
- Rust version: <!-- rustc --version -->
- OS + arch:
