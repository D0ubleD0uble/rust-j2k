# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-06-21

### Changed

- Reduced the public API to its intended surface: `decode`, `Image`, `Error`,
  and `Result`. The pipeline modules (`codestream`, `tier1`, `tier2`, `dwt`,
  `quant`) are now crate-internal. Nothing outside the crate referenced them;
  this keeps the documented surface to what a caller can use and lets the stages
  evolve without churning the committed API. Breaking for anyone who imported a
  pipeline module directly.
- Error messages and rustdoc describe the supported subset directly instead of
  internal roadmap phase numbers.

### Added

- `#![warn(missing_docs)]` to keep the public surface documented, a tested
  crate-level usage example, and `[package.metadata.docs.rs]` configuration.

## [0.1.0] - 2026-06-21

First release. Implements Phase 1: the GRIB2 template 5.40 (`grid_jpeg`) decode
path, end to end, gated against the OpenJPEG/eccodes oracle.

### Added

- Public API: `rust_j2k::decode(&[u8]) -> Result<Image>`, decoding a raw JPEG
  2000 codestream (Annex A, no JP2 boxes) into a single integer-component image.
- Codestream parsing (Annex A): SOC, SIZ, COD, QCD, SOT, SOD, EOC, with COM
  skipped. Out-of-subset markers and fields are rejected with `Error::Unsupported`;
  truncated or malformed input with `Error::Codestream` / `Error::Marker`.
- MQ arithmetic decoder (Annex C), verified against the standard's worked vectors.
- Tier-1 EBCOT bit-plane decoding (Annex D): significance, refinement, and
  cleanup passes with context formation.
- Tier-2 packet parsing (Annex B): single tile, single quality layer, LRCP
  progression, tag-tree decoding, no precinct subdivision.
- Inverse discrete wavelet transform (Annex F): 5/3 reversible integer lifting
  (bit-exact) and 9/7 irreversible float lifting, 2-D as 1-D over rows then
  columns with symmetric boundary extension.
- Dequantization (Annex E), DC level shift, clamping, and image assembly.
- Conformance harness (`tests/conformance.rs`) grading decodes against committed
  `expected.json` oracle snapshots: bit-exact for 5/3, within a stated tolerance
  for 9/7. Runs with no external tools.
- `cargo-fuzz` target over `decode` for robustness against malformed input.
- Minimum supported Rust version of 1.87, declared via `rust-version` and
  verified in CI.
- Project documentation: README install/usage/supported-subset sections,
  `CONTRIBUTING.md`, `SECURITY.md`, and `CODE_OF_CONDUCT.md`.

### Scope

This release decodes only the GRIB2 §5.40 subset: a single integer component,
one tile, one quality layer, LRCP progression, no precincts, no ROI, no JP2 box
wrapper, and no multi-component or color transform. Anything outside the subset
is rejected cleanly rather than half-decoded. Wider Part 1 coverage, the JP2 file
format, HTJ2K, and an encoder are later-phase work; see `docs/roadmap.md`.

[Unreleased]: https://github.com/D0ubleD0uble/rust-j2k/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/D0ubleD0uble/rust-j2k/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/D0ubleD0uble/rust-j2k/releases/tag/v0.1.0
