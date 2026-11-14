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

That is a narrow license, not a general one. **Match OpenJPEG by default.**
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

There are several known cases; "Where the decoders differ" below lists them
all. The first is the inverse 9/7: the standard scales the high-pass
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

`1.625732422` is 5.4e-5 (3.3e-5 relative) short of `2/K`. This crate keeps the
exact constant, because it is the standard's: Table F.4 gives `K`, and the
subband gain of Table E-1 supplies the factor of two. Both decoders use the
nearest `f32` to each lifting constant of Table F.4, so `two_invK` is the only
difference: with it, this decoder is bit-exact against `opj_decompress` on
`issue206_image-000.jp2`, `issue208.jp2`, both `issue559-eci-*` files,
`db11217111510058.jp2` and `jpx_disguised_as_jp2.jp2`, and on a synthetic
sweep at 12, 14 and 16 bits.

The measurements support the standard's constant, though not uniformly:

- On near-lossless round trips (`opj_compress -I -n 6 -r 1`, whose forward
  transform uses the exact `K`; 64x64 images, nine patterns, two seeds each),
  this decoder returns the encoder's input more closely than `opj_decompress`
  in 18 of 18 cases at 16 bits, 15 of 18 at 14 and 12 of 18 at 12. OpenJPEG is
  closer, by a small margin, on smooth gradients and a random walk. At `-r 4`
  and `-r 20`, where quantization error dominates, the comparison is mixed and
  the margins are negligible. `tests/irreversible_16bit.rs` checks one 16-bit
  case.
- `p0_09` decodes bit-exact against its ISO reference with either constant, so
  it does not decide the question.
