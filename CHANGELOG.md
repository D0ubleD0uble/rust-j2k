# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Two new error variants split what `Error::Unsupported` used to conflate
  (`Error` is `#[non_exhaustive]`, so existing matches keep compiling, but
  wildcard arms now see these instead of `Unsupported`): `Error::Limit` is
  returned when an input exceeds a decoder resource guard (image/component
  sample budget, precinct and code-block counts, bit-plane depth, progression
  volumes), and `Error::InvalidOptions` when the caller's `DecodeOptions` are
  invalid for the codestream (a `resolution_reduction` that consumes a whole
  pyramid). `Error::Unsupported` now means only a valid-but-not-decoded
  feature.

## [0.3.0] - 2026-07-18

General ISO/IEC 15444-1 codestream decoding: all 23 entries of the ISO/IEC
15444-4 conformance suite decode within their class-1 error bounds. This
release also carries the v0.3.0 pre-release review's security hardening and
API future-proofing.

### Added

- PPM (main-header packed packet headers, A.7.4): every tile-part's packet
  headers may be relocated into the main header, framed by 4-byte `Nppm`
  lengths whose chunks can straddle marker boundaries. The PPM markers are
  joined in `Zppm` order and each tile-part reads its headers from its own
  chunk, exactly as PPT feeds them per tile-part; a codestream carrying both
  PPM and PPT is rejected. This completes packed packet headers — and with
  them the corpus: all 23 ISO/IEC 15444-4 conformance entries decode within
  their class-1 bounds. Along the way the predictable-termination check was
  relaxed to OpenJPEG's tolerance: a terminated segment that synthesises
  marker bytes past its end is valid (`p1_05` carries one); only unconsumed
  padding still signals a wrong length.
- The bypass (lazy) code-block style (D.5): from the fifth coded bit-plane on,
  significance and refinement passes are packed as raw bits rather than
  MQ-coded decisions. Tier-2 splits the codeword into the lazy segment
  pattern so every segment is a homogeneous raw or MQ run, and Tier-1 decodes
  the raw runs with MSB-first, bit-unstuffed reads. All six Part 1 code-block
  styles now decode; only the two HTJ2K bits stay rejected.
- PPT (packed packet headers, tile-part): a single-tile codestream may move its
  packet headers out of the bitstream into `PPT` marker segments, leaving only the
  bodies inline. Tier-2 now reads headers from that packed buffer while bodies (and
  SOP markers) stay inline, with EPH moving into the packed buffer alongside the
  headers. Grades conformance entry `p1_02`.
