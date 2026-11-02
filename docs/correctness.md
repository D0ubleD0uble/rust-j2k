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
- a last tile-part with `Psot = 0` followed by more bytes in the slice it
  runs to: no `EOC`, a `jp2c` with `LBox` 0, or a `jp2c` length that does not
  end at the `EOC`. In both decoders the tile-part runs to the end of the
  file, so the bytes after it are tile data. OpenJPEG ignores them; the
  codestream parser rejects them as `Codestream` ("left after the last
  packet"). This is a known difference, not the tier-2 rule above: the
  packets themselves decode. The policy is pending owner issue #183.

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
output channel order, including its sequential swap rule: each colour entry
swaps its channel to `Asoc - 1`, later entries follow their channels, and an
entry may repeat a channel once every channel is named. The labels on
`Jp2Metadata::channels` have no OpenJPEG output to match: its library keeps
only the type, in `comps[].alpha`, which `opj_decompress` never writes out
(no PGX, PNG or TIFF writer reads it). That field is also written to the
position the entry named before its swap, so after a swap it can sit on the
wrong channel (an ARGB layout loses its opacity flag). Here each channel
carries the entry that names it, so the label moves with the samples, as
I.5.3.6 defines `Cn`.

Looser here, where OpenJPEG fails: a malformed header box after the
codestream (a second signature or `ftyp` box, a `jp2h` without `ihdr`, a
`colr` running past the end of the file, a bad `cdef`). OpenJPEG runs its box
handlers on what follows `jp2c`; this decoder only checks lengths there.

The cost is interop: a real-world encoder that produces such a stream decodes
under OpenJPEG and rejects here. No corpus or conformance file trips any of
these today. If one ever does, relax that check to match the oracle — the
protective argument loses to a demonstrated real file.

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

### sRGB output

`ColourOutput::Srgb` converts sYCC and the two restricted ICC profile forms
(I.5.3.3) to sRGB. The default output is as coded and is unaffected by
everything here. The measured error against the ISO references and against
`opj_decompress -p 8S -force-rgb` is tabulated per channel in
[`tests/fixtures/conformance/README.md`](../tests/fixtures/conformance/README.md).

**sYCC uses the IEC constants and rounds (owner decision, 2026-11-01, on
#165).** OpenJPEG's `sycc_to_rgb` computes `G = Y - (int)(0.344 Cb + 0.714
Cr)`, `R = Y + (int)(1.402 Cr)`, `B = Y + (int)(1.772 Cb)`: shortened
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
`opj_decompress` sample for sample. The only change is where OpenJPEG would
read past the chroma plane: that sample takes chroma zero here.

**ICC profiles are converted as LittleCMS converts them.** OpenJPEG hands
the profile to LittleCMS 2 (2.14 in the oracle build) with the profile's
rendering intent. `src/jp2/lcms.rs` ports the part of lcms these profiles
reach: the pipeline it builds (the tone curves and matrix into XYZ, the
white-point scaling of absolute colorimetric, black point compensation under
perceptual and saturation, since lcms's sRGB profile is ICC v4, then the
inverse sRGB matrix and curves), the optimisation it picks for that pipeline
(the 8-bit matrix-shaper in 1.14 fixed point, prelinearisation curves with a
33-point table, or resampling into a 33-point table, with the white fix-up;
for 16-bit RGB on a profile of gamma below 1.6, no optimisation), and its
16-bit interpolation, step for step at lcms's precision. It also applies
the checks lcms makes when it opens a profile (`_cmsReadHeader`): the `acsp`
signature, a version of at most 5.0, at most 100 tags, no two tags with the
same signature. Like lcms, it skips a tag entry with a zero offset or size,
or one that ends past the size the header declares (capped at the profile's
length), so a tag it needs is then missing. A profile lcms will not open is
a `FileFormat` error. The output matches `opj_decompress -force-rgb` sample
for sample (`tests/jp2_icc.rs`). The reference is OpenJPEG 2.5.0 linked
against LittleCMS 2.14 on x86-64 Linux (glibc). The output depends on the C
library's `powf`, so other LittleCMS versions or platforms can differ in the
last bit. The cases:

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
  0, 140 and 0xFFFFFFFF, and ones that cut off `wtpt` or an `A2B0` table).

Where lcms refuses a profile, the decode is a typed error.

During development the port also matched lcms 2.14 itself on every 8-bit
RGB input (2^24) and a million random 16-bit pixels for each of these
profiles. Against the ISO references the result is OpenJPEG's, inside the
class bounds (PAE 4, MSE 1):

| File | Ours vs ISO (MSE) | Ours vs OpenJPEG |
| ---- | ----------------- | ---------------- |
| file5 (RGB, gamma 1.8) | 0.0897 / 0.1750 / 0.2050 | identical |
| file7 (RGB, 16-bit, 2048-entry curves; after `>> 8`) | 0.1442 / 0.1770 / 0.1900 | identical at 16 bits |
| file8 (grey, `kTRC`) | 0.2739 | identical |

Peak error against ISO is 1 everywhere. The floating-point evaluation this
replaced was slightly closer to ISO on files 5 and 8 (MSE 0.0846 / 0.1723 /
0.2025 and 0.2115). OpenJPEG is the reference decoder, and nothing in the
standard asks for a closer match to these display references, so the
evidence rule above keeps OpenJPEG's output.

Two output shapes differ from `opj_decompress`, by choice of the grading
harness and not of the samples: file7 stays 16-bit, and the harness shifts
it down by 8 to compare with the 8-bit reference (its `shift_right`, as for
the as-coded grade), and file8 stays one grey channel where OpenJPEG writes
three identical ones (graded on that one channel).

**Stricter: inputs refused under `Srgb` that OpenJPEG decodes some other
way.** Each is `Unsupported` unless noted, and each is a row of
`srgb_reject_matrix_maps_each_input_to_its_typed_error` in
`src/jp2/srgb_tests.rs`:

- colour channels that differ in depth, sign or sub-sampling, other than
  the sYCC chroma layouts below (owner decision, 2026-11-01). OpenJPEG
  skips the ICC conversion silently and writes the samples as coded. For a
  three-component file with sub-sampled chroma, whatever `colr` declares
  (sRGB, greyscale, another space, a profile, or none) or a raw codestream,
  `opj_decompress` guesses sYCC and converts it; that guess is not made here.
  Checked: `jp2_sycc_420_odd.jp2` with its `EnumCS` patched to 17
  (greyscale) gives byte-identical `opj_decompress` output to the sYCC
  original, while `Srgb` here refuses it (a row of the reject matrix);
- sYCC other than full-resolution luma with chroma at 1x1, 2x1 or 2x2.
  OpenJPEG prints "CAN NOT CONVERT" and writes the samples as coded;
- signed sYCC (owner decision, 2026-11-01, on #165). OpenJPEG converts it
  without a message, as if it were unsigned, and clamps: on an 8-bit signed
  4:4:4 file, as-coded R `[-7, -62, 61, 114, ...]` comes out
  `[0, 0, 15, 0, ...]`. sYCC is defined on unsigned samples, so there is no
  right answer to match;
- three or more channels with an enumerated colour space other than sRGB,
  greyscale or sYCC, or with no `colr`, and a raw codestream of three or
  more components, which declares no colour space. For three components at
  the same sampling `opj_decompress -force-rgb` fails too; with sub-sampled
  chroma it guesses sYCC and converts, which owner decision (2) above
  refuses. One or two channels, from a raw codestream or a JP2 file with no
  `colr` or such an enumerated space, pass through as grey (and grey plus
  opacity), as OpenJPEG reads any image of two or fewer components; their
  metadata stays as declared, since nothing was converted;
- an ICC profile that is not one of the restricted forms: a class other than
  input (`scnr`), a colour space other than RGB or grey, a connection space
  other than XYZ, a lookup-table tag (`A2B0`..`A2B2`, `D2B0`..`D2B3`, which
  lcms would prefer over the matrix), a missing tag, or a curve type other
  than `curv` and `para` 0 to 4. lcms converts most of these;
- a rendering intent above 3. lcms refuses to build the transform and
  OpenJPEG writes the samples unconverted while reporting them converted;
  for that and for every refusal in this list the owner's rule from the
  2026-11-01 decisions on #165 applies: an error beats a silently wrong
  image;
- a `curv` tone curve of more than 32767 entries, which lcms does not read
  (OpenJPEG then converts nothing);
- an RGB profile on samples other than 8- or 16-bit unsigned, or a grey
  profile on samples other than 8-bit unsigned (owner decision, 2026-11-01,
  on #165). OpenJPEG picks an lcms format from the precision alone
  (`TYPE_RGB_8` up to 8 bits, else `TYPE_RGB_16`; `TYPE_GRAY_8` for grey)
  and so scales other depths wrongly, past the samples' own range. The
  review measured a 12-bit 4095 coming out 7413. With file5's profile on a
  4-bit RGB image, measured here, grey 15 comes out 18, grey 12 comes out
  13, and (15, 0, 0) comes out with red 29, all written into 4-bit planes;
- a profile shorter than its header, a tag table or tag past its end, or a
  tag cut short or of the wrong type: `FileFormat`. lcms reads some of these
  anyway: an `XYZ ` tag too short for its value is read on into the next
  tag;
- a monochrome profile on three colour channels. OpenJPEG runs its grey
  transform over the interleaved RGB samples, one byte per pixel, so the
  output is scrambled.
- `raw_channels` with `Srgb`: `InvalidOptions`, before anything is read.

Chroma replication grows the output, so it is checked against the decode's
sample budget before the tiles are read (`Limit`), as palette expansion is.
After a conversion `Image::jp2` reports what the samples now are, as
OpenJPEG does (owner decision, 2026-11-01, on #165): `ColourSpace::Srgb`,
or `ColourSpace::Greyscale` for a monochrome profile, with no ICC profile.
A monochrome profile whose output is not neutral (absolute colorimetric with
a tinted media white point) gives three channels, R, G and B in place of the
grey one, as `ColourSpace::Srgb`; this is checked against the sample budget
too. sYCC or an RGB profile on one or two colour channels passes through, as
OpenJPEG leaves it, with the metadata as declared.
A pass-through, the default `AsCoded` decode and `read_header` report the
colour space the file declares.

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
  by Kakadu, the ECW JPEG 2000 SDK and OpenJPEG, decoded and compared with
  `opj_decompress` or with their own extracted codestream, with the box
  metadata checked against the box dumps.

**The Part 4 JP2 references are display output.** `jp2_N.tif` is what a
viewer shows: sYCC and ICC-tagged files are already converted to sRGB, and
files 4, 6 and 8 are greyscale written as three identical channels. Files 2,
3, 5, 7 and 8 therefore can't be graded on the samples as coded. They are
decoded with `ColourOutput::Srgb` and graded against the references, and
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
- the ISO/IEC 15444-4 JP2 reader files, synthetic `.jp2` files built over the
  codestreams above, and real `.jp2` files from GDAL's test data, which gated
  Phase 3 (see "The JP2 corpus" above); HTJ2K codestreams for Phase 4.

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
