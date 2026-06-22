# Roadmap

Phased from the initial skeleton to full-codec parity with OpenJPEG. The
ordering rule: **deliver the GRIB2 decode path first**, because it is a hard,
end-to-end vertical slice with a waiting consumer, then widen the same engine
outward. Each phase states its goal, the work it contains, and the **gate** that
must pass before it counts as done. The gate is always defined by an external
oracle, never by self-consistency — see [correctness.md](correctness.md).

Feature-to-phase mapping lives in [scope.md](scope.md); this file is the
sequence and the rationale.

---

## Phase 0 — skeleton *(done)*

Every stage compiles, is stubbed with `todo!()`, and cites the ISO section it
owns. The module layout is the pipeline: `codestream → tier2 → tier1 → quant →
dwt → image`. Crate-level `allow(dead_code, unused_variables)` is in place and
comes off as stages land.

## Phase 1 — GRIB2 decode MVP *(done — shipped in v0.1.0)*

**Goal:** decode the exact codestream GRIB2 §5.40 produces, bit-exact for 5/3,
within tolerance for 9/7. This unblocks the fieldglass GRIB2 reader.

Scope is the single-component integer subset (see [scope.md](scope.md), the ✓
rows). The detailed milestone-by-milestone plan for this phase, with the oracle
that gates each step, is in [roadmap-phase1.md](roadmap-phase1.md); the build
order in brief, each step testable against an oracle before the next:

1. **Codestream parse** — SOC/SIZ/COD/QCD/SOT/SOD/EOC, COM skip. Reject
   out-of-subset features cleanly with `Error::Unsupported`.
2. **MQ decoder** — the arithmetic decoder in isolation, checked against the
   worked example vectors in the standard.
3. **Tier-1 EBCOT passes** — significance / refinement / cleanup, context
   formation, over the MQ stream; output quantized subband coefficients.
4. **Tier-2 packets** — single layer, LRCP, tag-tree parse feeding step 3.
5. **Inverse DWT** — 5/3 integer lifting (bit-exact) and 9/7 float lifting,
   2-D as 1-D over rows then columns, with symmetric boundary extension.
6. **Dequant + level shift + assemble** — reversible identity / irreversible
   scalar step, DC shift, clamp, package into `Image`.

**Gate:** the conformance harness decodes the corpus (the fieldglass
`jpeg2000_regular_latlon.grib2` fixture, a second 5/3 geometry, a rate-truncated
5/3 codestream, and a 9/7 re-encode) and matches the eccodes/OpenJPEG oracle —
bit-exact lossless, within tolerance lossy. Crate-level dead-code `allow` removed; clippy `-D warnings` clean.

**Status:** met. The gate passes for all four corpus fixtures, and the
crate-level dead-code `allow` is gone. Released as v0.1.0.

## Phase 2 — general Part 1 decoder *(next)*

**Goal:** decode any conformant Part 1 *codestream*, not just the GRIB2 subset.

The milestone-by-milestone build order, each step with the conformance oracle
that gates it, is in [roadmap-phase2.md](roadmap-phase2.md); in brief:

- Multiple tiles and tile-parts; multiple components with subsampling.
- Multiple component transform: RCT and ICT (color reconstruction).
- All five progression orders plus POC; multiple quality layers; precincts.
- Region of interest (maxshift); all code-block coding styles (bypass, reset,
  restart, vertically causal, predictable termination, segmentation symbols).
- Error-resilience markers (SOP/EPH); packed-header and length markers
  (PPM/PPT/PLM/PLT/TLM).

**Gate:** passes the relevant ISO/IEC 15444-4 conformance codestreams within
their compliance-class error bounds, plus a clean fuzzing run on malformed
input (no panics, no `unsafe`, bounded memory).

## Phase 3 — JP2 file format decode

**Goal:** open `.jp2` files, not just bare codestreams.

- Box parser: signature, ftyp, jp2h (ihdr, bpcc, colr, pclr, cmap, cdef, res),
  jp2c contiguous codestream.
- Enumerated colorspaces (sRGB, greyscale), restricted ICC profiles, palette
  and channel mapping.

**Gate:** decodes a corpus of real `.jp2` files matching OpenJPEG's output
within compliance bounds.

## Phase 4 — HTJ2K decode (Part 15)

**Goal:** decode High Throughput codestreams, the modern high-value extension.

- CAP marker and HTJ2K capability signalling.
- FBCOT block decoder (MEL + VLC + MagSgn) as an alternative Tier-1; the rest of
  the pipeline is shared with Part 1.

**Gate:** matches OpenJPEG ≥2.5 HTJ2K decode on the HTJ2K conformance set.

## Phase 5 — Part 1 encoder

**Goal:** the crate becomes a *codec*, not just a decoder, and the "decode-only"
framing falls away. The crate and repository are already named codec-neutrally
(`rust-j2k`, since the re-scope), so nothing needs renaming here.

- Forward 5/3 and 9/7 DWT; forward scalar quantization.
- MQ arithmetic encoder; EBCOT pass encoding.
- Rate control / PCRD-opt; Tier-2 packetization; codestream and JP2 writing.

**Gate:** round-trip (encode → decode → compare) and cross-decode (our encode
read back by OpenJPEG, and vice versa) within target rate/quality.

## Phase 6 — Part 2 extensions + HTJ2K encode

**Goal:** breadth toward full feature parity.

- JPX extended file format; TCQ; arbitrary decomposition and custom wavelet
  kernels; extended/array-based multiple-component transforms (multi- and
  hyperspectral); extended ROI.
- FBCOT block encoder (HTJ2K encode).

**Gate:** cross-check against OpenJPEG/Grok on Part 2 and HTJ2K encode corpora.

## Phase 7 — long tail

Motion JPEG 2000 (Part 3, MJ2 container), JPIP interactive streaming (Part 9),
JPWL wireless error protection (Part 11). Sequenced by demand; none blocks the
phases above.

---

## Cross-cutting, every phase

- **Correctness gate before merge** — no stage is "done" until it passes its
  oracle/conformance check (see [correctness.md](correctness.md)).
- **Quality gates** — `cargo fmt`, `cargo clippy -D warnings`, `cargo test`,
  `cargo deny check` all green before a PR (see `.claude/rules/conventions.md`).
- **Pure Rust** — no C/native codec dependency, ever. `unsafe`/SIMD only behind
  a benchmark that justifies the specific hot path.
- **Robustness** — the decoder parses untrusted binary; fuzz it and keep all
  failure modes as typed `Error`s, never panics.
