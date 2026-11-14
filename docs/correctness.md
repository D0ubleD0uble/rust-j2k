# Correctness & conformance

All 23 ISO/IEC 15444-4 conformance codestreams and all nine JP2 reader files
of the same suite decode within compliance class 1. Every lossless fixture,
the GRIB2 grids included, matches OpenJPEG bit for bit. Lossy fixtures stay
within a stated tolerance of OpenJPEG: at 8 to 12 bits the largest measured
difference is 1 (see [How close a decode must be](#how-close-a-decode-must-be)).

Those results count because each one compares this decoder with an authority
outside the crate: the ISO conformance suite, a reference decoder, or a worked
example from the standard. Nothing is graded against the crate's own earlier
output. This page explains which authority wins when they disagree, how close
a decode has to come, how the conformance suite is graded, and what makes a
test prove anything. The tests that run all of this are in `tests/` and in the
unit tests under `src/`.

Three reference pages hold the detailed record:

- [openjpeg-departures.md](openjpeg-departures.md) lists every place where this
  decoder deliberately gives a different answer from OpenJPEG, with the
  evidence and the decision behind each.
- [resource-bounds.md](resource-bounds.md) gives the bounds that refuse hostile
  files, and the valid files each one refuses.
- [srgb-output.md](srgb-output.md) gives what `ColorOutput::Srgb` does with
  every kind of input.

## The hierarchy of authority

From strongest to most convenient:

1. The **ISO/IEC 15444-4 conformance suite**: the official test codestreams
   and their *compliance classes*. Lossy decoders legitimately differ in the
   low bits, so Part 4 does not ask for bit-exact output. It bounds the
   per-pixel peak error and the mean-squared error against the reference
   image, at a stated compliance class. This is the authoritative test of
   whether the decoder is correct, and Phase 2 and later gate on it. The corpus
   is in `tests/fixtures/conformance/`, and `tests/conformance_part4.rs` grades
   it.
2. The **reference-decoder oracle**: decode the same input with OpenJPEG
   (`opj_decompress`), or with eccodes for GRIB2-sourced files, and compare.
   It runs on any real-world file, so it is the day-to-day check, and it was
   the Phase 1 gate.
3. **Worked examples from the standard**: the MQ decoder has the test
   sequence of ITU-T T.88 §H.2, and the 5/3 lifting steps have small vectors
   worked by hand from F.3.8.2, so a single stage can be tested before the
   stages around it exist.

Two narrower authorities cover what the three above cannot. Where OpenJPEG
decodes a feature wrongly (the `roi_bypass/` and `poc_precedence/` sets), the
encoder's own source image is the reference, since a lossless encode must give
it back. For sRGB output the reference is LittleCMS, the library OpenJPEG
hands ICC profiles to ([srgb-output.md](srgb-output.md)).

### When the oracle and the standard disagree

OpenJPEG is the default reference, but the standard outranks it. Every case
where the two could give different answers is decided in this order
(2026-11-02):

1. Follow the standard.
2. Otherwise, do what a user hoping for automatic error correction would
   expect. OpenJPEG is usually the guide.
3. Return an error rather than an incorrect or malformed image.

The other pages cite these as rule 1, rule 2 and rule 3.

Rule 1 applies only with proof, so this decoder matches OpenJPEG unless both of
two things hold. The first is proof from a documented official source: the
ISO/IEC 15444 text, or the 15444-4 conformance reference images. Arithmetic
that looks cleaner is not proof. The second is a better result: the output has
to move *toward* the authoritative reference, with a lower PAE or MSE against
the conformance images, or a fixture that now decodes bit-exactly where it did
not. A change that is more correct in principle while the numbers don't
improve rests on an incomplete analysis, and it isn't earned yet.

The inverse 9/7's high-pass scale is the one exception (#104). This decoder
keeps the standard's exact `2/K`, although OpenJPEG's constant scores up to
0.72% lower MSE against the ISO references, so there the standard wins
without a better result. No option selects OpenJPEG's constant, so the small
gap against OpenJPEG on multi-level 9/7 detail bands is permanent.
[The `2/K` entry](openjpeg-departures.md#the-inverse-97s-high-pass-scale-2k)
has the evidence.

## How close a decode must be

On the reversible path (5/3, lossless) every sample must equal OpenJPEG's.
Any difference is a bug.

On the irreversible path (9/7, lossy) each fixture states an absolute
tolerance against OpenJPEG, and the Part 4 entries must stay within their
compliance-class peak and MSE bounds ([Grading against
Part 4](#grading-against-part-4)). The two decoders' floating-point results
differ in the low bits, so an exact comparison would fail on correct output.

The 9/7 difference from OpenJPEG depends on the bit depth, because OpenJPEG's
`two_invK` adds an error that grows with the sample range ([the `2/K`
entry](openjpeg-departures.md#the-inverse-97s-high-pass-scale-2k)). Beyond
the bit depth it depends on the content: how many sharp edges the image has and
how they line up with the transform grid. The largest differences measured:

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
size. On all of them this decoder stays within 1 of the encoder's input.

On real files the gaps fit the table: the 8-bit
`issue206_image-000.jp2`, `issue208.jp2`, `issue559-eci-091-CIELab.jp2` and
`jpx_disguised_as_jp2.jp2` are within 1, and the 16-bit
`issue559-eci-090-CIELab.jp2` is within 2.

These are measured maxima, not a bound. For a rough scale: each high-pass
scaling is off by 5.4e-5 times the coefficient, and high-pass coefficients
range about as widely as the samples, so the gap scales with 2^depth. At five
levels, collecting that error once per level gives about 18 at 16 bits, 4 at
14, 1 at 12 and under 0.1 at 8. The measured gaps stay well under this and
don't grow with the level count. The committed `irreversible_16bit` fixture
reaches 5 on its full-swing component and 3 on its mid-range one, and is
graded at a tolerance of 5.

## Grading against Part 4

`tests/conformance_part4.rs` decodes every entry in the vendored corpus and
grades it at **compliance class 1**, which bounds each graded component
independently. Per component it computes the peak absolute error (the largest
difference from the reference sample) and the mean-squared error, and checks
both against the bounds `manifest.json` records for that entry. The reference
images are `.pgx` files, one per graded component; the harness reads them
directly, so no reference decoder is needed to run the suite.

Two rules keep the grading honest. The first is that exactness comes from the
manifest's `bit_exact` flag, not from the wavelet. A `bit_exact` entry is
graded against zero bounds whatever the manifest quotes, so a corpus edit
cannot quietly relax it. The flag can differ from the wavelet: `p0_09` is
irreversible 9/7 yet reproduces its reference exactly.

The second is that an unimplemented feature is not a pass. An entry the
decoder rejects with `Error::Unsupported` reports as *not yet decoded*. Any
other rejection, a panic, a geometry disagreement, or samples outside the class
is a failure. The `IN_CLASS` list names every entry that decodes in class,
which today is all 23. A regression that turns one red, or a corpus refresh
that adds one, fails the test until the list is updated.

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

The Part 4 JP2 references are display output. The upstream `jp2_N.tif`,
committed as `.pgx` planes, is what a viewer shows: sYCC and ICC-tagged files
are already converted to sRGB, and files 4, 6 and 8 are grayscale written as
three identical channels. Files 2, 3, 5, 7 and 8 therefore can't be graded on
the samples as coded. They are decoded with `ColorOutput::Srgb` and graded
against the references, and their default decode is graded bit-exactly against
`opj_decompress` output for the same file (`as_coded` references). Files 6
(12-bit) and 7 (16-bit) have 8-bit references, so the harness shifts their
decode right by 4 and 8 (the references were made with `opj_decompress -p 8S`, a
plain right shift). Beyond the references, `opj_decompress -force-rgb` is the
oracle for the conversion arithmetic ([srgb-output.md](srgb-output.md)).

## A passing entry is not proof the feature works

`IN_CLASS` growing is the goal, but it is not the same as a feature being
implemented. A conformance entry only exercises a feature if its *other*
parameters let that feature change the answer.

The sharp case is `p0_01`. It is RLCP, and it was the only entry whose sole
remaining blocker was the progression order, so it looked like the test for
progression support. It is not. `p0_01` has one component, one quality layer,
one tile, and one precinct, and under those conditions all five progression
orders enumerate the identical packet sequence, because three of the four nested
loops (layer, component and position) run once:

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
corpus entry the partition alone unblocked, so it looked like the test for it,
but it is 128x1 with **one** resolution and a 2^7 × 2^1 precinct, which is one
precinct. It grades that `SPcod`'s precinct bytes are *parsed* (get the count
wrong and the marker segment misreads) and nothing beyond that. Mutating the
partition confirms it: break the halving of the precinct onto the subband grid,
address the tag trees by band index instead of precinct index, drop the
code-block's cap to its precinct, or drop a tile's partial leading precinct, and
`p0_11` still decodes bit-exact in every case. Each of those mutations is caught
only by the synthetic `precincts_*` fixtures, and two of them by exactly one
fixture each, `precincts_varied_sizes_lossless` and
`precincts_tiled_cprl_lossless`. Adding `p0_11` to `IN_CLASS` is honest;
treating it as the oracle for the partition would not be.

The same reasoning cuts the other way: two features can be *indistinguishable*
rather than untested. With one precinct per resolution, PCRL and CPRL enumerate
the identical packet sequence: OpenJPEG's own output for the two differs in
exactly one byte, the progression code in COD. That was the state of this
decoder until the precinct partition landed, and it is why
`progression_pcrl_lossless` and `progression_cprl_lossless` graded one order
between them, not two. The `precincts_*` fixtures are what separate them: with
a real partition the position axis has more than one value, PCRL interleaves
the components inside one sweep of the canvas and CPRL finishes a component
before starting the next, and the two packet streams diverge. Such a pair is
recorded next to the code, so that one feature's fixture is not taken for
coverage of both.

A third shape is the nastiest, because the fixture looks like it targets the
feature by name. One feature can *mask* another. Under `restart` the MQ decoder
is re-initialized for every coding pass, so a decoder that ignores `segmentation
symbols` and never reads the four decisions ending each cleanup pass still
decodes correctly: the symbols it skipped are trailing bytes in a segment nobody
reads again. Turn `restart` off and the same decoder desynchronizes immediately.
So a `restart | segsym` codestream (`p0_02` sets both, and `pred_term` too)
grades `segsym`'s *verification* but not its *consumption*, and a fixture named
for both flags is weaker than one carrying `segsym` alone. Where features
compose, the fixture that isolates each one is the one that grades it.

So an entry is the oracle for a feature only if some change to that feature's
code would make the entry fail. [development.md](development.md#check-that-a-fixture-tests-the-feature)
says how to check that before adding an entry to `IN_CLASS`.

## Per-stage golden tests

The conformance suite and the oracle test the whole pipeline at once, so a
failure there doesn't say which stage is wrong. Each stage also has isolated
golden tests, which point at the stage that broke:

- **MQ decoder**: the decoded decisions for the test sequence of ITU-T T.88
  §H.2.
- **Tier-1 passes**: known code-block byte segments in, the recovered
  coefficient planes out. The vectors are sliced from reversible (5/3),
  single-resolution OpenJPEG codestreams. There the coefficients are just the
  DC-level-shifted samples, so `opj_decompress` gives a bit-exact oracle that
  doesn't depend on this decoder. Block decoding has no 5/3-versus-9/7
  branch, so reversible vectors exercise all of it. A 0-level 9/7 codestream
  is lossy, so its samples are not a trustworthy coefficient oracle, and the
  end-to-end lossy fixtures grade the 9/7 quantization and wavelet path
  instead. Regenerate the vectors with
  [`scripts/gen-tier1-vectors.py`](../scripts/gen-tier1-vectors.py).
- **Tag trees and packet headers**: inclusion, zero bit-planes and lengths
  decoded from hand-built packet headers.
- **Inverse DWT**: 5/3 integer lifting bit-exact on known signals, and 9/7
  float lifting within tolerance, including the symmetric boundary extension
  at the edges.

## The fixture corpus

The test suite needs no reference decoder at run time. Fixtures and their
oracle snapshots are committed, and OpenJPEG or eccodes is needed only to
regenerate a snapshot ([development.md](development.md)). Each fixture records
its provenance, the source file and the exact command that produced its
oracle, so the corpus can be rebuilt.

A single fixture is a file in `tests/fixtures/`, a `.j2k` codestream or a
`.jp2` file, with a sibling `<name>.expected.json` that holds the image area,
the tolerance, the per-component samples and the provenance. The JP2
fixtures add a `jp2_boxes` section. Larger sets live in subdirectories
(`conformance/`, `partial/`, `psot0/`, `work_bound/` and others), each with
its provenance in its own README or manifest or in `tests/fixtures/README.md`.

The corpus holds:

- the GRIB2 §5.40 fixtures from the fieldglass repo (such as
  `jpeg2000_regular_latlon.grib2`), plus a 9/7 re-encode of a real grid. No
  operational GRIB2 producer ships lossy 9/7 (HRRR and NDFD are
  complex-packed, ECMWF is CCSDS), so OpenJPEG's irreversible mode makes one;
- synthetic codestreams from `opj_compress` and the `scripts/gen-*.py`
  generators, one feature or edge case each;
- the ISO/IEC 15444-4 conformance codestreams, which gated Phase 2 and all
  decode in class;
- the ISO/IEC 15444-4 JP2 reader files, synthetic `.jp2` files and real
  `.jp2` files from GDAL's test data, which gated Phase 3 ([The JP2
  corpus](#the-jp2-corpus)).

## Robustness (the input is untrusted)

A decoder parses bytes it did not produce, so malformed input is an ordinary
case, not an edge case.

- **Fuzzing.** `cargo fuzz` (libFuzzer) runs in the detached
  [`fuzz/`](../fuzz/) workspace, with eight targets: `decode`, `header` and
  `decode_partial` over the public entry points, and `tile_body`,
  `tier1_block`, `mq`, `jp2_boxes` and `icc_srgb` over single stages. See
  [`fuzz/README.md`](../fuzz/README.md) to run them. The bar is no panics, no
  unbounded allocation and no infinite loops: every rejected input returns a
  typed `Error`. A malformed SIZ cannot steer the buffers into an
  overflowing or out-of-memory allocation. SIZ's declared image area,
  `(Xsiz − XOsiz)·(Ysiz − YOsiz)`, is held to the sample limit before any tile
  is read ([The sample limit](resource-bounds.md#the-sample-limit)), and the
  Table A-9 constraints on the image and tile origins are enforced before any
  geometry reaches the DWT.
- **Typed failures.** The flat `Error` enum names the kind of failure, so a
  caller can tell a damaged codestream (`Codestream`), an illegal field
  (`Marker`), a malformed JP2 box (`FileFormat`), a feature outside what
  this decoder reads (`Unsupported`) and a resource bound (`Limit`) apart.
  The decode path does not `unwrap` or panic on input. Review and fuzzing
  hold this, not a lint; the only `expect` calls outside tests invert
  constant matrices in `src/jp2/lcms.rs`.
- **The reject matrices.**
  `reject_matrix_maps_every_out_of_subset_input_to_its_typed_error` in
  `src/codestream/tests.rs` lists codestream inputs outside the decoded
  subset and pins the error a caller sees: `Codestream` for structural
  damage, `Marker` for an illegally encoded field, and `Unsupported` for
  valid JPEG 2000 this decoder does not read. The JP2 box layer has its own
  matrix, `reject_matrix_maps_every_malformed_box_shape_to_its_typed_error`
  in `src/jp2/tests.rs`, where a malformed box is `FileFormat`, and the sRGB
  output step has `srgb_reject_matrix_maps_each_input_to_its_typed_error` in
  `src/jp2/srgb_tests.rs`. The `Limit` refusals are pinned by their own
  tests ([resource-bounds.md](resource-bounds.md)). Rejecting is not a
  formality: a feature the decoder reads and then ignores yields an arbitrary
  image, not a slightly wrong one, so anything not decoded must be refused
  rather than skipped. When a feature lands, its row moves out of the matrix,
  and a row that stops rejecting fails the test.
- **No `unsafe`.** The library has none. If a measured hot path ever needs
  it, it must be justified, isolated and fuzzed. The only `unsafe` in the
  repository is in the test binaries that count allocations.
