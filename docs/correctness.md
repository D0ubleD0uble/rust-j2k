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

### When the oracle and the standard disagree

The hierarchy is ordered for a reason: OpenJPEG is convenient, not authoritative.
Where the two conflict, the conformance suite wins and OpenJPEG stops being the
oracle *for that quantity* — matching it would mean importing its bug.

That is a narrow licence, not a general one. **Match OpenJPEG by default.**
Deviating needs two things, and one is not enough:

1. **Proof from a documented official source** — the ISO/IEC 15444 text, or the
   15444-4 conformance reference images. That the arithmetic looks cleaner is not
   proof.
2. **A better result.** Show the output moves *toward* the authoritative
   reference: a lower PAE or MSE against the conformance images, or a fixture
   that now decodes bit-exactly where it did not. Being more correct in principle
   while the numbers do not improve means the analysis is incomplete, and the
   change is not yet earned.

Assume first that a divergence is our bug. It almost always is.

There is one known case. On the inverse 9/7, the standard scales the high-pass
samples by `1/K` and the subband gain (Table E-1) enters the quantization step,
so a high-pass coefficient is scaled by `2 · (1/K) = 1.6257861…`. OpenJPEG's
decoder instead zeroes the gain and folds the factor of two into the transform:

```c
/* tcd.c */
/* BUG_WEIRD_TWO_INVK (look for this identifier in dwt.c): */
const OPJ_INT32 log2_gain = (!isEncoder && l_tccp->qmfbid == 0) ? 0 : ...;

/* dwt.c */
/* Due to using two_invK instead of invK, we have to compensate in tcd.c */
const float two_invK = 1.625732422f;
```

`1.625732422` is 3.3e-5 short of `2/K`. Adopting OpenJPEG's split makes a
multi-level 9/7 decode bit-exact against `opj_decompress` — and slightly wrong.
This crate keeps the exact constant, and `p0_09` decodes bit-exact against the
ISO reference image with it, which is the evidence that settles the question.

So a 9/7 fixture graded `tolerance: exact` against OpenJPEG must not carry detail
bands whose values reach a rounding boundary; grade multi-level 9/7 against the
conformance reference instead, or within a stated tolerance. Two consequences
worth carrying into any future disagreement:

- **A bit-exact match with OpenJPEG is evidence, not proof.** It can mean the
  decoder reproduced a defect. Ask what the standard says before celebrating.
- **Emulate the oracle's convention completely or not at all.** Changing `K` to
  match `two_invK` while leaving the gain in the step makes the divergence
  *worse*, because the factor of two is then counted twice. A half-emulation
  reads as a refutation of a correct hypothesis.

### Stricter than the oracle on non-conforming streams

Three tier-2 checks reject streams OpenJPEG decodes with a warning: an `Nsop`
that does not match the packet sequence (OpenJPEG never checks it), a signalled
EPH that is missing (OpenJPEG continues without consuming one), and tile-part
bytes left over after the last packet (OpenJPEG ignores them). These are
deliberate, and they are a different axis from the numeric rule above: each
condition means the packet walk has probably desynchronized, and continuing
risks a silently wrong image — the one outcome this crate never accepts. A
typed error preserves the oracle-match property; warn-and-continue would bet
the output on a parse the evidence says is broken.

Two parse-time checks are also stricter than the oracle, for a different
reason: the codestream is *missing data the standard requires*, and OpenJPEG
fills the gap with invented values. A QCD/QCC carrying fewer than the
3·NL + 1 step entries its decomposition needs is rejected as `Marker`;
OpenJPEG zero-fills the missing step sizes and decodes, reconstructing those
subbands at a scale the encoder never used. The 9/7 wavelet paired with the
no-quantization style is rejected the same way; OpenJPEG derives steps from
the style-0 exponents with mantissa zero. In both cases the oracle's output
is a deterministic guess, not the encoder's image, and this crate does not
half-decode. (The converses stay oracle-matched: extra step entries are
ignored, and 5/3 with a scalar style decodes — the reversible path never
reads the mantissas.)

A third parse-time check is in the same spirit: a `POC` volume whose resolution
or component range is empty (`REpoc <= RSpoc`, or the component range inverted)
is rejected as `Marker`. Table A-33 requires `RSpoc < REpoc`, so such a volume is
malformed, but OpenJPEG performs no check and simply lets that volume's packet
loops iterate zero times. Rejecting is the stricter, spec-faithful reading; a
volume that covers nothing is almost certainly a corrupt field, not one the
encoder meant.

