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
that the decoder reads (decision 4).

### Transform and coding

| Feature | Notes |
| ------- | ----- |
| 5/3 reversible and 9/7 irreversible forward DWT, 0 to 32 levels | the decoder's inverse is the property-test partner |
| DC level shift, RCT and ICT on components 0 to 2 | on by default for 3 or more components of equal geometry, as in OpenJPEG |
| Scalar quantization: none (5/3), derived and expounded (9/7) | |
| Depth 1 to 31 bits unsigned, 1 to 32 signed | the decoder's range; guard bits chosen so that no band overflows `i32` |
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
| `Rsiz` 0, or a claim of Profile 0 or Profile 1, checked against its restrictions | |

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
| Digital cinema, IMF and broadcast profiles (`Rsiz` 3 and up) | profile rules from the Part 1 amendments; no user has asked |
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
(`qntsty=2`) with steps of `2^gain / norm(level, band)` (`gain` is 0 for
9/7), encoded by `opj_dwt_encode_stepsize` (`dwt.c:1092`, `dwt.c:1966`). The
steps do not depend on the rate: the rate comes only from truncation.

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
   not one to write.
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
   wasm) sets `default-features = false` and compiles none of it. CI gains a
   `cargo test --no-default-features` job. Tables the two sides share stay in
   the decoder's modules. The `Cargo.toml` and CI edits are the maintainer's.
6. **The reference tools are OpenJPEG, Grok and jpylyzer.** OpenJPEG 2.5.0 is
   the gate. Grok (20.4.25, built for performance.md) is a second decoder in a
   release round. jpylyzer checks JP2 validity when snapshots are
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