- On the lossy conformance entries the ISO references lean slightly the other
  way: OpenJPEG's constant lowers the MSE on 16 of the 26 graded components,
  raises it on 3 and leaves 7 the same. No change exceeds 0.72%, and no peak
  error moves (#104).

The error `two_invK` adds grows with the sample range, so the gap between the
two decoders grows with bit depth. "Agreement standard" below gives the
measured gaps.

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

## Agreement standard

- **Reversible path (5/3, lossless):** bit-exact sample equality. Any
  difference is a bug.
- **Irreversible path (9/7, lossy):** within a stated absolute tolerance per
  fixture, and within the compliance-class per-pixel and MSE bounds once we test
  against Part 4. Floating-point lifting means our result and
  OpenJPEG's will differ in the low bits; the tolerance is what makes the test
  meaningful rather than flaky.

The 9/7 difference from OpenJPEG depends on the bit depth, because OpenJPEG's
`two_invK` ("When the oracle and the standard disagree") adds an error that
grows with the sample range. Beyond that it depends on the content: how many
sharp edges the image has and how they line up with the transform grid. The
largest differences seen so far:

| Bit depth | Largest difference from `opj_decompress` |
| --------- | ---------------------------------------- |
| 8, 10, 12 | 1 |
| 14 | 2 |
| 16 | 7 |

The images: 128x128 single-component images coded by `opj_compress -I -n 6`
at rates 1, 4 and 20, in seven patterns (noise, a random walk, a checkerboard,
bars, two sizes of blocks and ragged edges) with two seeds each; and, at `-r 1`,
checkerboards of full-swing blocks from 1x1 to 5x5 on images from 32x32 to
256x256 with five or seven decomposition levels. The 7 is a 256x256
checkerboard of 5x5 blocks, and 3x3 blocks reach 6. The same 5x5 checkerboard
gives 5 at 64x64, 128x128, 200x200, 300x300 and 384x384, and at 512x512 and
1024x1024 with up to ten levels, so the gap does not simply grow with image
size. On all of them this decoder
stays within 1 of the encoder's input. Real files agree: the 8-bit
`issue206_image-000.jp2`, `issue208.jp2`, `issue559-eci-091-CIELab.jp2` and
`jpx_disguised_as_jp2.jp2` are within 1, and the 16-bit
`issue559-eci-090-CIELab.jp2` is within 2.

These are measured maxima, not a bound. For a rough scale: each high-pass
scaling is off by 5.4e-5 times the coefficient, high-pass coefficients range
about as widely as the samples, and a sample collects that error once per
decomposition level, so the gap stays roughly under 5.4e-5 × 2^depth × levels.
That is about 18 at 16 bits with five levels, 4 at 14, 1 at 12 and under 0.1
at 8. It is an estimate, not a hard bound. The measured gaps do not grow with
the level count. The committed `irreversible_16bit`
fixture reaches 5 on its full-swing component and 3 on its mid-range one, and
is graded at a tolerance of 5.

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

### The JP2 corpus

`tests/conformance_jp2.rs` grades the nine ISO/IEC 15444-4 JP2 reader files
(`file1.jp2` to `file9.jp2`, in `tests/fixtures/conformance/jp2/`) the same
way, at class 1, with the same ratchet: `IN_CLASS` lists all nine. Files 1, 4,
6 and 9 are bit-exact; the others are held to PAE 4 / MSE 1. Two more corpora
back it, neither needing a reference decoder at test time:

- the synthetic `jp2_*.jp2` files (`tests/jp2_synthetic.rs`), each graded
  bit-exactly against `opj_decompress` on the same file, or, where OpenJPEG
  fails, against a table lookup over its decode of the bare codestream;
- nine real-world files from GDAL's test data (`tests/jp2_real.rs`), written
  by Kakadu, the ECW JPEG 2000 SDK and OpenJPEG. Eight are graded against
  `opj_decompress` references, and one has a broken codestream and is
  refused with the codestream's own error.

The box metadata is checked against box dumps written by
`scripts/gen-jp2-box-snapshots.py`, a box walker independent of this
decoder: `metadata.json` for the real-world files, `boxes.json` (with
`edge_cases.json`, file 5 patched into the layouts no committed file has)
for the Part 4 files, and the `jp2_boxes` section of each synthetic file's
`expected.json`.

**The Part 4 JP2 references are display output.** `jp2_N.tif` is what a
viewer shows: sYCC and ICC-tagged files are already converted to sRGB, and
files 4, 6 and 8 are grayscale written as three identical channels. Files 2,
3, 5, 7 and 8 therefore can't be graded on the samples as coded. They are
decoded with `ColorOutput::Srgb` and graded against the references, and
their default decode is graded bit-exactly against `opj_decompress` output
for the same file (`as_coded` references). Files 6 (12-bit) and 7 (16-bit) have
8-bit references, so the harness shifts their decode right by 4 and 8 (the
references were made with `opj_decompress -p 8S`, a plain right shift). Beyond the references,
`opj_decompress -force-rgb` is the oracle for the conversion arithmetic
(see "sRGB output" above).

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
is re-initialized for every coding pass, so a decoder that ignores `segmentation
symbols` and never reads the four decisions ending each cleanup pass still
decodes correctly: the symbols it skipped are trailing bytes in a segment nobody
reads again. Turn `restart` off and the same decoder desynchronizes immediately.
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

The vertical oracle proves the *whole* pipeline; it does not localize a fault.
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
`<name>.expected.json` (image area, tolerance, per-component samples, provenance).

Seed it from:

- the GRIB2 §5.40 fixtures already in the fieldglass repo (e.g.
  `jpeg2000_regular_latlon.grib2`), plus a 9/7 re-encode for the irreversible
  path — no GRIB2 producer ships lossy 9/7 (HRRR/NDFD are complex-packed, ECMWF
  is CCSDS), so OpenJPEG's irreversible mode re-encodes a real grid;
- the ISO/IEC 15444-4 conformance codestreams, which gated Phase 2 (all 23
  entries now decode in class);
- the ISO/IEC 15444-4 JP2 reader files, synthetic `.jp2` files built over the
  codestreams above, and real `.jp2` files from GDAL's test data, which gated
  Phase 3 (see "The JP2 corpus" above); HTJ2K codestreams for Phase 4.

## Robustness (the input is untrusted)

A decoder parses bytes it did not produce, so malformed input is a first-class
case, not an edge case:

- **Fuzzing** — `cargo fuzz` (libFuzzer) in the detached [`fuzz/`](../fuzz/)
  workspace, seven targets: `decode` and `header` over the public entry
  points, and `tile_body`, `tier1_block`, `mq`, `jp2_boxes` and `icc_srgb`
  over the stages (run them per [`fuzz/README.md`](../fuzz/README.md)).
  The bar: no panics, no unbounded allocation, no infinite loops; every rejected
  input returns a typed `Error`, never crashes. A malformed SIZ cannot steer the
  buffers into an overflowing or out-of-memory allocation: the samples of the
  declared image — over the image area `(Xsiz − XOsiz)·(Ysiz − YOsiz)`, on the
  reference grid an origin may sit anywhere inside — are held to the sample
  limit before any tile is read (see [The sample limit](#the-sample-limit)),
  and the Table A-9 constraints on the image and tile origins are enforced
  before any geometry reaches the DWT.
- **Typed failures** — the flat `Error` enum names the stage that failed, so a
  malformed header, an out-of-scope feature, and a Tier-1 decode fault are
  distinguishable by a caller. No `unwrap`/`panic` on the decode path.
- **The reject matrix** — `reject_matrix_maps_every_out_of_subset_input_to_its_typed_error`
  in `src/codestream/tests.rs` enumerates every input outside the decoded subset
  and pins the typed error a caller sees: `Codestream` for structural damage,
  `Marker` for an illegally encoded field, `Unsupported` for valid JPEG 2000 we
  do not decode yet, `Limit` for an input past one of the decoder's resource
  guards. The JP2 box layer has its own matrix,
  `reject_matrix_maps_every_malformed_box_shape_to_its_typed_error` in
  `src/jp2/tests.rs`, where a malformed box is `FileFormat`, and the sRGB
  output step has `srgb_reject_matrix_maps_each_input_to_its_typed_error` in
  `src/jp2/srgb_tests.rs`. Rejecting is not
  a formality. A feature the decoder reads
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

## Where this decoder departs from OpenJPEG

Each section below records one place where this decoder and OpenJPEG 2.5.0
can give different answers on the same file. The first section is the index.

### Where the decoders differ

Every known place where this decoder and OpenJPEG 2.5.0 give a different
result on the same file:

- the inverse 9/7's `2/K` constant ("When the oracle and the standard
  disagree" above);
- the bypass switch under an RGN maxshift ("Bypass under a maxshift");
- a tile-part POC that replaces the main header's (#269; "A tile-part POC
  overrides the main header's");
- streams this decoder refuses where OpenJPEG warns or reads on ("Stricter
  than the oracle on non-conforming streams"), and POC sequences that leave
  out packets ("Packets a POC sequence leaves out");
- mixed tile-part counts around the `TPsot == TNsot` correction, and a later
  tile-part's COD, COC, QCD or QCC that differs from the value in force,
  both refused here ("Encoder mistakes OpenJPEG corrects");
- signed 32-bit components, which OpenJPEG refuses ("Signed 32-bit
  components");
- an image of more than 2^28 samples at the decode's reduction, which this
  decoder refuses by default and OpenJPEG decodes if it can allocate it
  ("The sample limit"), and files past the work bounds ("The tier-2 work
  bound", "The tier-1 work bound"); each has an option that raises it;
- under `ColorOutput::Srgb`: sYCC with the IEC constants, rounded, and the
  color channels picked by their `cdef` labels, not by position ("sRGB
  output"; the table there gives every refusal);
- the three cases below.

The three below are files that follow the standard and that OpenJPEG reads
another way. In each case the standard wins (rule 1 under "sRGB output").

- **More than 999 layers.** OpenJPEG starts every tag-tree node at the
  value 999 (`tgt.c`, `opj_tgt_reset`), so at a layer threshold of 1000 or
  more it takes a code-block not yet included as included, without reading
  its inclusion bit, and reads the rest of the packet header out of step.
  This decoder starts a node unresolved (`UNRESOLVED = u32::MAX` in
  `src/tier2/tagtree.rs`) and reads the bit, as B.10.2 says. `COD` allows
  65535 layers. A file with a code-block first included at layer 999 or
  later decodes here; OpenJPEG refuses it ("segment too long (3) with max
  (0) for codeblock 1998" on a 2048^2 file of 1000 layers and two new
  code-blocks a layer) or returns a wrong image (a 4x4 image of 1000 layers
  whose `jp2c` is followed by an `xml ` box gives
  `[128, 255, 255, 255, 128, 0, ...]` from that box's bytes, where every
  sample is 128). Up to 999 layers the two agree bit for bit on the same
  files. `tests/many_layers.rs` grades each such file against a twin of
  the same code-block data in a few layers, which OpenJPEG decodes, up to
  the 65535 layers `COD` allows. So OpenJPEG is not an oracle above 999
  layers.
- **A `Psot = 0` codestream followed by a box that ends in `FF 90`.** A
  single-tile codestream whose last tile-part has `Psot = 0`, in a `jp2c` of
  the right length, followed by a box whose last two bytes are `FF 90`,
  decodes here: the `jp2c` box holds the codestream. OpenJPEG reads the
  tile-part to the end of the file and fails ("failed to decode image").
- **A header box at the top level before `jp2h`** (owner decision,
  2026-11-11). Such a `colr`, or another header box, is always ignored,
  since the standard puts them inside `jp2h`. OpenJPEG reads it after
  any unknown box: its `JP2_STATE_UNKNOWN` is `0x7fffffff`, and ORing it
  into the state flags (`jp2.c`) marks every state as reached, so the
  `HEADER` test passes. The code warns that such a box "will be ignored",
  and then reads the next one; it is a mistake, not a rule.

### Bypass under a maxshift

The second case (#258). Under the bypass code-block style, a code-block's
significance and refinement passes switch to raw coding from its fifth coded
bit-plane on (D.6, Table D.9), counted from its first coded plane. Under an RGN
maxshift the shifted planes are coded planes (H.2): the first one is
`top = Mb + SPrgn - zero_bit_planes`. Tier-2 splits the segments by pass count
(10 passes, then 2, 1, 2, 1, …), which agrees with that count.

OpenJPEG's decoder starts at the same plane, `bpno_plus_one = roishift +
cblk->numbps`, but tests `bpno_plus_one <= cblk->numbps - 4` (t1.c,
`opj_t1_decode_cblk`). `cblk->numbps` leaves the shift out, so the switch comes
`SPrgn` planes late and the decoder MQ-decodes segments that were written raw.
Its own encoder counts from the first plane it codes (`bpno < cblk->numbps - 4`
in `opj_t1_encode_cblk`), which is the plane the decoder calls `top`. So
`opj_compress -ROI` with any bypass style writes a stream that `opj_decompress`
2.5.0 decodes wrong. This crate counts from `top`
(`src/tier1/passes.rs`, `decode_block`).

**Evidence.** The encode is lossless, so the source image is the reference. A
sweep of `opj_compress -i source.pgm -n {1,3,6} -ROI c=0,U={1,3,7,12} -M m` over
`m` in {0, 1, 3, 5, 9, 17, 33, 49, 63} (108 encodes of
`tests/fixtures/roi_bypass/source.pgm`):

| Decoder | Equal to the source |
| ------- | ------------------- |
| `opj_decompress` 2.5.0 | 12 of 108: the `-M 0` encodes only |
| this crate, counting like OpenJPEG | 12 of 108; 60 wrong, 36 errors |
| this crate, counting from `top` | 108 of 108 |

Without a maxshift both counts are the same, so no other fixture changes.
`tests/roi_bypass.rs` pins four of these encodes (`-M 1`, `5`, `33`, `17`, at
`U=3`, `-n 3`) against the source; provenance is in
`tests/fixtures/roi_bypass/README.md`.

**OpenJPEG's history.** The decoder rule has been there since the initial
import (2ad6a977, 2003, `libopenjpeg/t1.c`: `bpno = roishift + cblk->numbps - 1`
and `bpno <= (cblk->numbps - 1) - 4`). `git log -L` on the line shows only
renames, reformatting and sign-conversion fixes since, with no commit that
gives a reason. The encoder's rule is from the same import; the 2017 bypass
and termination fixes (81c53117, upstream #674) changed the encoder's
termination and left the decoder's raw decision alone. No upstream issue or
pull request covers ROI with bypass. Upstream ROI has no tests (#1035, "Add
unit tests for ROI feature", open since 2017). Its encoder shifts no
coefficients: `-ROI` only signals `SPrgn`, so the decoder reads every nonzero
coefficient of the component as region of interest (#1036, open). The rule
reads as an oversight, not a choice, so this crate follows the standard.
Recorded with the owner's approval on #226.

### Segmentation symbols

Under the `segmentation symbols` style each cleanup pass ends with four
decisions that spell `1010` (D.5). D.5 makes the symbol an error-detection
aid, and OpenJPEG never checks it (t1.c, the comparison is commented out).
This crate checks it, but not everywhere:

- A wrong symbol is a `Codestream` error while the MQ decoder has read only
  the segment's own bytes. Those bytes then decide the symbol, so they are
  corrupt.
- On a terminated cleanup pass a wrong symbol is an error even past the end.
  Its length is exact, and the MQ termination makes the codeword decode
  correctly from the fill. The terminated cleanups are the ones t1.c's
  `opj_t1_enc_is_term_pass` terminates: every pass under `restart`, the
  block's last cleanup (plane 1 at this crate's double scale), and under
  bypass every cleanup from the fourth coded plane down (`bpno - 1 <=
  raw_from` in `decode_block`, so counted from `top`, maxshift included).
  Predictable termination changes how a pass is terminated, not which.
  The last-cleanup rule holds for encoders that shift the region's
  coefficients as H.2 describes. `opj_compress -ROI` shifts none, so its
  last terminated cleanup lands at plane `SPrgn + 1`, not 1, and the last
  pass of such a stream goes unchecked. A lossless `-M 32 -ROI c=0,U=3`
  stream accepts 134 single-bit flips as wrong images that checking every
  symbol refuses; OpenJPEG decodes all of them too, and no valid stream is
  refused. Treating plane `SPrgn + 1` as terminated would refuse a standard
  maxshift encoder whose output is rate-cut at that plane, so it is not
  done.
- On any other cleanup pass, once the decoder has read past the end of the
  segment, a wrong symbol decodes. Rate control cuts such a pass at an
  estimated length: `opj_compress` uses the bytes written so far plus three
  (t1.c, `opj_t1_encode_cblk`), which can be a few bytes short. The decoder
  then reads the symbol from the `0xFF` fill and gets `0b1001`.

"Past the end" is `MqDecoder::ran_past_end`: the code register has taken in a
byte the decoder synthesized. It is set by the read that moves the byte
pointer to the segment length, or by the marker test after a final `0xFF`.
That is the first read of fill, and it can come up to about 16 code bits
before any decision depends on fill, so the flag errs early. It is often set
by the end of a segment even in an intact stream, because the MQ decoder
reads ahead. So on an unterminated pass the check mostly catches damage in the
earlier passes of a segment, not in its last one.

**Evidence.** 459 random `opj_compress -M 32|m -r ...` encodes, with `m` from
bypass, reset, restart, vertically causal and predictable termination, layer
counts from one to three, and 5/3 and 9/7. Checking every symbol, the crate
refused 10. With the rule above it refuses none, nine of the ten match
`opj_decompress` exactly, and the tenth (9/7) is within 1. A second run of
1,664 encodes (1,350 with `-r`, 314 with `-ROI`, the same style mix) refuses
none either, and every sample mismatch with `opj_decompress` is explained:
9/7, the sYCC-to-RGB conversion `opj_decompress` applies to three components
with sub-sampled chroma, or OpenJPEG's own maxshift + bypass bug (above).

Single-bit flips (`0x01`, `0x10`, `0x80`) of every body byte of
`cblksty_segsym_lossless` and `roi_bypass_m33` are refused and accepted
exactly as when every symbol is checked. Tolerating fill on every cleanup
outside `restart` would have accepted 250 more of the 2,580
`cblksty_segsym_lossless` flips, 229 of them decoding to a wrong image. A
narrower rule, which tolerated fill on the block's last decoded pass only,
refused one of the ten: in a two-layer encode the cut pass was followed by
passes with no bytes of their own. `cblksty_segsym_truncated` pins one such
encode. Its provenance is in `tests/fixtures/README.md`.

**Coverage gap.** No fixture has a background code-block (`zero_bit_planes`
at least `SPrgn`) under a maxshift with bypass. `opj_compress -ROI` makes the
whole component the region, and no conformance file has RGN with bypass. That
case rests on the standard and the model of `opj_t1_enc_is_term_pass` only:
all 64 `-M` styles over first coded planes 0 to 39 agree with it.

### A tile-part POC overrides the main header's

The third case (#269). A.6.6 orders the markers that set a tile's progression,
"Tile-part POC > Main POC > Tile-part COD > Main COD", "where the "greater
than" sign, >, means that the greater overrides the lessor marker segment".
It also says a tile's POC "must appear in the first tile-part header of that
tile". B.12.3 says the same: "The COD progression order and the main header
POC marker segment (if there is one) are overridden." So a tile whose first
tile-part has a POC walks only its own volumes, accumulated over all its
tile-parts in order, and a tile without one walks the main header's. This
crate does that (`resolve_tile_header` in `src/codestream/mod.rs`).

OpenJPEG copies the main header's coding parameters into each tile, POC
volumes included (`opj_j2k_copy_default_tcp_and_create_tcd`), and
`opj_j2k_read_poc` then appends a tile-part's volumes after the ones already
there (`l_old_poc_nb = l_tcp->POC ? l_tcp->numpocs + 1 : 0`). A tile with both
walks `[main volumes, tile volumes]`, and the walk takes each packet from the
first volume that reaches it, so the main volumes decide the order of every
packet they cover.

**Evidence.** `scripts/gen-poc-precedence-fixtures.py` encodes a 24x16,
three-component, three-layer image losslessly with `opj_compress -POC
T1=0,0,3,1,3,CPRL/T1=1,0,3,3,3,RLCP`, which writes the two volumes in the
tile-part header. `opj_decompress` 2.5.0 round-trips that file exactly. The
script then adds a main-header POC of one LRCP volume over resolution 0, every
component and every layer. LRCP and CPRL order resolution 0's nine packets
differently, so only the tile-part POC describes the packets as written.

| Decoder | `poc_main_and_tile.j2k` and `poc_main_and_split_tile.j2k` |
| ------- | ---------------------------------------------------------- |
| `opj_decompress` 2.5.0 | no error; 380, 383 and 384 of 384 samples wrong per component |
| this crate, appending | `Codestream`: "tile-part has 114 body byte(s) left after the last packet" |
| this crate, overriding | equal to the source |

The split file puts the second tile volume in its own POC in the second
tile-part header. `tests/poc_precedence.rs` grades both against the source;
provenance is in `tests/fixtures/poc_precedence/README.md`. No conformance
file or other committed fixture has a POC in both the main header and a
tile-part header, so none of them changes.

**A POC first seen in a later tile-part.** A tile whose first tile-part has no
POC, but a later one has, breaks A.6.6. Its first packets can only have
followed the main header's order, so this crate reads it as OpenJPEG does:
the main volumes, then the tile's. The 32-volume budget counts both.
`poc_main_then_later_tile_part.j2k` is such a file, and both decoders return
the source. So this crate differs from OpenJPEG only on files that follow
A.6.6.

The coverage check (see "Packets a POC sequence leaves out" below) reads the
same list the walk does. `poc_tile_leaves_out_packets.j2k` has a main POC that
covers the tile and a first-tile-part POC that leaves out two resolutions; it
is refused as `Codestream`, where OpenJPEG walks the main volume and returns
a wrong image.

**OpenJPEG's history.** The initial revision (2ad6a977, 2003) already read
tile-part POCs, and it *replaced*: `j2k_read_poc` wrote from index 0 and set
`numpocs = numpchgs`, and every SOT re-copied the default tcp. OpenJPEG 0.8
(4be82998, 2004) added two things in one commit: a `tcp->first` guard, so the
default tcp is copied into a tile once rather than at every SOT, and the
append (`old_poc = tcp->POC ? tcp->numpocs+1 : 0`). The append lets the POCs
of a tile's tile-parts build one list, which B.12.3 allows. The main header's
volumes ending up in that list is a side effect of the copy-once seed. The
append was carried into the v2 reader (1d45d751, 2008) and since then only
renamed, reformatted (563bd849) and given the `J2K_MAX_POCS` constant
(b8c4b450). No commit gives a reason, and no upstream issue covers a main and
a tile-part POC together. OpenJPEG's encoder never writes both kinds: it
writes a main-header POC only for the 4K cinema profile
(`opj_j2k_write_poc`), and a tile-part POC only outside the cinema profiles
(`opj_j2k_write_first_tile_part`). So its own files decode the same under
either rule. The append reads as an oversight, not a choice, so this crate
follows the standard.

### Code-block contributions of zero bytes

A packet may include a code-block with coding passes and a length of zero.
B.10.7.1 gives the length as a plain binary number ("The length of a codeword
segment is represented by a binary number of length: bits = Lblock +
⌊log2(coding passes added)⌋") and sets no lower bound. B.10.7 also says how a
contribution can end up empty: "in the event that an 0xFF would have appeared
at the end of a code-block's contribution to some packet, the 0xFF may be
safely moved to the subsequent packet which contains contributions from the
code-block, or dropped if there is no such packet." The B.10.8 header order
reads the length for every included block, with no zero case. D.4.1 says how
such passes decode: "the decoder shall extend the input bit stream to the
arithmetic coder with 0xFF bytes, as necessary, until all symbols have been
decoded."

So the decoder accepts passes with no bytes, in one layer or in every layer
that includes the block. The passes count and fill their codeword segment, so
a later layer's passes and lengths go to the right segment, under `restart`
and `bypass` too. A segment that never gets a byte is decoded from the fill,
as OpenJPEG 2.5.0 decodes it (`opj_mqc_init_dec` with a length of 0). Until
this was settled the decoder refused a block whose passes never got a byte,
which stopped openjpeg-data's `kodak_2layers_lrcp.j2c` (DCP-Werkstatt) on
2153 such blocks. That file now reaches the leftover-bytes refusal below.

One case is still refused. Under segmentation symbols, an empty segment whose
cleanup pass counts as terminated (under `restart`, on plane 1, or from the
fourth coded plane down under `bypass`; see "Segmentation symbols") reads its
symbol from the fill, gets a wrong value, and is refused as `Codestream`.
OpenJPEG decodes it. No encoder could have written that segment: a terminated
cleanup ends with a correct symbol in bytes of its own. So the decoder returns
an error rather than an image built on a segment that cannot be right.

A contribution of zero bytes reads its header bits like any other, and its
passes cost the same against the code-block work bound as passes with bytes.
`zero_length_contribution` and `zero_length_contribution_bypass` pin the
cases; `tests/work_limit.rs` pins the bound.

### Stricter than the oracle on non-conforming streams

Three tier-2 checks reject streams OpenJPEG decodes with a warning: an `Nsop`
that does not match the packet sequence (OpenJPEG never checks it), a signaled
EPH that is missing (OpenJPEG continues without consuming one), and tile-part
bytes left over after the last packet (OpenJPEG ignores them). A tier-1 check
is of the same kind: under predictable termination every MQ segment of a
code-block must end as the termination predicts, where OpenJPEG checks only
the last segment and only warns (`decode_block` in `src/tier1/passes.rs`; see
also "Segmentation symbols" above). So is a wrong segmentation symbol read
from the fill of an empty terminated segment, which OpenJPEG decodes (see
"Code-block contributions of zero bytes" above). These are deliberate, and they are a
different axis from the numeric rule above: each condition means the packet
walk has probably desynchronized, and continuing risks a silently wrong image
— the one outcome this crate never accepts. A typed error preserves the
oracle-match property; warn-and-continue would bet the output on a parse the
evidence says is broken.

`opj_compress` writes two patterns these checks refuse:

- overlapping POC volumes (`-n 1 -POC T1=0,0,1,1,1,LRCP/T1=0,0,1,1,1,PCRL`).
  OpenJPEG's encoder resets its include array for each tile-part (t2.c:248),
  so it writes the packets twice and the file doubles in size. OpenJPEG's
  decoder skips the repeats, ignores the bytes left over, and matches the
  source. This decoder refuses it ("tile-part has N body byte(s) left after
  the last packet"). A partial overlap
  (`-r 4,2,1`, layer ranges 2 then 3) is refused with "tile-part ends before
  the last packet";
- `-TP R` or `-TP C` with empty resolutions (`-F 2,99,1,1,u -n 6 -t 32,64
  -d 22,14 -TP R`). The encoder writes a stray `0x80` byte into each
  tile-part of an empty resolution. This decoder refuses it; OpenJPEG decodes
  it silently to a different image (101 of 198 samples differ), so refusing
  is the better outcome here.

With `strict` off, both decode with the leftover bytes skipped and reported
(see "Partial decode" below).

Real files from other encoders that these refusals stop:

- ECW JPEG 2000 SDK 6.0 (ERDAS APOLLO GeoCompressor 2021) numbers its SOP
  markers out of sequence. jpylyzer-test-files'
  `erdas-nullinput-uint8-rgb-null-2tileparts.jp2` is refused with "SOP
  sequence number 50 for packet 0, expected 0"; OpenJPEG decodes it.
- openjpeg-data's `kodak_2layers_lrcp.j2c` (DCP-Werkstatt) has 975304 bytes
  left after its last packet, refused with "tile-part has 975304 body
  byte(s) left after the last packet"; OpenJPEG ignores them. With the check
  turned off, this decoder's image is within 1 of OpenJPEG's (it is 9/7).

A tile-part that ends before its last packet is refused too ("tile-part ends
before the last packet"). That is not a desynchronized walk: B.11 lets "any
number of whole packets (in order)" be dropped from the end of a tile, so
this is the strict decode refusing missing data, which partial decode
accepts with the missing packets reported (see "Partial decode" below). openjpeg-data's `Marrin.jp2`
(Kakadu 5.2.1) declares two layers, but its one tile-part's `Psot` covers
only the packets of layer 0. OpenJPEG decodes the packets it finds, with no
warning.

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

Where a file has two faults, the two decoders can name different ones; both
refuse it. A code-block size past the limit in any COD or COC, main or
tile-part header, is `Marker` when that marker is read, as OpenJPEG refuses
it there. When the same COD or COC also has fewer levels than the
`resolution_reduction` asks for, OpenJPEG reports the reduction and this
decoder reports the code-block size. Matching it would mean passing the
reduction into every COD and COC parse.

The stricter checks in this section have a cost in interop: an encoder that
produces such a stream decodes under OpenJPEG and is refused here. No corpus
or conformance file trips them today, but `opj_compress` writes the two
tier-2 patterns listed above. If a real file ever needs one relaxed, relax
it to match the oracle: the protective argument loses to a demonstrated real
file.

### Packets a POC sequence leaves out

A `POC` sequence whose volumes leave out packets of a tile is refused as
`Codestream` before its packets are read. A.6.6 requires the volumes to define
every packet: "In the case where a POC marker segment is used, the progression
of every packet in the codestream (or for that tile of the codestream) shall
be defined in one or more POC marker segments", and B.12.3 adds that "there
will never be the case where a progression order volume is filled and the next
one is not defined". The volumes are the ones the tile walks: its own when its
first tile-part has a POC, which replaces the main header's (see "A tile-part
POC overrides the main header's"), and otherwise the main header's and then
any later tile-part's. A tile has a packet for every layer, every resolution
each component has (its own `COD` or `COC` level count) and every precinct of
it (B.6, B.9). A precinct or resolution with no code-blocks still has its
packets, which B.9 says must appear as empty packets, so a `POC` that skips it
is refused. A resolution with no precincts on the tile has no packets and
needs no volume. "The layer index always starts at zero" (A.6.6), so a
component's resolution has all its packets only when one volume reaching the
last layer covers it; the check costs `components × (volumes + resolutions)`
steps.

Such a file is not out of the standard in every reading, though: B.11 lets
"any number of whole packets (in order)" be dropped from the end of a tile,
and B.12.3 lets "the last progression order volume in each tile" be
incomplete. A tile whose volumes leave packets out is byte for byte a tile
whose covering volumes had those packets dropped. So this is the strict
decode refusing missing data, as it refuses a tile cut short; partial decode
accepts it, with the missing packets reported.
OpenJPEG decodes such a tile with them empty, and `opj_compress` writes such
a file when its `-POC` volumes do not cover the tile: `-n 2 -POC
T1=0,0,1,1,1,LRCP` leaves resolution 1 out, and OpenJPEG's decode of it has
every sample wrong (`tests/fixtures/poc_coverage/`).

The refusal holds under a `resolution_reduction` too, when the packets left
out are only those of resolutions the reduction discards. That is stricter
than `opj_decompress -r`, which gives the exact reduced image, and it is
consistent with refusing a tile cut short (B.11) at a reduction of 1, which
this decoder already does. Partial decode reports such a tile complete at
the reduction that drops the missing resolutions, and its image there is the
strict decode of a file whose volumes cover the tile.

The check is on the volumes' axes, not on each precinct: a positional order's
sweep can pass over a precinct with mixed sub-sampling (see "The tier-2 work
bound"), with or without a `POC`, as OpenJPEG's encoder and decoder both do,
and that is not refused. `read_header`'s walk of a `Psot = 0` tile does not
make the check: the packets the volumes reach end where they end, so for
such a tile in a JP2 file the codestream ends at its box and the boxes after
`jp2c` are reported, as for the raw codestream, which the decode refuses.
381 `opj_compress -POC` encodes whose volumes do cover the tile (two or three
volumes split by resolution or component, every order, 1 to 4 components,
some sub-sampled, 1 to 3 layers) decode as before.

### Encoder mistakes OpenJPEG corrects

Three mistakes real encoders make are corrected by OpenJPEG 2.5.0 on
purpose, and this decoder corrects them the same way. Each rests on rule 2
of the owner's rules (see "sRGB output" below: the standard first, then the
error correction a user would expect, OpenJPEG usually the guide, and
otherwise an error rather than a wrong image). In each case the standard
says the file is wrong but not what it means, OpenJPEG's correction is
deliberate (a named function, a warning, a fixed issue), and the checks that
follow still refuse a file the correction reads wrongly. Fixtures:
`quirk_*_lossless` in `tests/fixtures/`, from
`scripts/gen-encoder-quirk-fixtures.py`; tests in `tests/encoder_quirks.rs`
and the `encoder quirks` block of `src/codestream/tests.rs`.

**`TPsot == TNsot`.** A tile's tile-parts are numbered 0 to N with N
declared, so its last part has `TPsot` equal to `TNsot`. Adobe Photoshop's
plug-in writes this. OpenJPEG warns "Non conformant codestream
TPsot==TNsot" and adds one to every count
(`opj_j2k_need_nb_tile_parts_correction`, `j2k.c:9410`, called at
`j2k.c:9701`; its issue 254). Its trigger is copied exactly
(`count_is_one_short` in `src/codestream/mod.rs`): once per codestream,
when the first tile has all the parts it declares, the `SOT` chain is
walked onward, stepping over other tiles' parts by `Psot`, to the next part
of that tile. If that part has `TPsot == TNsot`, every count stated so far
and every count stated after is raised by one. The walk finds nothing at a
marker other than `SOT`, at a `Psot` below 14 on another tile's part, or at
a `Psot` past the end. It is safe because the raised counts are held to the
parts that arrive like any other count, and the parts are still read in
`TPsot` order.

The two decoders differ where the counts are mixed. When the correction is
on, a tile whose own count was right now carries one part fewer than its
count, and is refused as a tile with a missing part ("tile 3 declares 4
tile-parts but carries 3"). OpenJPEG decodes it as it decodes a truncated
tile, which here gives the full image. When the first tile to complete is
counted right and a later tile has an extra part, the correction is not
made, and the extra part is refused ("tile 1 tile-part 2 of a declared 2
tile-parts"). OpenJPEG decodes that tile from the parts before it and drops
the last one: 2293 of the 3072 samples then differ from the source. Both
were measured on `quirk_tpsot_tnsot_lossless` with its counts edited.

**A COD, COC, QCD or QCC restated in a later tile-part.** A.4.2 puts these
in a tile's first tile-part. BAP Image Systems writes the main header's COD
and QCD into every tile-part. OpenJPEG reads them in any tile-part header
(`J2K_STATE_TPH` in its marker table, `j2k.c:1411`) and applies them to the whole tile;
it removed its "one COD per tile" check on purpose so that files like these
decode (`opj_j2k_read_cod`, the `#if 0` at `j2k.c:2688`; its issue 1043, a
BAP bulk-scanner file). Here a later
part's marker is accepted only when it changes nothing in either reading,
and is then dropped (`check_restatement`):

- a COD must equal the tile's COD in force (its own, else the main
  header's), and every component of the tile must already code as it says,
  since OpenJPEG writes a COD over every component. So a tile or main COC
  that differs from the COD makes the restatement a refusal: this decoder
  would keep the COC and OpenJPEG would overwrite it;
- a QCD likewise, against the QCD in force and every QCC;
- a COC or QCC must equal what its component already has.

A marker that differs is `Codestream` ("a later tile-part of tile 0 carries
a COD that differs from the one in force"). OpenJPEG applies it to the
whole tile, including the parts read before it; which values the encoder
meant is not in the file, so this decoder does not guess. An RGN in a later
part is still refused, as before. This is safe because an accepted
restatement leaves every value the decode reads as it was.

**The color transform on one or two components.** `SGcod`'s MCT byte set
on an image of fewer than three components (openjpeg-data `123.j2c`). The
transform is defined over components 0, 1 and 2 (G.2), so it has nothing
to apply to. OpenJPEG says "Number of components (1) is inconsistent with a
MCT. Skip the MCT step." and decodes the components as coded
(`opj_tcd_mct_decode`, `tcd.c:2188`), the only image such a file can
describe. This decoder skips it too (`mct::applies`); before, it refused the
file as `Marker`. Three or more components whose sub-sampling or sizes
differ are still refused, as OpenJPEG refuses them (`tcd.c:2085-2120`).
Three that differ only in depth or sign are refused too; OpenJPEG decodes
those, so that refusal is this decoder's own stricter choice
(`mct::check_geometry`).

Real files, decoded with `opj_decompress` 2.5.0 from their bare
codestreams and compared sample by sample (the files are not committed;
their licenses vary):

| File | Quirk | Result |
| ---- | ----- | ------ |
| openjpeg-data `issue235.jp2`, `issue254.jp2`, `text_GBR.jp2` | `TPsot == TNsot` | bit-exact (5/3) |
| openjpeg-data `issue206_image-000.jp2`, `issue208.jp2`, `issue559-eci-091-CIELab.jp2`; jpylyzer `jpx_disguised_as_jp2.jp2` (and `wrong-number-of-tileparts.jpf`, byte-identical to it) | `TPsot == TNsot` | within 1 (9/7) |
| openjpeg-data `issue559-eci-090-CIELab.jp2` | `TPsot == TNsot` | within 2 (9/7, 16-bit) |
| openjpeg-data `issue390.jp2` | `TPsot == TNsot` | both refuse a later fault |
| openjpeg-data `db11217111510058.jp2` | COD and QCD restated | within 1 (9/7) |
| openjpeg-data `123.j2c` | MCT on one component | bit-exact (5/3) |

`issue559-eci-090-CIELab.jp2` decodes the same, in both decoders, with its
counts edited to be right, so its difference of 2 (740 of 89888 samples of
component 0) is the 9/7 difference at 16 bits, not the correction. Every
other committed fixture and conformance file decodes as it did before.

The difference of 2 comes from the `2/K` constant. With both decoders dumping
each stage of every tile-component, the dequantized coefficients agree
bit-for-bit (all 269664 of them), and the first inverse-DWT level is the first
stage that differs: 1121 of its 1134 values, by up to 1.08. The gap grows to
1.91 at the last level, before rounding. With OpenJPEG's `two_invK` in place
of `2/K`, every stage agrees bit for bit and the file decodes bit-exact.

### Partial decode

With `DecodeOptions::strict` off (#214) the decoder takes missing data and
bytes left after a tile's last packet, as `opj_decompress -allow-partial`
does, and reports them on `Image::report`. Everything else is refused as
when strict. A strict decode is unchanged: its output and error text match
the decoder before this option on every committed fixture and the round-2
corpus at reductions 0 to 3.

The oracle is `opj_decompress -allow-partial` (OpenJPEG 2.5.0), on the
fixtures in `tests/fixtures/partial/` (`scripts/gen-partial-fixtures.py`,
provenance in that directory's README) and checked by
`tests/partial_decode.rs`:

- **Prefixes.** For a one-tile LRCP codestream of five levels, the shortest
  prefix that `opj_decompress -allow-partial -r r` decodes to the whole
  file's image is, for each r from 0 to 5, the shortest prefix this decoder
  reports complete at r, and both decoders give the same samples for it and
  for the prefix one byte shorter. A strict decode refuses that shorter
  prefix; a partial one reports it complete at r + 1, where its image equals
  the whole file's. Every prefix from the first `SOT` marker on decodes, and
  a prefix the report calls complete at r gives the whole file's image at r.
- **Missing tiles** are 0 after the DC level shift, as OpenJPEG writes them.
- **`Psot = 0` without an `EOC`** (`tests/fixtures/psot0/`) matches OpenJPEG.
- **Overlapping `POC` volumes**, whose duplicate packets leave bytes over,
  decode to the source, as OpenJPEG's do, and report `LeftoverBytes`.
- **`-TP R` with empty resolutions** decodes to OpenJPEG's image, which is
  not the source: on the committed fixture 117 of 198 samples differ from it
  (107 on the file measured when this was designed). The phantom bytes sit
  at tile-part boundaries and nothing tells them from packet data, so the
  report gives `LeftoverBytes` and the tile counts as incomplete at every
  reduction.

On real files from the round-2 corpus, not committed: openjpeg-data's
`Marrin.jp2` (a `Psot` that covers one of two layers) and
`kodak_2layers_lrcp.j2c` (975304 leftover bytes) are within 1 of OpenJPEG
(both are 9/7), and GDAL's `small_world_truncated.jp2` (cut inside its tile)
matches it exactly.

One difference from OpenJPEG is deliberate. When the data ends inside a
packet's body, B.11 allows only whole packets to be dropped, so a cut packet
has no meaning of its own. This decoder keeps each code-block whose
contribution to the packet is all there, and for the rest keeps only the
layers before the cut packet, so each code-block decodes as a codestream
truncated after some whole layer would. OpenJPEG keeps the bytes that are
there for every code-block of the cut packet, a block's data cut mid-layer
included. The two images can differ in the code-blocks the cut packet
reaches; the report says the tile is incomplete either way.
`a_cut_packet_keeps_each_code_blocks_whole_layers` in
`src/tier2/partial_tests.rs` checks each code-block of a cut three-layer
packet against layer-limited walks.

A `POC` sequence that leaves packets out (`tests/fixtures/poc_coverage/`)
decodes with those packets empty and reported. At the reduction that drops
them the image matches OpenJPEG's. At full resolution the two differ (1023 of
1024 samples for `poc_res_missing.j2k`) because they fill the gap
differently. OpenJPEG decodes a tile only up to the highest resolution it
read a packet of (`resno_decoded`, t2.c:518) and pastes that smaller image
into the top-left corner of a zeroed full-size frame. This decoder sets the
missing coefficients to zero and runs the whole inverse wavelet, so its
even-indexed samples are the reduction-1 image and its largest error against
the source is 193, where OpenJPEG's is 993. Either way the report says the
tile is incomplete, with a `complete_reduction` of `Some(1)`.

`TNsot` is advisory: a count that disagrees with the tile-parts found is
reported and the tile decodes from them. The decode needs the main header
through the first `SOT` marker; a shorter prefix is an error.

Partial decode bounds its work like a strict one, with two more charges
against the packet work bound: each tile-component of a missing tile costs
one step, and the end of a tile's data costs 32 steps for each of its
(component, resolution) pairs, with one more for each packet after it.
Without them, a header of a few kilobytes that declares 65535 tiles of 16384
components with none present took 38 s, and a run of empty tile-parts, each
making its tile set up its geometry for no data, took 19 s. With them the
first is refused before any tile is decoded and the second is refused in
under a second (`missing_tiles_are_charged_against_the_work_bound` and
`the_end_of_each_tiles_data_is_charged_against_the_work_bound` in
`tests/partial_decode.rs`). The end-of-data charge falls on every tile whose
data ends, so a prefix cut late in a codestream that interleaves the
tile-parts of very many tiles can be refused with `Error::Limit` though the
whole file decodes: each tile whose later parts have not arrived is charged.
`DecodeOptions::packet_work_limit` raises the bound for such a stream. The
report holds at most five entries per tile-part, plus two.

A packet header packed in a `PPM` or `PPT` marker that reads past the end
of the packed bytes is an `Error::Codestream`, with `strict` on or off.
Those bytes are in the main or tile-part headers, which are there whole or
not at all, so running past them is damage, not the data ending. That
includes a header whose last byte is 0xFF with no stuffed byte after it
(B.10.1). OpenJPEG refuses that case too (`opj_bio_inalign`, t2.c:1323),
but reads the other missing bits as 0 and decodes with no error; that is
not a deliberate correction, so this decoder refuses
(`tests/packed_overrun.rs`, fixtures in `tests/fixtures/packed_overrun/`).
An inline header whose bits run past the end of the data is the data
ending, and the packet adds nothing.

### Signed 32-bit components

**Looser than OpenJPEG, approved by the owner (2026-11-01).** OpenJPEG 2.5.0
refuses every precision over 31 bits when it reads SIZ (`j2k.c`, around lines
2289 and 6618). This decoder accepts a signed 32-bit component, because
ISO/IEC 15444-1 allows `Ssiz` up to 38 bits and a signed 32-bit sample fits
the `i32` container exactly. It refuses what the container cannot hold, as
`Unsupported`, from SIZ and before any tile is read: an unsigned component of
32 bits or more, and any component over 32 bits. A palette column follows the
same rule. No oracle decodes such a file, so `tests/signed_32bit.rs` derives the
expected samples by hand: a signed 8-bit lossless codestream with only `Ssiz`
patched to 32 bits must decode to the same samples. That shows the depth is
accepted and the sign, level shift and clamp are right for small values; it
does not exercise coefficients near 2^31.

### The JP2 box layer

The JP2 box layer follows OpenJPEG 2.5.0 where a file bends Annex I in a
way it tolerates: `ihdr` anywhere in `jp2h`, a second `jp2h` merged into the
first, a header box misplaced at the top level after `jp2h` applied as if it
were inside, and a `jp2c` box whose length is wrong (past the end of the
file, shorter than its header, or shorter than its codestream): the codestream
is read from the box to its last tile-part, as OpenJPEG ignores the length.
After the codestream it walks the boxes from where OpenJPEG's decoder stops
and reads the header boxes again, failing where OpenJPEG fails. An
`ihdr.NC` that differs from `Csiz` decodes, as there: the codestream decides
the image, `bpcc` is still held to `NC`, and the palette's `cmap` is checked
against `Csiz`, as OpenJPEG checks it (owner decision, 2026-11-01). An
`ihdr` with a zero width, height or `NC`, or an `NC` over 16384, is refused
as OpenJPEG refuses it. The `res ` box, which OpenJPEG never reads, never
fails a decode: a malformed `resc` or `resd` is reported as absent. The two
decoders still differ in both directions.

The header is checked before any tile data is read, in OpenJPEG's order up to
the codestream's main header: the boxes, the main header, `ihdr` against SIZ.
So a file whose `ihdr` disagrees with SIZ and whose codestream is also cut
short reports the `ihdr` fault, as OpenJPEG does. The palette and channel
definition checks come next, still before the tiles; OpenJPEG checks the
`cmap` mapping and the `cdef` entries against the channels only after
decoding the tiles, so a file with both a palette or `cdef` fault and a tile
fault reports the box fault here and the tile fault there. Both reject it.
The header boxes after the codestream are read last, also before the tiles,
where OpenJPEG reads them after decoding. This order is what lets `read_header`
return the same error as `decode` for every header fault.

Stricter here, where OpenJPEG decodes:

- a file whose `ftyp` does not claim `jp2 ` compatibility (pure JPX, JPM or
  MJ2), refused as `Unsupported`. OpenJPEG decodes its first codestream. The
  deviation is deliberate (owner decision, 2026-11-01): such a file says a
  JP2 reader cannot present it correctly (composited codestreams, Part 2
  extensions, `rreq` features), so its first codestream can be the wrong
  image with no error. Refusing it lets a later JPX reader produce the right
  image without silently changing output that already "worked";
- a last tile-part with `Psot = 0` followed by more bytes, when the `jp2c`
  box does not end where its codestream does: `LBox` 0, or a length that
  ends inside the codestream with more bytes after the codestream. The
  tile-part then runs to the end of the file in both decoders, so the bytes
  after its last packet are tile data. OpenJPEG ignores them; the codestream
  parser rejects them as `Codestream` ("left after the last packet"), as it
  does for a raw codestream (owner decision on #183, 2026-11-10). The
  packets themselves decode, so this is not the tier-2 rule above.

A `Psot = 0` last tile-part ends at the end of its `jp2c` box, with or without
an `EOC`, when the box declares a length that ends at least two bytes before
the end of the file and holds every tile-part's `SOT`, and tier-2 walks the
packets of that tile-part's tile to exactly the box's end. Tier-2 refuses
bytes left after the last packet and a tile-part that ends before its last
packet, so this holds only when the packets end there, and OpenJPEG, which
reads the tile-part on to the end of the file, reads the same packets when the
tile has 999 layers or fewer (see "Where the decoders differ" below for more).
Read to the end of the file, those packets are followed by bytes left after
them, which are refused, so a fault in tier-1 is the same fault either way.
When tier-2 does not reach the box's end (the box ends inside the codestream),
the codestream is read to the end of the file instead, with the result above.
The check is part of reading the file's structure, so `read_header` makes it
too: it walks that one tile's packet headers, without decoding any samples.
Every step of the walk is bounded, since a one-byte packet header can step
over each row of code-blocks of its precinct and a positional progression
order can sweep millions of positions that hold no packet: a codestream of
more than 4096 tiles or 1024 components, or of fewer than 14 bytes per tile,
is not walked, nor is a tile of more than 2^14 code-blocks or precincts, and
the walk stops after max(2^22, 8 × the tile's data bytes) steps, counted as
the decode's bound counts them (see below). Past a bound the check answers no,
and the codestream is read to the end of the file as above (`jp2_t2bomb_*` and
`jp2_t2_sweep_*` in `tests/fixtures/psot0/`). When the box's end is used, the
boxes after it are walked by the rules for the boxes after the codestream and
reported as metadata (`xml `, `uuid` and the like); otherwise the bytes after
the box are tile data and nothing is reported from them. Neither decoder reads
header boxes there: OpenJPEG's walk after the decode starts at the end of the
file.

A `jp2c` whose length is wrong (`LBox` 2..7, `XLBox` below 16, or shorter
than its codestream) decodes as in OpenJPEG, which ignores the length: the
codestream ends where its tile-parts end. The boxes after the codestream
are walked from that point and their header boxes are read again, as
OpenJPEG does once it has decoded, so a malformed header box there, or
padding inside the `jp2c` box that reads as one, is `FileFormat` in both.

The palette (`pclr` + `cmap`) follows OpenJPEG on what it accepts and on
its output samples, including its quirks: `cmap` holds `NPC` entries
whatever its length, a palette channel `i` must read column `i` (another
column is `Unsupported`, as OpenJPEG refuses it), a direct channel `i` takes
column `i`'s depth, signed entries are not sign-extended, and a
single-component file whose `cmap` leaves a column unused is read as the
identity mapping. Two differences, both approved by the owner on 2026-11-01:

- stricter: a palette column too deep for the `i32` sample container
  (unsigned 32 bits, or more than 32) is `Unsupported` when the palette is
  applied, as such a codestream component is. OpenJPEG decodes it;
- looser: palette and direct channels from components of different
  sub-sampling each keep their own component's grid. OpenJPEG sizes every
  channel by the first one's grid and crashes or leaves samples unset, so
  `jp2_palette_subsampled_*` are graded by table lookup over OpenJPEG's
  decode of the bare codestream.

Channel definitions (`cdef`) follow OpenJPEG on what it accepts and on the
output channel order, including its sequential swap rule: each color entry
swaps its channel to `Asoc - 1`, later entries follow their channels, and an
entry may repeat a channel once every channel is named. The labels on
`Jp2Metadata::channels` have no OpenJPEG output to match: its library keeps
only the type, in `comps[].alpha`, which `opj_decompress` never writes out
(no PGX, PNG or TIFF writer reads it). That field is also written to the
position the entry named before its swap, so after a swap it can sit on the
wrong channel (an ARGB layout loses its opacity flag). Here each channel
carries the entry that names it, so the label moves with the samples, as
I.5.3.6 defines `Cn`.

Without a `cdef`, libopenjp2 labels nothing (every `comps[].alpha` is 0),
so the default labels have no oracle either. `opj_decompress` decides by
component count, not by a label: its writers (`convertpng.c`, `convert.c`)
and `color.c` take the last component of two or four as alpha (PNG caps at
four; PGX and raw output have no alpha), and read two components as gray plus
alpha. That is why the gray ICC case below matches. Here the first N channels
are colors 1 to N, N being the color space's number of colors, and each
channel past N is unspecified, since I.5.3.6 gives it no meaning. This
decides one `ColorOutput::Srgb` case: two components with a grayscale ICC
profile and no `cdef` convert as gray plus opacity, and match
`opj_decompress -force-rgb` sample for sample (test
`a_gray_profile_on_two_channels_without_cdef_is_gray_plus_opacity`).
It decides a second case: an enumerated sRGB file of four components and no
`cdef`, whose fourth differs in format from the colors (signed 8-bit, say),
passes through as coded, as with a `cdef` and as in OpenJPEG (test
`an_srgb_file_with_a_differently_formatted_fourth_channel_passes_through`).
One check stays on every channel: a file declared sRGB or grayscale on
three components laid out 4:2:0 or 4:2:2 (luma at 1x1, both chroma
channels at 2x2 or both at 2x1) is refused, whatever its `cdef` labels or
without one. Checked on such sRGB files with a `cdef` of color 1 and two
opacity or unspecified channels, of no color, and of colors 2 and 3
only: `opj_decompress` 2.5.0, with and without `-force-rgb`, writes three
full-size planes byte-identical to those of the file with no `cdef` (test
`srgb_4_2_0_or_4_2_2_is_refused_whatever_cdef_labels`). `opj_decompress`
guesses sYCC whenever the second component is sub-sampled in x and the
first has equal x and y sampling, whatever the file declares
(`opj_decompress.c:1620-1623`), but `color_sycc_to_rgb` converts only those
two layouts (`color.c:403-436`); this decoder does not make the guess. Any
other three-component grayscale file passes through as coded, as OpenJPEG
writes it: one whose third channel is signed or the only one sub-sampled,
whose luma is sampled 1x2 with chroma 2x1, or whose layout is in the guess
shape but not one of the two (1x1, 2x1, 1x1, say), where OpenJPEG prints
"CAN NOT CONVERT".

Checked with `opj_decompress` on 1x1:2x1:1x1, 1x1:2x2:1x1, 1x1:2x1:2x2,
1x1:2x2:2x1 and 2x2:2x2:2x2, as grayscale and under an RGB profile: each
is written as coded, except 2x2:2x2:2x2 under the profile, which the
profile converts. A grayscale palette of three columns over a 2x2 index is
written as coded too.

A malformed header box after the codestream (a second signature or `ftyp`
box, a `jp2h` without `ihdr`, a `colr` running past the end of the file, a
bad `cdef`) is `FileFormat`, as in OpenJPEG, which runs its box handlers on
what follows `jp2c` once it has decoded (`src/jp2/after.rs`).

### The `jp2i` and `uinf` boxes

OpenJPEG 2.5.0 skips both box types, so ISO/IEC 15444-1 (I.6, I.7.3) is the
reference for their contents. Neither affects the samples. `jp2i` contents
are returned as bytes: their format is reserved for ISO, so nothing about
them is checked, and the `ihdr` `IPR` flag is not held to their presence.

A `uinf` is reported only when it is well formed, and a malformed one is
left out of `Jp2Metadata::uuid_info` as a whole: the decode goes on, as it
does in OpenJPEG, whose `opj_decompress` decodes every malformed variant
(owner decision, 2026-11-01). Nothing partial is reported. Well formed means:

- Its contents are whole boxes that fill it exactly. A child box that
  overruns the `uinf`, has length 0, a length of 2..7, an `XLBox` below 16
  or past the `uinf`, or 1..7 bytes left after the last child make it
  malformed. A `uinf` whose own length is bad is a top-level fault, handled
  like any other box's.
- One `ulst` and one `url `, in either order. Figure I.20 shows `ulst`
  first, but I.7.3 does not require it, so the order is not checked. A
  missing or second `ulst` or `url ` makes it malformed. Other boxes inside
  are ignored (I.8).
- The `ulst` holds its 2-byte `NU` and exactly `NU` UUIDs ("the number of
  UUIDi fields shall be the same as the value of the NU field").
- The `url ` holds at least its `VERS` and `FLAG` (4 bytes).

Three requirements are reported rather than checked: `VERS` and `FLAG`
"shall be 0", and `LOC` is "a null terminated string of UTF-8 characters".
`UuidInfo` keeps the version, flags and location bytes as stored, and
`UuidInfo::location_str` returns `None` for a location that is not such a
string.

The boxes are read wherever `xml ` and `uuid` are collected: at the top level
before the codestream, and after it as far as OpenJPEG's walk there goes. A
`uinf` or `jp2i` inside `jp2h` or another superbox is not read. No real file
in the corpus carries either box.

### The `rreq` box

OpenJPEG 2.5.0 has no handler for the Reader Requirements box, so ISO/IEC
15444-2 M.11.1 is the reference for its layout: `ML`, then `FUAM` and `DCM`
of `ML` bytes each, `NSF` standard features (`SF` and an `ML`-byte `SM`), and
`NVF` vendor features (a 16-byte `VF` and an `ML`-byte `VM`). The first
top-level `rreq`, before or after the codestream, is reported as
`Jp2Metadata::reader_requirements` when it holds exactly those fields. One
that is cut short or has bytes left over gives `None` and the decode goes on,
as in OpenJPEG: `opj_decompress` decodes `file5.jp2` with its `NSF` raised to
65535, or its `ML` to 255, to the same samples as the unmodified file. The
values (an `ML` outside 1, 2, 4 and 8, an unknown feature) are reported, not
checked. Files 5 and 7 of the Part 4 set carry an `rreq`.

## Resource bounds

The caps this decoder puts on what a hostile file can make it allocate or do.

### The sample limit

The decode holds every component in its own plane, so what it allocates
grows with the sum of the component areas, not with the file. A file of a
few hundred bytes can declare a 65536 x 65536 image, 16 GiB of `i32`
samples. The decode counts the samples of every output channel, at the
resolution reduction asked for, from SIZ and the JP2 boxes alone, and
refuses an image of more than 2^28 samples (1 GiB as `i32`) with `Limit`
before it reads a tile. The count covers the codestream's components, a
palette's expanded channels, and the channels sRGB output adds (sYCC chroma
replicated to the luma grid, a gray channel a monochrome profile maps to
R, G and B). `read_header_with` makes the same check, so it refuses what
the decode would. OpenJPEG has no such limit: it allocates what the header
declares and fails only when the allocation does.

`DecodeOptions::sample_limit` raises the limit (`WorkLimit::Raised(n)`,
n times 2^28) or lifts it (`WorkLimit::Unlimited`). The refusal names the
option and the smallest `Raised` factor that admits the file. Even
`Unlimited` refuses an image whose 4-byte samples would pass `isize::MAX`
bytes, so the buffer sizes cannot overflow `usize`, on 32-bit targets
included; past that, an allocation that fails aborts the process.

The default admits a Sentinel-2 10 m band (10980 x 10980, 120.6 M samples)
and RGB images up to about 89 MP, and a reduced decode of a larger image
when the reduced image fits: GDAL's `single_block_32769_16385.jp2` (537 M
samples) decodes at reduction 3 (4097 x 2049), bit-exact against
`opj_decompress -r 3` (owner decision on #281, 2026-11-12). Measured on a
release build against `opj_decompress` 2.5.0 (wall time on a shared,
loaded machine; peak resident memory from `/usr/bin/time -v`):

| File | Decode | Time | Peak RSS | OpenJPEG time | OpenJPEG peak RSS |
| ---- | ------ | ---- | -------- | ------------- | ----------------- |
| `T31UDQ_20240105T105339_AOT_10m.jp2`, 10980^2, 690 KB | full, bit-exact | 4.2 to 4.6 s | 470 MiB | 3.6 to 4.4 s | 471 MiB |
| the same | reduction 2 (2745^2) | 0.78 s | 32 MiB | | 36 MiB |
| `single_block_32769_16385.jp2` | reduction 3, bit-exact | 1.05 s | 86 MiB | 0.25 s | 92 MiB |

The peak is about 1.0 times the 4-byte output for the Sentinel-2 band (460
MiB of samples) and about 1.35 times for one square tile of one component
(see the table in `src/samples.rs`), so the default allows a peak of about
1.4 GiB. A tall, narrow tile peaks higher: about 1.5 to 1.7 times when it is
8 to 1024 samples wide, and about 2.6 times when it is one sample wide. A
smaller narrow image can peak at a higher multiple, as the code-blocks' fixed
overhead is a larger share of it: a 9/7 tile one sample wide with 2^24 samples
peaks at about 2.6 times at 3 wavelet levels, 2.7 at 5 and 3.2 at 8.
While a palette expands the peak is about twice the output. Tests:
`tests/sample_limit.rs` and `tests/sample_limit_alloc.rs`.

### The tier-2 work bound

A packet header names every code-block of its precinct, in raster order,
and reads tag-tree bits for each one not yet included. Once a node of the
inclusion tree is known to lie past the current layer, every code-block
under it is excluded from that packet without reading a bit. The decode
steps over them in one go: the rest of the node's span on the current row,
or all of its rows when it spans the precinct's width. It keeps the spans it
has found for the rows below. This yields the same inclusion and tag-tree
state as reading each code-block in turn
(`stepping_over_unread_spans_reads_as_every_leaf_does` checks this over
3000 random precincts and layer sequences). The work of a header is then
proportional to the bits it reads plus the code-blocks it includes, plus
one step for each span it steps over.

That matters for real files: OpenJPEG's encoder sets every packet's
"present" bit, so each layer's header names all of its code-blocks even
when almost none are included. `opj_compress -b 4,4` with 64 or 100 layers
on a nearly flat 2048^2 image (`tests/fixtures/work_bound/opj_*`) used to
visit 262144 code-blocks per layer; it now decodes in 0.15 s.

What remains is still bounded (owner decision on #235, 2026-11-11). Walking
every tile's packets may take at most max(2^24, 32 × the tiles' data
bytes) steps. A step is a packet read, a span stepped over, or a sweep
position, once for each (component, resolution) checked there. Only the
pairs that hold a precinct are checked, and RPCL sweeps no resolution
where none does. Code-block
entries that read a bit are not counted, since the header's bytes already
pay for them. Past the bound the decode is `Limit`. It is one bound for the
whole decode, not one per tile. `DecodeOptions::packet_work_limit`
raises it (`WorkLimit::Raised(n)`, n times the floor and the rate) or lifts
it (`WorkLimit::Unlimited`), for the walk `read_header_with` makes too (below).
The POC bitmap stays capped at the default bound's size, so raising the
bound raises the time a file may take, not its memory: past that size the
packets read go in the set.

A header of one byte can still step over each row of code-blocks in its
precinct, and a positional order can sweep millions of positions that hold
no packet. Measured on release builds (`scripts/gen-work-bound-fixtures.py`;
times are this decoder before this change, then after, then
`opj_decompress` 2.5.0):

| Shape | Size | Before | After | OpenJPEG |
| ----- | ---- | ------ | ----- | -------- |
| LRCP, 2^17 4x4 code-blocks, 1000 one-byte packets | 1.1 KB | 4.6 s | decodes, 14 ms | 6.8 s |
| LRCP, 2^14 code-blocks, 65535 layers | 66 KB | 28.7 s | decodes, 8 ms | 0.3 s |
| LRCP, 2^19 code-blocks, 65535 layers | 66 KB | | decodes, 0.09 s | 26.5 s |
| As above, first packet resolves the root (`t2skip`), 2^19 blocks | 66 KB | | `Limit`, 0.5 s | 37.7 s |
| `t2skip`, 512 x 256 code-blocks, 40000 layers | 40 KB | | `Limit`, 0.4 s | 6.1 s |
| PCRL sweep, 1024 components, step 2 on a 2048^2 tile | 9.3 KB | 28.9 s | `Limit`, 0.3 s | 37.4 s |
| CPRL, one 2^24-row column, 31 POC volumes | 819 B | 6.5 s | `Limit`, 0.1 s | 20.6 s |

The bound scales with size, so a large file of the same shape still takes as
long as its bytes allow: `t2skip` in 153 tiles, 10 MB, is refused after 3.1 s,
about what 10 MB of real data takes to decode. A step costs 2 to 15 ns. The
conformance codestreams over 100 KB decode at 0.19 (`p1_05`) to 0.73 (`p1_04`)
µs a byte, the best of five release-build decodes of each. Over all 23 the
range is wider, 0.06 to 0.75 µs a byte. The cheapest refusals are in the layer
orders over resolutions with no precinct, at about 2 ns a step: `layer_edge`
(655 KB, 33 steps a byte) is refused in 0.015 to 0.04 s, 0.02 to 0.06 µs a
byte, and OpenJPEG decodes it in 0.08 s.

At full resolution, setting up a tile's geometry is not counted. A tile of
one sample at an odd origin has 32 resolutions per component that hold no
precinct, and each component of each tile costs about a byte. Those resolutions now get
no band or code-block geometry, and the walk checks no component there. Measured
on release builds (`deep` in `scripts/gen-work-bound-fixtures.py`, a row
of one-sample tiles at y = 1, 32 levels, one layer, every packet 0x00;
times are before this change, then after, then `opj_decompress` 2.5.0):

| Shape | Size | Before | After | OpenJPEG |
| ----- | ---- | ------ | ----- | -------- |
| 1 tile, 16384 components | 66 KB | 0.5 s, 383 MB | 0.18 s | 0.4 s, 192 MB |
| 64 tiles, 1024 components | 70 KB | 1.7 s | 0.41 s | 0.24 s |
| 64 tiles, 16384 components | 1.1 MB | `Limit`, 21 s | 10.5 s, 226 MB | 5.5 s, 1.3 GB |

The last is about 10 µs a byte, twice OpenJPEG, and the slowest shape
known per byte. The same change lets the layer orders decode 8 such
components in one tile, and the positional orders a 16-component column
of 2^17 rows, which were `Limit` and which OpenJPEG decodes
(`tests/work_bound.rs`).

At a resolution reduction the geometry is counted too: one step for each
code-block of a resolution the reduction drops (orchestrator decision on
#281). Every tile builds its geometry at full resolution whatever the
reduction, and the per-tile caps on precincts and code-blocks do not bound a
file of many tiles. Once the sample limit was counted at the reduction, a
small file could build a great deal of geometry it never decodes: 2800^2
tiles in 4x4 code-blocks, six levels, one empty byte per packet, decoded at
reduction 6, took 10.3 s for 256 tiles (5.5 KB), 147 s for 4096 tiles
(87 KB) and about 40 minutes for 65025 tiles (1.4 MB). Each is now refused in
about 1.1 to 1.7 s.

Nothing else of the geometry needs a charge. Each precinct is already paid
for: the walk reads each precinct's packet in each layer, a step each,
whatever the reduction. The code-blocks of the kept resolutions are no more
than the reduced image's samples / 16, which the sample limit holds to
2^24, about a second to build. A decode at full resolution drops no
resolution, so it spends exactly what it did before; a file built to spend
exactly the bound (`the_layer_orders_charge_each_component_they_check` in
`tests/work_bound.rs`) still decodes.

A code-block costs about 30 to 70 ns to build, a few steps' worth, but one
step each keeps real files far inside the bound. At reductions 1 to 3 the
committed files spend at most 1.6% of it (a sparse 2048^2 image in 4x4
code-blocks at reduction 3); the Sentinel-2 10 m band about 0.15%; a
2^28-sample single tile in 64x64 code-blocks about 0.4%; and of the
Sentinel-2, GDAL and OpenJPEG test files measured, none more than 1.9%
(`s2_MSK_QUALIT_B02.jp2` at reduction 3).

A POC marker's volumes may reach the same packet more than once; the
first reads it and the rest skip it (OpenJPEG's `include` array). The
decode keeps one bit per packet the volumes reach, and skips a packet
already read from that bitmap in about 4 ns, without a step: there are at
most 31 skips per packet read, and charging them would refuse the 1 MB
file below. The bitmap is used when it has no more bits than the steps
left, since a walk that reads every packet takes a step for each; that
keeps it under 2 MB below the floor and 4 bytes per byte of data above it.
`read_header` holds the same bitmap, sized from the decode's bound for the
same tile data, so it holds a bitmap wherever the decode does. A
walk in layer orders that reaches more packets than its steps left cannot
finish and is `Limit` before it starts. With more packets than the bitmap
holds, one with a
positional order can pass over precincts, so the decode keeps the packets
read in a set, at most one per byte of packet headers and about 24 bytes
each. A skip from the set is a lookup that misses the cache, 80 to 180 ns
against at most 15 ns for a step, so each one is charged 12 steps.
`read_header`'s walk keeps no set: past its bitmap, only where the decode
would keep one, it gives up. For a `Psot = 0` codestream in a JP2 file the
codestream then runs to the end of the file, like the other tiles too
costly to walk (`a_psot_zero_tile_too_costly_to_walk_reads_to_the_end`):
boxes after `jp2c` are not reported, and the decode refuses their bytes as
left after the last packet. `setskip(1024, 31)`, 1.0e7 packets reached,
ends at its box and decodes, as on OpenJPEG; `setskip(2048, 2)`, 2.0e7,
runs to the end (`a_psot_zero_poc_sequence_ends_at_its_box_within_the_decodes_bitmap`
in `tests/work_bound.rs`). Measured on
release builds (`pocdup` in `scripts/gen-work-bound-fixtures.py`; times
and peak memory are before this change, then after, then `opj_decompress`
2.5.0; `read_header` is on the `Psot = 0` JP2 form):

| Shape | Size | `decode` before | `decode` after | `read_header` before | `read_header` after | OpenJPEG |
| ----- | ---- | --------------- | -------------- | -------------------- | ------------------- | -------- |
| 16 precincts, 65535 layers, 31 identical volumes | 1 MB | 5.7 s, 105 MB | 0.15 s, 3.5 MB | 5.3 s, 105 MB | 0.15 s, 3.5 MB | 0.10 s, 9 MB |
| 160 components, 65535 layers, 2 volumes | 10.5 MB | 4.5 s, 824 MB | 0.32 s, 14 MB | 4.2 s, 824 MB | 0.33 s, 14 MB | 0.57 s, 37 MB |
| 160 precincts, 65535 layers, 32 volumes | 10.5 MB | 65 s, 824 MB | 1.5 s, 14 MB | 71 s, 824 MB | 1.5 s, 14 MB | refused, 32 POCs |
| 2^14 precincts, 65535 layers, 2 volumes, 10^7 packets in the data | 10 MB | `Codestream`, 1.7 s, 828 MB | `Limit`, 0.003 s, 16 MB | 1.8 s, 828 MB | 0.003 s, 16 MB | 53 s, 2.1 GB |
| The same in RPCL | 10 MB | `Codestream`, 1.9 s, 828 MB | `Codestream`, 1.2 s, 238 MB | 2.0 s, 828 MB | not walked, 0.004 s, 16 MB | 153 s, 2.1 GB |
| `setskip(65535, 31)`: RPCL, 6.5e8 packets reached, 1.4e7 read | 14 MB | decodes, 46.7 s, 240 MB | `Limit`, 5.5 s, 240 MB | 44 s | not walked, 0.003 s, 19 MB | |

Encoder output stays far below the bound. Every conformance file spends
under 0.5% of it (`p1_05` the most, at 0.247 steps per byte; `p0_13` takes
0.76). `opj_compress` output takes 0.04 to 88 steps per byte; the high end
is files of a few hundred bytes with 4x4 code-blocks and 64 or 100 layers,
which stay under the floor (the committed ones use under 0.1% of it). The
largest share seen is 12.8%: a sparse 2048^2 image in 4x4 code-blocks with
64 layers, 469 KB at 4.6 steps per byte.
`every_committed_file_walks_far_below_the_work_bound` prints the table for
the committed files and asserts each is under 1/64.

The bound does refuse some valid files. Take one tile, no wavelet levels,
4x4 code-blocks in one precinct, LRCP, and many layers that each include
one new code-block at a scattered place (one pass, one byte), with the
blocks included earlier reading a "no new passes" bit. The new blocks keep
the inclusion tree from settling high up, so each layer steps over many
spans narrower than the precinct, one step per row each: about W·√n steps
a layer for a precinct W blocks wide after n layers. The steps per byte
grow with the layer count, so no constant per byte admits the whole class.
At 2048^2 or more it passes the bound from about 600 to 800 layers:

| Image | Layers | Size | Refused after | OpenJPEG |
| ----- | ------ | ---- | ------------- | -------- |
| 2048^2 | 800 | 527 KB | 1.1 s | 6.9 s |
| 2896^2 | 600 | 343 KB | 1.2 s | 10.5 s |
| 2896^2 | 800 | 585 KB | 1.5 s | 15.4 s |

OpenJPEG decodes all three. No encoder seen writes this shape; the files
were built by hand for the review of #235.

The charge on skips from a POC sequence's set refuses another class. Take
one 38608 x 1 tile in RPCL, no wavelet levels, 1 x 1 precincts, one
component at `XRsiz` 254 and 64 at 255. The first sets the sweep's step to
254, which meets only the first of each other component's 152 precincts,
so the volumes reach 9880 packets a layer but read 216. That is more than
the bitmap may hold, so the packets read go in a set, and every volume
after the first skips each of them at 12 steps. With 2048 layers (442 KB)
one volume decodes and 31 are `Limit` after 0.05 s; OpenJPEG decodes both
in 0.14 s or less. `setskip` in `scripts/gen-work-bound-fixtures.py`
writes the shape, and `skips_from_the_poc_set_are_bounded` in
`tests/work_bound.rs` builds it. It came from the review of PR #261.

The bound covers the walk `read_header` makes for a `Psot = 0` tile-part (see
above). That walk counts the same steps under its own bound, max(2^22, 8 × the
tile's data bytes), no larger at any size, so for a single-tile codestream a
tile that walk finishes is one the decode's walk finishes too. A JP2 decode
makes that walk first, and then walks the tile again under a fresh bound, so
it can spend about 1.25 times the decode's bound (`jp2_t2_sweep_cprl_tall` in
`tests/fixtures/psot0/README.md`). The decode's bound does not replace that
walk's geometry bound of 2^14 code-blocks: `read_header` allocates for its
walk, and a geometry bound at the decode's 2^19 code-blocks would let
`read_header` hold about 75 MB for a 66 KB file.

### The tier-1 work bound

Each coding pass of a code-block scans all of the block's samples, however
few bytes the pass carries, and a packet header can ask for a block's 88
passes (3 x 30 - 2, for 30 bit-planes) in about 4 bytes. The decode counts
the work of each tile as the passes the block decoder runs times each
block's samples, before it decodes the tile's code-blocks. A block runs its
passes, but no more than its coded bit-planes hold (`3 × top − 2`), and the
blocks a resolution reduction drops are not counted. The decode may spend
max(2^26, 2^15 × the tiles' data bytes) such sample-passes, one bound for
every tile; past it the decode is `Limit` (owner decision on #250,
2026-11-12). `DecodeOptions::code_block_work_limit` raises or lifts it.

A sample-pass costs about 3.3 ns where the pass finds little to code, but
7 to 8 ns on the data slowest to decode (every byte 0x4e), so the floor is
about 0.5 s and the rate about 0.25 ms a byte. A crafted 1024^2 file of 64
passes per block that spends the floor decodes in 0.49 s, and a 4096^2 file
of 27 passes per block that fits just under the bound (14 KB) in 3.9 s,
where `opj_decompress` takes 0.83 s. Measured on
release builds (`scripts/gen-tier1-work-bound-fixtures.py`; `t1bomb` is one
tile, no wavelet levels, every 64x64 code-block included with 88 passes
and one byte; times are this decoder before this change, then after, then
`opj_decompress` 2.5.0):

| Shape | Size | Sample-passes | Before | After | OpenJPEG |
| ----- | ---- | ------------- | ------ | ----- | -------- |
| `t1bomb`, 8192^2 | 76 KB | 5.9e9 | decodes, 19.3 s | `Limit`, 6 ms | decodes, 9.0 s |
| `t1bomb`, 1024^2 (committed) | 1.3 KB | 9.2e7 | decodes, 0.29 s | `Limit`, under 1 ms | decodes, 0.14 s |

With the bound lifted, the 8192^2 file decodes in the same 19 s
(`the_issue_bomb_decodes_with_the_limit_lifted`, ignored by default).

The bound is about 2.35 times the most encoder output was seen to spend.
Over the committed files, the conformance corpus and the real-world files,
most spend under 120 sample-passes per byte; `budget/flat_1024_lossless`
spends 7100 in 202 bytes, under the floor. Encoder output that spends the
most per byte has many code-blocks that each run many passes for a few
bytes. `opj_compress` 2.5.0 writes the most with no wavelet level
(`-n 1`) on a flat image, where every block does: a flat 16-bit 8192^2
image (221 KB) spends 13933 per byte, 42.5% of the bound, and RLCP,
`-M 8` or 2048^2 tiles change little; flat 8-bit spends 7230, 22%. With one
level (`-n 2`), a flat 16-bit image with 256 specks spends 12396, 38%. At
the defaults, a flat 16-bit image with 4 to 64 sparse specks spends about
5600, 17% (8192^2 with 64 specks is 20 KB). Smooth fields, noise and
photographs (lossless and 9/7 at ratios from 20 to 10000, 16x16 to 64x64
code-blocks) spend far less. The committed `opj_flat2048_16bit_n1`
(2048^2, 16-bit, `-n 1`, 14 KB) spends 13933 per byte, 42.5%, and
`opj_specks2048_16bit` (2048^2, 16-bit, 128 specks, 28 KB) 3600, 11%.
`every_committed_file_decodes_far_below_the_tier1_work_bound` prints the
table for the committed files and asserts each is under half the bound,
which the worst encoder output seen passes with room.

A crafted file can spend about 100000 per byte (88 passes over 4096
samples for about 4 bytes of packet header and data), so the gap between
what encoders write and what a file can ask for is only about 7 times.
The bound cuts the worst case to a third of the unbounded one and refuses
it before doing the work, but a file at the bound still takes about 0.25
ms a byte, against the 0.2 to 0.7 µs a byte real data takes; a valid file
a little past what was measured, such as a flat image coded with no
wavelet level at a little more than twice the passes per byte, would be
refused, and `code_block_work_limit` is the way to decode it. The rate
stays 2^15 rather than 2^14, which would refuse the `-n 1` files, and it is
scaled by the tile bytes, packet headers included, as the tier-2 bound is
(orchestrator decisions on #278, 2026-11-12).

### Tile header cost

A tile's header is the main header with the tile's own COD, COC, QCD, QCC,
RGN and POC markers laid over it. Resolving one is sized by the component
count, up to 16384, and a codestream can hold 65535 tiles. Resolving every
tile's header up front cost tiles × components, before any check ran (#256).

The parse checks each tile's markers against the main header without
resolving a header, in time proportional to the tile's own markers. A
component the tile names in a COC or QCC is checked on its own. A tile COD
or QCD lands on every other component, so it is checked once, against the
most demanding of them: the fewest QCD steps under a tile COD, the most
levels under a tile QCD, and a count of each wavelet. Only a tile found at
fault is resolved in full, so the error reads as it always did.
`the_quick_tile_check_agrees_with_the_full_resolution` checks that the two
agree on 20000 random main and tile headers, with up to 32 levels, 97 step
entries and 300 components, and
`the_quick_tile_check_agrees_with_the_full_resolution_from_the_bytes` on
5000 written as codestreams of two tile-parts, a POC in either.

At decode, a tile with no markers runs under the main header, borrowed. A
tile with markers is resolved in full, sized by the component count, and
the decode then spends at least that much on its components: every
tile-component must hold a sample, so tiles × components is at most the
sample limit for a file that decodes. TLM entries are checked and dropped,
since nothing reads them. Measured on release builds
(`scripts/gen-tile-header-fixtures.py --big`; before, after, then
`opj_decompress` 2.5.0):

| Shape | Size | Before | After | OpenJPEG |
| ----- | ---- | ------ | ----- | -------- |
| 4096 tiles × 16384 components | 110 KB | 2.2 s | 3 ms | fails to read the header, 10 ms |
| 128² tiles × 16384, heavy header | 295 KB | 63.9 s | 9 ms | 22 s, 55 GB |
| 65025 tiles × 16384 | 1.0 MB | 45.8 s | 17 ms | 25 s, 55 GB |
| The same, a COD and a QCD in every tile | 2.3 MB | 78.8 s | 26 ms | |
| 65025 tiles, 1 MB of TLM entries, decodes | 2.0 MB | 13.2 s | 56 ms | 0.34 s |
| 65025 tiles, 4 MB of TLM entries, decodes | 5.2 MB | 85.5 s | 63 ms | 0.35 s |

The first four are refused at tile 0, whose components but the first hold no
sample. The OpenJPEG figures for the 16384-component shapes are from the
review that filed #256.

## sRGB output

`ColorOutput::Srgb` converts sYCC and the two restricted ICC profile forms
(I.5.3.3) to sRGB. The default output is as coded and is unaffected by
everything here. The measured error against the ISO references and against
`opj_decompress -p 8S -force-rgb` is tabulated per channel in
[`tests/fixtures/conformance/README.md`](../tests/fixtures/conformance/README.md).

**The rules (owner, 2026-11-02).** Every outcome below is decided in this
order:

1. Follow the standard.
2. Otherwise, do what a user hoping for automatic error correction would
   expect. OpenJPEG is usually the guide.
3. Return an error rather than an incorrect or malformed image.

Each deviation from OpenJPEG below names the rule it rests on.

### Outcomes

This table is the complete list of what `Srgb` does. The rustdoc and the
CHANGELOG summarize it. The checks run in the order of the rows, and the
first row that matches decides. "Channels" are the output channels, after
the palette and the channel definitions; the color channels are the ones
the `cdef` labels color 1, 2 and 3 (see "Color channels are picked by their
labels" below). OpenJPEG means `opj_decompress -force-rgb` 2.5.0 with
LittleCMS 2.14, which follows `opj_decompress.c:1620-1720` and `color.c`.

| # | Input | OpenJPEG | `Srgb` here | Rule |
| - | ----- | -------- | ----------- | ---- |
| 1 | `raw_channels` set | (no such option) | `InvalidOptions`, before anything is read | |
| 2 | A raw codestream of one or two components | read as gray, or gray and opacity | as coded | matches |
| 3 | A raw codestream of three or more components | at 4:2:0 or 4:2:2, converted as sYCC; otherwise fails | `Unsupported` | 3 |
| 4 | One or two channels, any color space or none, unless a monochrome profile converts them (rows 18 to 20) | read as gray, or gray and opacity | as coded, metadata as declared | matches |
| 5 | Three channels at 4:2:0 or 4:2:2 (luma 1x1, both chroma 2x1 or both 2x2), declared sRGB, grayscale or by an ICC profile (any profile, any `cdef`) | converted as sYCC, before the profile is read | `Unsupported` | 3 |
| 6 | sRGB or grayscale (`EnumCS` 16, 17), color channels of one format | written as coded | as coded | matches |
| 7 | sRGB or grayscale, color channels that differ in depth, sign or sub-sampling | written as coded | `Unsupported` | 3 |
| 8 | sYCC (`EnumCS` 18) without colors 1, 2 and 3, or with colors that differ in depth or sign | converts positions 0 to 2 | `Unsupported` | 3 |
| 9 | Signed sYCC | converted as unsigned, then clamped | `Unsupported` | 3 |
| 10 | sYCC other than luma at 1x1 with both chroma channels at 1x1, 2x1 or 2x2 | "CAN NOT CONVERT", written as coded | `Unsupported` | 3 |
| 11 | sYCC whose replicated chroma passes the sample limit | | `Limit` | |
| 12 | sYCC otherwise | converted, constants shortened and truncated | converted, IEC constants rounded | 1 |
| 13 | An ICC profile LittleCMS refuses to open (table below) | written as coded | as coded, `RestrictedIcc` kept | 2 |
| 14 | A profile in a color space other than RGB, gray or YCbCr | written as coded (no transform) | as coded, `RestrictedIcc` kept | 2 |
| 15 | An RGB profile whose first four channels (all, if fewer) differ from the first in depth, sign or sub-sampling (the AFL test) | written as coded | as coded, `RestrictedIcc` kept | 2 |
| 16 | A profile LittleCMS refuses to build a transform from (table below) | written as coded | as coded, `RestrictedIcc` kept | 2 |
| 17 | A profile LittleCMS applies through a lookup table or the Lab connection space | converted | `Unsupported` | 3 |
| 18 | A monochrome profile without exactly one color channel, or on more than two channels | more than two: the planes go to a gray transform as RGB | `Unsupported` | 3 |
| 19 | A monochrome profile on samples other than 8-bit unsigned | scaled from the wrong range | `Unsupported` | 3 |
| 20 | A monochrome profile otherwise | converted | the same; a tinted output gives R, G, B in place of the gray channel (`Limit` past the sample limit) | matches |
| 21 | An RGB profile without colors 1, 2 and 3 | converts positions 0 to 2, whatever they are | `Unsupported` | 3 |
| 22 | An RGB profile whose colors differ in depth, sign or sub-sampling (possible only with a color past the first four positions) | converts positions 0 to 2 | `Unsupported` | 3 |
| 23 | An RGB profile on samples other than 8- or 16-bit unsigned | scaled from the wrong range | `Unsupported` | 3 |
| 24 | An RGB profile otherwise | converts positions 0 to 2 | the same samples when the colors come first; the colors are picked by label | 1 |
| 25 | A YCbCr profile on three or more channels | converted as 16-bit YCbCr (8-bit crashes) | `Unsupported` | 3 |
| 26 | CMYK (`EnumCS` 12) on four or more channels of one sampling | converted to three RGB planes | `Unsupported` | 3 |
| 27 | CIELab (`EnumCS` 14) on three channels of one size | converted to 16-bit RGB | `Unsupported` | 3 |
| 28 | e-sYCC (`EnumCS` 24) on three channels of one sampling | converted | `Unsupported` | 3 |
| 29 | Any other enumerated space, or no `colr`, on three or more channels | fails, written as coded | `Unsupported` | 3 |

Rows 26 to 28 are conversions this crate does not have (they are out of
scope for JP2, `docs/roadmap-phase3.md`); without one, sRGB output would be
a wrong image. Where it is not stated, OpenJPEG at 4:2:0 or 4:2:2 is row 5.

**One rule for files OpenJPEG writes as coded.** When `opj_decompress`
leaves the samples as coded (and `-force-rgb` fails), `Srgb` goes by what
the file declares, not by what OpenJPEG does:

- sRGB or grayscale need no conversion, so they pass through (rows 6, 4);
- a profile OpenJPEG's color step declines passes through, with the profile
  kept in `Jp2Metadata::color`, so a caller can tell that nothing was
  converted (rows 13 to 16; rule 2, owner decision on #199);
- anything else is refused: an enumerated space this crate has no
  conversion for, no color space on three or more channels, or color
  channels the conversion cannot take (rows 7 to 10 and 29; rule 3).

**Color channels are picked by their labels (owner decision, 2026-11-02;
rule 1).** I.5.3.6 makes the `cdef` association say which channel is which
color, so `Srgb` converts the channels labeled color 1, 2 and 3 wherever
they sit. OpenJPEG converts the first three positions. The two agree
whenever the colors come first, which is every file seen but this one:
`jp2_cdef_rgba` with the `cdef` (0,0,1), (1,0,2), (3,0,3), (2,0,3), (2,1,0)
puts color 3 after an opacity channel. Here planes 0, 1 and 3 are
converted; `opj_decompress` converts planes 0, 1 and 2. Test:
`srgb_picks_the_color_channels_by_their_cdef_labels` in
`tests/jp2_icc.rs`. A file with fewer than three color labels under sYCC or
an RGB profile is refused (rows 8 and 21): three channels with an RGB
profile and a `cdef` of (color 1, color 2, opacity) are `Unsupported`,
where OpenJPEG converts all three planes and treats the opacity channel as
blue.

**sYCC uses the IEC constants and rounds (owner decision, 2026-11-01, on #165;
rule 1).** OpenJPEG's `sycc_to_rgb` computes
`G = Y - (int)(0.344 Cb + 0.714 Cr)`, `R = Y + (int)(1.402 Cr)`,
`B = Y + (int)(1.772 Cb)`: shortened
constants and each product truncated toward zero. This decoder uses IEC
61966-2-1 Amd 1's `0.344136` and `0.714136` and rounds to nearest. Both meet
the evidence rule above. The proof is the conformance references for files 2
and 3; the result is an MSE against them 20 to 1000 times lower:

| File | Ours vs ISO (MSE R/G/B) | OpenJPEG vs ISO (MSE R/G/B) | Ours vs OpenJPEG (MSE R/G/B) |
| ---- | ----------------------- | --------------------------- | ---------------------------- |
| file2 | 0.0003 / 0.0155 / 0.0242 | 0.3853 / 0.4748 / 0.5135 | 0.3850 / 0.4803 / 0.4895 |
| file3 | 0.0002 / 0.0151 / 0.0239 | 0.3862 / 0.4724 / 0.5109 | 0.3860 / 0.4774 / 0.4872 |

The peak error is 1 in every column. OpenJPEG's formula, reproduced in
`src/jp2/srgb_tests.rs` (`sycc_sample_openjpeg`), is bit-exact against
`opj_decompress` on both files and on the two synthetic sYCC fixtures, so
the difference is the formula alone. The IEC constants with truncation fit
neither (green off OpenJPEG by up to 2), so the two changes go together.

**Chroma replication follows OpenJPEG**, including where it is a convention
rather than the standard. Sub-sampled chroma is replicated to the luma grid
in the order of `sycc422_to_rgb` and `sycc420_to_rgb` in the oracle
build, Ubuntu's OpenJPEG 2.5.0-2ubuntu0.5. That build carries upstream
master's `comp12w` guard (below), which tag v2.5.0 (`a589155`) does not
have; the committed fixtures are its output. At an odd image
origin (`XOsiz` or `YOsiz` odd, on the full-resolution grid even under
resolution reduction) OpenJPEG gives the first column of each row (4:2:2),
or the first row and the first column of the upper row of each pair and of
a last unpaired row (4:2:0), chroma of zero, which comes out tinted. A last
odd column whose pair index reaches the chroma width also takes zero
(`comp12w`). `jp2_sycc_420_odd` and `jp2_sycc_422_odd` pin this against
`opj_decompress` sample for sample, and `tests/jp2_sycc_replication.rs`
grades them and the even-origin `jp2_sycc_420_even` and `jp2_sycc_422_even`
against `opj_decompress -r 1` too: there the reduced origin's parity
differs from the full one's in y (and in x for the even fixtures), so a
replication keyed on the reduced origin fails. The only change is where
OpenJPEG would read past the chroma plane: that sample takes chroma zero
here.

**ICC profiles are converted as LittleCMS converts them.** OpenJPEG hands the
profile to LittleCMS 2 (2.14 in the oracle build) with the profile's rendering
intent. `src/jp2/lcms.rs` ports the part of lcms these profiles reach: the
pipeline it builds (the tone curves and matrix into XYZ, the white-point
scaling of absolute colorimetric, black point compensation under perceptual
and saturation, since lcms's sRGB profile is ICC v4, then the inverse sRGB
matrix and curves), the optimization it picks for that pipeline (the 8-bit
matrix-shaper in 1.14 fixed point, prelinearization curves with a 33-point
table, or resampling into a 33-point table, with the white fix-up; for 16-bit
RGB on a profile of gamma below 1.6, no optimization), and its 16-bit
interpolation, step for step at lcms's precision. It also applies the checks
lcms makes when it opens a profile (`_cmsReadHeader`): the `acsp` signature, a
version of at most 5.0, a device class lcms knows, at most 100 tags, no two
kept entries with the same signature. Like lcms, it first skips a tag entry
with a zero offset or size, or one that ends past the size the header declares
(capped at the profile's length), so a tag it needs is then missing, and a
skipped entry does not count as a duplicate. When building the transform it
makes lcms's checks too: a named color profile, a device link or abstract
profile without a lookup table, or a connection space other than XYZ or Lab
builds none, and of the lookup tables lcms would read for the intent (`D2Bx`,
then `A2Bx`, then `A2B0`) it checks the type, the channel counts, the grid and
the table's length as lcms's reader does. It reads tags as `cmsReadTag` does:
a tag under 8 bytes or of a type lcms does not allow for it fails, lcms's
legacy types are read (an old Monaco tool's curve type for `rTRC`, `gTRC` and
`bTRC`, an old Corbis tool's XYZ type), and a value is read without checking
the tag's size, so one that runs past its tag takes the bytes after it and
fails only past the end of the profile. A media white point lcms can't read is
D50 (`_cmsReadMediaWhitePoint`). lcms's black-preserving intents 10 to 15 on a
profile that is not CMYK become perceptual, relative colorimetric or
saturation without the black point compensation lcms forces for intents 0 and
2, which for these profiles is relative colorimetric in every case; other
intents above 3 build no transform.

The output matches `opj_decompress -force-rgb` sample for sample
(`tests/jp2_icc.rs`). The reference is OpenJPEG 2.5.0 linked against
LittleCMS 2.14 on x86-64 Linux (glibc 2.39, on a CPU with FMA and AVX2).

LittleCMS calls the C library's `pow` and `log`, and a platform's own can
differ in the last bit, which moves an output level. So `src/jp2/math.rs`
ports glibc 2.39's `pow` and `log` (Arm's optimized-routines code), and the
output is the same on every platform whose `fma` is correctly rounded. CI
checks this on x86-64 Linux (glibc), macOS on aarch64 and wasm32-wasip1, which
does `fma` in software; Windows is not run. MinGW's `fma` (`*-pc-windows-gnu`)
may round twice (rust-lang/rust#140515), so the output there can differ. i586
targets use x87 arithmetic, which is not exact IEEE, so the port's own
arithmetic can differ there too. On x86-64, glibc picks a variant built with
FMA when the CPU has FMA and AVX2, and a plain SSE2 one otherwise. The port
follows the FMA variant, the one the reference machine runs: each multiply-add
GCC fused there is a `mul_add` in the port. The two variants differ in the
last bit on 296 of 5.2 million `log` inputs and 3,251 of 3.7 million `pow`
inputs of an earlier test sweep, so oracles regenerated on an x86-64 CPU
without FMA could differ. The current ones do not: regenerating them with
glibc held to its SSE2 variant (`GLIBC_TUNABLES=glibc.cpu.hwcaps=-AVX2,-FMA`)
gives the same hashes. The default test (`src/jp2/math_tests.rs`) checks the
port's results on a sweep of inputs against digests recorded from glibc, on
every platform. An ignored, larger sweep compares the port bit for bit with
the platform's `f64::ln` and `f64::powf` on x86-64 Linux with glibc and FMA.

The cases:

- files 5, 7 and 8 at rendering intents 0 to 3, and at intent 3 with the
  media white point set to D65, (1.2, 1.1, 1.0) and (0.9, 0.95, 0.7): 21
  variants, file7 at its native 16 bits;
- 63 synthetic RGB profiles at 8 and 16 bits and 10 monochrome ones: edge
  cases (degenerate, descending and noisy curves, every parametric type,
  extreme matrices and white points, nonzero black points, both sides of
  the gamma 1.6 threshold) and the 24 malformed profiles of the PR #184
  review;
- 15 RGB profiles at 8 and 16 bits on either side of each header check
  above (a bad signature; versions 5.0, 5.1, 4.F and FF; 100 and 101 tags;
  duplicate `rTRC` and `rXYZ`, and a duplicate lcms skips; declared sizes of
  0, 140 and 0xFFFFFFFF, and ones that cut off `wtpt` or an `A2B0` table);
- 25 RGB profiles at 8 and 16 bits and 4 monochrome ones on how lcms reads
  intents, legacy tag types and tags that run short (the table below);
- 58 RGB profiles at 8 and 16 bits and 13 monochrome ones on device
  classes, connection and color spaces, lookup tables and missing tags
  (the table below).

During development the port also matched lcms 2.14 itself on every 8-bit
RGB input (2^24) and a million random 16-bit pixels for each of these
profiles. Against the ISO references the result is OpenJPEG's, inside the
class bounds (PAE 4, MSE 1):

| File | Ours vs ISO (MSE) | Ours vs OpenJPEG |
| ---- | ----------------- | ---------------- |
| file5 (RGB, gamma 1.8) | 0.0897 / 0.1750 / 0.2050 | identical |
| file7 (RGB, 16-bit, 2048-entry curves; after `>> 8`) | 0.1442 / 0.1770 / 0.1900 | identical at 16 bits |
| file8 (gray, `kTRC`) | 0.2739 | identical |

Peak error against ISO is 1 everywhere. The floating-point evaluation this
replaced was slightly closer to ISO on files 5 and 8 (MSE 0.0846 / 0.1723 /
0.2025 and 0.2115). OpenJPEG is the reference decoder, and nothing in the
standard asks for a closer match to these display references, so the
evidence rule above keeps OpenJPEG's output.

Two output shapes differ from `opj_decompress`, by choice of the grading
harness and not of the samples: file7 stays 16-bit, and the harness shifts
it down by 8 to compare with the 8-bit reference (its `shift_right`, as for
the as-coded grade), and file8 stays one gray channel where OpenJPEG writes
three identical ones (graded on that one channel).

**A profile lcms refuses passes through, as in OpenJPEG (owner decision,
2026-11-01, on #199; rule 2).** When lcms will not open the profile or build a
transform from it, `color_apply_icc_profile` returns without converting and
`opj_decompress` writes the samples as coded. `Srgb` does the same, and
`Jp2Metadata::color` stays `RestrictedIcc`, so a caller can tell. Each
case was checked with `opj_decompress` 2.5.0 on a crafted file, and is a
case of `icc/profiles.json` and a test in `src/jp2/srgb_tests.rs`:

| Case | OpenJPEG + lcms 2.14 | `Srgb` here |
| ---- | -------------------- | ----------- |
| profile under 132 bytes, no `acsp`, version above 5.0, unknown class (`zzzz`), more than 100 tags, tag table past the end, two kept entries with one signature | unconverted (lcms won't open it) | as coded |
| a duplicate whose first copy lcms skips | converted from the second | the same |
| class `mntr`, `prtr`, `spac` or 0 with a matrix and curves | converted as `scnr` (a version 2 `mntr` takes D50 as its white point) | the same |
| class `nmcl`; class `link` or `abst` with no lookup table | unconverted | as coded |
| intent 4 to 9 or above 15 | unconverted | as coded |
| connection space other than XYZ or Lab | unconverted | as coded |
| color space other than RGB, gray or YCbCr | unconverted (no transform for it) | as coded |
| lookup table lcms would read of the wrong type, wrong channel counts, a grid of 1, or cut short | unconverted | as coded |
| lookup table for another intent only | converted from the matrix | the same |
| lookup table lcms reads | converted through it | `Unsupported` (below) |
| Lab connection space with a matrix and curves | converted | `Unsupported` (below) |
| intent 10 to 15 | converted as intent 1 | converted as intent 1 |
| needed tag missing or skipped, under 8 bytes, of the wrong type, or with a value past the profile's end; `para` function above 4; `curv` of more than 32767 entries | unconverted | as coded |
| value past its tag but inside the profile | converted from the bytes after it | the same |
| Monaco curve type, Corbis XYZ type | converted | converted |
| `wtpt` unreadable at intent 3 | converted with D50 | converted with D50 |

The order follows OpenJPEG's too (owner decision, 2026-11-02, on #199): every
refusal above, whether lcms makes it opening the profile or building the
transform, comes before this decoder's own checks below. So a profile lcms
refuses passes through whatever the samples are (bar the 4:2:0 and 4:2:2
exception below): an intent of 4 on 12-bit RGB, 16-bit gray, or a monochrome
profile on three channels gives the samples as coded, as `opj_decompress`
does. The refusals below (#165,
#197) apply only where lcms would build the transform. Test:
`refusals_are_ordered_as_in_openjpeg`.

One exception comes before all of it: a three-channel 4:2:0 or 4:2:2 file.
`opj_decompress` guesses sYCC and converts it before `color_apply_icc_profile`
runs (`opj_decompress.c:1620-1635`), so a profile lcms refuses does not
leave it as coded. Checked on `jp2_sycc_420_odd` and `jp2_sycc_422_odd`
under `lcms_intent_4`, a refused gray profile (`gray_lcms_intent_4`), a
profile cut short (`bad_short_header`) and a CMYK profile (`space_cmyk`):
`-force-rgb` writes the sYCC original's planes each time. `Srgb` makes no
sYCC guess, so it refuses all of them, before the profile is read, with the
sub-sampling message. Tests:
`an_lcms_refused_profile_on_4_2_0_or_4_2_2_is_refused`, and
`an_lcms_refused_profile_passes_through_outside_the_sycc_layouts` for the
layouts that stay as coded.

**An RGB profile is skipped when the first four channels differ, as in
OpenJPEG (owner decision, 2026-11-02, on #206; rule 2).** In
`color_apply_icc_profile` (OpenJPEG 2.5.0, `src/bin/common/color.c`), lcms
opens the profile first (`cmsOpenProfileFromMem`, :468-477). For an RGB
profile OpenJPEG then returns on fewer than three components (:491-494),
and otherwise runs its "AFL test" (:495-519): it compares
`comps[1..min(numcomps, 4)]` with `comps[0]` by `dx`, `dy`, `prec` and
`sgnd`, and returns without converting if any differs. Only then does lcms
build the transform (`cmsCreateTransform`, :586). The gray (:530-534) and
YCbCr (:535-543) branches have no such test. The components are the image
after the palette and `cdef`, by position, so a differing opacity channel
counts and a fifth channel does not.

`Srgb` runs the same test in the same place: after the profile is opened
and before the transform is built, so before every check of this
decoder's own (a lookup table, the Lab connection space, a `cdef` without
colors 1 to 3, an unsupported depth). One case is refused before the
test: a 4:2:0 or 4:2:2 image, which `opj_decompress` converts
as sYCC before it reads the profile (`opj_decompress.c:1620-1630`). Other
layouts in its sYCC-guess shape stay as coded there, so the test applies
to them: 1x1:2x1:1x1 and 1x1:2x2:1x1 pass through, and 2x2:2x2:2x2
matches and converts.

Checked against `opj_decompress` 2.5.0 with lcms 2.14. With and without a
`cdef`, a signed, a 12-bit, a 2x1 and a 1x2 sub-sampled fourth channel,
and a signed or sub-sampled third color, pass through as coded
(`-force-rgb` fails, as nothing was converted). A lookup-table or Lab
profile, or a `cdef` of two colors, passes through with a signed fourth
channel. A matching fourth channel, a sub-sampled fifth, and a gray
profile with a signed opacity channel convert. Tests (`tests/jp2_icc.rs`):
`an_rgb_profile_passes_through_when_the_first_four_channels_differ`,
`the_afl_test_comes_before_this_decoders_refusals`,
`the_afl_test_covers_only_rgb_profiles_and_four_channels`,
`the_afl_test_applies_where_openjpeg_cannot_convert_as_sycc`.

**Stricter: inputs refused under `Srgb` that OpenJPEG decodes some other
way.** These are the refusals of the outcome table, with their evidence.
Each is `Unsupported` unless noted, and each is a row of
`srgb_reject_matrix_maps_each_input_to_its_typed_error` in
`src/jp2/srgb_tests.rs`:

- color channels that differ in depth, sign or sub-sampling, other than the
  sYCC chroma layouts below (owner decision, 2026-11-01; rule 3). For an
  enumerated sRGB or grayscale file there is no conversion to skip:
  `opj_decompress -force-rgb` writes the file as coded. Under an RGB profile
  they pass through as in OpenJPEG instead (the AFL test above), except for
  4:2:0 and 4:2:2, and except where a `cdef` leaves a differing color past the
  first four positions. There the AFL test passes and OpenJPEG converts
  whatever sits at positions 0 to 2, color or not, so this decoder refuses the
  file rather than give a wrong image. Checked: the fuzz seed
  `icc_rgb_uniform_after_afl` (five palette channels, the 4-bit color 3 moved
  to position 4 by a second color-3 entry) comes out of `opj_decompress` with
  positions 0 to 2 converted and the 4-bit channel as coded. For a
  three-component 4:2:0 or 4:2:2 file, whatever `colr` declares (sRGB,
  grayscale, another space, a profile, or none) and whatever its `cdef`
  labels, or a raw codestream, `opj_decompress` guesses sYCC and converts it;
  that guess is not made here (rule 3: the file does not say it is sYCC).
  Checked: `jp2_sycc_420_odd.jp2` with its `EnumCS` patched to 17 (grayscale)
  gives byte-identical `opj_decompress` output to the sYCC original, while
  `Srgb` here refuses it (a row of the reject matrix);
- sYCC other than full-resolution luma with chroma at 1x1, 2x1 or 2x2
  (rule 3). OpenJPEG prints "CAN NOT CONVERT" and writes the samples as
  coded;
- signed sYCC (owner decision, 2026-11-01, on #165; rule 3). OpenJPEG converts it
  without a message, as if it were unsigned, and clamps: on an 8-bit signed
  4:4:4 file, as-coded R `[-7, -62, 61, 114, ...]` comes out
  `[0, 0, 15, 0, ...]`. sYCC is defined on unsigned samples, so there is no
  right answer to match;
- three or more channels with an enumerated color space other than sRGB,
  grayscale or sYCC, or with no `colr`, and a raw codestream of three or more
  components, which declares no color space (rule 3). For most such spaces at
  one sampling `opj_decompress -force-rgb` fails too; at 4:2:0 or 4:2:2 it
  converts them as sYCC, which rows 3 and 29 refuse. Three spaces it does
  convert: CMYK (12) on four channels gives three RGB planes
  (`color_cmyk_to_rgb`), CIELab (14) on three gives 16-bit RGB
  (`color_cielab_to_rgb`), and e-sYCC (24) on three converts
  (`color_esycc_to_rgb`). This crate has no such conversion (they are out of
  scope for JP2), so it refuses them. One or two channels pass through as gray
  (and gray plus opacity) whatever the color space, as OpenJPEG reads any
  image of two or fewer components: from a raw codestream, or a JP2 file with
  no `colr`, any enumerated space (sRGB included, owner decision, 2026-11-02,
  on #211, even when the two channels differ in depth or sign), or an RGB or
  YCbCr profile. Only a monochrome profile converts them, on 8-bit unsigned
  samples only (other depths are refused, below). Their metadata stays as
  declared, since nothing was converted (tests
  `a_gray_jp2_without_a_converted_color_space_passes_through`,
  `srgb_on_one_or_two_channels_passes_through`);
- a monochrome (`GRAY`) ICC profile on more than two channels (owner
  decision, 2026-11-01, on #197; rule 3), for example one color channel and two
  opacity channels in `cdef`, or a palette of three columns. OpenJPEG takes
  its `numcomps > 2` branch, which passes the planes interleaved to a gray
  transform as if they were RGB, so its three-channel output is not a
  conversion. Checked on a 24x16 file with a tinted gray profile and a
  `cdef` of (color 1, opacity, opacity): `opj_decompress` writes an RGB
  PNG; the earlier output here was five channels. One gray channel (1 to 1
  or 3) and gray plus opacity (2 to 2 or 4) match OpenJPEG and are
  unchanged. Test:
  `a_gray_profile_on_more_than_two_channels_is_unsupported`;
- an ICC profile lcms would apply through a lookup table (`D2Bx` or
  `A2Bx`, which it prefers over the matrix) or the Lab connection space
  (rule 3). OpenJPEG converts these; this decoder ports only the matrix/TRC pipeline
  into XYZ. A table whose header passes lcms's checks but whose contents
  lcms rejects (an `mpet` with no elements, say) is refused here too, where
  OpenJPEG writes the samples as coded;
- a YCbCr (`YCbr`) ICC profile on three or more channels that LittleCMS
  would build a transform from (orchestrator decision, 2026-11-02, on
  #199; rule 3). OpenJPEG runs it as 16-bit YCbCr whatever the depth:
  `opj_decompress` 2.5.0 converts 16-bit samples and crashes on 8-bit ones.
  The refusal is a deviation at 16 bits, kept because YCbCr is not one of
  Annex I's restricted forms. On one or two channels, or where LittleCMS
  refuses the profile (intent 4, a missing tag), it passes through as in
  OpenJPEG (`refusals_are_ordered_as_in_openjpeg`);
- an RGB profile on samples other than 8- or 16-bit unsigned, or a gray
  profile on samples other than 8-bit unsigned (owner decision, 2026-11-01,
  on #165; rule 3). OpenJPEG picks an lcms format from the precision alone
  (`TYPE_RGB_8` up to 8 bits, else `TYPE_RGB_16`; `TYPE_GRAY_8` for gray)
  and so scales other depths wrongly, past the samples' own range. The
  review measured a 12-bit 4095 coming out 7413. With file5's profile on a
  4-bit RGB image, measured here, gray 15 comes out 18, gray 12 comes out
  13, and (15, 0, 0) comes out with red 29, all written into 4-bit planes;
- fewer than three color channels under sYCC or an RGB profile (rule 3;
  see "Color channels are picked by their labels" above);
- `raw_channels` with `Srgb`: `InvalidOptions`, before anything is read.

Chroma replication grows the output, so it is checked against the decode's
sample limit before the tiles are read (`Limit`), as palette expansion is.
After a conversion `Image::jp2` reports what the samples now are, as
OpenJPEG does (owner decision, 2026-11-01, on #165): `ColorSpace::Srgb`,
or `ColorSpace::Grayscale` for a monochrome profile, with no ICC profile.
A monochrome profile whose output is not neutral (absolute colorimetric with
a tinted media white point) gives three channels, R, G and B in place of the
gray one, as `ColorSpace::Srgb`; this is checked against the sample limit
too. sYCC or an RGB profile on one or two channels passes through, as
OpenJPEG leaves it, with the metadata as declared.
A pass-through, the default `AsCoded` decode and `read_header` report the
color space the file declares.
