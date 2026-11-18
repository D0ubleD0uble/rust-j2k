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
| Depth 1 to 38 bits as SIZ allows, as long as every code-block needs at most 30 coded bit-planes, maxshift included | the decoder's cap (`MAX_BIT_PLANES`), which OpenJPEG shares; refused up front from the worst case (the plane rule, P5.3), so in practice about 27 bits for 5/3 with 2 guard bits (estimated; P5.2 and P5.3 measure it) |
| MQ encoder and the three EBCOT passes | each pass's rate and distortion recorded for rate control |
| All six code-block style bits | bypass, reset, restart, vertically causal, predictable termination, segmentation symbols |
| Code-block sizes 4 to 1024 per side, area at most 4096 | OpenJPEG's encoder stops at 64×64 |
| Region of interest, maxshift (`RGN`, `Srgn = 0`) | a caller-given region per component |

### Codestream

| Feature | Notes |
| ------- | ----- |
| SOC, SIZ, COD, QCD, SOT, SOD, EOC | |
| COC and QCC, in the main header and in tile headers | per-component, and per-tile, coding and quantization |
| Tiles, image offset and tile offset | |
| Component sub-sampling | `XRsiz`, `YRsiz` from `Component::x_sampling`, `y_sampling` |
| Precinct partitions, per resolution | |
| All five progression orders, and POC | POC volumes must cover each packet once (see P5.16) |
| Quality layers with a rate per layer, and a last lossless layer | |
| Rate control: PCRD-opt over each tile | decision 3 |
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

Each is refused with `Error::Unsupported` if asked for, never ignored.

| Feature | Why later |
| ------- | --------- |
| A PSNR target per layer (`opj_compress -q`) | decision 2 |
| Writing to `io::Write`, caller-provided buffers | decision 1 |
| The `PRF` marker | Table A.55 defines no profile number for it to carry |
| Packaging outside the codestream: DCP and IMF wrapping (MXF), the XYZ color rules of digital cinema | not part of T.800 |
| The HTJ2K IMF and broadcast profiles | Part 15; with HTJ2K encode in Phase 6 |
| Threads and SIMD | #301, #302 |
| Narrower input sample types | with #303 |
| HTJ2K (FBCOT) encode, and Part 2 (arbitrary MCT, TCQ, custom kernels) | Phase 6 |

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
(`qntsty=2`). The `QCD` carries `1 / norm(level, band)` with the exponent
counted from the component depth alone: `opj_dwt_calc_explicit_stepsizes`
uses a gain of 0 for 9/7 (`dwt.c:1966`), and `opj_dwt_encode_stepsize`
encodes it (`dwt.c:1092`). The encoder then quantizes with the nominal
subband gain added back (`log2_gain` in `tcd.c:1005`, which is 0 only when
decoding), to match its forward transform, which scales the high-pass by `K`
and the low-pass by `1/K` (`dwt.c:1076`). Its decoder and this crate's leave
the gain out of the step and scale the inverse high-pass by `2/K`
(`src/dwt.rs`, `src/quant.rs`). The steps do not depend on the rate: the rate
comes only from truncation.

What the encoder can write, from `j2k.c`: COC, QCC, POC, RGN, PLT, TLM, COM
and the Part 2 MCT markers. There is no writer for PPM, PPT or CRG, and
`jp2.c` writes no `pclr` or `cmap`. COC and QCC are written only when the
components' parameters differ (`opj_j2k_write_all_coc`,
`opj_j2k_write_all_qcc`); `opj_compress` has no option that sets them per
component.

### Rate and quality at given settings

One run each. Size is the whole codestream. PSNR is the `opj_decompress`
decode against the source over all samples, with peak `2^depth − 1`.

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

- `-r` hits its target closely: 58976 bytes of a 58982-byte target (RGB),
  103691 of 103824 (16-bit) and 209730 of 209715 (gray). The target is
  `numcomps · w · h · prec / (8 · ratio · dx · dy)`, with `prec`, `dx` and
  `dy` taken from component 0, less a per-tile header estimate
  (`opj_j2k_update_rates`, `j2k.c:5260`). So the ratio is measured against the
  declared depth, and it is wrong for components of mixed depth or sampling.
- `-I` with no rate is not small at high depth. On the 16-bit grid it is 1.76
  times the lossless size, because the fixed steps keep nearly every bit.
- `-q` is a PSNR target and was met within 0.02 dB. On the 16-bit grid,
  40 dB takes 601 bytes.
- `-r 1`, or any rate of 1 or less, means "all passes" (`j2k.c:8070`).

### How OpenJPEG allocates rate

`opj_tcd_rateallocate` (`tcd.c:443`) works per tile and per layer. It
computes every pass's slope `ΔD/ΔR`, then bisects a slope threshold 128
times. Each trial (`opj_tcd_makelayer`) takes, in every block, the passes
whose slope is at least the threshold, and runs Tier-2 to measure the size.
It does not prune the passes to the convex hull, so a pass on a non-convex
stretch can be taken or left inconsistently. `ΔD` is a weighted-MSE estimate
from the pass coder (`opj_t1_getwmsedec`, with the wavelet and MCT norms),
not a measured distortion. A pass that is not terminated is counted as its
MQ byte count plus 3 (`opj_t1_encode_cblk`). Grok's default (`-A 1`) adds
the convex hull; its `-A 0` is OpenJPEG's method.

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

- OpenJPEG writes the cinema and IMF parameters Table A.46 and A.51 ask for,
  so its headers are a field-level oracle for those presets.
- It replaces conflicting options with a warning (cinema), or drops the
  profile and writes `Rsiz` 0 with exit status 0 (IMF). Neither suits a
  library, which has no warning channel (decision 9).
- Its IMF sublevel is not held: the byte limit is printed and then not
  applied. A sublevel of 1 at 24 fps printed 1041666 bytes and wrote
  3068017.
- Grok's broadcast streams leave out TLM, which Table A.48 requires, and its
  multi-tile reversible `Rsiz` of `0x0300` is not a Table A.10 value (only
  `0x0306` and `0x0307` are). Grok is a field-level oracle for the choices
  Table A.48 leaves open, not a conformance oracle.

### Speed and memory

Median of 3 runs of the whole process (start-up and file I/O included),
measured with `/usr/bin/time` at a load of 6:

| Input | Options | Seconds | Peak MB |
| ----- | ------- | ------- | ------- |
| 2048² gray | (default) | 0.16 | 31 |
| | `-I` | 0.20 | 32 |
| | `-I -r 20` | 0.23 | 31 |
| | `-threads ALL_CPUS` | 0.09 | 33 |
| RGB 768×512 | each option set above | 0.11–0.14 | 13–14 |
| 16-bit 1440×721 | each option set above | 0.17–0.27 | 13–15 |

### Kakadu and Grok

Not measured. According to their documentation, Kakadu's `kdu_compress`
defaults to the irreversible transform, 5 levels, 64×64 blocks, LRCP, one
layer and `Qstep = 1/256`, and Grok's `grk_compress` takes OpenJPEG's
defaults and options.

### Which defaults to mirror

The encoder mirrors OpenJPEG's defaults, so that `encode(&image)` agrees with
`opj_compress -i` and the round trip is lossless by default: 5/3, 5 levels,
64×64 blocks, maximal precincts, LRCP, one layer, RCT for 3 or more
components, 2 guard bits, one tile, `Rsiz` 0 and no optional markers. It
departs in two places, and both are recorded in openjpeg-departures.md:

