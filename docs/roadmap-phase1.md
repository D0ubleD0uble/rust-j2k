# Phase 1 roadmap — GRIB2 decode MVP

> **Status: complete.** All milestones below are done and the phase gate passes;
> Phase 1 shipped in v0.1.0. This file is kept as the build-order record — each
> milestone's "Done" line describes the exit state it was written against, not
> the current state of the crate.

The overall [roadmap.md](roadmap.md) sequences the whole project; this file zooms
into Phase 1 alone. It breaks the single line item "GRIB2 decode MVP" into
ordered milestones, each tied to the module it fills in, the oracle or golden
test that proves it, and the exact condition that lets the next milestone start.

The feature rows this phase covers are the ✓ entries in [scope.md](scope.md). The
gating philosophy (per-stage golden test plus a phase oracle, "done" means the
test is green) is [correctness.md](correctness.md); this file says in what order
to earn those greens.

## What Phase 1 decodes

GRIB2 template 5.40 (`grid_jpeg`) hands us a *raw* JPEG 2000 codestream (Annex A,
no JP2 boxes). The GRIB2 layer owns the reference value and scale factors, so our
job stops at integer samples; the caller un-scales them. The codestream is the
plain end of Part 1:

- one component (a single grayscale grid), so no multi-component or color transform;
- integer samples, signed or unsigned, up to 32 bits (the SIZ depth);
- one tile, one quality layer, LRCP progression, no precinct subdivision, no ROI;
- either wavelet, 5/3 reversible (lossless) or 9/7 irreversible (lossy). No
  operational GRIB2 producer ships lossy 9/7 (HRRR and NDFD are complex-packed,
  ECMWF is CCSDS, and eccodes' `grid_jpeg` encoder always uses 5/3), so the 9/7
  path is graded by re-encoding a real grid with OpenJPEG's irreversible mode.

Anything outside that subset is rejected, not half-decoded: parsing returns
`Error::Unsupported` the moment it sees a feature a later phase owns (COC/QCC,
POC, multiple tiles, multiple layers, a JP2 box wrapper). Rejecting cleanly is
itself a Phase 1 deliverable, not an afterthought.

## Build order and why

The pipeline is `codestream → tier2 → tier1 → quant → dwt → image`, but that is
the *data* order, not the *build* order. We build in the order each stage can be
proven against an oracle before the next is wired on top, so no untested stage
ever sits on another untested stage.

Two independent tracks fall out of that rule:

- **Entropy track** (P1.1 → P1.2 → P1.3 → P1.4): codestream header, then the MQ
  decoder, then the Tier-1 passes it drives, then the Tier-2 packets that feed
  Tier-1. Each step is the input to the next, so it is strictly sequential.
- **Reconstruction track** (P1.5, P1.6): the inverse DWT and dequantization take
  coefficients as input. They can be written and golden-tested on synthetic
  coefficient arrays without any entropy decoding, so this track runs in
  parallel with the entropy track and only meets it at integration (P1.7).

```text
P1.0 harness ──┬─ P1.1 codestream ─ P1.2 MQ ─ P1.3 Tier-1 ─ P1.4 Tier-2 ─┐
               │                                                          ├─ P1.7 integrate ─ gate
               └─ P1.5 inverse DWT ──────────── P1.6 dequant ─────────────┘
```

## Milestones

### P1.0 — Fixture harness and oracle plumbing

**Goal:** the test scaffolding exists before the code it grades. Stand up
`tests/conformance.rs` so it loads a fixture and its sibling
`<name>.expected.json` (decoded samples, geometry, tolerance, provenance) and
compares, bit-exact for reversible and within tolerance for irreversible.

**Work:** the fixture loader and comparator; the `expected.json` schema; one
recorded command per fixture for regenerating its oracle with `opj_decompress`
or eccodes. Seed the corpus with the fieldglass `jpeg2000_regular_latlon.grib2`
fixture plus a second 5/3 geometry, a rate-truncated 5/3 codestream, and a 9/7
re-encode for the irreversible path.

**Done:** the harness runs and reports every seed fixture as *not yet decoded*
(expected, since `decode` is still `todo!()`). Provenance for each fixture is
committed so the corpus is reproducible without the reference decoder installed.

### P1.1 — Codestream parsing (`src/codestream/`)

**Goal:** turn header bytes into a `MainHeader` of decode parameters plus the
byte ranges of each tile's packet data, with no interpretation of entropy-coded
bytes yet.

**Work:** parse SOC, SIZ, COD, QCD, SOT, SOD, EOC, and skip COM
(`src/codestream/markers.rs` already holds the marker codes). Read from SIZ the
image and tile geometry, component count, depth and sign; from COD the wavelet
choice, decomposition levels, progression order, layer count, code-block size;
from QCD the quantization style and step sizes. Enforce the subset: one
component, one tile, one layer, LRCP, no precincts. Map every out-of-subset
marker or field to `Error::Unsupported`, and every truncation to
`Error::Codestream`.

**Oracle:** assert the parsed `MainHeader` fields against `opj_dump` output for
each seed codestream. Add hand-built malformed headers that must each return the
right typed `Error`.

**Done:** all seed codestreams parse to the correct `MainHeader`; out-of-subset
and malformed inputs reject with the expected variant. Unblocks every later
stage (they all read `MainHeader`).

### P1.2 — MQ arithmetic decoder (`src/tier1/mq.rs`)

**Goal:** the binary arithmetic decoder in isolation, decoupled from context
modelling. `decode(cx)` returns one decision bit and updates the context state
via the Annex C transition table (`QeEntry`).

**Work:** the decoder registers (C, A, CT, byte pointer), INITDEC, DECODE with
MPS/LPS exchange, RENORMD, and BYTEIN with the 0xFF carry handling. Fill the
`QeEntry` probability-estimation table from ISO Table C-2.

**Oracle:** the standard's worked test sequence for the arithmetic coder. Feed
the known input bytes, assert the exact decoded decision stream. This is a small
deterministic vector, so the decoder is proven before any pass exists.

**Done:** the test sequence decodes bit-exact. Unblocks Tier-1.

### P1.3 — Tier-1 EBCOT passes (`src/tier1/passes.rs`, `src/tier1/mod.rs`)

**Goal:** decode one code-block's MQ stream into quantized subband coefficients.

**Work:** the per-block state (magnitudes, signs, significance flags); context
formation from the 3×3 neighbourhood (D.3 tables); the three passes in order,
significance propagation (D.3.1), magnitude refinement (D.3.2), cleanup (D.3.3)
with run-length mode; sign coding with the neighbour-sign context (D.3.4).
Bit-planes run MSB→LSB, starting from the cleanup pass of the top non-zero
plane. Phase 1 needs only the default code-block style (no bypass, reset,
restart, vertically-causal, or segmentation, which are Phase 2).

**Oracle:** feed known code-block byte segments (extracted from a seed
codestream, or hand-built) and assert the recovered coefficient plane.

**Done:** golden code-blocks decode to the expected coefficients. Unblocks the
quant/DWT track's real inputs and the integration test.

### P1.4 — Tier-2 packet decoding (`src/tier2/`)

**Goal:** parse tile-part data into per-code-block coded byte segments, in
progression order, and hand them to Tier-1. This stage reads structure only; it
never runs the arithmetic decoder.

**Work:** the tag-tree reader (`tagtree.rs`) with its rising-threshold
incremental reads and retained state across packets; the packet header parse
(inclusion, zero-bitplane count, coding-pass count, contribution lengths) for a
single layer in LRCP order; assembly of each code-block's byte ranges across
resolutions and subbands.

**Oracle:** hand-built packet headers asserting inclusion / zero-bitplane /
length decode (per correctness.md), then a full seed tile whose code-block
segment boundaries match what `opj_dump` reports.

**Done:** seed tiles split into the correct code-block segments; tag-tree unit
tests pass. The entropy track now runs end to end into raw coefficients.

### P1.5 — Inverse DWT (`src/dwt.rs`)

**Goal:** reconstruct samples from subband coefficients, one resolution level at
a time, for both filter banks. Independent of the entropy track.

**Work:** 1-D lifting applied over rows then columns, with whole-sample
symmetric (mirror) extension at boundaries (F.3.6). The 5/3 reversible integer
lifting (F.3.8.2) must be bit-exact; the 9/7 irreversible float lifting
(F.3.8.1) uses the four lifting coefficients and two scaling constants. Loop the
level merge (LL + HL/LH/HH → next LL) for the declared decomposition depth.

**Oracle:** synthetic signals with a known transform. Assert 5/3 bit-exact
(including a round-trip against a forward reference) and 9/7 within tolerance,
exercising odd and even lengths and the boundary extension explicitly.

**Done:** both filters pass on synthetic data. Buildable in parallel with
P1.1–P1.4 since it needs no decoded bitstream.

### P1.6 — Dequant, level shift, assemble (`src/quant.rs`, `src/image.rs`)

**Goal:** turn quantized coefficients into the final `Image`.

**Work:** reversible path is identity (only the implicit bit-plane shift, kept
exact); irreversible path applies the per-subband scalar step from QCD (derived
or expounded, with guard bits). Then DC level shift, clamp to the declared
depth, and pack row-major into `Image` with width/height/depth/sign.

**Oracle:** unit-test dequant on known coefficient/step pairs; level-shift and
clamp on boundary values. Final correctness lands at P1.7.

**Done:** dequant and assembly unit tests pass. With P1.5, the reconstruction
track is complete.

### P1.7 — Integration and the phase gate

**Goal:** wire `decode()` through all stages and pass the conformance gate.

**Work:** connect codestream → Tier-2 → Tier-1 → quant → DWT → image in
`src/lib.rs`; remove the crate-level `allow(dead_code, unused_variables)` so
clippy's `-D warnings` does real work; fix whatever the seam reveals.

**Gate (this is the Phase 1 exit):** `tests/conformance.rs` decodes the full
corpus (the fieldglass `jpeg2000_regular_latlon.grib2` fixture, a second 5/3
geometry, a rate-truncated 5/3 codestream, and a 9/7 re-encode) and matches the
eccodes/OpenJPEG oracle, bit-exact for 5/3 and within tolerance for 9/7. All
quality gates green: `cargo fmt --all -- --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo deny check`.
A `cargo fuzz` target over `decode` runs clean (no panics, no unbounded
allocation), so malformed GRIB2-sourced input is a typed `Error`, never a crash.

## Sequencing notes

- **Critical path** is the entropy track P1.1 → P1.2 → P1.3 → P1.4. The 9/7 DWT
  (P1.5) is the other hard piece and the main parallelization win: start it
  early alongside P1.1.
- **Highest-risk stages** are P1.3 (Tier-1 context modelling) and P1.5 (9/7
  lifting and boundary extension). Both have small deterministic golden vectors,
  so budget for getting those vectors exactly right before the surrounding code.
- **One reject path per stage.** Every milestone that parses input also lands its
  `Error::Unsupported` / `Error::Codestream` cases, so the subset boundary is
  enforced from the first commit, not retrofitted.

## Out of scope for Phase 1

Everything below is Phase 2+, and Phase 1 must reject it cleanly rather than
attempt it: COC/QCC/RGN/POC, multiple tiles or tile-parts, multiple components
and color transforms, the other four progression orders, multiple quality
layers, precinct subdivision, the non-default code-block styles, the
error-resilience and length markers (SOP/EPH/PPM/PPT/PLM/PLT/TLM), and any JP2
box wrapper. See [scope.md](scope.md) for the full mapping.
