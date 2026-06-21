# Correctness & conformance

Decoding a binary format has exactly one truth: the bytes mean what the standard
says they mean. We never grade our output against our own expectations. Every
stage is checked against an external authority — a reference decoder, the ISO
conformance suite, or worked examples from the standard text. This document is
the strategy; the harness that runs it lives in `tests/`.

## The hierarchy of authority

From strongest to most convenient:

1. **ISO/IEC 15444-4 conformance suite** — the official test codestreams and the
   *compliance classes*. Part 4 defines decoder conformance not as bit-exact
   equality (lossy decoders legitimately differ) but as staying within bounded
   per-pixel maximum error and bounded mean-squared error against the reference
   decoded image, at a stated compliance class. This is the authoritative bar
   for "is the decoder correct" and is what Phase 2+ gates on.
2. **Reference-decoder oracle** — decode the same input with OpenJPEG
   (`opj_decompress`), or eccodes for GRIB2-sourced files, and compare. Easy to
   run over any real-world file, so it is the day-to-day workhorse and the
   Phase 1 gate.
3. **Worked examples from the standard** — the MQ coder and the lifting steps
   have small deterministic vectors we can assert against in isolation, so a
   single stage can be proven before the stages around it exist.

## Agreement standard

- **Reversible path (5/3, lossless):** bit-exact sample equality. Any
  difference is a bug.
- **Irreversible path (9/7, lossy):** within a stated absolute tolerance per
  fixture, and within the compliance-class per-pixel and MSE bounds once we test
  against Part 4. Floating-point lifting means our result and
  OpenJPEG's will differ in the low bits; the tolerance is what makes the test
  meaningful rather than flaky.

## Per-stage golden tests

The vertical oracle proves the *whole* pipeline; it does not localise a fault.
So each stage also gets isolated golden tests, which is what makes a failure
debuggable:

- **MQ decoder** — assert the decoded decisions for the standard's worked
  example byte sequences.
- **Tier-1 passes** — feed known code-block byte segments, assert the recovered
  coefficient planes. The vectors are sliced from reversible (5/3),
  single-resolution OpenJPEG codestreams, where the coefficients are just the
  DC-level-shifted samples, so `opj_decompress` gives a bit-exact, decoder-
  independent oracle. Block decoding has no 5/3-vs-9/7 branch, so reversible
  vectors fully exercise it; a 0-level 9/7 codestream is lossy, so its samples
  are not a trustworthy coefficient oracle, and the 9/7 quantization/DWT path is
  graded instead by the end-to-end lossy fixture. Regenerate with
  [`scripts/gen-tier1-vectors.py`](../scripts/gen-tier1-vectors.py).
- **Tag-tree / packet parse** — assert inclusion, zero-bitplane, and length
  decode for hand-built packet headers.
- **Inverse DWT** — assert 5/3 integer lifting is bit-exact on known signals,
  and 9/7 float lifting within tolerance, including the symmetric boundary
  extension at the edges.

Build the pipeline in the order these can be checked (see Phase 1 in
[roadmap.md](roadmap.md)) so every stage has a green isolated test before the
next is wired on top.

## The fixture corpus

The runtime test suite is dependency-free: we commit fixtures and their oracle
snapshots, and the reference decoder is only needed to *(re)generate* an oracle,
never to run the tests. For each fixture, record provenance — source file, and
the exact command that produced its oracle — so the corpus is reproducible.

Layout: a fixture `tests/fixtures/<name>.j2k` with a sibling
`<name>.expected.json` (decoded samples + geometry + tolerance + provenance).

Seed it from:

- the GRIB2 §5.40 fixture already in the fieldglass repo
  (`jpeg2000_regular_latlon.grib2`), plus HRRR (lossy 9/7) and MRMS samples;
- the ISO/IEC 15444-4 conformance codestreams, as Phase 2 brings general Part 1
  features online;
- real `.jp2` files for Phase 3, HTJ2K codestreams for Phase 4.

## Robustness (the input is untrusted)

A decoder parses bytes it did not produce, so malformed input is a first-class
case, not an edge case:

- **Fuzzing** — `cargo fuzz` (libFuzzer) over the public `decode` entry point,
  in the detached [`fuzz/`](../fuzz/) workspace (run it per [`fuzz/README.md`](../fuzz/README.md)).
  The bar: no panics, no unbounded allocation, no infinite loops; every rejected
  input returns a typed `Error`, never crashes. A malformed SIZ cannot steer the
  buffers into an overflowing or out-of-memory allocation: the declared image
  area is bounded at parse time, and the Phase 1 geometry subset (single
  canvas-origin tile) is enforced before any out-of-subset origin reaches the
  DWT.
- **Typed failures** — the flat `Error` enum names the stage that failed, so a
  malformed header, an out-of-scope feature, and a Tier-1 decode fault are
  distinguishable by a caller. No `unwrap`/`panic` on the decode path.
- **No `unsafe`** on the correctness-critical path; if a measured hot path later
  needs it, it is justified, isolated, and fuzzed.

## Round-trip (once the encoder lands, Phase 5)

When encoding exists, add two checks beyond the decode oracle:

- **Round-trip** — encode → decode → compare against the original within the
  path's tolerance.
- **Cross-decode** — our encoder's output read back by OpenJPEG, and OpenJPEG's
  output read by us. Interoperating with the reference implementation in both
  directions is the real proof of encoder conformance.

## What "done" means

A stage is done when its isolated golden tests pass **and** it advances the
vertical oracle/conformance gate for its phase, with the quality gates
(`cargo fmt`, `clippy -D warnings`, `cargo test`, `cargo deny check`) green. Not
before. "It looks right" is not a gate.