1. **Levels on small images.** The default is `min(5, the levels the
   smallest tile-component allows)`, where OpenJPEG refuses the image. A
   GRIB2 grid can be 16×16. An explicit level count that does not fit is
   still `InvalidOptions`.
2. **No COM marker by default.** The encoder's name is not image data, and
   leaving it out keeps the output the same across crate versions. A comment
   is one option away.

## The gate

### 1. Round trip

- **Reversible:** every input in the corpus, with every option set in the
  reversible sweep, decodes bit for bit to the input through this decoder and
  through `opj_decompress`. The source image is the reference, so the check
  is external even with this crate's own decoder.
- **Irreversible with fixed quantization (no rate):** PAE and MSE against the
  source are at most OpenJPEG's for the same input and options, plus 1 PAE.
  OpenJPEG measured PAE 4 and MSE 0.49 on the RGB input, and PAE 1 and MSE
  0.09 on the 16-bit grid.
- **Irreversible at a rate:** graded by parity (part 3), not by an absolute
  PSNR, since the right PSNR depends on the image.
- **The two decoders agree on our output:** bit for bit on 5/3, and on 9/7
  within the decoder's documented gap (1 at 8 to 12 bits, 2 at 14, 7 at 16;
  see correctness.md).

### 2. Cross-decode

- `opj_decompress` 2.5.0 decodes every encoded file with no warning or
  error, and its output meets part 1. Grok decodes them as a second opinion
  in a release round (decision 6).
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
  98% of it. OpenJPEG lands within 0.12% (measured, above). The target uses
  this crate's definition of bpp (decision 2), and OpenJPEG is given the ratio
  that means the same number of bytes.
- **Quality:** PSNR, with both files decoded by `opj_decompress`, is at
  least OpenJPEG's minus 0.1 dB in every case, and the mean difference is at
  least 0. *Estimated:* a convex-hull PCRD-opt with measured distortion should
  match or beat OpenJPEG's threshold search, so 0.1 dB leaves room for noise
  without hiding a broken allocator.
- **How it is measured:** `scripts/gen-encoder-parity.py` runs both encoders
  and `opj_decompress`, and commits one JSON row per case: OpenJPEG's size
  and PSNR, and the SHA-256, size and PSNR of this encoder's output. The test
  encodes again, checks the digest (so a changed encoder fails until the
  snapshot is rerun), and checks both inequalities on the committed numbers.
  `cargo test` needs no external tool.

### 4. ISO/IEC 15444-4 encoder conformance

T.803 Annex D is informative. It asks only that the output be a legal
codestream, shown by the reference decoder (T.804 | ISO/IEC 15444-5) "fully
decoding" it, over test images and parameters the implementer chooses (D.4 a
to f). Whether a reconstructed image is acceptable is "up to the
implementer": a lossless encode must be identical, and a lossy one "might"
be held to PAE or MSE bounds (the note under Figure D.1). Annex F is an
implementation compliance statement with a table of marker usage (Table
F.1). Part 4 has no encoder test files and no encoder bounds.

So the Part 4 corpus helps an encoder only as input; D.4 a) says the decoder
references "are acceptable". The plan applies Annex D this way:

- the 78 Part 4 reference images (`tests/fixtures/conformance/references/`)
  are encoder inputs, beside the other corpora;
- the parameter sweep covers each axis of D.4 e) and f): tile size, levels,
  rates; small, large and odd image sizes; component count, depth and
  sampling;
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

The checker proves the output meets the rules as they were transcribed. It
cannot catch a rule transcribed wrongly, since the encoder would follow the
same reading; the field comparisons with two other encoders, and a clause
cited beside every rule for review, are the guard against that. Neither
proves anything outside T.800, such as the DCI or SMPTE rules a cinema
package must also meet.

## Decisions

Settled on 2026-11-17.

1. **The API is `encode` and `encode_with`.** `encode(&Image)` and
   `encode_with(&Image, &EncodeOptions)` mirror `decode` and `decode_with`.
   `EncodeOptions` is a `#[non_exhaustive]` struct with `with_*` setters, like
   `DecodeOptions`. The output is `Result<Vec<u8>>`, a raw codestream by
   default or a JP2 file under `with_format(Format::Jp2)`. Input the encoder
   cannot encode (component sizes that disagree with their sampling, a sample
   outside its declared depth) is a new `Error::InvalidImage`, and bad options
   are `Error::InvalidOptions`. An `Encoder` builder was not chosen, because
   it has no state to keep until threads arrive (#301), and it can wrap the
   options then. `io::Write` output was not chosen either: rate control holds
   a tile's coded data before writing, and TLM and `Psot` need lengths first.
   A borrowed input view comes only with #303, so that input and output take
   the same sample types. `Image::jp2` is ignored on input: the JP2 boxes come
   from the options, because `Jp2Metadata` describes a file that was read,
   not one to write. `Image::report` is ignored too: it describes what a
   decode left out, and the encoder codes the samples it is given. The image
   origin comes from #254, on the same grid as `width` and `height` (reduced
   and windowed), so that the component sizes `ceil(x1/dx) − ceil(x0/dx)` and
   the DWT parity come out right for any decoded `Image`. A palette-expanded
   image can hold samples outside the depth its channels declare
   (`src/image.rs`, `Component`), so encoding one is `InvalidImage`; sRGB
   output clamps to the depth (`src/jp2/srgb.rs`) and encodes.
2. **Rate is given in bits per pixel.** `Rate::Lossless` (all passes) is the
   default. `Rate::Layers` takes cumulative, increasing bits per pixel per
   layer, with a last layer of all passes allowed, and a `Rate::ratio` helper
   converts a compression ratio. bpp is compressed bits over the image area's
   pixels, headers included. The ratio is against the true uncompressed size,
   the sum over components of `w · h · depth / 8`. For equal components that
   is OpenJPEG's ratio; for mixed depth or sampling it departs from OpenJPEG,
   which uses component 0 alone, and openjpeg-departures.md records it. PSNR
   targets are deferred: they need a distortion model in the sample domain,
   and no user has asked for them.
3. **Lossless and fixed quantization first, then true PCRD-opt.** The first
   end-to-end encodes take all passes, so the pipeline is gated before rate
   control exists. Rate control is then PCRD-opt: a convex hull of each
   block's truncation points, one slope bisected per tile to the byte target,
   and layers by decreasing slope. Distortion is measured, not estimated: the
   decrease in squared coefficient error times the square of the band's
   synthesis norm. OpenJPEG's threshold search without the hull was not
   chosen; its results are a floor (gate part 3), not a method to copy.
4. **Everything in Part 1 is in 0.6.0.** That includes POC, ROI (maxshift),
   PPM and PPT, COC and QCC, CRG and palette encode, beside the transforms,
   tiling, precincts, progressions, layers, code-block styles, markers and
   tile-parts. Each has its own step and oracle below. POC and ROI have the
   weakest external oracles, and their steps say what a cross-decode can and
   cannot prove.
5. **The encoder sits behind an `encode` cargo feature, on by default.** A new
   user gets it from `cargo add rust-j2k`. A decode-only user (fieldglass, or
   wasm) sets `default-features = false` and compiles none of it. The
   feature is declared (`encode = []`, in `default`) before P5.0, whose tests
   need it. Every encode test, bench (`required-features`) and `src/fuzz.rs`
   entry is gated on it; `Error::InvalidImage` is not, so the error enum is
   the same in both builds. CI gains `cargo test --no-default-features` and
   `cargo clippy --no-default-features --all-targets -- -D warnings`. Code
   both sides call stays in the decoder's modules; code only the encoder calls
   lives in `src/encode/` or behind the feature, so a decode-only build has no
   dead code. The `Cargo.toml` and CI edits are the maintainer's.