The cost is interop: a real-world encoder that produces such a stream decodes
under OpenJPEG and rejects here. No corpus or conformance file trips any of
these today. If one ever does, relax that check to match the oracle — the
protective argument loses to a demonstrated real file.

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

The sharp case is `p0_01`. It is RLCP, and it was the only entry whose sole
remaining blocker was the progression order — so it looked like the test for
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

`p0_11` is the same trap wearing the precinct partition's name. It was the only
corpus entry the partition alone unblocked, so it looked like the test for it — but
it is 128×1 with **one** resolution and a 2^7 × 2^1 precinct, which is one
precinct. It grades that `SPcod`'s precinct bytes are *parsed* (get the count
wrong and the marker segment misreads) and nothing beyond that. Mutating the
partition confirms it: break the halving of the precinct onto the subband grid,
address the tag trees by band index instead of precinct index, drop the
code-block's cap to its precinct, or drop a tile's partial leading precinct, and
`p0_11` still decodes bit-exact in every case. Each of those mutations is caught
only by the synthetic `precincts_*` fixtures — and two of them by exactly one
fixture each, `precincts_varied_sizes_lossless` and `precincts_tiled_cprl_lossless`.
Adding `p0_11` to `IN_CLASS` is honest; treating it as the oracle for the
partition would not be.

The same reasoning cuts the other way: two features can be *indistinguishable*
rather than untested. With one precinct per resolution, PCRL and CPRL enumerate
the identical packet sequence — OpenJPEG's own output for the two differs in
exactly one byte, the progression code in COD. That was the state of this crate
until the precinct partition landed, and it is why `progression_pcrl_lossless`
and `progression_cprl_lossless` graded one order between them, not two. The
`precincts_*` fixtures are what separate them: with a real partition the position
axis has more than one value, PCRL interleaves the components inside one sweep of
the canvas and CPRL finishes a component before starting the next, and the two
packet streams diverge. A pair of features that cannot be told apart is worth
recording next to the code rather than mistaking one's fixture for coverage of
both.

A third shape is the nastiest, because the fixture looks like it targets the
feature by name. One feature can *mask* another. Under `restart` the MQ decoder
is re-initialised for every coding pass, so a decoder that ignores `segmentation
symbols` and never reads the four decisions ending each cleanup pass still
decodes correctly: the symbols it skipped are trailing bytes in a segment nobody
reads again. Turn `restart` off and the same decoder desynchronises immediately.
So a `restart | segsym` codestream — which is exactly what `p0_02` is — grades
`segsym`'s *verification* but not its *consumption*, and a fixture named for both
flags is weaker than one carrying `segsym` alone. Where features compose, the
fixture that isolates each one is the one that grades it.

So before adding an entry to `IN_CLASS`, ask what about that entry would have to
change for the new code to be wrong. If nothing would, the entry is not the
oracle for that feature, and the milestone needs a fixture that can tell the
difference — a multi-layer codestream to separate LRCP from RLCP, multiple
precincts to separate RPCL from RLCP, and so on. Mutating the new code and
watching the entry fail is the cheapest way to check; if no mutation makes it
fail, the entry is not testing it. Run the mutation against *every* fixture, not
just the one you expect to break: the fixtures that survive tell you which ones
were never testing the feature.

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
- the ISO/IEC 15444-4 conformance codestreams, which gated Phase 2 (all 23
  entries now decode in class);
- real `.jp2` files for Phase 3, HTJ2K codestreams for Phase 4.

## Robustness (the input is untrusted)

A decoder parses bytes it did not produce, so malformed input is a first-class
case, not an edge case:

- **Fuzzing** — `cargo fuzz` (libFuzzer) over the public `decode` entry point,
  in the detached [`fuzz/`](../fuzz/) workspace (run it per [`fuzz/README.md`](../fuzz/README.md)).
  The bar: no panics, no unbounded allocation, no infinite loops; every rejected
  input returns a typed `Error`, never crashes. A malformed SIZ cannot steer the
  buffers into an overflowing or out-of-memory allocation: the declared image
  area — `(Xsiz − XOsiz)·(Ysiz − YOsiz)`, on the reference grid an origin may sit
  anywhere inside — is bounded at parse time, and the Table A-9 constraints on the
  image and tile origins are enforced before any geometry reaches the DWT.
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