Two smaller choices follow from the defaults: the levels clamp on small
images, and no COM marker by default (see [Which defaults to
mirror](#which-defaults-to-mirror)).

## Corpora

| Set | Source and provenance | Size | Committed? |
| --- | --------------------- | ---- | ---------- |
| Decoded fixtures | every lossless fixture in `tests/fixtures/`, decoded by this decoder (already graded against OpenJPEG) | no new bytes | already |
| Part 4 references | `tests/fixtures/conformance/references/*.pgx`, 78 files: the D.4 a) inputs | no new bytes | already; excluded from the crate |
| Synthetic | a seeded generator in `tests/support/synth_image.rs`: noise, ramps, checkerboards, edges, flat fields; sizes from 1×1 to 1024×1024, including odd and prime sizes; 1 to 4 components; depths 1, 8, 12, 16 and 31 unsigned, 32 signed; sampling 1×1, 2×1 and 2×2 | none (generated) | the generator only |
| GRIB2 fields | the committed `jpeg2000_regular_latlon`, `eta_lambert_*` and GRIB2-shaped bench codestreams, decoded | no new bytes | already |
| Parity snapshots | `tests/fixtures/encode/parity.json` | about 50 KB, estimated | yes |
| Cross-decode digests | `tests/fixtures/encode/crossdecode.json` | about 100 KB, estimated | yes |
| Large timing input | `tests/fixtures/bench/tiled_2048_gray.j2k`, decoded | no new bytes | already |

Every snapshot row records its input (fixture path or generator seed), the
options, the tool versions and the command, in `tests/fixtures/README.md`.

## Build order and why

```text
P5.0 harness ─┬─ P5.1 forward DWT ──┐
              ├─ P5.2 forward MCT ──┤
              ├─ P5.3 quantization ─┼─ P5.7 lossless file ─┬─ P5.8 9/7 fixed quant ─ P5.9 rate control ──────────────────┐
              ├─ P5.4 MQ ─ P5.5 EBCOT passes ─┤            ├─ P5.10 tiles, sampling ─ P5.11 precincts + orders ─ P5.16 POC ┤
              └─ P5.6 tag trees + packets ────┘            ├─ P5.12 code-block styles ─ P5.17 ROI ─────────────────────┤
                                                           ├─ P5.13 markers + tile-parts ─ P5.15 COC/QCC ─ P5.18 PPM/PPT ┼─ P5.21 gate
                                                           └─ P5.14 JP2 writer ─ P5.19 palette ─────────────────────────┤
P5.20 fuzz targets + reject matrix (from P5.4, then throughout) ─────────────────────────────────────────────────────────┘
Before P5.1: #142 (one coefficient type) and #254 (the origin on Image). #414 (the pipeline module) is in review as #447.
```

The stage pieces land first, each against its decoder inverse and an
external oracle, before anything is wired together, as Phase 2 built the
decoder stage by stage. P5.1 to P5.4 and P5.6 touch disjoint files and can
run in parallel. P5.7 is the first file out. It fixes the API shape early, so
later steps add options instead of reshaping it. Rate control (P5.9) follows
a fixed-quantization 9/7 encode (P5.8), so that a rate bug cannot hide behind
a transform bug.

After P5.7 the work splits into five lanes that can run side by side:

- **Rate:** P5.8, then P5.9.
- **Tier-2 and geometry:** P5.10, P5.11, then P5.16 (POC needs the five orders
  and several precincts to mean anything).
- **Tier-1:** P5.12, then P5.17 (ROI changes the bypass count and the plane
  count, so it follows the styles).
- **Headers:** P5.13, then P5.15 (tile-header COC and QCC need tiles, so P5.10
  too), then P5.18 (packed headers move what P5.11's packets write, so P5.11
  too).
- **File format:** P5.14, then P5.19.

### Files and collisions

P5.7 creates `src/encode/markers.rs`, with one writer function per marker
segment, and `src/encode/codestream.rs`, which only orders the calls. A step
that adds a marker appends its function to `markers.rs` and one call to
`codestream.rs`, so concurrent merges conflict only on appended lines.

| Step | Writes | Touches in the decoder |
| ---- | ------ | ---------------------- |
| P5.1 | `src/encode/dwt.rs` | `src/dwt.rs`: lifting constants made `pub(crate)` |
| P5.2 | `src/encode/mct.rs` | `src/mct.rs`: constants only |
| P5.3 | `src/encode/quant.rs` | `src/quant.rs`: the band gain and norm table, shared |
| P5.4 | `src/encode/mq.rs` | `src/tier1/ebcot/mq.rs`: `QE_TABLE` is already `pub` |
| P5.5 | `src/encode/passes.rs` | `src/tier1/ebcot/passes.rs`: context tables made `pub(crate)` |
| P5.6 | `src/encode/tagtree.rs`, `src/encode/packet.rs` | `src/tier2/geometry.rs`, `progression.rs`: read only |
| P5.7 | `src/encode/mod.rs`, `options.rs`, `codestream.rs`, `markers.rs`; `src/lib.rs` re-exports | `src/codestream/markers.rs`: shared structs |
| P5.8 | `src/encode/mod.rs` (9/7 branch) | none |
| P5.9 | `src/encode/rate.rs` | none |
| P5.10 | `src/encode/tile.rs` | none |
| P5.11 | `src/encode/packet.rs` | none |
| P5.12 | `src/encode/passes.rs`, `src/encode/mq.rs` | none |
| P5.13 | `src/encode/markers.rs`, `codestream.rs` | none |
| P5.14 | `src/encode/jp2.rs` | `src/jp2/boxes.rs`: box type constants |
| P5.15 | `src/encode/options.rs` (per-component and per-tile), `markers.rs`, `tile.rs` | none |
| P5.16 | `src/encode/poc.rs`, `packet.rs`, `markers.rs` | `src/tier2/progression.rs`: read only |
| P5.17 | `src/encode/roi.rs`, `passes.rs`, `markers.rs` | `src/tier1/ebcot/passes.rs`: `undo_maxshift`, read only |
| P5.18 | `src/encode/packed.rs`, `codestream.rs` | none |
| P5.19 | `src/encode/jp2.rs`, `options.rs` | `src/jp2/palette.rs`: the public `Palette` type, read only |
| P5.20 | `fuzz/`, `src/encode/tests.rs` | `src/fuzz.rs` |

The collisions left are within lanes, which run in order (P5.10 and P5.15 on
`tile.rs`; P5.12 and P5.17 on `passes.rs`; P5.14 and P5.19 on `jp2.rs`), and
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
new fixture ships.

**Oracle:** OpenJPEG's output only; this step is the harness.

**Gate:** the parity snapshot holds OpenJPEG's figures for every case; every
case reports *not yet encoded*; `cargo test` is green.

### P5.1 — Forward DWT (`src/encode/dwt.rs`)

**Goal:** the 5/3 and 9/7 analysis transforms over a tile-component, at any
origin parity, with 0 to 32 levels.

**Work:** lifting analysis with the same symmetric extension as the inverse,
writing the `Bands` layout the decoder's inverse reads (one shared type after
#142).

**Oracle:** (1) property tests: `inverse(forward(x)) == x` exactly for 5/3,
and within 1e-3 relative for 9/7, over random sizes from 1 to 300 per side,
every origin parity and 0 to 8 levels; (2) external: on `opj_compress -n k`
lossless 5/3 files, the coefficients our tier-1 decoder reads equal our
forward 5/3 of the same image, bit for bit, for `k` from 1 to 6. With no
quantization, the coded coefficients are the transform's output.

**Gate:** both pass, and a mutation of each lifting step fails a test.

### P5.2 — Forward MCT and DC level shift (`src/encode/mct.rs`)

**Goal:** the forward RCT and ICT, and the level shift.

**Oracle:** property: the inverse RCT of the forward RCT is the identity
over the full range of each depth. External: at `-n 1` (no transform), the
coefficients of `opj_compress -mct 1` read back by our tier-1 equal our
forward RCT bit for bit, and our ICT is within 1 of `-I -mct 1 -n 1`'s.

**Gate:** both pass at depths 1, 8, 16 and 31. Either the RCT's range holds
at 31 bits unsigned, or such depths refuse the RCT with `InvalidOptions`;
measure which first.

### P5.3 — Forward quantization (`src/encode/quant.rs`)

**Goal:** step sizes and quantization indices for 9/7.

**Oracle:** the `QCD` (`SPqcd` exponent and mantissa of every band) equals
`opj_compress -I`'s byte for byte, at depths 1 to 16 and 0 to 10 levels, read
with `opj_dump`. Property: the decoder's dequantization of our indices is
within half a step of the input coefficient.

**Gate:** both pass. Guard bits keep every band within 31 magnitude planes,
or the options are refused.

### P5.4 — MQ encoder (`src/encode/mq.rs`)

**Goal:** the Annex C encoder, with the standard flush and the predictable
termination and raw (bypass) variants.

**Oracle:** the ITU-T T.88 H.2 test sequence: encoding its 256 decisions
gives exactly the bytes the decoder's test already decodes. Property: our
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
zero bit-planes, pass counts, `Lblock` and lengths.

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
(`InvalidImage`, `InvalidOptions`); the levels clamp; the `encode` feature;
`benches/encode.rs` and an `encode` mode in `tools/perf/run.py`.

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

**Goal:** SOP, EPH, PLT, TLM, COM, CRG, tile-parts split by R, L or C, and a
claim of Profile 0 or 1.

**Work:** each marker as an option. A Profile 0 or 1 claim is checked
against Part 1's profile restrictions, and options that break them are
`InvalidOptions`. The tile-part split avoids the two OpenJPEG writing faults
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
cost is measured in this step.

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

### P5.20 — Fuzz targets and the encoder reject matrix (`fuzz/`)

**Work:** `mq_encode` (from P5.4); `encode_roundtrip`: arbitrary small images
(at most 64×64, 1 to 4 components, any depth, sign and sampling) with
arbitrary `EncodeOptions` give `Ok` or a typed error, never a panic, and on
`Ok` our decoder decodes the output, exactly for reversible and within bound
for irreversible. Each later step adds its options to the target. An encoder
reject matrix in `src/encode/tests.rs` pins each refused option and input.

**Gate:** a session of 60 s per target finds nothing.

### P5.21 — Integration and the phase gate

**Work:** the scope.md encoding rows; roadmap.md; an encoder section in
correctness.md; openjpeg-departures.md (the levels clamp, no COM, the ratio,
the spare ROI plane, the POC sequences refused, and any difference found);
`docs/encoder-ics.md` (Annex F); an encoder table in performance.md;
resource-bounds.md (the encoder's allocations are bounded by the input,
which the caller already holds); README, `lib.rs` docs and CHANGELOG; the
RELEASING.md pre-release review.

**Gate (the Phase 5 exit):** all four gate parts pass over the full sweep;
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
  ROI shift and the POC volume checks; the parity, byte and prefix checks
  are the tests meant to catch them, so a survivor there is a gap.
- **New rounds for `docs/release-validation.md`:**
  - *Encoder cross-decode sweep:* the full sweep through `opj_decompress` and
    Grok, bit for bit on 5/3 and within the gap on 9/7, leaving out the
    combinations each tool decodes wrongly (bypass with ROI for OpenJPEG;
    ROI for Grok);
  - *Rate parity:* rerun `gen-encoder-parity.py` and compare with the
    committed figures;
  - *Encoder fuzz replay through OpenJPEG* (above);
  - *JP2 validity:* jpylyzer over every JP2 output;
  - *Kakadu, by hand and optional:* `kdu_expand` over the sweep. Its results
    go in the round's notes and nowhere else;
  - *Downstream consumer:* a GRIB2 template 5.40 message carrying our
    codestream in section 7, read by eccodes and fieldglass. eccodes decodes
    with OpenJPEG, so this checks the packing, not a third decoder.

## Out of scope for Phase 5

Refused with `Error::Unsupported`, never attempted: PSNR targets; the
digital cinema, IMF and broadcast profiles; `io::Write` output; threads and
SIMD (#301, #302); narrower input types (#303); HTJ2K encode and Part 2
(Phase 6). Visual weighting (CSF), or any quality model other than MSE, is
not planned. Image file readers (PNM, TIFF, PNG) stay outside the crate: the
input is samples.

## Prerequisites and open questions

- **#414** (decode orchestration out of `lib.rs` into the pipeline module) is
  in review as #447, so the encoder's orchestration does not grow `lib.rs`.
- **#142** (one coefficient type for both wavelets) comes first, since the
  forward path is the fourth copy that issue waits for.
- **#254** (the image origin on `Image` and `Header`) comes first, so an
  offset image round-trips through `&Image` with no extra option.
- **Whether our tier-1 bytes can match OpenJPEG's** is unmeasured (P5.5). The
  answer decides whether P5.7, P5.10 to P5.12 and P5.16 get byte comparisons
  or only round trips.
- **The cost of the spare ROI plane** (P5.17) is unmeasured. If it is large,
  the alternative is the H.1 minimum with ROI cases graded by this decoder
  alone, which would need its own decision.
- **The profile claims** (P5.13) take Profile 0 and 1 from the Part 1 text;
  the cinema, IMF and broadcast profiles stay out. That reading of "all of
  Part 1" may want confirming.
- **A CI job with OpenJPEG installed**, to run the cross-decode live instead
  of from digests, would be a CI edit for the maintainer. The plan does not
  need it.

## Sources

- ITU-T T.800 | ISO/IEC 15444-1: Annexes A to D, F and H, and the informative
  Annex J on rate control.
- ITU-T T.803 (02/2024) | ISO/IEC 15444-4:2024, free PDF at
  `https://www.itu.int/rec/T-REC-T.803-202402-I`: 6.3, 6.6, Annex D (D.1 to
  D.4, Figure D.1) and Annex F (Table F.1).
- ITU-T T.88 (08/2018) H.2: the MQ test sequence.
- OpenJPEG 2.5.0 source, tag `v2.5.0`: `tcd.c` (`opj_tcd_rateallocate` at
  line 443, `opj_tcd_makelayer` at 237), `j2k.c` (`opj_j2k_update_rates` at
  5260, the rate-of-1 rule at 8070, the `opj_j2k_write_*` marker writers),
  `dwt.c` (`opj_dwt_calc_explicit_stepsizes` at 1966,
  `opj_dwt_encode_stepsize` at 1092), `t1.c` (`opj_t1_encode_cblk`),
  `jp2.c`; and `opj_compress -h`.
- Grok's `grk_compress` manual page, the `-A` rate-control algorithms:
  `https://manpages.debian.org/testing/grokj2k-tools/grk_compress.1.en.html`.
- D. Taubman, "High performance scalable image compression with EBCOT",
  IEEE Transactions on Image Processing 9(7), 2000: PCRD-opt.