6. **The reference tools are OpenJPEG, Grok and jpylyzer.** OpenJPEG 2.5.0 is
   the gate. Grok (20.4.25, built for performance.md) is a second decoder in a
   release round, and `grk_compress -U` is the field-level oracle for the
   broadcast presets, which OpenJPEG cannot write. jpylyzer checks JP2
   validity when snapshots are
   regenerated. Kakadu may be run by hand as an extra check; its results are
   never committed as oracles and never feed CI. None of the tools is a build
   or test dependency.
7. **No new images are committed.** The inputs are the committed fixtures
   decoded, the 78 Part 4 references and seeded synthetic images. What is
   committed are snapshots: the parity JSON (gate part 3) and, for each
   cross-decode case, the digest of this encoder's output and of
   `opj_decompress`'s decode of it. The Part 4 references keep their
   conformance-only grant and stay excluded from the crate, and the tests
   that use them are excluded with them, as the conformance tests are.
8. **One release, 0.6.0,** when the whole gate passes, as Phases 3 and 4
   shipped. Decoder fixes from the 0.5.x milestone may ship before it as
   0.5.1 patches. With 0.6.0, `Cargo.toml`'s `description` and keywords
   change, and CONTRIBUTING's "The crate only decodes" goes; both are the
   maintainer's edits.
9. **Every Part 1 profile is in 0.6.0, as a preset.** Settled later the same
   day. Profile 0 and 1, the five cinema profiles, the three broadcast
   profiles and the six IMF profiles, with their levels, are selected with
   `EncodeOptions::with_profile(Profile::…)`, for example
   `Profile::Cinema2k { fps: 24 }` or
   `Profile::Imf { size: ImfSize::K4, reversible: false, main_level: 6, sub_level: 3, fps: Some(24) }`.
   `Profile` is `#[non_exhaustive]`. A preset fills in every option the
   caller did not set, from the profile's table. An option the caller did
   set that breaks the profile, or an image the profile cannot hold (its
   size, depth, component count or sampling), is `Error::InvalidOptions`,
   naming the option, the rule and its table. The preset never replaces a
   value the caller set. A library has no warning channel, so a silent
   override hands back a file that is not what was asked for, and OpenJPEG
   shows both ways it goes wrong (measured above): it replaces options under
   `-cinema2K`, and under `-IMF` it drops the profile and writes `Rsiz` 0.
   An override would also make the result depend on the order of the
   `with_*` calls. A caller who wants the preset's value leaves the option
   unset. The frame rate is required for cinema, and for broadcast and IMF
   whenever a level above 0 sets a limit, since the byte limit per frame is
   the bit rate over the frame rate (Equation A-8). A rate above that limit
   is refused up front; a lossless encode that comes out over it is refused
   after coding, with its size. The `PRF` marker is not written, since no
   profile number is defined for it.

