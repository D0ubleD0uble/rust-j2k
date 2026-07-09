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
   for "is the decoder correct" and is what Phase 2+ gates on. The corpus sits
   in `tests/fixtures/conformance/`; `tests/conformance_part4.rs` grades it.
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

## Grading against Part 4

`tests/conformance_part4.rs` decodes every entry in the vendored corpus and
grades it at **compliance class 1**, which bounds each graded component
independently. Per component it computes the peak absolute error (the largest
difference from the reference sample) and the mean-squared error, and checks both
against the bounds `manifest.json` records for that entry. The reference images
are `.pgx` files, one per graded component; the harness reads them directly, so
no reference decoder is needed to run the suite.

Two rules keep the grading honest:

- **Exactness comes from the manifest's `bit_exact` flag, not from the wavelet.**
  A `bit_exact` entry is graded against zero bounds whatever the manifest quotes,
  so a corpus edit cannot quietly relax it. The flag and the wavelet disagree in
  both directions: `p0_09` is irreversible 9/7 yet reproduces its reference
  exactly, and reversible entries can be graded lossily.
- **An unimplemented feature is not a pass.** An entry the decoder rejects with
  `Error::Unsupported` reports as *not yet decoded*. Any other rejection, a
  panic, a geometry disagreement, or samples outside the class is a failure. The
  `IN_CLASS` list names the entries that decode in class today, so a milestone
  that turns one green — or a regression that turns one red — fails the test
  until the list is updated.

## A passing entry is not proof the feature works

`IN_CLASS` growing is the goal, but it is not the same as a feature being
implemented. A conformance entry only exercises a feature if its *other*
parameters let that feature change the answer.

The sharp case is `p0_01`. It is RLCP, and it is the only entry whose sole
remaining blocker is the progression order — so it looks like the test for
progression support. It is not. `p0_01` has one component, one quality layer,
one tile, and one precinct, and under those conditions all five progression
orders enumerate the identical packet sequence, because four of the five nested
loops run once:

```text
LRCP  l -> r -> c -> p       PCRL  p -> c -> r -> l
RLCP  r -> l -> c -> p       CPRL  c -> p -> r -> l
RPCL  r -> p -> c -> l       all collapse to: r
```

Accepting the RLCP code and changing nothing else makes `p0_01` decode
bit-exact. That is the same shape of bug as reading `SPcod`'s code-block style
byte and discarding it: the decoder reports success on a feature it does not
implement.

The same reasoning cuts the other way: two features can be *indistinguishable*
rather than untested. With one precinct per resolution, PCRL and CPRL enumerate
the identical packet sequence — OpenJPEG's own output for the two differs in
exactly one byte, the progression code in COD. No fixture separates them until
the precinct partition lands. That is worth recording next to the code rather
than mistaking one order's fixture for coverage of both.

So before adding an entry to `IN_CLASS`, ask what about that entry would have to
change for the new code to be wrong. If nothing would, the entry is not the
oracle for that feature, and the milestone needs a fixture that can tell the
difference — a multi-layer codestream to separate LRCP from RLCP, multiple
precincts to separate RPCL from RLCP, and so on. Mutating the new code and
watching the entry fail is the cheapest way to check; if no mutation makes it
fail, the entry is not testing it.

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

- the GRIB2 §5.40 fixtures already in the fieldglass repo (e.g.
  `jpeg2000_regular_latlon.grib2`), plus a 9/7 re-encode for the irreversible
  path — no GRIB2 producer ships lossy 9/7 (HRRR/NDFD are complex-packed, ECMWF
  is CCSDS), so OpenJPEG's irreversible mode re-encodes a real grid;
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
- **The reject matrix** — `reject_matrix_maps_every_out_of_subset_input_to_its_typed_error`
  in `src/codestream/tests.rs` enumerates every input outside the decoded subset
  and pins the typed error a caller sees: `Codestream` for structural damage,
  `Marker` for an illegally encoded field, `Unsupported` for valid JPEG 2000 we
  do not decode yet. Rejecting is not a formality. A feature the decoder reads
  and then ignores does not yield a slightly wrong image, it yields an arbitrary
  one, so anything not decoded must be refused rather than skipped. As each
  milestone lands, its row leaves the table for the decoded set; a row that stops
  rejecting fails the test first.
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