- Two code-block coding styles: **vertically causal context** (the significance
  context of a stripe's bottom row ignores the next stripe) and **reset context
  probabilities** (the MQ context model reinitialises after every coding pass).
  Both are graded bit-exact against OpenJPEG.
- Inverse ICT (irreversible colour transform): a three-component 9/7 codestream
  that signals the multiple-component transform now reconstructs colour by
  inverting the float YCbCr→RGB transform (Annex G.3), applied after the IDWT and
  before the level shift. The wavelet picks the transform — 5/3 is RCT, 9/7 is
  ICT — read off the first colour component; a mix across the three is rejected.
  The inverse matrix matches OpenJPEG's constants, so the decode lands within the
  compliance-class bounds. Grades conformance entry `p0_04`.
- CRG (component registration): the per-component `(Xcrg, Ycrg)` sub-pixel
  registration offsets are parsed and recorded on the decoded header. They are
  informational — how a display should align the components, not a change to the
  samples — so they are recorded, not resampled. Grades conformance entries
  `p0_03` and `p0_15`.
- POC (progression-order change): a codestream may now carry a `POC` marker that
  replaces `COD`'s single progression with a sequence of volumes, each an order
  over its own layer/resolution/component sub-range. The packet iterator walks the
  volumes in turn and emits each packet once, by the first volume that reaches it.
  A `POC` is decoded in the main header and in any tile-part header, where its
  volumes accumulate across the tile's parts. Grades conformance entries `p0_07`
  and `p0_13`.
- Non-zero image and tile origins (`XOsiz`/`YOsiz`, `XTOsiz`/`YTOsiz`): the
  canvas no longer has to start at the reference-grid origin. Subband extents,
  the code-block partition, and the inverse DWT's interleave parity already read
  absolute coordinates, so the origin flows through unchanged; the SIZ validation
  now enforces the Table A-9 constraints a non-zero origin makes load-bearing
  (`XOsiz < Xsiz`, `XTOsiz <= XOsiz`, `XTOsiz + XTsiz > XOsiz`) and bounds the
  decode by the image area `(Xsiz − XOsiz)·(Ysiz − YOsiz)` rather than
  `Xsiz·Ysiz`. Grades conformance entries `p1_01` (odd offset) and `p1_07`.
- Precinct partition: codestreams that subdivide each resolution into precincts
  (`Scod`/`Scoc` bit 0 and the `SPcod`/`SPcoc` precinct sizes) now decode, with
  one packet and one pair of tag trees per precinct. Because a resolution can now
  hold more than one precinct, the position axis of the packet order stops being
  a degenerate counter: RPCL, PCRL and CPRL now enumerate precincts by sweeping
  the reference grid, which is also what tells PCRL and CPRL apart for the first
  time. Precinct-partitioned codestreams are capped at 2^18 precincts.
- Multiple tiles and tile-parts: the reference grid may be tiled, and each tile
  decodes independently in its own coordinate frame — its tile-part headers
  resolved over the main header's markers in the standard's precedence order
  (a tile COD outranks a main-header COC), its own packets and wavelet
  pyramid — before writing its rectangle of the canvas. Tile-parts interleave
  across tiles in codestream order.
- `decode_with` and `DecodeOptions`: decode at a resolution reduction. Each
  dropped level halves the output in both axes (rounding up) using the wavelet
  pyramid's own lower resolutions — the natural way to get a thumbnail or a
  quick preview without paying for the full image. `decode` is unchanged and
  equals `decode_with` at the default options. A reduction that would consume
  some component's whole pyramid is rejected as `Error::Unsupported`.
- The length markers TLM, PLM, and PLT (A.7): informational pointers into the
  codestream the decode does not depend on. TLM is decoded, validated, and
  recorded on the header for callers; PLM and PLT are validated and skipped, as
  OpenJPEG does, since their length chains may split mid-entry across markers.
- Region of interest (the RGN marker and the inverse maxshift, A.6.3 / H.2): a
  region lifted above its background by `SPrgn` bit-planes starts its code-blocks
  that much higher, and any magnitude at or above `1 << SPrgn` is shifted back
  down after the passes. A tile-part RGN replaces the main header's for its
  tile. The packet header's zero-bit-plane count is signalled against the
  maxshift-raised plane count (H.2's Kmax), which is also where a
  background-only block's starting plane is measured from. Grades conformance
  entry `p0_06` (maxshift 11 in the main header, 9 in the tile-part's
  override).
- Per-component coding and quantization overrides (COC, A.6.2; QCC, A.6.5),
  resolved override-else-default onto each component: COC over COD, QCC over
  QCD. The wavelet, decomposition depth, and code-block geometry become
  per-component. `Ccoc`/`Cqcc` widen to two bytes at 257 components — `p0_13`
  sits exactly on that boundary. A COC that moves one of the colour-transform
  components off the shared wavelet is rejected.
- The `predictable termination` and `segmentation symbols` code-block styles
  (D.4.2, D.5). Segmentation symbols are part of the codeword: four decisions
  in the uniform context close every cleanup pass, consumed first and judged
  second — anything but 1010 reports the coded data corrupt. Predictable
  termination adds OpenJPEG's per-segment residue check.
- The `restart` code-block style (termination on each coding pass, Annex D).
  Each coding pass is a terminated codeword segment: Tier-2 reads a length field
  per pass rather than one per contribution, and Tier-1 re-initialises the MQ
  decoder at each segment while carrying the context states, the significance
  map, and the bit-plane counter across. Conformance codestream `p0_12` now
  decodes bit-exact. A style byte that mixes `restart` with styles that are not
  yet decoded names only the parts that block it.
- The packet delimiters SOP and EPH (Annex A.8). `COD`'s `Scod` bits 1 and 2 are
  honoured independently. SOP *may* precede each packet even when signalled, so
  its absence is tolerated; its `Lsop` and `Nsop` are validated, which is
  stricter than OpenJPEG (whose source reads `/* TODO : check the Nsop value */`).
  EPH *shall* follow every packet header, an empty packet's included, and its
  absence is a `Codestream` error. A reserved `Scod` bit is a `Marker` error.
- The remaining progression orders: RLCP, RPCL, PCRL, and CPRL (Annex B.12.1).
  The packet walk is now driven by `COD`'s progression code rather than assuming
  LRCP. Under maximal precincts the position axis has one value, so PCRL and CPRL
  enumerate the same sequence; they are distinguishable only once the precinct
  partition lands. A reserved progression code is a `Marker` error.
- Multiple quality layers (Annex B.10). A code-block's coding passes and byte
  contributions now accumulate across the layers that include it, and its
  inclusion tag tree, zero-bitplane tag tree, and `Lblock` length indicator
  persist across the packets of a precinct instead of being rebuilt per packet.
  Tier-1 decodes the concatenated contributions as one MQ codeword. `COD`
  declaring zero layers is a `Marker` error.
- The inverse reversible color transform (RCT, Annex G.2). When `COD` signals
  the multiple-component transform on the 5/3 path, the first three components
  are recombined after the inverse DWT and before the DC level shift. The
  wavelet selects the transform: 5/3 means RCT, 9/7 means ICT (decoded above);
  Part 2's array MCT (`Smct = 2`) stays rejected. A codestream that signals the
  transform without three components of matching depth, sign, and sub-sampling
  is rejected as a `Marker` error rather than silently decoded without it.
  Conformance codestream `p0_14` now decodes bit-exact.
- Multi-component decoding. Every component a codestream declares reconstructs
  onto its own sample grid, honoring its bit depth, sign, and `XRsiz`/`YRsiz`
  sub-sampling. The component axis runs through Tier-2 packet enumeration
  (LRCP's resolution-major, component-minor order), Tier-1, dequantization, and
  the inverse DWT.
- Two synthetic multi-component fixtures, generated and graded against OpenJPEG
  by `scripts/gen-multicomponent-fixtures.py`. The ISO/IEC 15444-4 corpus cannot
  grade multi-component decoding on its own: every one of its multi-component
  entries also needs a feature that is not decoded yet.
- A grading harness over the vendored ISO/IEC 15444-4 conformance corpus
  (`tests/conformance_part4.rs`). It decodes all 23 entries and grades each
  graded component against its class-1 reference by peak absolute error and
  mean-squared error, reading the `.pgx` reference images directly. Entries
  using features the decoder does not implement yet report as *not yet
  decoded*; `p0_09` decodes and matches its reference exactly.
- SIZ now parses every component a codestream declares (`Csiz` up to 16384),
  with each component's bit depth, sign, and `XRsiz`/`YRsiz` sub-sampling, and
  validates them: a zero or oversized component count, a zero sub-sampling
  factor, a bit depth above 38, and a component-record count that disagrees with
  `Csiz` are all typed errors. `Siz::component_extent` derives a component's own
  sample-grid dimensions from the reference-grid equations.

### Changed

- **Breaking.** `Image`, `Component`, and `DecodeOptions` are now
  `#[non_exhaustive]`, so adding a field — decoded metadata on the image, a new
  decode option — stops being a semver break. Construct options with
  `DecodeOptions::default()` and the new `with_resolution_reduction` method;
  construct images by hand (a test harness's expected values, say) with the new
  `Image::new` and `Component::new`. `DecodeOptions` also drops its
  `PartialEq`/`Eq` derives, which a future non-comparable option would forbid.
- **Breaking.** The never-constructed `Error::Packet` and `Error::Tier1`
  variants are gone — tier-2 and tier-1 corruption reports as
  `Error::Codestream`, as it always did — and `Error` is now
  `#[non_exhaustive]`, so future failure classes are not semver-major breaks.
- **Breaking.** `Image` now carries the image area on the reference grid
  (`width`, `height`) and a `components` vector, instead of describing a single
  component inline. The per-component fields moved to the new `Component` type,
  which also records the `x_sampling`/`y_sampling` sub-sampling factors, and
  `Image::sample` moved to `Component::sample`. Reach a component with
  `image.component(0)`. This settles the output shape that multi-component
  decoding threads the pipeline toward.
- The main header is now walked before it is judged. Marker segments are located
  first, then interpreted, so a codestream carrying a feature outside the decoded
  subset is still traversed end to end rather than abandoned mid-header. An
  unrecognized marker segment is stepped over by its length during the walk, and
  the reserved `0xFF30`–`0xFF3F` range is treated as carrying no segment.
- A marker the decoder does not recognize is now reported as `Unsupported`
  instead of `Codestream`. It is walked past, not decoded past: every marker code
  is allocated by some part of the standard, so an unknown one may change what
  the packet data means. `CAP` (which an HTJ2K codestream carries), `PPM`, `PPT`,
  `PLM`, and `CRG` are named for a clearer message, and `PPT` now rejects in a
  tile-part header where it previously read as a structural error.

### Fixed

- The 9/7 subband gain is applied per high-pass filtering inside the inverse
  DWT (an exact `2/K` on each high-pass scale) instead of baked into the
  quantization step as `2^gain`. The two agree only while a coefficient is
  high-pass exactly its nominal-gain many times, which an empty coarse
  resolution breaks: `p1_06`'s small odd-offset tiles decoded with a constant
  DC offset per component. Bit-for-bit identical on the ordinary case, so no
  other fixture moves.
- The progression-volume list is capped at 32, matching OpenJPEG's `pocs`
  array, and the cap binds everywhere volumes accumulate: per marker, across a
  tile's parts, and on the combined main-plus-tile list a tile's walk runs.
  Uncapped, the walk re-enumerates the packet space once per volume and a
  duplicate emission consumes no bitstream bytes, so a crafted megabyte of POC
  markers over one-byte packets could buy minutes of compute — quadratic work
  from linear input. Found in the v0.3.0 pre-release security review. A POC
  volume declaring zero layers (`LYEpoc == 0`, illegal per Table A-33) is also
  now rejected at parse as `Error::Marker` instead of surfacing later as a
  packet desync.
- Four resource and desync guards from a whole-repo review. A QCD/QCC step
  table keeps at most the 97 entries a 32-level decomposition can use —
  excess is dropped the way OpenJPEG caps at `J2K_MAXBANDS` — so a crafted
  65535-byte segment can no longer multiply into gigabytes through the
  per-component parameter clones. The total code-block count is capped at 2^19
  before any per-block state is allocated, closing the counterpart bomb built
  from legal 4×4 code-blocks; the precinct count is capped at 2^18 alongside it,
  which the block count does not bound (a band empty in one axis has no blocks
  while its resolution still has a full column of precincts). And the fixture
  harness now grades a panicking decode as a failure — the skeleton-era `Pending`
  outcome had let one pass CI silently.
- Arithmetic hardening against hostile coefficients: the 5/3 lifting sums run
  in `i64` and saturate (they could leave `i32` near the Tier-1 magnitude
  ceiling), and the MQ decoder's BYTEIN adds wrap explicitly (matching the C
  reference's `OPJ_UINT32` semantics in every build profile).
- The 9/7 dequantization applied the interval mid-point reconstruction twice:
  Tier-1 already hands over magnitudes reconstructed at the mid-point, and the
  half-step bias dequantization added on top canceled only against the integer
  halving's truncation on odd double-scale magnitudes. The irreversible path
  now scales the double-scale value by half the step with no added bias,
  matching OpenJPEG's fold of the halving into the float multiply.
- Error classification now matches the crate's own variant conventions:
  reserved decomposition-level counts (33–255), short quantization step tables,
  and the 9/7-wavelet-with-no-quantization pairing reject at parse as
  `Error::Marker` (the latter two previously surfaced deep in dequantization
  as `Inconsistent`; both are documented stricter-than-oracle calls — see
  `docs/correctness.md`); `0xFF00`/`0xFFFF` in marker position and TLM/SOP/EPH
  in a tile-part header reject as `Error::Codestream`; standard-legal bit
  depths 33–38 report `Unsupported` rather than `Marker`.
- A codestream that sets any `SPcod` code-block style flag — selective
  arithmetic coding bypass, reset context probabilities, termination on each
  coding pass, vertically causal context, predictable termination, segmentation
  symbols, or either HTJ2K block-coding flag — decoded to wrong samples instead
  of being rejected. The flag was parsed, passed to Tier-1, and discarded. It
  was first rejected as `Unsupported`, naming the flags that are set; each Part
  1 style has since been decoded (see above), leaving only the HTJ2K flags
  rejected.
- A JP2 file is now rejected as `Unsupported`, naming the wrapper and pointing at
  the contained codestream, instead of reporting a missing `SOC` marker. JP2 is
  valid JPEG 2000; it is simply not the bare codestream this decoder reads.

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

[Unreleased]: https://github.com/D0ubleD0uble/rust-j2k/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/D0ubleD0uble/rust-j2k/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/D0ubleD0uble/rust-j2k/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/D0ubleD0uble/rust-j2k/releases/tag/v0.1.0