Two smaller choices follow from the defaults: the levels clamp on small
images, and no COM marker by default (see [Which defaults to
mirror](#which-defaults-to-mirror)).

## Corpora

| Set | Source and provenance | Size | Committed? |
| --- | --------------------- | ---- | ---------- |
| Decoded fixtures | every lossless fixture in `tests/fixtures/`, decoded by this decoder (already graded against OpenJPEG) | no new bytes | already |
| Part 4 references | `tests/fixtures/conformance/references/*.pgx`, 78 files: the D.4 a) inputs | no new bytes | already; excluded from the crate |
| Synthetic | a seeded generator in `tests/support/synth_image.rs`: noise, ramps, checkerboards, edges, flat fields; sizes from 1×1 to 1024×1024, including odd and prime sizes; 1 to 4 components; depths 1, 8, 12 and 16, and the measured plane ceiling and one bit over it; 31 unsigned and 32 signed only as reject cases, or at small magnitudes with no DWT where the plane rule allows (`tests/signed_32bit.rs` proves depth 32 only for small magnitudes); sampling 1×1, 2×1 and 2×2 | none (generated) | the generator only |
| Profile frames | the same generator at the profiles' sizes: 2048×1080, 4096×2160, 1920×1080 and 8192×4320, 8 to 16 bits, 3 or 4 components, 4:4:4 and 4:2:2 | none (generated) | the generator only |
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

P5.0b opens every decoder seam the encoder needs in one PR, before the
parallel steps start, so that no two encoder steps edit the same decoder
file and each refactor is checked once against the whole decoder suite.

The stage pieces land first, each against its decoder inverse and an
external oracle, before anything is wired together, as Phase 2 built the
decoder stage by stage. After P5.0b, P5.1 to P5.4 and P5.6 touch disjoint
files and can run in parallel. P5.7 is the first file out. It fixes the API shape early, so
later steps add options instead of reshaping it. Rate control (P5.9) follows
a fixed-quantization 9/7 encode (P5.8), so that a rate bug cannot hide behind
a transform bug.

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
- **Profiles:** P5.20 once the headers lane has P5.13 and P5.15 (the
  profiles constrain TLM, PLT, tile-parts and where COD to QCC sit). Then
  P5.21, P5.23 and P5.24 side by side, each in its own file, once the
  features they preset exist: rate control for the byte limits, precincts,
  tiles, and POC for 4K. P5.22 follows P5.21, whose file it extends. A
  preset only chooses values for options that already work, so this lane
  adds no coding path of its own.

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
| P5.6 | `src/encode/tagtree.rs`, `src/encode/packet.rs` | uses `resolution_geoms`, `band_rects`, `for_each_packet` and `next_segment`, re-exported in P5.0b |
| P5.7 | `src/encode/mod.rs`, `options.rs`, `codestream.rs`, `markers.rs`; `src/lib.rs` re-exports | `src/codestream/markers.rs`: shared structs; `src/pipeline.rs`: module doc |
| P5.8 | `src/encode/mod.rs` (9/7 branch) | none |
| P5.9 | `src/encode/rate.rs` | none |
| P5.10 | `src/encode/tile.rs` | uses the geometry re-exports of P5.0b |
| P5.11 | `src/encode/packet.rs` | none |
| P5.12 | `src/encode/passes.rs`, `src/encode/mq.rs` | uses the segment and pass-mode helpers of P5.0b |
| P5.13 | `src/encode/markers.rs`, `codestream.rs` | none |
| P5.14 | `src/encode/jp2.rs`, with its own box-type constants | none: the decoder has no box-type constants and `jp2::boxes` is private |
| P5.15 | `src/encode/options.rs` (per-component and per-tile), `markers.rs`, `tile.rs` | none |
| P5.16 | `src/encode/poc.rs`, `packet.rs`, `markers.rs` | uses `check_poc_coverage`, re-exported in P5.0b |
| P5.17 | `src/encode/roi.rs`, `passes.rs`, `markers.rs` | uses `undo_maxshift` (`pub(crate)`) and `top_coded_plane` (`pub`) as they are |
| P5.18 | `src/encode/packed.rs`, `codestream.rs` | none |
| P5.19 | `src/encode/jp2.rs`, `options.rs` | none: uses the public `Palette` type |
| P5.20 | `src/encode/profile/mod.rs`, `profile/rules.rs`, `profile/check.rs`, `options.rs` | `src/codestream/mod.rs`: keep `Rsiz` and each tile-part's marker list in the parsed header, `pub(crate)` |
| P5.21 | `src/encode/profile/cinema.rs` | none |
| P5.22 | `src/encode/profile/cinema.rs` | none |
| P5.23 | `src/encode/profile/broadcast.rs` | none |
| P5.24 | `src/encode/profile/imf.rs` | none |
| P5.25 | `fuzz/`, `src/encode/tests.rs` | `src/fuzz.rs`: new entries behind the `encode` feature |

The collisions left are within lanes, which run in order (P5.10 and P5.15 on
`tile.rs`; P5.12 and P5.17 on `passes.rs`; P5.14 and P5.19 on `jp2.rs`;
P5.21 and P5.22 on `cinema.rs`), and
`options.rs`, which every step may grow by one field: each option lands in
its own commit to keep the merge mechanical. P5.9 lands its interface (a
`CodedTile` of per-block pass lists in `src/encode/mod.rs`) before the
geometry lane builds on it.

## Steps

### P5.0 — Encoder harness and corpus

**Goal:** the grading scaffold exists before the encoder, as P2.0 to P4.0
did.

**Work:** the synthetic image generator; the input list (decoded fixtures,
Part 4 references, synthetic images, GRIB2 grids);
`scripts/gen-encoder-parity.py` and `scripts/gen-encoder-crossdecode.py`,
which run `opj_compress` and `opj_decompress` and write the two snapshots;
`tests/encode_roundtrip.rs`, `tests/encode_crossdecode.rs` and
`tests/encode_parity.rs`, each reporting *not yet encoded* per case through an
`ENCODED` ratchet like `IN_CLASS`; a `cargo publish --dry-run` check that no
new fixture ships. Every test and bench added here is gated on the `encode`
feature (benches through `required-features`), which the maintainer declares
first (decision 5).

**Oracle:** OpenJPEG's output only; this step is the harness.

**Gate:** the parity snapshot holds OpenJPEG's figures for every case; every
case reports *not yet encoded*; `cargo test` is green with and without
default features.

### P5.0b — Open the decoder seams

**Goal:** every decoder item the encoder shares is reachable and free of
decode-only assumptions, in one behavior-preserving PR.

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
  `next_segment` `pub(crate)` with no reader in its signature.
- `src/tier1/ebcot/mq.rs`: move the H.2 vectors out of the private test
  module into a `#[cfg(test)] pub(crate)` item.
- `src/error.rs`: `Inconsistent` reads "internal bug (please report it)", not
  "decoder bug", since encoder paths can reach it.
- `src/tier2/mod.rs`: a `Meter::unlimited()` constructor behind the `encode`
  feature, for walks over the encoder's own geometry.

**Oracle:** the whole decoder suite, unchanged, plus the decode benchmark on
the HT and Part 1 rows of performance.md, since `passes.rs` and the context
lookups are the hot path.

**Gate:** every test passes with and without default features, clippy is
clean in both builds, and no benchmark row moves by more than its noise.

### P5.1 — Forward DWT (`src/encode/dwt.rs`)

**Goal:** the 5/3 and 9/7 analysis transforms over a tile-component, at any
origin parity, with 0 to 32 levels, writing the `Bands<T>` layout the
inverse reads (generic already, so no new coefficient type is needed).

**Work:** analysis on the shared `lift` and `lift_split` sweeps. The 9/7
convention is the exact inverse of this decoder's: the low-pass scaled by
`1/K` and the high-pass by `K/2` at every filtering, and no subband gain in
the step (P5.3). OpenJPEG's encoder scales the high-pass by `K` and puts the
nominal gain in the step instead. The two differ only by powers of two, which
are exact in floating point, so on ordinary tile-components they should give
the same quantization indices (estimated; this step measures it). They
differ where a coarse resolution is empty and a coefficient is high-pass a
different number of times than its nominal gain says (#126). There the
per-filter convention is the one the decoders invert.

**Oracle:**
- Property: `inverse(forward(x)) == x` exactly for 5/3, and within 1e-3
  relative for 9/7, over random sizes from 1 to 300 per side, every origin
  parity and 0 to 8 levels, through the decoder's own inverse, which is the
  exact partner of the `K/2` convention.
- Property: the same coefficients equal a reference forward in OpenJPEG's
  convention (`K`, then divided by `2^gain`), bit for bit on ordinary
  shapes.
- The #126 shapes (tile-components with empty coarse resolutions): the round
  trip holds, and how OpenJPEG's own `-I` encode of the same shape differs is
  measured and recorded.
- External: on `opj_compress -n k` lossless 5/3 files, the coefficients our
  tier-1 decoder reads equal our forward 5/3 of the same image, bit for bit,
  for `k` from 1 to 6.

**Gate:** all pass, and a mutation of each lifting step fails a test.

### P5.2 — Forward MCT and DC level shift (`src/encode/mct.rs`)

**Goal:** the forward RCT and ICT, and the level shift. Neither shares code
with the inverse.

**Oracle:** property: the inverse RCT of the forward RCT is the identity
over the full range of each depth. External: at `-n 1` (no transform), the
coefficients of `opj_compress -mct 1` read back by our tier-1 equal our
forward RCT bit for bit, and our ICT is within 1 of `-I -mct 1 -n 1`'s.

**Work also:** measure the lossless depth ceiling under the 30-plane cap
(P5.3) for 0 to 6 levels, with and without the RCT, which adds a bit to the
two chroma components. The measured table goes into scope.md.

**Gate:** both pass at depths 1, 8, 16 and the measured ceiling, and one bit
above the ceiling is refused with `InvalidOptions`.

### P5.3 — Forward quantization (`src/encode/quant.rs`)

**Goal:** step sizes and quantization indices for 9/7, and the plane rule.

**Work:**
- The step is the one the decoder reads (`half_step_of` in `src/quant.rs`):
  `Δb = 2^(R − εb) · (1 + μb / 2^11)`, with `R` the component depth and no
  subband gain. `εb` and `μb` are chosen as OpenJPEG chooses them,
  `floor(8192 / norm_b)` encoded against the depth, so the `QCD` matches. The
  index is `sign(c) · floor(|c| / Δb)` on coefficients from P5.1's `K/2`
  forward, which equals OpenJPEG's quantization of its `K` forward with the
  gain in the step. The gain table and the synthesis-norm table are new code
  here; the decoder has neither. P5.9 reads the norms for its distortion.
- **The plane rule.** A code-block's first coded plane is
  `Mb + s − zero bit-planes`, with `Mb = G + εb − 1` (E.1) and `s` the
  maxshift. Both decoders refuse more than 30 (`MAX_BIT_PLANES` in
  `src/tier1/ebcot/passes.rs`, and OpenJPEG's `bpno_plus_one >= 31`). The
  encoder refuses, with `InvalidOptions` before any coding, any options
  whose worst case `Mb + s` exceeds 30 in some band: `Mb` from the declared
  depth, the transform, the RCT and the guard bits. The worst case was chosen
  over the actual data because the answer is then known from the options
  alone: the same options either always work for an image shape or never
  do, and a caller learns it before spending the encode. The cost is that a
  high declared depth holding small values is refused even though its coded
  planes would fit. A 31-bit unsigned image fits only with no DWT, no RCT and
  0 guard bits, and a 32-bit signed one never fits (estimated from
  `Mb = G + εb − 1`; P5.2 and this step measure it). The maxshift is the one
  exception (P5.17): `s` depends on the data by definition, so its check
  runs per block once `s` is known, still before any byte is written.

**Oracle:** the `QCD` (`SPqcd` exponent and mantissa of every band) equals
`opj_compress -I`'s byte for byte, at depths 1 to 16 and 0 to 10 levels, read
with `opj_dump`. This depends on choosing `εb` and `μb` as OpenJPEG does, and
P5.5's block-byte comparison further depends on the indices matching (P5.1).
Property: the decoder's dequantization of our indices is within half a step
of the input coefficient. The plane rule: every refused option set has a
reject-matrix row, and the largest accepted depth at each level count
decodes in both decoders.

**Gate:** all pass.

### P5.4 — MQ encoder (`src/encode/mq.rs`)

**Goal:** the Annex C encoder, with the standard flush and the predictable
termination and raw (bypass) variants.

**Oracle:** the ITU-T T.88 H.2 test sequence: encoding its 256 decisions
gives exactly the bytes the decoder's test already decodes (the shared
vectors from P5.0b). Property: our
`MqDecoder` decodes any random sequence of contexts and decisions we encode,
in each termination mode.

**Gate:** both pass, and an `mq_encode` fuzz target round-trips for 60 s.

### P5.5 — EBCOT pass encoding (`src/encode/passes.rs`)

**Goal:** a code-block's significance, refinement and cleanup passes, with
each pass's byte count and distortion decrease, at code-block style 0.

**Oracle:** our tier-1 decoder decodes every block we encode back to the
quantized coefficients at every truncation point (property; random blocks
from 4×4 to 64×64 in every sub-band kind). External: on lossless
`opj_compress` files, the block bytes we write for the same coefficients
equal OpenJPEG's. That is a hypothesis this step measures, since both
follow the FLUSH procedure of Annex C. If they differ, the step records
where, and the gate keeps only the decoder round trip.

**Gate:** the round trip passes, and the OpenJPEG comparison is recorded.

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

**Work:** SIZ, COD, QCD, SOT, SOD and EOC in `markers.rs`; `EncodeOptions`
with levels, block size, transform and MCT; validation of input and options
(`InvalidImage`, `InvalidOptions`); the levels clamp; the `encode` feature.
`EncodeOptions` records, for every option, whether the caller set it, so
that a profile preset (decision 9) can fill in only the unset ones;
`benches/encode.rs` and an `encode` mode in `tools/perf/run.py`.

Every option and the image are checked before any shared decoder helper
runs. The helpers (`resolution_geoms`, `for_each_packet`) return decode
errors (`Marker`, `Codestream`, a `Limit` "for a tile") and need a `Meter`;
the encoder passes `Meter::unlimited()`, and a helper error during an encode
is treated as a bug and reported as `Inconsistent`, never passed through. The
encoder also refuses options that would make a tile hold more than the
decoder's fixed caps of 2^18 precincts or 2^20 code-blocks, since no decode
by this crate could read the result (see open questions). Two small edits
ride along: `src/pipeline.rs`'s module doc stops calling itself "the decode
pipeline", and `pipeline::report` may be renamed `pipeline::partial`.

**Oracle:** the source image (lossless); `opj_decompress`; a byte comparison
with `opj_compress` at the same settings with its COM marker removed. The
bytes are expected identical if P5.5 matched; otherwise each difference is
explained.

**Gate:** every reversible round-trip case at these settings is in `ENCODED`
and bit-exact through both decoders, and the first performance row is
recorded against `opj_compress`.

### P5.8 — Irreversible end to end with fixed quantization (`src/encode/mod.rs`)

**Goal:** the equivalent of `opj_compress -I`: 9/7, ICT and expounded steps,
with all passes.

**Oracle:** the source, graded by gate part 1 (PAE and MSE at most
OpenJPEG's plus 1 PAE); `opj_decompress` within the decoder's 9/7 gap of our
decode.

**Gate:** every fixed-quantization case passes.

### P5.9 — Rate control and layers (`src/encode/rate.rs`)

**Goal:** PCRD-opt to per-layer bpp targets (decision 3).

**Work:** each block's convex hull; a slope bisection per tile against the
byte target, headers included; layers from the steepest slope down; the last
lossless layer; each layer's size measured by a dry run of the packet writer.

**Oracle:** gate part 3, the parity snapshot.

**Gate:** every parity case meets both inequalities, and a mutant that drops
the hull or offsets the slope comparison fails a case.

### P5.10 — Tiles, offsets and sub-sampling (`src/encode/tile.rs`)

**Oracle:** the source round trip; `opj_decompress`; for tiles,
`opj_compress -t` at the same settings, compared byte for byte as in P5.7.

**Gate:** the tiled, offset, 4:2:0 and 4:2:2 cases of the sweep are in
`ENCODED`.

### P5.11 — Precincts and the five progression orders (`src/encode/packet.rs`)

**Oracle:** the source round trip; `opj_decompress`; the packet order
against `opj_compress -p` files of the same settings, byte for byte where
P5.7 matched. With one precinct, one layer and one component, the orders
cannot be told apart (correctness.md), so the sweep uses several of each.

**Gate:** each order gives a different stream on the multi-precinct cases,
and all of them round-trip.

### P5.12 — Code-block styles (`src/encode/passes.rs`, `mq.rs`)

**Oracle:** the source round trip; `opj_decompress`; the block bytes of
`opj_compress -M m` for each style bit, as in P5.5. Each bit is also tested
alone, because `restart` masks `segsym` (correctness.md), as the
`cblksty_*_lossless` fixtures do.

**Gate:** each style alone and all 64 combinations round-trip through both
decoders.

### P5.13 — Markers and tile-parts (`src/encode/markers.rs`, `codestream.rs`)

**Goal:** SOP, EPH, PLT, TLM, COM, CRG, and tile-parts split by R, L or C.

**Work:** each marker as an option. The tile-part split avoids the two
OpenJPEG writing faults
recorded in #443.

**Oracle:** `opj_dump` marker lists; our decoder's PLT and TLM readers;
`opj_decompress` decodes each file. CRG has no decoder oracle (both decoders
read and ignore it), so its bytes are checked against the A.9 layout by a
unit test.

**Gate:** each marker and each split round-trips, and the PLT and TLM
lengths equal the packet and tile-part lengths.

### P5.14 — JP2 writer (`src/encode/jp2.rs`)

**Goal:** `Format::Jp2` with `ihdr`, `bpcc`, `colr` (enumerated, or ICC bytes
from the caller), `res`, `cdef`, and the passthrough boxes.

**Oracle:** `scripts/gen-jp2-box-snapshots.py`, the independent box walker;
`opj_dump`; jpylyzer's validity check (decision 6); our `read_header`
returns what the options asked for.

**Gate:** every combination round-trips through `decode` to the same
`Jp2Metadata`, and `opj_decompress` decodes each file.

### P5.15 — COC and QCC, per component and per tile (`src/encode/options.rs`, `markers.rs`, `tile.rs`)

**Goal:** per-component coding (levels, block size, styles, precincts) and
quantization, in the main header and in a tile's first tile-part header.

**Work:** a per-component and per-tile override in the options. The rules
the decoder enforces are refused with `InvalidOptions` up front: a tile
override that changes a component's wavelet, and RCT or ICT over components
0 to 2 with different wavelets. The writer emits a COC or QCC only where a
value differs from what is in force, as OpenJPEG does.

**Oracle:** the source round trip; `opj_decompress`; on mixed-depth inputs,
the QCC that OpenJPEG writes for the same depths, compared field by field if
`opj_compress` can be made to write one (measured in this step; its command
line sets no per-component values).

**Gate:** every override shape in the sweep round-trips through both
decoders, and each refused shape has a reject-matrix row.

### P5.16 — POC (`src/encode/poc.rs`, `packet.rs`, `markers.rs`)

**Goal:** a progression made of volumes, in the main header or in a tile
header.

**Work:** POC volumes as an option. A sequence that leaves a packet out or
sends one twice is refused with `InvalidOptions`: OpenJPEG and this decoder
read such sequences differently ([departures](openjpeg-departures.md#packets-a-poc-sequence-leaves-out)),
and `opj_compress` writes overlapping volumes wrongly (measured above). A
tile-header POC is not combined with a main-header POC for the same tile,
since the two decoders disagree on which one wins
([departures](openjpeg-departures.md#a-tile-part-poc-overrides-the-main-headers)).

**Oracle:**
- the source round trip and `opj_decompress`;
- the packet order against `opj_compress -POC` with the same disjoint
  volumes, byte for byte where P5.7 matched;
- the order itself, checked from outside: cut the codestream after the
  first volume and decode the prefix with `opj_decompress -allow-partial`; the
  resolutions, components and layers that volume covers decode, and the
  others are zero.

What a cross-decode proves: OpenJPEG reads the marker and assigns each packet
where this encoder put it, or a lossless decode fails. What it cannot prove:
that two encoders' packet orders differ in the way the caller meant on cases
where every order gives the same packets (one precinct, one layer), and
anything about the sequences refused above. The prefix check covers the
first.

**Gate:** every POC case in the sweep is lossless through both decoders, and
each prefix decodes only the volumes before the cut.

### P5.17 — Region of interest, maxshift (`src/encode/roi.rs`, `passes.rs`, `markers.rs`)

**Goal:** a caller-given region per component (a rectangle, or a mask on the
component grid) coded by maxshift, with `RGN` in the main or a tile header.

**Work:** map the region to the wavelet coefficients it affects (Annex H);
choose the shift `s`; shift the region's coefficients up `s` planes before
Tier-1; carry the shifted planes through the bypass count (D.6, counted from
the first coded plane). Choose `s` one plane above the background's largest
magnitude. H.1 needs only `2^s` above it, but OpenJPEG takes a coefficient as
region one plane lower ([departures](openjpeg-departures.md#region-of-interest-threshold-part-1-and-ht)),
and the spare plane makes both decoders sort every coefficient alike. Its
cost is measured in this step. The 30-plane cap (P5.3) counts the shift:
since `s` is at least the background's planes, a region block needs about
twice its own planes, so maxshift fits only at moderate depths (estimated
around 12 to 14 bits; measured here). A block that would exceed the cap is
refused with `InvalidOptions` naming the region, before any byte is written.

**Oracle:**
- lossless ROI encodes decode to the source in both decoders, for any
  region shape: the source is the reference;
- with bypass, OpenJPEG decodes its own maxshift streams wrongly
  ([departures](openjpeg-departures.md#bypass-under-a-maxshift)), so those
  cases are graded against the source through this decoder alone, whose
  bypass counting the `roi_bypass` fixtures already grade against a source
  image; Grok is not an oracle here either, given its region-of-interest bug
  (release-validation.md);
- lossy: at the same rate, the region's PSNR is higher than in the same
  encode without ROI, and the background's is lower, in both decoders;
- `opj_compress -ROI c=,U=` is the oracle for the whole-component case only:
  the `RGN` bytes match.

What a cross-decode proves: OpenJPEG reads `RGN`, undoes the shift and sorts
region from background as this encoder did. What it cannot prove: the
bypass cases (OpenJPEG is wrong there), and anything about OpenJPEG's own
ROI encoding beyond a whole component, since `opj_compress -ROI` makes no
background (measured above).

**Gate:** every lossless ROI case is exact through both decoders (with bypass,
through this one); every lossy case meets the PSNR inequalities.

### P5.18 — PPM and PPT (`src/encode/packed.rs`, `codestream.rs`)

**Goal:** packet headers moved into the main header (PPM) or into tile-part
headers (PPT), split across marker segments by their length limit.

**Oracle:** the source round trip; `opj_decompress` (OpenJPEG 2.5.0 has no
PPM or PPT writer, but reads both); a structural check by an independent
script, `scripts/check-packed-headers.py`: putting each packet header back in
front of its body gives the same bytes as the same encode without packing.

**Gate:** PPM and PPT cases, with one or several tiles and tile-parts, are
lossless through both decoders and pass the structural check; asking for both
is `InvalidOptions`.

### P5.19 — Palette encode, `pclr` and `cmap` (`src/encode/jp2.rs`, `options.rs`)

**Goal:** a JP2 file whose codestream holds palette indexes, with the palette
in `pclr` and the mapping in `cmap`.

**Work:** the options take the public `Palette` type and say which component
holds the indexes. An index outside the palette is `InvalidImage`. Palette
columns up to the depths the decoder reads.

**Oracle:** `opj_decompress` applies the palette (OpenJPEG writes none but
reads them): its output equals the input with the palette applied;
`decode` gives the same, and `with_raw_channels(true)` gives the indexes
back; jpylyzer validates the boxes.

**Gate:** palettes of 8-bit, 12-bit, signed and mixed columns, and with
`cdef`, round-trip through both decoders.

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
  IMF preset exists.
- `profile/check.rs`, compiled only for tests: evaluates those rows on a
  codestream as this crate's parser reads it. That covers `Rsiz`; image and
  tile sizes and origins; component count, depth and sampling; COD, COC, QCD
  and QCC and where they sit; levels, block size and style, transform and
  precincts; layers and order; POC contents; RGN, PPM, PPT, TLM, PLT, SOP
  and EPH; the tile-part count and what each holds; and the byte totals per
  frame, per component and per layer against Equation A-8.
- The presets in later steps read the same rows, so a preset and the checker
  cannot drift apart. The checker uses only the rows and the parsed stream,
  never preset code.

**Oracle:** the checker is itself tested two ways. Each row gets a passing
stream and a hand-edited one that breaks only that row, which the checker
must reject. The checker must also accept the `opj_compress` and
`grk_compress` streams measured above, except on the rows they were
measured to break, and must flag exactly those rows. For Profile 0 and 1,
no other encoder writes them (`opj_compress` has no option for them), so
the oracle is the checker, both decoders and the source round trip.

What it proves: that our output meets every rule as transcribed, and that
the checker tells a passing stream from a failing one on every rule. What it
cannot prove: that a rule was transcribed right. Each row cites its table and
row so review can check it, and P5.21, P5.23 and P5.24 compare fields with
encoders that read the same tables themselves.

**Gate:** every row has its passing and failing test; the Profile 0 and 1
sweep passes the checker and both decoders; each conflict between a set
option and the profile has a reject-matrix row.

### P5.21 — 2K and 4K digital cinema (`src/encode/profile/cinema.rs`)

**Goal:** `Profile::Cinema2k { fps }` and `Profile::Cinema4k { fps }`, `Rsiz`
3 and 4 (Table A.46).

**Work:** the presets: one tile at the origin, 3 components of 12 bits, 9/7
(5/3 allowed), 5 levels at most (2K) or 1 to 6 (4K), 32×32 blocks with style
0, precincts of 128 at the lowest resolution and 256 above, CPRL, one layer,
TLM, COD to QCC in the main header only. 2K writes 3 tile-parts, one per
component. 4K writes 6, with the two-progression POC of Table A.46. The byte
limits per frame and per component come from Equation A-8 at the given
frame rate, at most 60 (2K) or 30 (4K).

**Oracle:**
- Fields: on the same synthetic frames, the main header (SIZ, COD, QCD, POC,
  TLM layout) equals what `opj_compress -cinema2K 24`, `-cinema2K 48` and
  `-cinema4K` write, field for field, apart from COM. So do the tile-part
  count and the order of components in them. The coded bytes are not
  compared, since the two rate allocators truncate differently (decision 3).
- The rule checker, including the per-frame and per-component byte limits.
- Both decoders, and gate parts 1 and 3 at the profile's rate.

What it proves: we choose the same parameters as an independent encoder,
the output meets the rules as transcribed, and it decodes. What it cannot
prove: the DCI and SMPTE rules for a cinema package beyond T.800, such as
XYZ color and MXF wrapping, which are out of scope.

**Gate:** 2K at 24 and 48 fps and 4K at 24 fps, at full size and smaller,
match OpenJPEG's fields, pass the checker, decode in both decoders, and stay
within the byte limits. Each conflicting option (for example
`-cinema2K 24 -n 7 -b 64,64 -p LRCP`, which OpenJPEG rewrites) is refused.

### P5.22 — Scalable cinema and long-term storage (`src/encode/profile/cinema.rs`)

**Goal:** `Rsiz` 5, 6 and 7 (Table A.46).

**Work:**
- Scalable 2K and 4K: two layers; the two- or four-progression POC; 6 or 12
  tile-parts in the order of Figures A.25 to A.29; PLT in every tile-part; no
  SOP or EPH; 9/7 only. Layer 0 must also meet the 2K or 4K cinema byte
  limits.
- Long-term storage: up to 16384×8640 and 8 components, one tile or tiles of
  at least 1024×512, at most 5 layers, SOP and EPH on every packet, PLT, 5/3
  or 9/7, and one tile-part per component.

**Oracle:** neither OpenJPEG nor Grok writes these profiles, so there is no
field comparison. The oracle is:
- the rule checker;
- both decoders, and the source round trip for 5/3 storage encodes;
- a structural check of the purpose Table A.46 states, that a distribution
  stream "can be obtained by simply stripping some tile-parts". Removing the
  layer-2 tile-parts of a scalable stream, and fixing TLM and `Rsiz` to 3 or
  4, gives a stream that passes the checker as 2K or 4K cinema and decodes in
  both decoders.

What it proves: the rules as transcribed hold, and the layered structure
does what the profile is for. What it cannot prove: that our reading of
the layer-0 byte limit and the figures' tile-part order matches what another
implementation reads, since none exists to compare with.

**Gate:** each of the three profiles, at full and smaller sizes, passes the
checker and both decoders, and every scalable stream passes the stripping
check.

### P5.23 — Broadcast contribution (`src/encode/profile/broadcast.rs`)

**Goal:** the single-tile, multi-tile and multi-tile reversible profiles with
their main levels (Tables A.48 to A.50).

**Work:** the presets: 1 tile, or 1 or 4 tiles of the same size; up to 4
components of 8 to 12 bits, 4:4:4 or 4:2:2; 1 to 5 levels; one layer; blocks
of 32 to 128 wide and 32 or 64 high, the same in every component; style 0;
precincts of 128 then 256; CPRL with no POC; one tile-part per tile-component;
TLM. 9/7 for single and multi-tile, 5/3 for reversible. The main level limits
the sample rate (components × pixels × lines × frame rate) and the bit rate,
so a level above 0 needs the frame rate. A main level outside the table
(reversible allows only 6 and 7) is refused.

**Oracle:**
- Fields: `grk_compress -U SINGLE`, `-U MULTI` and `-U MULTI_R` on the same
  frames, for the choices Table A.48 leaves open (levels, block size). They
  are compared and any difference is explained. Grok is not a conformance
  oracle: its streams lack the TLM Table A.48 requires, and it writes
  `Rsiz` `0x0300`, which Table A.10 does not list (measured above).
- The rule checker; both decoders; the source round trip for the reversible
  profile.

What it proves: the rules as transcribed hold, and the open choices agree
with, or knowingly differ from, one other encoder. What it cannot prove:
that the level arithmetic matches the reading of any other implementation
that holds it, since Grok was not measured enforcing it.

**Gate:** each profile at levels 0, 1 and the top of its table passes the
checker and both decoders; a frame rate that breaks the level, and each
conflicting option, are refused.

### P5.24 — IMF (`src/encode/profile/imf.rs`)

**Goal:** the six IMF profiles with main level and sublevel (Tables A.51 to
A.54).

**Work:** the presets: up to 2048×1556, 4096×3112 or 8192×6224; up to 3
components of 8 to 16 bits, 4:4:4 or 4:2:2; 32×32 blocks with style 0;
precincts of 128 then 256; CPRL with no POC; one layer; one tile-part per
tile-component; TLM. The lossy profiles use one tile and 9/7, with 1 to 5, 6
or 7 levels. The reversible profiles use 5/3, with one tile or tiles of 1024,
2048 or 4096, and the level limit set by the tile width. A sublevel larger
than its main level allows (Table A.53) is refused. The sublevel's bit rate
over the frame rate is a byte limit the output holds.

**Oracle:**
- Fields: `opj_compress -IMF 2K|4K|8K|2K_R|4K_R|8K_R`, with and without
  `mainlevel`, `sublevel` and `framerate`, on the same frames: the main
  header and the tile-part layout, field for field apart from COM.
- The rule checker. This alone grades the sublevel byte limit, which
  OpenJPEG 2.5.0 prints but does not hold (measured above).
- Both decoders; the source round trip for the reversible profiles.

What it proves: as for P5.21, and that the byte limit holds where OpenJPEG
does not hold it. What it cannot prove: the IMF application rules of SMPTE
ST 2067 beyond T.800.

**Gate:** each profile at full and smaller sizes, single- and multi-tile
where allowed, matches OpenJPEG's fields, passes the checker, decodes in both
decoders, and stays within the sublevel's limit. Each conflicting option
(OpenJPEG falls back to `Rsiz` 0 for `-n 7` and for `-ROI`) is refused.

### P5.25 — Fuzz targets and the encoder reject matrix (`fuzz/`)

**Work:** `mq_encode` (from P5.4); `encode_roundtrip`: arbitrary small images
(at most 64×64, 1 to 4 components, any depth, sign and sampling) with
arbitrary `EncodeOptions` give `Ok` or a typed error, never a panic, and on
`Ok` our decoder decodes the output, exactly for reversible and within bound
for irreversible. Each later step adds its options to the target. An encoder
reject matrix in `src/encode/tests.rs` pins each refused option and input.

**Gate:** a session of 60 s per target finds nothing.

### P5.26 — Integration and the phase gate

**Work:** the scope.md encoding rows; roadmap.md; an encoder section in
correctness.md; openjpeg-departures.md (the levels clamp, no COM, the ratio,
the spare ROI plane, the POC sequences refused, the plane rule, the 9/7
gain placed per filtering on #126 shapes, profile conflicts refused
where OpenJPEG overrides or drops the profile, the IMF byte limit held, and
any difference found);
`docs/encoder-ics.md` (Annex F); an encoder table in performance.md;
resource-bounds.md (the encoder's allocations are bounded by the input,
which the caller already holds); README, `lib.rs` docs and CHANGELOG; the
RELEASING.md pre-release review.

**Gate (the Phase 5 exit):** all five gate parts pass over the full sweep;
every earlier fixture is unchanged; fuzzing is clean; the quality gates are
green with and without the `encode` feature.

## Quality plan

- **Fuzzing.** The two targets above. libFuzzer cannot run OpenJPEG in
  process, so "both decoders accept it" is a release round: replay the
  `encode_roundtrip` corpus, write each output, and run `opj_decompress` on
  it under a memory cap. Any refusal is a finding.
- **Determinism.** The same image and options give the same bytes on every
  run and every target. The 9/7 path uses `f32` and `f64` without fused
  multiply-add, which Rust does not insert on its own, so x86-64, aarch64 and
  wasm32 must agree. The committed digests enforce it: CI already runs
  `cargo test` on macOS aarch64, and the 32-bit release round adds wasm32.
  Threads (#301), when they come, must not change a byte.
- **Performance.** `benches/encode.rs` (std only) and the `encode` mode of
  `tools/perf/run.py`, from P5.7, timed against `opj_compress` on one thread
  and on all cores, on the three inputs measured above, with peak memory.
  *Estimated* target for 0.6.0: within 2× of `opj_compress` on one thread,
  the same line performance.md draws for the decoder. Each later step reports
  its change on the same table.
- **Mutation checks.** `cargo mutants --in-diff` on each step's PR over
  `src/encode/`, and over the whole phase before the tag. Survivors are most
  likely in rate control (slope comparisons, hull pruning), termination, the
  ROI shift, the POC volume checks and the profile limits (a `<` for a
  `<=`); the parity, byte, prefix and checker tests are meant to catch them,
  so a survivor there is a gap.
- **New rounds for `docs/release-validation.md`:**
  - *Encoder cross-decode sweep:* the full sweep through `opj_decompress` and
    Grok, bit for bit on 5/3 and within the gap on 9/7, leaving out the
    combinations each tool decodes wrongly (bypass with ROI for OpenJPEG;
    ROI for Grok);
  - *Rate parity:* rerun `gen-encoder-parity.py` and compare with the
    committed figures;
  - *Encoder fuzz replay through OpenJPEG* (above);
  - *JP2 validity:* jpylyzer over every JP2 output;
  - *Profile fields:* rerun `opj_compress` and `grk_compress` with each
    profile option and compare with the committed `profiles.json`;
  - *Kakadu, by hand and optional:* `kdu_expand` over the sweep. Its results
    go in the round's notes and nowhere else;
  - *Downstream consumer:* a GRIB2 template 5.40 message carrying our
    codestream in section 7, read by eccodes and fieldglass. eccodes decodes
    with OpenJPEG, so this checks the packing, not a third decoder.

## Out of scope for Phase 5

Refused with `Error::Unsupported`, never attempted: PSNR targets; `io::Write`
output; the `PRF` marker; threads and SIMD (#301, #302); narrower input
types (#303); HTJ2K encode, its Part 15 profiles, and Part 2 (Phase 6).
Cinema and IMF packaging (MXF, DCI color rules) is outside the codestream
and outside the crate. Visual weighting (CSF), or any quality model other than MSE, is
not planned. Image file readers (PNM, TIFF, PNG) stay outside the crate: the
input is samples.

## Prerequisites and open questions

- **#414** (decode orchestration out of `lib.rs` into the pipeline module)
  landed as #447, so the encoder's orchestration does not grow `lib.rs`.
- **The `encode` feature** (`encode = []`, on by default) is declared in
  `Cargo.toml` before P5.0, with the two `--no-default-features` CI steps
  (decision 5). Both are maintainer edits.
- **#254** (the image origin on `Image` and `Header`) comes before P5.7, so
  an offset image round-trips through `&Image` with no extra option. The
  origin must be on the same grid as `width` and `height`:
  `ceil(XOsiz / 2^r)` under a reduction, the window's corner under `region`.
- **#142** (one coefficient type) is no longer a prerequisite: the forward
  path reuses `Samples` and `SubbandCoeffs` and needs only `Bands<i32>`,
  which the generic `Bands<T>` already gives.
- **Should the encoder refuse a stream its own decoder refuses at the
  default limits?** Recommended: yes for the fixed per-tile caps (2^18
  precincts, 2^20 code-blocks), which no `DecodeOptions` can raise, so such
  a file could never be read back by this crate; no for the sample limit
  (2^29 by default), which a caller raises with an option, and which the
  encoder's caller is already past, since it holds the image in memory. The
  docs then say which limit to raise to read a large output back.
- **The plane rule** (P5.3) is the worst case from the options. The other
  choice, `εb` from each band's actual largest magnitude, would let a 32-bit
  image of small values encode, as the stream `tests/signed_32bit.rs` reads
  shows is legal, at the cost of a result that depends on the pixels and a
  `QCD` that no longer matches OpenJPEG's. Recommended: the worst case for
  0.6.0, with the data-derived exponents left as a possible option later.
- **Whether our tier-1 bytes can match OpenJPEG's** is unmeasured (P5.5). The
  answer decides whether P5.7, P5.10 to P5.12 and P5.16 get byte comparisons
  or only round trips. For 9/7 it also depends on P5.1's indices matching
  OpenJPEG's under the two gain conventions.
- **The cost of the spare ROI plane** (P5.17) is unmeasured. If it is large,
  the alternative is the H.1 minimum with ROI cases graded by this decoder
  alone, which would need its own decision.
- **Which edition the profile rules come from.** The rows are transcribed
  from T.800 (06/2019), which the ITU publishes free. The current edition is
  T.800 (07/2024), which was not free to download when this plan was written.
  P5.20 checks the 2024 tables for changes before transcribing, or records
  that it could not.
- **The rule checker reads this crate's parser.** A parser bug that hides a
  field from the checker would hide it from the encoder's tests too. The
  field comparisons in P5.21, P5.23 and P5.24 cover the fields OpenJPEG and
  Grok write; for P5.22 and Profile 0 and 1, an independent script reading
  the headers, like the box walker of P5.14, may be worth adding. Decide in
  P5.20.
- **The per-component and layer-0 byte limits** of the cinema profiles are
  read as Table A.47 lays them out. Whether `opj_compress` holds the
  per-component limit was not measured (its measured tile-parts were all
  under it).
- **A CI job with OpenJPEG installed**, to run the cross-decode live instead
  of from digests, would be a CI edit for the maintainer. The plan does not
  need it.

## Sources

- ITU-T T.800 | ISO/IEC 15444-1: Annexes A to D, F and H, and the informative
  Annex J on rate control. The profile rules are from T.800 (06/2019), free
  at `https://www.itu.int/rec/T-REC-T.800-201906-S`: Table A.10, A.5.3 (PRF),
  and A.10 (Tables A.45 to A.55, Equation A-8).
- ITU-T T.803 (02/2024) | ISO/IEC 15444-4:2024, free PDF at
  `https://www.itu.int/rec/T-REC-T.803-202402-I`: 6.3, 6.6, Annex D (D.1 to
  D.4, Figure D.1) and Annex F (Table F.1).
- ITU-T T.88 (08/2018) H.2: the MQ test sequence.
- OpenJPEG 2.5.0 source, tag `v2.5.0`: `tcd.c` (`opj_tcd_rateallocate` at
  line 443, `opj_tcd_makelayer` at 237), `j2k.c` (`opj_j2k_update_rates` at
  5260, the rate-of-1 rule at 8070, the `opj_j2k_write_*` marker writers),
  `dwt.c` (`opj_dwt_calc_explicit_stepsizes` at 1966,
  `opj_dwt_encode_stepsize` at 1092), `t1.c` (`opj_t1_encode_cblk`),
  `jp2.c`; `opj_j2k_set_cinema_parameters`, `opj_j2k_is_cinema_compliant`,
  `opj_j2k_set_imf_parameters` and `opj_j2k_is_imf_compliant` in `j2k.c`;
  and `opj_compress -h`.
- Grok 20.4.25 `grk_compress -h` and its broadcast messages.
- Grok's `grk_compress` manual page, the `-A` rate-control algorithms:
  `https://manpages.debian.org/testing/grokj2k-tools/grk_compress.1.en.html`.
- D. Taubman, "High performance scalable image compression with EBCOT",
  IEEE Transactions on Image Processing 9(7), 2000: PCRD-opt.
