# Phase 5 roadmap — Part 1 encoder

> **Status: planned.** The decisions were settled on 2026-11-17 and no step
> has started. Every number below is marked *measured* (run on 2026-11-17,
> with the method given) or *estimated*.

The overall [roadmap.md](roadmap.md) sequences the whole project; this file
covers Phase 5 alone. It breaks "encode Part 1 codestreams and JP2 files" into
ordered steps, each tied to the module it adds, the oracle that checks it and
the gate that lets the next step start. The decisions that shape the plan are
in [Decisions](#decisions), after the background they rely on.

The feature rows are the "Encoding" rows of [scope.md](scope.md). The
specification is ITU-T T.800 | ISO/IEC 15444-1. Encoder conformance is ITU-T
T.803 (02/2024) | ISO/IEC 15444-4:2024, Annexes D and F, which the ITU
publishes free.

## What Phase 5 encodes

The input is integer samples, one grid per component, as `decode` returns
them. The output is a Part 1 codestream, raw or in a JP2 file, that both
OpenJPEG 2.5.0 and this crate's decoder read. The scope is all of Part 1
that the decoder reads (decision 4), and every profile Part 1 defines
(decision 9).

### Transform and coding

| Feature | Notes |
| ------- | ----- |
| 5/3 reversible and 9/7 irreversible forward DWT, 0 to 32 levels | the decoder's inverse is the property-test partner |
| DC level shift, RCT and ICT on components 0 to 2 | on by default for 3 or more components of equal geometry, as in OpenJPEG |
| Scalar quantization: none (5/3), derived and expounded (9/7) | |
| Depth 1 to 38 bits as SIZ allows, while each band's bit-plane count `Mb` plus the maxshift `s` is at most 30 (P5.3 defines both) | 30 is this decoder's and OpenJPEG's cap (`MAX_BIT_PLANES`), not the standard's (37, B.10.6). At 2 guard bits: 27 bits lossless 5/3, 26 with the RCT; with maxshift 11, or 10 with the RCT (decision 10) |
| MQ encoder and the three EBCOT passes | each pass's rate and distortion recorded for rate control |
| All six code-block style bits | bypass, reset, restart, vertically causal, predictable termination, segmentation symbols |
| Code-block sizes 4 to 1024 per side, area at most 4096 | the range `opj_compress -b` takes (measured: 1024×4, 4×1024, 128×32 and 32×128 round-trip losslessly, and 128×64 is refused) |
| Region of interest, maxshift (`RGN`, `Srgn = 0`) | a caller-given region per component |

### Codestream

| Feature | Notes |
| ------- | ----- |
| SOC, SIZ, COD, QCD, SOT, SOD, EOC | |
| COC and QCC, in the main header and in tile headers | per-component, and per-tile, coding and quantization |
| Tiles, image offset and tile offset | |
| Component sub-sampling | `XRsiz`, `YRsiz` from `Component::x_sampling`, `y_sampling` |
| Precinct partitions, per resolution | |
| All five progression orders, and POC | a POC volume is one entry: a range of layers, resolutions and components in one order; the volumes must cover each packet once (P5.16) |
| Quality layers with a rate per layer, and a last lossless layer | |
| Rate control: PCRD-opt (post-compression rate-distortion optimization) over each tile | decision 3 |
| SOP, EPH, PLT, TLM, COM, CRG | each one an option |
| Tile-parts split by resolution, layer or component | as `opj_compress -TP` does |
| PPM or PPT packed packet headers | one or the other, never both, as the decoder requires |

### Profiles

Each profile is a preset: it fills in the options the caller left unset and
refuses the ones that break it (decision 9). Clauses are from T.800
(06/2019), A.10.

| Profile | `Rsiz` | Restrictions |
| ------- | ------ | ------------ |
| None (Part 1 with no restriction) | 0 | the default |
| Profile 0 and Profile 1 | 1, 2 | Table A.45 |
| 2K and 4K digital cinema | 3, 4 | Table A.46 |
| Scalable 2K and 4K digital cinema | 5, 6 | Table A.46 |
| Long-term storage | 7 | Table A.46 |
| Broadcast contribution: single tile, multi-tile, multi-tile reversible, each with a main level | `0x010w`, `0x020w`, `0x0306`, `0x0307` | Tables A.48 to A.50 |
| IMF 2K, 4K and 8K: single-tile lossy, and single- or multi-tile reversible, each with a main level and a sublevel | `0x04yw` to `0x09yw` | Tables A.51 to A.54 |

### JP2 file format

| Feature | Notes |
| ------- | ----- |
| Signature, `ftyp`, `jp2h`, `jp2c` | |
| `ihdr`, `bpcc` | |
| `colr`: enumerated (sRGB, grayscale, sYCC) or restricted ICC bytes from the caller | |
| `pclr` and `cmap`: palette encode | the caller gives the index component and the palette |
| `cdef`, `res` | |
| `xml `, `uuid`, `uinf`, `jp2i` | passed through as given |

### Left for later

PSNR targets and `PRF` are refused with `Error::Unsupported`. The rest have
no option to ask for them.

| Feature | Why later |
| ------- | --------- |
| A PSNR target per layer (`opj_compress -q`) | decision 2 |
| Writing to `io::Write`, caller-provided buffers | decision 1 |
| The `PRF` marker | Table A.55 defines no profile number for it to carry |
| Packaging: DCP and IMF wrapping (MXF), the XYZ color rules of digital cinema | outside the codestream and outside the crate |
| The HTJ2K IMF and broadcast profiles | Part 15; with HTJ2K encode in Phase 6 |
| Threads and SIMD | #301, #302 |
| Narrower input sample types | with #303 |
| HTJ2K (FBCOT) encode, and Part 2 (arbitrary MCT, TCQ, custom kernels) | Phase 6 |
| Visual weighting (CSF), or any quality model other than MSE | not planned |
| Image file readers (PNM, TIFF, PNG) | not planned: the input is samples |

## What OpenJPEG 2.5.0 writes

Measured with `/usr/bin/opj_compress` (Ubuntu `2.5.0-2ubuntu0.5`) and read
back with `opj_dump`, on three inputs: `file1.jp2` from the Part 4 corpus
decoded to a 768×512 RGB image; the GRIB2-shaped bench fixture decoded to a
1440×721 16-bit grid; and `tiled_2048_gray.j2k` decoded to a 2048×2048 gray
image. The source was read at tag `v2.5.0`.

### Defaults with no options

| Field | Default | Where |
| ----- | ------- | ----- |
| Transform | 5/3 reversible (`qmfbid=1`), lossless | `opj_dump` |
| Levels | 5 (`-n 6` resolutions) | `opj_dump` |
| Code-block | 64×64, style 0 | `opj_dump` |
| Precincts | maximal (2^15) | `opj_dump` |
| Progression | LRCP | `opj_dump` |
| Layers | 1 | `opj_dump` |
| MCT | 1 (RCT) for 3 or more components, 0 otherwise | `opj_dump`: 1 on RGB, 0 on the gray and GRIB inputs |
| Quantization | none (`qntsty=0`), 2 guard bits | `opj_dump` |
| Tiles | one tile, no offsets | `opj_dump` |
| Rsiz | 0 | the SIZ bytes |
| Main-header markers | SOC, SIZ, COD, QCD, COM (`"Created by OpenJPEG version 2.5.0"`); no PLT, TLM, SOP or EPH | `opj_dump` marker list |
| Tile header | SOT and SOD only; COD and QCD never in a tile header | help text, `opj_dump` |
| Small images | refused: a 16×16 gray image fails at `-n 6` ("failed to encode image: opj_start_compress") and encodes at `-n 5` | measured |

With `-I`: 9/7, ICT for 3 or more components, and expounded quantization
(`qntsty=2`). The steps do not depend on the rate, which comes only from
truncation. OpenJPEG places the 9/7 subband gain differently from this
crate's decoder; P5.1 and P5.3 say how and what follows.

From `j2k.c` and `jp2.c`, it writes COC, QCC, POC, RGN, PLT, TLM, COM and
the Part 2 MCT markers, but no PPM, PPT, CRG, `pclr` or `cmap`. It writes
COC and QCC only when components differ, which no `opj_compress` option
sets.

### Rate and quality at given settings

One run each. Size is the whole codestream. PAE is the peak absolute error
against the source, and PSNR uses the peak `2^depth − 1`, both over all
samples of the `opj_decompress` decode.

| Input | Options | Bytes | bpp | PAE | PSNR (dB) |
| ----- | ------- | ----- | --- | --- | --------- |
| RGB 768×512 | (default) | 649450 | 13.21 | 0 | lossless |
| | `-I` | 497790 | 10.13 | 4 | 51.21 |
| | `-r 20` (5/3) | 58976 | 1.20 | 59 | 33.27 |
| | `-I -r 20` | 58976 | 1.20 | 49 | 33.52 |
| | `-I -q 40` | 186165 | 3.79 | 19 | 39.98 |
| | `-I -r 80,40,20,10` | 117854 | 2.40 | 32 | 36.98 |
| 16-bit 1440×721 | (default) | 699120 | 5.39 | 0 | lossless |
| | `-I` | 1227318 | 9.46 | 1 | 106.95 |
| | `-I -r 20` | 103691 | 0.80 | 454 | 57.10 |
| | `-I -q 40` | 601 | 0.005 | 4391 | 40.08 |
| 2048² gray | `-I -r 20` | 209730 | 0.40 | | |

Findings:

- `-r` lands within 0.13% of its target, and once 15 bytes over: 58976 of
  58982 bytes (RGB), 103691 of 103824 (16-bit), 209730 of 209715 (gray).
- Its target is `numcomps · w · h · prec / (8 · ratio · dx · dy)` less a
  header estimate (`j2k.c:5260`). It reads component 0 alone, so it is
  wrong for mixed depth or sampling.
- `-I` with no rate keeps nearly every bit: 1.76 times the lossless size on
  the 16-bit grid.
- `-q` met its PSNR within 0.02 dB, and `-r 1` or less means all passes
  (`j2k.c:8070`).

### How OpenJPEG allocates rate

`opj_tcd_rateallocate` (`tcd.c:443`) bisects one slope threshold 128 times
per tile and layer, taking each block's passes at or above it, with no
convex-hull pruning. Its `ΔD` is a weighted-MSE estimate (`opj_t1_getwmsedec`),
not a measured distortion. Grok's default `-A 1` adds the hull.

### POC and ROI in `opj_compress`

Measured on the RGB input at `-n 3`:

| Options | Bytes | `opj_decompress` against the source |
| ------- | ----- | ----------------------------------- |
| `-r 4,1` (no POC) | 652407 | lossless |
| `-r 4,1 -POC T1=0,0,1,3,3,CPRL/T1=0,0,2,3,3,LRCP` (the volumes overlap in layer 0) | 294833 | PAE 14: not lossless, though the last layer asks for all passes |
| `-POC T1=0,0,1,2,3,CPRL/T1=2,0,1,3,3,LRCP` (disjoint resolutions, one layer) | 651788 | lossless |
| `-r 4,1 -POC T1=0,0,2,2,3,CPRL/T1=2,0,2,3,3,LRCP` (disjoint, two layers) | 652439 | lossless |
| `-ROI c=0,U=3` | | lossless; `roishift=3` on component 0 |
| `-I -r 40 -ROI c=0,U=7` | 29501 | PSNR 30.73 dB, the same as without `-ROI` (29494 bytes) |

So OpenJPEG writes POC correctly only when the volumes are disjoint, which
adds to the overlap fault in
[openjpeg-departures.md](openjpeg-departures.md#bytes-left-after-the-last-packet).
Its `-ROI` shifts a whole component up and does nothing else: every
coefficient is then in the region, so it tests the `RGN` marker and the shift
alone, not a region shape or a background.

### High depths

From a separate measurement on 2026-11-17: 64×64 one-component images of
uniform noise, a mid-range field (`2^(d−1) ± 128`) and a min/max
checkerboard, decoded by `opj_decompress` and by this crate.

| Encoder | Highest depth it encodes right | What happens above it |
| ------- | ------------------------------ | --------------------- |
| `opj_compress` 2.5.0, 5/3, 5 levels | 24 bits; 26 at `-n 1`; 24 with `-I` | exits 0 with no warning, and both decoders decode the same wrong image: full-range data from 25 bits, the mid-range field from 30 |
| `grk_compress` 20.4.25 | 16 bits | refused ("precision = 17 not supported") |
| `ojph_compress` 0.33.0 (HTJ2K) | 32 bits, with its own decoder | raises the guard bits past 29 bits and uses 64-bit blocks; `opj_decompress` refuses its output from 29 bits ("number of bitplane, 31, is larger than 30") |

OpenJPEG has no depth check in the encoder. Coefficients are held as
`coef << 6` in `int32` (`t1.c:2232`), which wraps from `2^25`. An `εb` of 32
or more wraps to 0 in the `QCD` byte (`j2k.c:10960`), and the zero bit-plane
count can go negative (`t2.c:759`). Its `εb` has no RCT bit, and
`-GuardBits` does not move the `int32` limit. Signed input could not be
measured: the PGX reader replaces the declared precision with the data's
(`convert.c:1326`), and raw and TIFF input stop at 16 bits.

Neither Kakadu nor JJ2000 was run. By its documentation, Kakadu bounds `εb`
from the options (`Qabs_ranges`, `Qguard` 1 by default), and defaults to
9/7, 5 levels, 64×64 blocks, LRCP, one layer and `Qstep = 1/256`. JJ2000,
by its source, drops planes past its `int` silently.

### Profiles in `opj_compress` and `grk_compress`

`opj_compress` 2.5.0 has `-cinema2K <24|48>`, `-cinema4K` and
`-IMF <2K|4K|8K|2K_R|4K_R|8K_R>[,mainlevel=X][,sublevel=Y][,framerate=FPS]`.
It has no option for Profile 0 or 1, the scalable cinema profiles,
long-term storage or broadcast. Grok 20.4.25's `grk_compress` adds
`-U <SINGLE|MULTI|MULTI_R>` for broadcast. Measured on seeded synthetic
12-bit RGB frames of 2048×1080 and 4096×2160, with the headers read by a
short marker walker:

| Command | `Rsiz` | Bytes | What it wrote |
| ------- | ------ | ----- | ------------- |
| `opj_compress -cinema2K 24` | 3 | 1302055 (limit 1302083) | 9/7, 5 levels, 32×32, precincts 128 then 256, CPRL, 1 layer, TLM, 3 tile-parts |
| `opj_compress -cinema2K 48` | 3 | 651011 (limit 651041) | the same |
| `opj_compress -cinema2K 24 -n 7 -b 64,64 -p LRCP` | 3 | 1302055 | the same as without the options: levels, block size and order replaced |
| `opj_compress -cinema4K` | 4 | 1302074 | 5 levels, the two-progression POC of Table A.46, 6 tile-parts |
| `opj_compress -IMF 2K` | `0x0400` | 5178616 | 9/7, 5 levels, 32×32, CPRL, TLM, 3 tile-parts |
| `opj_compress -IMF 2K_R` | `0x0700` | 5834503 | the same with 5/3 |
| `opj_compress -IMF 2K,mainlevel=4,sublevel=2,framerate=24` | `0x0424` | 5179129 | prints "Setting max codestream size to 2083333 bytes" and writes 2.5 times that |
| `opj_compress -IMF 2K -n 7` | 0 | 5177759 | warns "Non-IMF codestream will be generated" and exits 0 |
| `opj_compress -IMF 2K -ROI c=0,U=3` | 0 | 5178623 | the same, with `RGN` |
| `grk_compress -U SINGLE` | `0x0100` | 5130262 | 9/7, 4 levels, 64×64, CPRL, 3 tile-parts, no TLM |
| `grk_compress -U MULTI_R` | `0x0300` | 5784650 | the same with 5/3 |

`opj_decompress` decodes every one of them. What this shows:

- OpenJPEG writes the cinema and IMF fields Tables A.46 and A.51 ask for, so
  it is a field-level oracle for them: its header fields can be compared
  with ours, though its coded bytes cannot.
- It replaces conflicting options under cinema, with a warning, and drops an
  IMF profile to `Rsiz` 0 with exit status 0.
- It prints the IMF sublevel byte limit and does not apply it: at sublevel 1
  and 24 fps it printed 1041666 bytes and wrote 3068017.
- Grok's broadcast streams leave out the TLM Table A.48 requires, and
  `0x0300` is not a Table A.10 value. Grok is a field-level oracle only for
  the choices Table A.48 leaves open.

### Speed and memory

Median of 3 runs of the whole process, measured with `/usr/bin/time` at a
1-minute load average of about 6:

| Input | Options | Seconds | Peak MB |
| ----- | ------- | ------- | ------- |
| 2048² gray | (default) | 0.16 | 31 |
| | `-I` | 0.20 | 32 |
| | `-I -r 20` | 0.23 | 31 |
| | `-threads ALL_CPUS` | 0.09 | 33 |
| RGB 768×512 | each option set above | 0.11–0.14 | 13–14 |
| 16-bit 1440×721 | each option set above | 0.17–0.27 | 13–15 |

### Which defaults to mirror

The encoder mirrors OpenJPEG's defaults, so that `encode(&image)` agrees with
`opj_compress -i` and the round trip is lossless by default. Those are 5/3, 5
levels, 64×64 blocks, maximal precincts, LRCP, one layer, RCT for 3 or more
components, 2 guard bits, one tile, `Rsiz` 0 and no optional markers. It
departs in two places, both recorded in openjpeg-departures.md:

1. **Levels on small images.** The default is `min(5, the levels the
   smallest tile-component allows)`, where OpenJPEG refuses the image. A
   GRIB2 grid can be 16×16. An explicit level count that does not fit is
   still `InvalidOptions`.
2. **No COM marker by default.** The encoder's name is not image data, and
   leaving it out keeps the output the same across crate versions. A comment
   is one option away.

## The gate

### 1. Round trip

- **Reversible:** every input in the corpus, with every reversible option
  set in the sweep (P5.0), decodes bit for bit to the input through this
  decoder and through `opj_decompress`. The source image is the reference,
  so the check is external even with this crate's own decoder.
- **Irreversible with fixed quantization (no rate):** PAE against the source
  is at most OpenJPEG's plus 1, and MSE at most OpenJPEG's, for the same
  input and options, up to 24 bits. OpenJPEG measured PAE 4 and MSE 0.49 on
  the RGB input, and PAE 1 and MSE 0.09 on the 16-bit grid. Above 24 bits,
  where OpenJPEG's `-I` is wrong, the bound is set in advance: the bound
  for the same pattern and seed generated at 24 bits, times `2^(B−24)`. The
  PAE P5.8 commits there is a regression baseline, not a correctness check.
- **Irreversible at a rate:** graded by parity (part 3), not by an absolute
  PSNR, since the right PSNR depends on the image.
- **The two decoders agree on our output:** bit for bit on 5/3. On 9/7, the
  bound is the decoder's 9/7 gap plus a margin. The gap is the largest
  difference measured between the two decoders on OpenJPEG's files
  ([correctness.md](correctness.md#how-close-a-decode-must-be)): 1 at 8 to 12
  bits, 2 at 14 and 7 at 16. It is a measured figure, not a proven bound.
  The margin is 1 at 8 to 12 bits and 2 at 14 and 16, so the bound is 2, 4
  and 9. Depths 1 to 7 take the 8-bit row, and a depth between rows takes
  the higher row. The margin is 2 at 16 bits, the spread measured there (5
  to 7 for one pattern). At 8 to 14 bits it is an estimate, since
  correctness.md shows no spread; the data are from OpenJPEG's files, not
  ours. A case over the bound fails the step until the cause is found and
  recorded in correctness.md; the bound is not raised to pass it. From 17
  to 24 bits there is no two-decoder bound: each decode is checked against
  the source as above, and P5.8 commits the difference as a regression
  baseline.

### 2. Cross-decode

- `opj_decompress` 2.5.0 decodes every encoded file with no warning or
  error, and its output meets part 1. Grok decodes them as a second opinion
  in a release round, one of the checks in
  [release-validation.md](release-validation.md) (decision 6).
- This decoder reads `opj_compress` output. The decoder already does, and the
  parity corpus keeps that in the gate, since the decoder must read
  OpenJPEG's files.
- Where OpenJPEG decodes a feature wrongly, it is not the oracle for it. For
  bypass under a maxshift, the reference is the source image (see P5.17).

### 3. Rate and quality parity with `opj_compress`

The parity set is every corpus input at 0.125, 0.25, 0.5, 1 and 2 bpp with
one layer, and at `-r 80,40,20,10`. At equal settings (levels, block size,
layers, transform, MCT, and targets that mean the same bytes):

- **Size:** each layer's cumulative size is at most its target and at least
  98% of it. OpenJPEG lands within 0.13%, and once 15 bytes over (measured,
  above), so this gate is stricter than OpenJPEG on the upper side. The
  target uses this crate's definition of bpp (decision 2), and OpenJPEG is
  given the ratio that means the same number of bytes.
- **Quality:** PSNR, with both files decoded by `opj_decompress`, is at
  least OpenJPEG's minus 0.1 dB in every case, and the mean difference is at
  least 0. *Estimated:* a convex-hull PCRD-opt with measured distortion should
  match or beat OpenJPEG's threshold search, so 0.1 dB leaves room for noise
  without hiding a broken allocator.
- **How it is measured:** `scripts/gen-encoder-parity.py` commits one JSON
  row per case: OpenJPEG's size and PSNR, and the SHA-256, size and PSNR of
  this encoder's output. The test encodes again, checks the digest (so a
  changed encoder fails until the snapshot is rerun), and checks both
  inequalities on the committed numbers, with no external tool.
- **Depth range:** parity, and every byte or field comparison with
  `opj_compress`, covers depths up to 24 bits only. From 25 bits OpenJPEG
  writes streams that decode wrong (measured; see [High
  depths](#high-depths)), so there is nothing to compare against. Above 24
  bits the gate is part 1 alone: the source image, through both decoders.
  `opj_decompress` reads our streams there correctly, since its Part 1
  decoder checks coded planes, not `Mb`.

### 4. ISO/IEC 15444-4 encoder conformance

T.803 Annex D is informative, and Part 4 has no encoder test files or
bounds. Annex D asks that a reference decoder fully decode the output, over
test images and parameters the implementer chooses (D.4 a to f). Annex F is
an implementation compliance statement (Table F.1). So:

- the 78 Part 4 reference images (`tests/fixtures/conformance/references/`)
  are encoder inputs, beside the other corpora;
- the sweep covers each axis of D.4 e) and f): tile size, levels, rates;
  small, large and odd image sizes; component count, depth and sampling;
- "fully decode" means two decoders, OpenJPEG and this one, instead of the
  T.804 reference software, which is not maintained;
- `docs/encoder-ics.md` fills in Annex F: Table F.1 with this encoder's
  marker usage, and the options it supports.

### 5. Profile conformance

Every encode with a profile passes the rule checker (P5.20): each
restriction of T.800 A.10 for that profile, evaluated on the codestream as
this crate's parser reads it, with the clause it comes from. It also decodes
in both decoders and meets parts 1 to 3. Where `opj_compress` or
`grk_compress` writes the same profile, the header fields match theirs,
except where they are measured to break the profile (above).

### Oracle limits

Where the external oracle is weakest, this is what a passing gate shows and
what it does not.

| Step | What it proves | What it cannot prove |
| ---- | -------------- | -------------------- |
| P5.16 POC | OpenJPEG reads the marker and places each packet where this encoder put it, or a lossless decode fails; the prefix check shows the order is the one meant, even where every order gives the same packets | anything about the refused sequences |
| P5.17 ROI | OpenJPEG reads `RGN`, undoes the shift and sorts region from background as this encoder did | the bypass cases, where OpenJPEG is wrong; OpenJPEG's own ROI beyond a whole component, since `opj_compress -ROI` makes no background |
| P5.20 rule checker | the output meets every rule as transcribed, and the checker tells a passing stream from a failing one on every rule | that a rule was transcribed right: each row cites its table and row for review, and P5.21, P5.23 and P5.24 compare with encoders that read the tables themselves |
| P5.21 cinema | this encoder chooses the parameters an independent encoder chooses, meets the rules and decodes | the DCI and SMPTE rules for a cinema package beyond T.800, such as XYZ color and MXF wrapping |
| P5.22 scalable cinema and storage | the rules hold, and stripping layer 1 gives a cinema stream | that our reading of the layer-0 byte limit and the tile-part order matches another implementation's, since none exists |
| P5.23 broadcast | the open choices agree with, or knowingly differ from, Grok's | that the level arithmetic matches another implementation's, since Grok was not measured enforcing it |
| P5.24 IMF | as P5.21, and the sublevel byte limit holds where OpenJPEG does not hold it | the IMF application rules of SMPTE ST 2067 beyond T.800 |

## Decisions

Settled on 2026-11-17, except where marked proposed.

1. **The API is `encode` and `encode_with`.** `encode(&Image)` and
   `encode_with(&Image, &EncodeOptions)` mirror `decode` and `decode_with`.
   `EncodeOptions` is a `#[non_exhaustive]` struct with `with_*` setters, like
   `DecodeOptions`. The output is `Result<Vec<u8>>`: a raw codestream, or a
   JP2 file under `with_format(Format::Jp2)`. Input the encoder cannot encode
   is a new `Error::InvalidImage`, such as a sample outside its declared
   depth, and bad options are `Error::InvalidOptions`. There is no `Encoder`
   builder until threads (#301) give it state, and no `io::Write` output,
   since rate control, TLM and `Psot` need lengths first.
   `Error::InvalidOptions` is documented as decode-only (`src/error.rs`, the
   variant and the error table), so that rustdoc is widened when
   `EncodeOptions` reuses it. A borrowed input view comes with #303.
   `Image::jp2` and `Image::report` are ignored: the boxes come from the
   options, and a report describes a decode. The `encode` rustdoc says so. The
   origin comes from #254, on the grid of `width` and `height`, so component
   sizes and DWT parity come out right. A palette-expanded image can hold
   samples outside its declared depth (`src/image.rs`), so it is
   `InvalidImage`; sRGB output clamps to the depth and encodes.
2. **Rate is given in bits per pixel.** `Rate::Lossless` (all passes) is the
   default. `Rate::Layers` takes cumulative, increasing bpp per layer, with
   a last layer of all passes allowed, and `Rate::ratio` converts a
   compression ratio. bpp is compressed bits, headers included, over the
   image area's pixels. The ratio is against the sum over components of
   `w · h · depth / 8`. That departs from OpenJPEG, which reads component 0
   alone, for mixed depth or sampling. PSNR targets are deferred: they need
   a distortion model in the sample domain, and no user has asked for them.
3. **Lossless and fixed quantization first, then true PCRD-opt.** The first
   end-to-end encodes take all passes, so the pipeline is gated before rate
   control exists. PCRD-opt then takes a convex hull of each block's
   truncation points, bisects one slope per tile to the byte target, and
   orders layers by decreasing slope. Distortion is measured: the decrease in
   squared coefficient error times the band's squared synthesis norm.
   OpenJPEG's results are a floor (gate part 3), not a method to copy.
4. **Everything in Part 1 that the decoder reads is in 0.7.0.** That
   includes POC, ROI (maxshift), PPM and PPT, COC and QCC, CRG and palette
   encode, beside the transforms, tiling, precincts, progressions, layers,
   code-block styles, markers and tile-parts. Each has its own step and
   oracle below. POC and ROI have the weakest external oracles, and [Oracle
   limits](#oracle-limits) says what a cross-decode can and cannot prove.
5. **The encoder sits behind an `encode` cargo feature.** Whether it is on by
   default is proposed, to be confirmed in 0.7: the #515 answer of 2026-11-26
   does not decide the encoder default. If it is on by default, a decode-only
   user (fieldglass, or wasm) sets `default-features = false` and compiles
   none of it. The feature (`encode = []`, in `default`) is declared before
   P5.0. Every encode test, bench and `src/fuzz.rs` entry is gated on it;
   `Error::InvalidImage` is not, so the error enum is the same in both builds.
   CI gains `cargo test` and `cargo clippy --all-targets` with
   `--no-default-features`. Code only the encoder calls lives in `src/encode/`
   or behind the feature, so a decode-only build has no dead code. The
   `Cargo.toml` and CI edits are the maintainer's.
6. **The reference tools are OpenJPEG, Grok and jpylyzer.** OpenJPEG 2.5.0 is
   the gate. Grok (20.4.25, built for performance.md) is a second decoder in a
   release round. `grk_compress -U` is the field-level oracle for the
   broadcast presets, which OpenJPEG cannot write. jpylyzer checks JP2
   validity when snapshots are regenerated. Kakadu may be run by hand as an
   extra check; its results are never committed as oracles and never feed
   CI. None of the tools is a build or test dependency.
7. **No new images are committed.** The inputs are the committed fixtures
   decoded, the 78 Part 4 references and seeded synthetic images. Only
   snapshots are committed: the parity JSON, and per cross-decode case the
   digests of our output and of `opj_decompress`'s decode of it. The Part 4
   references and the tests that use them stay excluded from the crate.
8. **One release, 0.7.0,** when the whole gate passes, as Phases 3 and 4
   shipped. The decoder hardening ships before it as 0.6.0. With 0.7.0,
   `Cargo.toml`'s `description` and keywords change, and CONTRIBUTING's
   "The crate only decodes" goes; both are the maintainer's edits.
9. **Every Part 1 profile is in 0.7.0, as a preset.** That is Profile 0 and
   1 and the five cinema, three broadcast and six IMF profiles, with their
   levels. Each is selected with `EncodeOptions::with_profile(Profile::…)`,
   a `#[non_exhaustive]` enum, for example `Profile::Cinema2k { fps: 24 }`.
   A preset fills in every option the caller did not set, from the
   profile's table. A set option that breaks the profile, or an image the
   profile cannot hold, is `InvalidOptions`, naming the option, the rule and
   its table. The preset never replaces a set value: a library has no
   warning channel, and OpenJPEG shows how overrides go wrong (measured
   above). The frame rate is required whenever a level sets a byte limit,
   which is the bit rate over the frame rate (Equation A-8). A rate above it
   is refused up front, and a lossless encode that comes out over it is
   refused after coding. `PRF` is not written.
10. **Depth: the worst case from the options, plus a data check.** Before
    coding, the options are `InvalidOptions` when any band's worst-case `Mb`
    plus the maxshift `s` exceeds 30, the decoder's cap. `Mb` comes from
    Equation E-10 plus 1 on the components the RCT widens, and reversible
    coding needs at least 2 guard bits (P5.3 has the formula and the
    reasons). The data check, while coding, compares each block's top bit
    with its band's `Mb`. A block over it is an internal error
    (`Inconsistent`), never a clamp, and nothing may wrap before the check.
    There is no 64-bit block path and no `εb` sized from the data. The
    limits: 27 bits lossless 5/3, 26 with the RCT; with maxshift 11, or 10
    with the RCT. Chosen after measuring three encoders and reading two more
    ([High depths](#high-depths)); P5.3 gives the reasons. The encoder never
    writes a file that no `DecodeOptions` lets its decoder read, where
    OpenJPEG writes a broken one from 25 bits.
11. **The encoder refuses a tile its decoder cannot read.** Options that
    give a tile more than the decoder's fixed caps, 2^18 precincts or 2^20
    code-blocks (`src/tier2/geometry.rs`), are `InvalidOptions`. No
    `DecodeOptions` raises those caps, so this follows decision 10's rule.
    The sample limit (2^29 by default) is not checked: a caller can raise it
    with an option, and the encoder's caller already holds the image. The
    encoder's docs say which option to raise to read a large output back.

Two smaller choices follow from the defaults: the levels clamp on small
images, and no COM marker by default (see [Which defaults to
mirror](#which-defaults-to-mirror)).

## Corpora

| Set | Source and provenance | Size | Committed? |
| --- | --------------------- | ---- | ---------- |
| Decoded fixtures | every lossless fixture in `tests/fixtures/`, decoded by this decoder (already graded against OpenJPEG) | no new bytes | already |
| Part 4 references | `tests/fixtures/conformance/references/*.pgx`, 78 files: the D.4 a) inputs | no new bytes | already; excluded from the crate |
| Synthetic | a seeded generator in `tests/support/synth_image.rs`: noise, ramps, checkerboards, edges, flat fields; sizes from 1×1 to 1024×1024, including odd and prime sizes; 1 to 4 components; sampling 1×1, 2×1 and 2×2. Unsigned depths 1, 8, 12, 16 and 24 (the last that OpenJPEG writes right), and 25, 26 and 27 (graded by the source alone). Signed depths 17 to 27, graded by the source alone. Reject cases: 28 and 31 bits unsigned, 32 signed. Each high depth as uniform noise, a mid-range field and a min/max checkerboard | none (generated) | the generator only |
| Profile frames | the same generator at the profiles' sizes, 8 to 16 bits, 3 or 4 components, 4:4:4 and 4:2:2: 2048×1080, 1998×1080, 2048×858, 4096×2160, 3996×2160, 4096×1716, 1920×1080, 2048×1556, 4096×3112, 3840×2160, and 7680×4320 and 8192×6224 (release round only) | none (generated) | the generator only |
| Profile snapshots | `tests/fixtures/encode/profiles.json`: per case, the header fields `opj_compress` or `grk_compress` wrote, and the digest of this encoder's output | about 20 KB, estimated | yes |
| GRIB2 fields | the committed `jpeg2000_regular_latlon`, `eta_lambert_*` and GRIB2-shaped bench codestreams, decoded | no new bytes | already |
| Parity snapshots | `tests/fixtures/encode/parity.json` | about 50 KB, estimated | yes |
| Cross-decode digests | `tests/fixtures/encode/crossdecode.json` | about 100 KB, estimated | yes |
| Large timing input | `tests/fixtures/bench/tiled_2048_gray.j2k`, decoded | no new bytes | already |

Every snapshot row records its input (fixture path or generator seed), the
options, the tool versions and the command, in `tests/fixtures/README.md`.

## Build order and why

```text
P5.0 harness ─ P5.0b decoder seams ─┬─ P5.1 forward DWT ──┐
                                    ├─ P5.2 forward MCT ──┤
                                    ├─ P5.3 quantization ─┼─ P5.7 lossless file ─┬─ P5.8 9/7 fixed quant ─ P5.9 rate control ──────────────────┐
                                    ├─ P5.4 MQ ─ P5.5 EBCOT passes ─┤      ├─ P5.10 tiles, sampling ─ P5.11 precincts + orders ─ P5.16 POC ┤
                                    └─ P5.6 tag trees + packets ────┘      ├─ P5.12 code-block styles ─ P5.17 ROI ─────────────────────┤
                                                           ├─ P5.13 markers + tile-parts ─ P5.15 COC/QCC ─ P5.18 PPM/PPT ┼─ P5.26 gate
                                                           └─ P5.14 JP2 writer ─ P5.19 palette ─────────────────────────┤
P5.13 + P5.15 ─ P5.20 profile presets + rule checker ─┬─ P5.21 cinema 2K/4K ─ P5.22 scalable cinema + storage ─────────┤
   (P5.21, P5.22 also need P5.9, P5.11, P5.16;         ├─ P5.23 broadcast ─────────────────────────────────────────────┤
    P5.23, P5.24 also need P5.9, P5.10, P5.11)         └─ P5.24 IMF ───────────────────────────────────────────────────┤
P5.25 fuzz targets + reject matrix (from P5.4, then throughout) ─────────────────────────────────────────────────────────┘
Before P5.0: the `encode` feature declared. Before P5.7: #254 (the origin on Image). #414 has landed (#447).
```

That is 28 steps: P5.0, P5.0b and P5.1 to P5.26. The diagram's right-hand
columns are schematic; the dependencies are the ones the lanes below list.

P5.0b opens every decoder seam in one PR, except where a row below names a decoder file, so no two encoder steps edit the
same decoder file. The stage pieces then land in parallel on disjoint files
(P5.1 to P5.4 and P5.6), each against its decoder inverse and an external
oracle, as Phase 2 built the decoder. P5.7 is the first file out and fixes
the API shape early. Rate control (P5.9) follows a fixed-quantization 9/7
encode (P5.8), so that a rate bug cannot hide behind a transform bug.

After P5.7 the work splits into six lanes that can run side by side:

- **Rate:** P5.8, then P5.9.
- **Tier-2 and geometry:** P5.10, P5.11, then P5.16 (POC needs the five orders
  and several precincts to mean anything).
- **Tier-1:** P5.12, then P5.17 (ROI changes the bypass count and the plane
  count, so it follows the styles).
- **Headers:** P5.13, then P5.15 (tile-header COC and QCC need tiles, so P5.10
  too), then P5.18 (packed headers move what P5.11's packets write, so P5.11
  too).
- **File format:** P5.14, then P5.19.
- **Profiles:** P5.20 once the headers lane has P5.13 and P5.15. Then
  P5.21, P5.23 and P5.24 side by side, each once the features it presets
  exist (rate control, precincts, tiles, and POC for 4K). P5.22 follows
  P5.21, whose file it extends. A preset only chooses values for options
  that already work, so this lane adds no coding path.

### Files and collisions

P5.7 creates `src/encode/markers.rs`, with one writer function per marker
segment, and `src/encode/codestream.rs`, which only orders the calls. A step
that adds a marker appends its function to `markers.rs` and one call to
`codestream.rs`, so concurrent merges conflict only on appended lines.

| Step | Writes | Touches in the decoder |
| ---- | ------ | ---------------------- |
| P5.0b | none in `src/encode/` | every seam below (see P5.0b) |
| P5.1 | `src/encode/dwt.rs` | uses `lift` and `lift_split` (`src/dwt.rs`), opened in P5.0b |
| P5.2 | `src/encode/mct.rs` | none: the forward RCT and ICT share no code with the inverse |
| P5.3 | `src/encode/quant.rs`: the gain and synthesis-norm tables, which the decoder does not have | none |
| P5.4 | `src/encode/mq.rs` | uses `QE_TABLE` (already `pub`) and the H.2 vectors, opened in P5.0b |
| P5.5 | `src/encode/passes.rs` | uses `src/tier1/ebcot/context.rs`, split out in P5.0b |
| P5.6 | `src/encode/tagtree.rs`, `src/encode/packet.rs` | uses `resolution_geoms` and `for_each_packet` (already re-exported), and `band_rects` and `next_segment` (opened in P5.0b) |
| P5.7 | `src/encode/mod.rs`, `options.rs`, `codestream.rs`, `markers.rs`; `src/lib.rs` re-exports | `src/codestream/markers.rs`: shared structs; `src/pipeline.rs`: module doc |
| P5.8 | `src/encode/mod.rs` (9/7 branch) | none |
| P5.9 | `src/encode/rate.rs` | none |
| P5.10 | `src/encode/tile.rs` | uses the geometry re-exports |
| P5.11 | `src/encode/packet.rs` | none |
| P5.12 | `src/encode/passes.rs`, `src/encode/mq.rs` | uses the segment and pass-mode helpers of P5.0b |
| P5.13 | `src/encode/markers.rs`, `codestream.rs` | none |
| P5.14 | `src/encode/jp2.rs`, with its own box-type constants | none for box types: the decoder has none and `jp2::boxes` is private. Open choice: the options take the public box types (`ImageHeader`, `ColorSpecification`, `Resolution`, `XmlBox`, `UuidBox` and the rest), which needs a constructor on each in `src/jp2/metadata.rs`, new API on frozen types, or the options' own box types, which needs none |
| P5.15 | `src/encode/options.rs` (per-component and per-tile), `markers.rs`, `tile.rs` | none |
| P5.16 | `src/encode/poc.rs`, `packet.rs`, `markers.rs` | uses `check_poc_coverage`, re-exported in P5.0b |
| P5.17 | `src/encode/roi.rs`, `passes.rs`, `markers.rs` | uses `undo_maxshift` (`pub(crate)`) and `top_coded_plane` (`pub`) as they are |
| P5.18 | `src/encode/packed.rs`, `codestream.rs` | none |
| P5.19 | `src/encode/jp2.rs`, `options.rs` | `src/jp2/palette.rs`: a validating `Palette::new` (NE 1..=1024, NPC 1..=255, column depths) and a `PaletteColumn::new`, new API on frozen types (`Palette` has private fields and is `#[non_exhaustive]`; `PaletteColumn` is `#[non_exhaustive]`, so a caller cannot build one). |
| P5.20 | `src/encode/profile/mod.rs`, `profile/rules.rs`, `profile/check.rs`, `options.rs` | `src/codestream/mod.rs`: keep `Rsiz` and each tile-part's marker list in the parsed header, `pub(crate)` |
| P5.21 | `src/encode/profile/cinema.rs` | none |
| P5.22 | `src/encode/profile/cinema.rs` | none |
| P5.23 | `src/encode/profile/broadcast.rs` | none |
| P5.24 | `src/encode/profile/imf.rs` | none |
| P5.25 | `fuzz/`, `tests/encode_reject.rs` | `src/fuzz.rs`: new entries behind the `encode` feature |

The collisions left are within lanes, which run in order: P5.10 and P5.15
on `tile.rs`, P5.12 and P5.17 on `passes.rs`, P5.14 and P5.19 on `jp2.rs`,
and P5.21 and P5.22 on `cinema.rs`. Every step may also grow `options.rs` by
one field, and each option lands in its own commit to keep the merge
mechanical. P5.9 lands its interface (a `CodedTile` of per-block pass lists
in `src/encode/mod.rs`) before the geometry lane builds on it.

## Steps

### P5.0 — Encoder harness and corpus

**Goal:** the grading scaffold exists before the encoder, as P2.0 to P4.0
did.

**Work:**
- The synthetic image generator, and the input list (decoded fixtures,
  Part 4 references, synthetic images, GRIB2 grids).
- The sweep, `tests/support/encode_sweep.rs`: a committed list of option
  sets per axis (tile size, levels, block size including 1024×4, 4×1024
  and 128×32, rates, image size, component count, depth, sampling). Each
  later step adds its options' axes.
- `scripts/gen-encoder-parity.py` and `scripts/gen-encoder-crossdecode.py`,
  which run the OpenJPEG tools and write the two snapshots.
- `tests/encode_roundtrip.rs`, `encode_crossdecode.rs` and
  `encode_parity.rs` over the sweep. `ENCODED` lists the cases that must
  pass, and any other case must report *not yet encoded*. The list only
  grows, like the `IN_CLASS` ratchet of `tests/conformance_ht.rs`.
- The reject matrix, `tests/encode_reject.rs`: one row per refused option
  set or input, with its error variant and message. Each step adds rows,
  and P5.25 checks it is complete.
- A `cargo publish --dry-run` check that no new fixture ships. Everything
  here is gated on the `encode` feature (decision 5).

**Oracle:** OpenJPEG's output only; this step is the harness.

**Gate:** the parity snapshot holds OpenJPEG's figures for every case; every
case reports *not yet encoded*; the PR states the sweep's case count; `cargo
test` is green with and without the `encode` feature.

### P5.0b — Open the decoder seams

**Goal:** the seams, meaning the decoder items the encoder shares, are
reachable and free of decode-only assumptions, in one behavior-preserving PR.

**Work:**
- `src/dwt.rs`: make the `lift` and `lift_split` sweeps `pub(crate)`, with
  the lifting constants.
- `src/tier1/ebcot/context.rs` (new): move the packed per-sample flag word,
  the zero-coding and sign tables, and the context lookups out of
  `passes.rs`, so the pass encoder reads the same tables.
- `src/tier1/ebcot/passes.rs`: lift the raw-or-MQ and terminate rule out of
  `decode_block` into a `pub(crate)` function of the style, the first coded
  plane and the pass, which takes no reader.
- `src/tier2/mod.rs`: re-export `band_rects` and `check_poc_coverage`
  (`geometry` and `progression` are private modules), and make
  `next_segment` `pub(crate)`.
- `src/tier1/ebcot/mq.rs`: move the H.2 vectors out of the private test
  module into a `#[cfg(test)] pub(crate)` item.
- `src/error.rs`: `Inconsistent` reads "internal bug (please report it)", not
  "decoder bug", since encoder paths can reach it. Its rustdoc ("A decoder
  invariant was violated") needs the same widening.
- `src/tier2/mod.rs`: a `Meter::unlimited()` constructor behind the `encode`
  feature, for walks over the encoder's own geometry.

**Oracle:** the whole decoder suite, unchanged, plus the decode benchmark on
the HT and Part 1 rows of performance.md, since `passes.rs` and the context
lookups are the hot path.

**Gate:** every test passes with and without the `encode` feature, clippy is
clean in both builds, and no benchmark row moves by more than its noise.

### P5.1 — Forward DWT (`src/encode/dwt.rs`)

**Goal:** the 5/3 and 9/7 analysis transforms over a tile-component, at any
origin parity, with 0 to 32 levels, writing the `Bands<T>` layout the
inverse reads (generic already, so no new coefficient type is needed).

**Work:** analysis on the shared `lift` and `lift_split` sweeps. The 9/7
convention is the exact inverse of this decoder's: the low-pass scaled by
`1/K` and the high-pass by `K/2` at every filtering, and no subband gain in
the step (P5.3). OpenJPEG's encoder scales the high-pass by `K`
(`dwt.c:1076`) and puts the nominal gain in the step (`tcd.c:1005`), while
its `QCD` carries `1 / norm(level, band)` with a gain of 0 (`dwt.c:1966`).
Both decoders scale the inverse high-pass by `2/K`. The two conventions
differ only by powers of two, exact in floating point, so on ordinary
tile-components they should give the same indices (estimated; this step
measures it). They differ on the #126 shapes: tile-components with empty
coarse resolutions, where a coefficient is high-pass a different number of
times than its nominal gain says. There the per-filter convention is the
one the decoders invert.

**Oracle:**
- Property, through the decoder's own inverse: `inverse(forward(x)) == x`
  exactly for 5/3, and for 9/7 every sample within `1e-3 · 2^B` of its
  input, with `B` the depth. Sizes 1 to 300 per side, every origin parity,
  0 to 8 levels.
- Property: the coefficients equal a reference forward in OpenJPEG's
  convention (`K`, then divided by `2^gain`), bit for bit on ordinary shapes.
  On the #126 shapes the round trip holds, and how OpenJPEG's `-I` differs
  is recorded in correctness.md.
- External: on `opj_compress -n k` lossless 5/3 files, the coefficients our
  tier-1 decoder reads equal our forward 5/3 of the same image, bit for bit,
  for `k` from 1 to 6.

**Gate:** all pass, and a mutation of each lifting step fails a test.

### P5.2 — Forward MCT and DC level shift (`src/encode/mct.rs`)

**Goal:** the forward RCT and ICT on components 0 to 2, and the level
shift. Neither shares code with the inverse.

**Work:** the transforms and the shift. The RCT widens the two chroma
components by a bit, so their `εb` is one higher (decision 10), written in
a QCC for components 1 and 2. OpenJPEG adds no such bit
(`dwt.c:1977-1986`).

**Oracle:** property: the inverse RCT of the forward RCT is the identity
over the full range of each depth from 1 to 26 bits. External, at 1, 8 and
16 bits, since `opj_compress` reads multi-component input only up to 16: at
`-n 1`, our tier-1 reads back the coefficients of `opj_compress -mct 1`.
They equal our forward RCT bit for bit, and our ICT is within 1 of
`-I -mct 1 -n 1`'s. Above 16 bits, the property and the data check are the
only oracles.

**Gate:** the external comparison passes at 1, 8 and 16 bits, and the
property at 1 to 26. 27 bits with the RCT is `InvalidOptions`. A min/max
checkerboard at 26 bits passes the data check (decision 10) in every block.

### P5.3 — Forward quantization (`src/encode/quant.rs`)

**Goal:** step sizes and quantization indices for 9/7, and the depth rule
of decision 10 for both transforms.

**Work:**
- **The step** is the one the decoder reads (`half_step_of` in
  `src/quant.rs`): `Δb = 2^(B − εb) · (1 + μb / 2^11)`, with `B` the
  component depth in bits and no subband gain. `εb` and `μb` are chosen as
  OpenJPEG does, `floor(8192 / norm_b)` encoded against the depth, so the
  `QCD` matches. The index `sign(c) · floor(|c| / Δb)` on P5.1's `K/2`
  forward equals OpenJPEG's on its `K` forward. The gain and synthesis-norm
  tables are new; P5.9 reads the norms for its distortion.
- **The depth rule (decision 10).** The symbols are T.800's: `G` is the
  guard bits, `εb` band b's exponent, and `Mb` its count of magnitude
  bit-planes, with `Mb = G + εb − 1` (Equation E-2). For reversible coding,
  Equation E-10 gives `εb = B + ⌈log2 gain_b⌉`, with gain 1, 2, 2 and 4 for
  LL, HL, LH and HH. This encoder adds 1 on the RCT's two chroma
  components. For 9/7, `εb` comes from the chosen step. If `Mb + s > 30` in
  any band, tile-component or region, with `s` the maxshift of P5.17, the
  options are `InvalidOptions`, before coding.
- **Guard bits.** Reversible coding requires `G ≥ 2`. The 5/3 worst-case
  growth is about 8.2 in HH (the L1 norm of its analysis filters, which
  saturates by 7 levels). A coefficient then stays under `2^(B+3)`, HH's
  `Mb` at `G = 2`, at any level count. Folding the growth into `εb`, as
  OpenJPH does, would allow `G = 1`, but `εb` would then match neither E-10
  nor OpenJPEG and the `QCD` comparison below would be lost. `G = 2` is
  OpenJPEG's default.
- **The data check.** While coding, each block's top bit must be at or
  below its band's `Mb`, so the zero bit-plane count is never negative. A
  block over it is `Inconsistent`, never a clamp. The block coder finds the
  largest magnitude anyway, so the check is free. Nothing may wrap before
  it: 5/3 coefficients stay in `i32` with no fixed-point left shift
  (OpenJPEG's `coef << 6` wraps from 25 bits), and distortion is computed in
  `i64` or `f64`.
- **What it allows.** Lossless 5/3 at `G = 2`: `B ≤ 27`, or `B ≤ 26` with
  the RCT. 31-bit unsigned and 32-bit signed inputs are always refused.

**Oracle:**
- The `QCD` (`SPqcd` exponent and mantissa of every band) equals
  `opj_compress -I`'s byte for byte, at depths 1 to 16 and 0 to 10 levels,
  read with `opj_dump`. With the RCT, the chroma QCC's `εb` is one higher
  than OpenJPEG's, by decision 10. P5.5's block-byte comparison also depends
  on the indices matching (P5.1).
- Property: the decoder's dequantization of our indices is within half a
  step of the input coefficient.
- Depth rule: every refused option set has a reject-matrix row, and 28
  bits is refused. On uniform noise and on a min/max checkerboard, 27-bit
  lossless 5/3 at 1 to 6 levels passes the data check. It decodes exactly
  in both decoders. The checkerboard drives HH to its worst case, and
  takes the `−2^(B−1)` extreme at 0 levels. Above 24 bits the source is the
  only reference.
- Signed input at 17 to 27 bits gets the same checks. `opj_compress`
  cannot be given it, since its PGX reader replaces the declared precision.

**Gate:** every check above passes, the signed cases included. A data-check
failure on any case fails the gate.

### P5.4 — MQ encoder (`src/encode/mq.rs`)

**Goal:** the Annex C encoder, with the standard flush and the predictable
termination and raw (bypass) variants.

**Oracle:** the ITU-T T.88 H.2 test sequence: encoding its 256 decisions
gives exactly the bytes the decoder's test already decodes (the shared
vectors from P5.0b). Property: our `MqDecoder` decodes any random sequence
of contexts and decisions we encode, in each termination mode.

**Gate:** both pass, and an `mq_encode` fuzz target round-trips for 60 s.

### P5.5 — EBCOT pass encoding (`src/encode/passes.rs`)

**Goal:** a code-block's significance, refinement and cleanup passes, with
each pass's byte count and distortion decrease, at code-block style 0.

**Oracle:**
- Property: our tier-1 decoder decodes every block we encode back to the
  quantized coefficients at every truncation point. Blocks are random, from
  4×4 to 64×64 and in the non-square shapes 1024×4, 4×1024 and 128×32, in
  every sub-band kind.
- External: on lossless `opj_compress` files, the block bytes we write for
  the same coefficients equal OpenJPEG's. That is a hypothesis this step
  measures, since both follow the FLUSH procedure of Annex C. If they
  differ, the step records where in correctness.md, and the gate keeps only
  the decoder round trip.

**Gate:** the round trip passes, and the OpenJPEG comparison is recorded in
correctness.md.

### P5.6 — Tag trees and packet headers (`src/encode/tagtree.rs`, `packet.rs`)

**Goal:** tag-tree encoding and packet headers for one precinct: inclusion,
zero bit-planes, pass counts, `Lblock` and lengths. Where a block's passes
form several codeword segments, the split comes from the decoder's
`next_segment` (P5.0b), so writer and reader cannot disagree on it.

**Oracle:** our tier-2 parser reads every header we write back to the same
fields (property); the B.10 examples written byte for byte; the headers of
single-layer lossless `opj_compress` files written again from their decoded
fields.

**Gate:** all three pass.

### P5.7 — First file: lossless, one tile (`src/encode/`, `src/lib.rs`)

**Goal:** `encode` and `encode_with` exist and write a lossless codestream at
OpenJPEG's defaults: one tile, LRCP, one layer, all passes.

**Work:**
- SIZ, COD, QCD, SOT, SOD and EOC in `markers.rs`; `EncodeOptions` with
  levels, block size, transform and MCT, recording for every option whether
  the caller set it (for decision 9's presets).
- Validation of input and options, the levels clamp, and the tile caps of
  decision 11. All of it runs before any shared decoder helper. The helpers
  (`resolution_geoms`, `for_each_packet`) return decode errors and need a
  `Meter`, so the encoder passes `Meter::unlimited()`, and a helper error
  during an encode is reported as `Inconsistent`, never passed through.
- `benches/encode.rs`, an `encode` mode in `tools/perf/run.py`, and the
  `opj_compress` speed table (above) in performance.md.
- `src/pipeline.rs`'s module doc stops calling itself "the decode
  pipeline", and `pipeline::report` may be renamed `pipeline::partial`.

**Oracle:** the source image (lossless); `opj_decompress`; a byte comparison
with `opj_compress` at the same settings with its COM marker removed. The
bytes are expected identical if P5.5 matched. Otherwise each difference is
recorded in correctness.md with its cause.

**Gate:** every reversible round-trip case at these settings is in `ENCODED`
and bit-exact through both decoders, and the first performance row is
recorded against `opj_compress`.

### P5.8 — Irreversible end to end with fixed quantization (`src/encode/mod.rs`)

**Goal:** the equivalent of `opj_compress -I`: 9/7, ICT and expounded steps,
with all passes.

**Work:** the 9/7 branch of the pipeline. It commits three sets of figures
in correctness.md, each with the cases measured:
- a PAE ceiling for every depth from 1 to 27 bits at all passes: the
  largest PAE against the source over the sweep, which P5.25 uses as its
  fuzz bound. At 25 to 27 bits the ceiling is at most part 1's ceiling set
  in advance;
- the two decoders' difference on our output at 17 to 24 bits;
- above 24 bits, the PAE of our decode against the source.

The last two are regression baselines, not correctness checks: a later
change that goes past one must say why. scope.md records how far an `f32`
9/7 path stays accurate above 24 bits; the depth rule still decides what is
refused.

**Oracle:** the source, graded by gate part 1. Up to 24 bits: PAE at most
OpenJPEG's plus 1, MSE at most OpenJPEG's, and the two decoders within part
1's 9/7 bound where it has one. Above 24 bits, where OpenJPEG's `-I` is
wrong, the check is the source round trip through both decoders against the
ceiling set in advance: the bound for the same pattern and seed generated
at 24 bits, times `2^(B−24)`.

**Gate:** every fixed-quantization case passes, and the three sets of
figures are committed.

### P5.9 — Rate control and layers (`src/encode/rate.rs`)

**Goal:** PCRD-opt to per-layer bpp targets (decision 3).

**Work:** each block's convex hull; a slope bisection per tile against the
byte target, headers included; layers from the steepest slope down; the last
lossless layer. Each layer's size is measured by a dry run of the packet
writer.

**Oracle:** gate part 3, the parity snapshot.

**Gate:** every parity case meets both inequalities, and a mutant that drops
the hull or offsets the slope comparison fails a case.

### P5.10 — Tiles, offsets and sub-sampling (`src/encode/tile.rs`)

**Goal:** several tiles, the image and tile offsets, and component
sub-sampling.

**Work:** the tile partition on the decoder's geometry re-exports; the
levels clamp per tile-component; tile size and tile offset as options; the
image origin from `Image` (#254); `XRsiz` and `YRsiz` from each component's
sampling.

**Oracle:** the source round trip; `opj_decompress`; for tiles,
`opj_compress -t` at the same settings, compared byte for byte as in P5.7.

**Gate:** the tiled, offset, 4:2:0 and 4:2:2 cases of the sweep are in
`ENCODED`.

### P5.11 — Precincts and the five progression orders (`src/encode/packet.rs`)

**Goal:** precinct partitions per resolution, and packets in each of the
five orders.

**Work:** precinct sizes as an option (powers of two per resolution, with
the maximal 2^15 as the default); the packet walk in each order through
`for_each_packet`; the order as an option.

**Oracle:** the source round trip; `opj_decompress`; the packet order
against `opj_compress -p` files of the same settings, byte for byte where
P5.7 matched. With one precinct, one layer and one component, the orders
cannot be told apart (correctness.md), so the sweep uses several of each.

**Gate:** each order gives a different stream on the multi-precinct cases,
and all of them round-trip.

### P5.12 — Code-block styles (`src/encode/passes.rs`, `mq.rs`)

**Goal:** the six code-block style bits, alone and together.

**Work:** bypass, reset, restart, vertically causal, predictable termination
and segmentation symbols in the pass and MQ encoders. Raw passes and
terminations follow the P5.0b helper, and segments split by
`next_segment`. The style is an option.

**Oracle:** the source round trip; `opj_decompress`; the block bytes of
`opj_compress -M m` for each style bit, as in P5.5. Each bit is also tested
alone, because `restart` masks `segsym` (correctness.md), as the
`cblksty_*_lossless` fixtures do.

**Gate:** each style alone and all 64 combinations round-trip through both
decoders.

### P5.13 — Markers and tile-parts (`src/encode/markers.rs`, `codestream.rs`)

**Goal:** SOP, EPH, PLT, TLM, COM, CRG, and tile-parts split by R, L or C.

**Work:** each marker as an option. The tile-part split avoids the two
faults in
[departures](openjpeg-departures.md#streams-opj_compress-writes-that-openjpeg-then-decodes-wrong):
`-TP` with mixed sub-sampling, and more than 255 tile-parts.

**Oracle:** `opj_dump` marker lists; our decoder's PLT and TLM readers;
`opj_decompress` decodes each file. CRG has no decoder oracle (both decoders
read and ignore it), so its bytes are checked against the A.9 layout by a
unit test.

**Gate:** each marker and each split round-trips; the PLT and TLM lengths
equal the packet and tile-part lengths; the CRG unit test passes; a COM
option writes its bytes, and `opj_dump` lists it.

### P5.14 — JP2 writer (`src/encode/jp2.rs`)

**Goal:** `Format::Jp2` with `ihdr`, `bpcc`, `colr` (enumerated, or ICC bytes
from the caller), `res`, `cdef`, and the passthrough boxes.

**Oracle:** `scripts/gen-jp2-box-snapshots.py`, the independent box walker;
`opj_dump`; jpylyzer's validity check (decision 6); our `read_header`
returns what the options asked for.

**Gate:** the sweep's JP2 axes are each `colr` kind (sRGB, grayscale, sYCC,
ICC) with and without `cdef`, `res` and each passthrough box. Every case
round-trips through `decode` to the same `Jp2Metadata`, and
`opj_decompress` decodes each file.

### P5.15 — COC and QCC, per component and per tile (`src/encode/options.rs`, `markers.rs`, `tile.rs`)

**Goal:** per-component coding (levels, block size, styles, precincts) and
quantization, in the main header and in a tile's first tile-part header.

**Work:** a per-component and per-tile override in the options. The rules
the decoder enforces are refused with `InvalidOptions` up front: a tile
override that changes a component's wavelet, and RCT or ICT over components
0 to 2 with different wavelets. The writer emits a COC or QCC only where a
value differs from what is in force, as OpenJPEG does.

**Oracle:** the source round trip; `opj_decompress`. On mixed-depth inputs,
the QCC OpenJPEG writes for the same depths is compared field by field, if
`opj_compress` can be made to write one. Its command line sets no
per-component values, so this step measures whether it can.

**Gate:** every override shape in the sweep round-trips through both
decoders, and each refused shape has a reject-matrix row.

### P5.16 — POC (`src/encode/poc.rs`, `packet.rs`, `markers.rs`)

**Goal:** a progression made of volumes, in the main header or in a tile
header.

**Work:** POC volumes as an option. A sequence that leaves a packet out or
sends one twice is `InvalidOptions`: the two decoders read such sequences
differently
([departures](openjpeg-departures.md#packets-a-poc-sequence-leaves-out)),
and `opj_compress` writes overlaps wrongly (measured above). A tile-header
POC is never combined with a main-header one for the same tile, since the
decoders disagree on which wins
([departures](openjpeg-departures.md#a-tile-part-poc-overrides-the-main-headers)).

**Oracle:**
- the source round trip and `opj_decompress`;
- the packet order against `opj_compress -POC` with the same disjoint
  volumes, byte for byte where P5.7 matched;
- the order itself, checked from outside: cut the codestream after the
  first volume and decode the prefix with `opj_decompress -allow-partial`;
  the resolutions, components and layers that volume covers decode, and the
  others are zero.

**Gate:** every POC case in the sweep is lossless through both decoders, and
each prefix decodes only the volumes before the cut.

### P5.17 — Region of interest, maxshift (`src/encode/roi.rs`, `passes.rs`, `markers.rs`)

**Goal:** a caller-given region per component (a rectangle, or a mask on the
component grid) coded by maxshift, with `RGN` in the main or a tile header.

**Work:**
- Map the region to the wavelet coefficients it affects (Annex H). Shift
  the region's coefficients up `s` planes before Tier-1, and carry the
  shifted planes through the bypass count (D.6, counted from the first
  coded plane).
- The spare plane: `s` is one plane above the largest magnitude the
  background can hold. H.1 needs only `2^s` above it, but OpenJPEG takes a
  coefficient as region one plane lower
  ([departures](openjpeg-departures.md#region-of-interest-threshold-part-1-and-ht)).
  The spare plane makes both decoders sort every coefficient alike.
- Under decision 10, `s` is the largest worst-case `Mb` of the component's
  bands plus the spare plane. `Mb + s ≤ 30` then reads `2·Mb + 1 ≤ 30` in
  HH, where `Mb = B + 3`, so maxshift fits up to 11 bits lossless, or 10
  with the RCT (computed). Options past it are `InvalidOptions`, naming the
  region. The spare plane costs one bit of depth, and this step measures
  its cost in bytes. If that is too much, the H.1 minimum with ROI graded
  by this decoder alone would need its own decision.

**Oracle:**
- lossless ROI encodes decode to the source in both decoders, for any
  region shape: the source is the reference;
- with bypass, OpenJPEG decodes its own maxshift streams wrongly
  ([departures](openjpeg-departures.md#bypass-under-a-maxshift)), so those
  cases are graded against the source through this decoder alone, as the
  `roi_bypass` fixtures are. Grok's region-of-interest bug
  (release-validation.md) rules it out too;
- lossy: at the same rate, the region's PSNR is higher than in the same
  encode without ROI, and the background's is lower, in both decoders;
- `opj_compress -ROI c=,U=` is the oracle for the whole-component case only:
  the `RGN` bytes match.

**Gate:** every lossless ROI case is exact through both decoders (with
bypass, through this one); every lossy case meets the PSNR inequalities; the
byte cost of the spare plane is recorded in correctness.md.

### P5.18 — PPM and PPT (`src/encode/packed.rs`, `codestream.rs`)

**Goal:** packet headers moved into the main header (PPM) or into tile-part
headers (PPT), split across marker segments by their length limit.

**Oracle:** the source round trip; `opj_decompress`, which reads both
though OpenJPEG writes neither; and an independent script,
`scripts/check-packed-headers.py`: putting each packet header back in front
of its body gives the same bytes as the same encode without packing.

**Gate:** PPM and PPT cases, with one or several tiles and tile-parts, are
lossless through both decoders and pass the structural check; asking for both
is `InvalidOptions`.

### P5.19 — Palette encode, `pclr` and `cmap` (`src/encode/jp2.rs`, `options.rs`)

**Goal:** a JP2 file whose codestream holds palette indexes, with the palette
in `pclr` and the mapping in `cmap`.

**Work:** the options take a `Palette` built by the new `Palette::new` and
say which component holds the indexes. An index outside the palette is
`InvalidImage`. `Palette::new` accepts column depths the decoder applies:
those that fit `i32`, signed up to 32 bits and unsigned up to 31.

**Oracle:** `opj_decompress` applies the palette (OpenJPEG writes none but
reads them): its output equals the input with the palette applied;
`decode` gives the same, and `with_raw_channels(true)` gives the indexes
back; jpylyzer validates the boxes.

**Gate:** palettes of 8-bit, 12-bit, signed and mixed columns, and with
`cdef`, round-trip through both decoders and pass jpylyzer. An index outside
the palette is `InvalidImage`, with a reject-matrix row.

### P5.20 — Profile presets, Profile 0 and 1, and the rule checker (`src/encode/profile/`)

**Goal:** `with_profile` and the preset mechanism of decision 9, the `Rsiz`
value with its level bits, the two general profiles, and a checker that
every later profile step is graded by.

**Work:**
- `Profile`, `with_profile`, and the resolution of each option: unset takes
  the preset's value, set is checked against the profile's rules.
- Presets for Profile 0 and 1 (Table A.45).
- `profile/rules.rs`: every restriction of Tables A.45, A.46 and A.48 to
  A.54 as one row of data (the profile, the field, the limit, the table and
  row). Transcribed from the spec text alone, before any cinema, broadcast or
  IMF preset exists. The 2024 edition's tables are checked for changes
  first (see open questions).
- `scripts/gen-encoder-profiles.py`, which runs the `opj_compress` and
  `grk_compress` profile commands and writes `profiles.json`; P5.21, P5.23
  and P5.24 add their cases.
- `profile/check.rs`, compiled only for tests, evaluates those rows on a
  codestream as this crate's parser reads it. It covers `Rsiz`, sizes and
  origins, components, every COD to QCC field and where it sits, POC, the
  optional markers, the tile-parts, and the byte totals of Equation A-8.
- Later presets read the same rows, so preset and checker cannot drift
  apart. The checker uses only the rows and the parsed stream, never preset
  code.

**Oracle:** each row gets a passing stream and a hand-edited one that
breaks only that row, which the checker must reject. The checker must
accept the `opj_compress` and `grk_compress` streams measured above, and
flag exactly the rows they were measured to break. No other encoder writes
Profile 0 or 1, so their oracle is the checker, both decoders and the
source round trip.

**Gate:** every row has its passing and failing test. The Profile 0 and 1
cases of the sweep pass the checker and both decoders. Each conflict between
a set option and the profile has a reject-matrix row. The PR records the
2024-edition check and whether an independent header script is added (see
open questions).

### P5.21 — 2K and 4K digital cinema (`src/encode/profile/cinema.rs`)

**Goal:** `Profile::Cinema2k { fps }` and `Profile::Cinema4k { fps }`, `Rsiz`
3 and 4 (Table A.46).

**Work:** the presets, from Table A.46:
- one tile at the origin, 3 components of 12 bits, 9/7 (5/3 allowed), at
  most 5 levels (2K) or 1 to 6 (4K);
- 32×32 blocks of style 0, precincts of 128 at the lowest resolution and
  256 above, CPRL, one layer, TLM, COD to QCC in the main header only;
- 3 tile-parts for 2K, one per component, or 6 for 4K with the
  two-progression POC;
- the byte limits per frame and per component from Equation A-8, at up to
  60 fps (2K) or 30 (4K).

**Oracle:** on the same frames, the main header (SIZ, COD, QCD, POC, TLM
layout), the tile-part count and the component order equal what
`opj_compress -cinema2K 24`, `-cinema2K 48` and `-cinema4K` write, apart
from COM. The coded bytes are not compared, since the rate allocators
differ (decision 3). Also the rule checker with its byte limits, both
decoders, and gate parts 1 and 3 at the profile's rate.

**Gate:** 2K at 24 and 48 fps and 4K at 24 fps pass at these sizes: 2048×1080,
1998×1080 and 2048×858 for 2K, and 4096×2160, 3996×2160 and 4096×1716 for
4K. Each matches OpenJPEG's fields, passes the checker, decodes in both
decoders, and stays within the byte limits. Each conflicting option is
refused, for example `-cinema2K 24 -n 7 -b 64,64 -p LRCP`, which OpenJPEG
rewrites.

### P5.22 — Scalable cinema and long-term storage (`src/encode/profile/cinema.rs`)

**Goal:** `Rsiz` 5, 6 and 7 (Table A.46).

**Work:**
- Scalable 2K and 4K: two layers; the two- or four-progression POC; 6 or 12
  tile-parts in the order of Figures A.25 to A.29; PLT in every tile-part; no
  SOP or EPH; 9/7 only. Layer 0 must also meet the 2K or 4K cinema byte
  limits.
- Long-term storage: up to 16384×8640 and 8 components, one tile or tiles of
  at least 1024×512, 5/3 or 9/7. At most 5 layers, SOP and EPH on every
  packet, PLT, and one tile-part per component.

**Oracle:** no other encoder writes these profiles. The oracle is the rule
checker, both decoders, the source round trip for 5/3 storage, and a check
of the purpose Table A.46 states: a distribution stream "can be
obtained by simply stripping some tile-parts". Removing the second layer's
(layer 1's) tile-parts, and fixing TLM and `Rsiz` to 3 or 4, gives a stream
that passes the checker as 2K or 4K cinema and decodes in both decoders.

**Gate:** scalable cinema at 2048×1080, 1998×1080, 4096×2160 and 3996×2160,
and storage at 4096×2160 as one tile and as 1024×512 tiles, pass the
checker and both decoders. Every scalable stream passes the stripping check.
The full storage size is not run: 16384×8640 with 8 components is about
4.5 GB of `i32` coefficients (estimated).

### P5.23 — Broadcast contribution (`src/encode/profile/broadcast.rs`)

**Goal:** the single-tile, multi-tile and multi-tile reversible profiles with
their main levels (Tables A.48 to A.50).

**Work:** the presets:
- 1 tile, or 1 or 4 tiles of one size; up to 4 components of 8 to 12 bits,
  4:4:4 or 4:2:2; 9/7, or 5/3 for reversible;
- 1 to 5 levels, one layer, style 0, blocks 32 to 128 wide and 32 or 64
  high in every component, precincts of 128 then 256;
- CPRL with no POC, one tile-part per tile-component, and TLM;
- the main level's limits on sample rate and bit rate, so a level above 0
  needs the frame rate. A level outside the table is refused; reversible
  allows only 6 and 7.

**Oracle:** the rule checker; both decoders; the source round trip for the
reversible profile. `grk_compress -U SINGLE`, `MULTI` and `MULTI_R` on the
same frames give the choices Table A.48 leaves open (levels, block size),
and any difference is explained. Grok is not a conformance oracle
here (measured above: no TLM, and `Rsiz` `0x0300`).

**Gate:** each profile at levels 0, 1 and the top of its table passes the
checker and both decoders; a frame rate that breaks the level, and each
conflicting option, are refused.

### P5.24 — IMF (`src/encode/profile/imf.rs`)

**Goal:** the six IMF profiles with main level and sublevel (Tables A.51 to
A.54).

**Work:** the presets:
- up to 2048×1556, 4096×3112 or 8192×6224, with up to 3 components of 8 to
  16 bits, 4:4:4 or 4:2:2;
- 32×32 blocks of style 0, precincts of 128 then 256, CPRL with no POC, one
  layer, one tile-part per tile-component, and TLM;
- lossy: one tile, 9/7, 1 to 5, 6 or 7 levels; reversible: 5/3, one tile
  or tiles of 1024, 2048 or 4096, with the level limit set by the width;
- a sublevel above what its main level allows (Table A.53) is refused, and
  the sublevel's byte limit holds.

**Oracle:** `opj_compress -IMF 2K|4K|8K|2K_R|4K_R|8K_R`, with and without
`mainlevel`, `sublevel` and `framerate`: the main header and tile-part
layout, field for field apart from COM. The rule checker alone grades the
sublevel byte limit, which OpenJPEG prints but does not hold. Both decoders,
and the source round trip for the reversible profiles.

**Gate:** the 2K and 4K profiles pass at 2048×1556 and 1920×1080 (2K) and
4096×3112 and 3840×2160 (4K), single- and multi-tile where allowed. Each
matches OpenJPEG's fields and passes the checker. It decodes in both
decoders and stays within the sublevel's limit. The 8K profiles run in the
release round only, at 7680×4320 and 8192×6224, for their memory: about 0.4
and 0.6 GB of `i32` coefficients for 3 components (estimated). Each
conflicting option is refused; OpenJPEG falls back to `Rsiz` 0 for `-n 7`
and for `-ROI`.

### P5.25 — Fuzz targets and the encoder reject matrix (`fuzz/`)

**Work:**
- `mq_encode` (from P5.4).
- `encode_roundtrip`: arbitrary small images (at most 64×64, 1 to 4
  components, any depth, sign and sampling) with arbitrary `EncodeOptions`
  give `Ok` or a typed error, never a panic. On `Ok` our decoder decodes the
  output. A reversible encode must be exact. An irreversible encode with all
  passes must be within P5.8's PAE ceiling for its depth; at a rate, it
  only has to decode.
- Each later step adds its options to the target.

**Gate:** a session of 60 s per target finds nothing. Every
`InvalidOptions` and `InvalidImage` construction in `src/encode/` has a
reject-matrix row, checked by listing them with `git grep` against the rows.

### P5.26 — Integration and the phase gate

**Work:**
- the scope.md encoding rows; roadmap.md; an encoder section in
  correctness.md;
- openjpeg-departures.md: one entry for each departure named in [Which
  defaults to mirror](#which-defaults-to-mirror), decisions 2, 9 and 10,
  P5.1, P5.3, P5.16 and P5.17, plus any difference found;
- `docs/encoder-ics.md` (Annex F); an encoder table in performance.md;
- resource-bounds.md: the encoder's allocations are bounded by the input,
  which the caller already holds;
- README, `lib.rs` docs and CHANGELOG; the RELEASING.md pre-release review.

**Gate (the Phase 5 exit):** all five gate parts pass over the full sweep;
every earlier fixture is unchanged; fuzzing is clean; the quality gates are
green with and without the `encode` feature.

## Quality plan

- **Fuzzing.** The two targets above. libFuzzer cannot run OpenJPEG in
  process, so "both decoders accept it" is a release round. It replays the
  `encode_roundtrip` corpus, writes each output, and runs `opj_decompress`
  on it under a memory cap. Any refusal is a finding.
- **Determinism.** The same image and options give the same bytes on every
  run and every target. The 9/7 path uses `f32` and `f64` without fused
  multiply-add, which Rust does not insert on its own, so x86-64, aarch64 and
  wasm32 must agree. The committed digests enforce it: CI already runs
  `cargo test` on macOS aarch64, and the 32-bit release round adds wasm32.
  Threads (#301), when they come, must not change a byte.
- **Performance.** `benches/encode.rs` (std only) and the `encode` mode of
  `tools/perf/run.py`, from P5.7, timed against `opj_compress` on one thread
  and on all cores, on the three inputs measured above, with peak memory.
  *Estimated* target for 0.7.0: within 2× of `opj_compress` on one thread,
  the same line performance.md draws for the decoder. Each later step reports
  its change on the same table.
- **Mutation checks.** `cargo mutants --in-diff` on each step's PR over
  `src/encode/`, and over the whole phase before the tag. A survivor in rate
  control, termination, the ROI shift, the POC checks or the profile limits
  is a gap in the parity, byte, prefix or checker tests.
- **New rounds for `docs/release-validation.md`:**
  - *Encoder cross-decode sweep:* the full sweep through `opj_decompress` and
    Grok, within gate part 1, leaving out bypass with ROI for OpenJPEG and
    ROI for Grok;
  - *Rate parity* and *profile fields:* rerun `gen-encoder-parity.py` and
    `gen-encoder-profiles.py` and compare with the committed snapshots;
  - *Encoder fuzz replay through OpenJPEG* (above); jpylyzer over every JP2
    output; the 8K IMF cases of P5.24;
  - *Kakadu, by hand and optional:* `kdu_expand` over the sweep, with results
    in the round's notes only;
  - *Downstream consumer:* a GRIB2 template 5.40 message carrying our
    codestream in section 7, read by eccodes and fieldglass. eccodes decodes
    with OpenJPEG, so this checks the packing, not a third decoder.

## Prerequisites and open questions

- **#254's origin grid** (#254 lands before P5.7): the
  origin is on the grid of `width` and `height`, `ceil(XOsiz / 2^r)` under
  a reduction and the window's corner under `region`.
- **Whether our tier-1 bytes can match OpenJPEG's** is unmeasured (P5.5). It
  decides whether P5.7, P5.10 to P5.12 and P5.16 get byte comparisons or
  only round trips; for 9/7 it also needs P5.1's indices to match.
- **Which edition the profile rules come from.** The rows come from T.800
  (06/2019), which is free. The current edition, T.800 (07/2024), was not
  free to download. P5.20 checks its tables for changes, or records that it
  could not.
- **The rule checker reads this crate's parser,** so a parser bug could hide
  a field from both. The comparisons in P5.21, P5.23 and P5.24 cover what
  OpenJPEG and Grok write. For P5.22 and Profile 0 and 1, P5.20 decides
  whether to add an independent header script like P5.14's box walker.
- **The cinema per-component and layer-0 byte limits** are read as Table
  A.47 lays them out. Whether `opj_compress` holds the per-component limit
  was not measured: its tile-parts were all under it.
- **A CI job with OpenJPEG installed**, to run the cross-decode live, would
  be a maintainer CI edit. The plan does not need it.

## Sources

- ITU-T T.800 | ISO/IEC 15444-1: Annexes A to D, F and H, and the informative
  Annex J on rate control. The profile rules are from T.800 (06/2019), free
  at `https://www.itu.int/rec/T-REC-T.800-201906-S`: Table A.10, A.5.3 (PRF),
  and A.10 (Tables A.45 to A.55, Equation A-8).
- ITU-T T.803 (02/2024) | ISO/IEC 15444-4:2024, free PDF at
  `https://www.itu.int/rec/T-REC-T.803-202402-I`: 6.3, 6.6, Annex D (D.1 to
  D.4, Figure D.1) and Annex F (Table F.1).
- ITU-T T.88 (08/2018) H.2: the MQ test sequence.
- OpenJPEG 2.5.0 source, tag `v2.5.0`, at the lines cited above: `tcd.c`,
  `j2k.c` (with the `opj_j2k_write_*` writers and the cinema and IMF
  parameter and compliance functions), `dwt.c`, `t1.c`, `t2.c`, `jp2.c` and
  `convert.c`; and `opj_compress -h`.
- Grok 20.4.25: `grk_compress -h`, its broadcast messages, and `grok.h:162`
  (`GRK_MAX_SUPPORTED_IMAGE_PRECISION 16`). OpenJPH 0.33.0:
  `ojph_params.cpp:1546-1590` and `:1748-1769` (`εb`, guard bits, 64-bit
  blocks). The `kdu_compress -usage` text. JJ2000
  `StdEntropyCoder.java:1380-1383`. T.800 (06/2019) for depth: Table A.11
  and its footnote, E-2, E-10, the B.10.6 note.
- Grok's `grk_compress` manual page, the `-A` rate-control algorithms:
  `https://manpages.debian.org/testing/grokj2k-tools/grk_compress.1.en.html`.
- D. Taubman, "High performance scalable image compression with EBCOT",
  IEEE Transactions on Image Processing 9(7), 2000: PCRD-opt.
