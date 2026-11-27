# Conformance fixtures

Each fixture is a raw JPEG 2000 codestream (`<name>.j2k`, Annex A, no JP2 boxes)
paired with a sibling oracle snapshot `<name>.expected.json`. The conformance
harness decodes the codestream and compares its output against the snapshot. The
snapshot is what a trusted reference decoder (OpenJPEG's `opj_decompress`, or
eccodes for GRIB2-sourced files) produced, committed so the corpus is graded
offline without the reference decoder installed.

The JP2 corpora sit beside this one. The synthetic JP2 files and the ICC
oracles are below. The real-world `.jp2` files are in
[`jp2_real/`](jp2_real/README.md), each with its source, commit, SHA-256 and
oracle; they are excluded from the published crate, because their rights are
not confirmed. The ISO/IEC 15444-4 JP2 reader files and their references are
in [`conformance/`](conformance/README.md), with the HTJ2K conformance files in
`conformance/ht/`. The OpenJPH test codestreams are in
[`openjph/`](openjph/README.md). Both are excluded from the published crate.

The JP2 files with an ICC profile are ISO/IEC 15444-4 files 5, 7 and 8, at
`conformance/jp2/files/file5.jp2`, `file7.jp2` and `file8.jp2`. Their oracles
are in `conformance/jp2/icc_variants.json` (see "ICC to sRGB oracles" below).
The conformance corpus is excluded from the published crate.

## The corpus

Every fixture is the exact subset Phase 1 decodes: one component, one tile, one
quality layer, LRCP, no precincts. Each is within ISO/IEC 15444 Part 1 and
squarely inside the Phase 1 scope. The first three are GRIB2 §5.40 (`grid_jpeg`)
codestreams; the 9/7 one is an OpenJPEG re-encode of a GRIB2 grid (see the note
below the table).

| Fixture | Grid | Depth | Wavelet | Tolerance |
| ------- | ---- | ----- | ------- | --------- |
| `jpeg2000_regular_latlon`  | 16×31 | 16-bit unsigned | 5/3 reversible | `exact` |
| `eta_lambert_lossless`     | 93×65 | 13-bit unsigned | 5/3 reversible | `exact` |
| `regular_latlon_lossy`     | 16×31 | 16-bit unsigned | 5/3 reversible (rate-truncated) | `exact` |
| `eta_lambert_irreversible` | 93×65 | 13-bit unsigned | 9/7 irreversible | `absolute`, 2.0 |

The three 5/3 fixtures gate bit-exact: 5/3 is exact integer lifting, so any
difference from OpenJPEG is a bug. That includes `regular_latlon_lossy`, whose
codestream is lossy by rate truncation (fewer bit-planes) but whose decode is
still deterministic integer math. Only `eta_lambert_irreversible` uses the 9/7
float lifting, so it gates within a per-sample absolute bound: how far our 9/7
reconstruction may drift from OpenJPEG's, both decoding the same codestream, not
a compression budget. Measured drift is ≤ 1; the 2.0 bound leaves headroom.

No operational producer ships lossy 9/7 JPEG 2000 in GRIB2. HRRR and NDFD are
complex-packed, ECMWF is CCSDS, and eccodes' `grid_jpeg` encoder always uses the
reversible 5/3 transform regardless of `typeOfCompressionUsed`. So the only way
to exercise the irreversible path against the oracle is to re-encode a real grid
with OpenJPEG's irreversible (`-I`) mode; `eta_lambert_irreversible` is that
re-encode.

### Provenance and license

All four derive from small public GRIB2 fixtures already vendored in the sibling
`fieldglass` repo. This keeps the corpus tiny and regenerable offline, with no
network fetch and no large operational grids.

- `jpeg2000_regular_latlon` — the codestream from `fieldglass`'s
  `jpeg2000_regular_latlon.grib2`, itself an eccodes `grid_jpeg` repack of ECMWF's
  `regular_latlon_surface.grib2` test sample. eccodes and its samples are
  Apache-2.0.
- `eta_lambert_lossless` — eccodes `grid_jpeg` repack of `fieldglass`'s
  `eta_lambert_msg0.grib2` (first message of NOAA Eta output via the pygrib sample
  corpus). NOAA model output is U.S. government work in the public domain.
- `regular_latlon_lossy` — eccodes lossy (`typeOfCompressionUsed=1`) `grid_jpeg`
  repack of ECMWF's `regular_latlon_surface.grib2`. Still a 5/3 codestream: that
  eccodes option truncates the bit-stream, it does not switch to the 9/7
  transform. Apache-2.0.
- `eta_lambert_irreversible` — the samples of `eta_lambert_lossless` re-encoded
  with OpenJPEG's irreversible 9/7 transform (`opj_compress -I`). Same NOAA Eta
  grid, public domain; only the codestream's wavelet differs.
- `multicomponent_lossless` — three independent 8-bit components at unit
  sub-sampling, reversible 5/3. Synthetic, generated with OpenJPEG by
  `scripts/gen-multicomponent-fixtures.py`.
- `multicomponent_subsampled_lossless` — the same, sub-sampled 2×2, so the 23×15
  reference grid differs from each 12×8 component grid.
- `multicomponent_16bit_lossless` — four independent 16-bit components, 11×9,
  at unit sub-sampling (`opj_compress -i in.raw -o
  multicomponent_16bit_lossless.j2k -F 11,9,4,16,u -mct 0 -p LRCP -n 3 -b
  16,16`, OpenJPEG 2.5.0, from the script's big-endian planes). The
  `jp2_boxes` fuzz target pins it for the 16-bit RGB ICC conversion and the
  four-channel `cdef` seeds.
- `multilayer_lossless` — one component in three quality layers, the last
  lossless, so a full decode is bit-exact. Generated by
  `scripts/gen-multilayer-fixtures.py`.
- `multilayer_multicomponent_lossless` — three components in three layers, which
  crosses the two axes LRCP nests (`for layer { for resolution { for component }}`).
- `cblksty_restart_lossless` — the `restart` code-block style over two
  components in two quality layers, so a layer contributes several coding passes
  and one packet carries several codeword segments per block. `p0_12` is the
  corpus oracle for `restart` but is single-layer, so it never exercises that.
  Generated by `scripts/gen-cblksty-fixtures.py`.
- `cblksty_segsym_lossless` — `segmentation symbols` alone. The four uniform
  decisions closing each cleanup pass are part of the codeword, so a decoder
  that ignores the flag desynchronizes. This must be graded *without* `restart`:
  under `restart` each pass is its own segment, and the skipped symbols become
  harmless trailing bytes. See `docs/correctness.md`.
- `cblksty_segsym_truncated` — `segmentation symbols` with rate control
  (`opj_compress -F 1,64,3,7,u@1x1:1x2:1x2 -n 5 -d 38,17 -r 10 -M 32` on a
  128-byte raw image). Rate control cuts the last cleanup pass it includes
  for one code-block, which the encoder did not terminate, a few bytes
  short, so the decoder reads its symbol from the fill past the segment and
  gets `0b1001`. It must decode, as in OpenJPEG; the oracle is
  `opj_decompress`. The input was found by a random search over `-M 32 -r`
  encodes. Generated by `scripts/gen-segsym-truncated-fixture.py`, which
  embeds the input and checks the codestream's SHA-256.
- `zero_length_contribution`, `zero_length_contribution_bypass` — 8x8
  code-blocks in two layers whose contributions include coding passes with a
  length of zero (B.10.7). Each block's data is `opj_compress -n 1 -b 8,8`
  (with `-M 1` for the bypass one) of one 8x8 block of the script's image,
  in one lossless layer; the script writes the codestream by hand with each
  block's passes split over the two layers. In both, block 0 adds passes
  with no bytes in layer 0 and gets every byte in layer 1, so it decodes to
  the source (checked against `opj_decompress`), and block 1 has passes and
  no bytes at all, so it decodes from the `0xFF` fill. In the bypass one,
  block 0's layer 0 ends inside a raw segment with no bytes, and block 2
  gets its first segment whole and then a raw and an MQ segment with no
  bytes. The oracle is `opj_decompress -i <name>.j2k -o out.pgx`. Generated
  by `scripts/gen-zero-length-fixtures.py`.
- `cblksty_pterm_lossless` — `predictable termination` alone. It constrains the
  encoder, not the decoder, so this must reconstruct exactly what the default
  style does; the fixture pins that the flag is accepted and changes nothing.
- `cblksty_restart_pterm_segsym_lossless` — all three at once, which is `p0_02`'s
  style byte (`0x34`). Grades that they compose. It cannot grade `segsym`
  consumption, for the reason above.
- `cblksty_vcausal_lossless` — `vertically causal context` alone. The 3×3
  significance context of a stripe's bottom row excludes the next stripe, so it
  changes the MQ context of every affected decision; a decoder that ignores the
  flag reconstructs garbage. The corpus's own vcausal entries (`p1_02`, `p1_06`)
  are blocked on PPT, so this is the only bit-exact grade of the mode.
- `cblksty_reset_lossless` — `reset context probabilities` alone. The MQ context
  model reinitializes after every coding pass; again the corpus entries are
  blocked on PPT, so this synthetic vector is the bit-exact grade.
- `qcc_lossless` — per-component quantization overrides (QCC). Built by
  `scripts/gen-qcc-fixture.py` from `multicomponent_lossless.j2k`: QCD's guard
  bits and exponents are corrupted and a QCC restores the original values for
  each component. QCD is only a *default*, so the codestream stays valid and
  OpenJPEG decodes it to the same image — but only a decoder that honors QCC
  does. One that ignores it reads the corrupted defaults, gets the wrong `Mb`
  for every subband, and reconstructs garbage. No corpus entry grades QCC: both
  that carry one (`p0_13`, `p0_06`) also carry an RGN marker.
- `irreversible_flat_lossless`, `irreversible_lossy` — the 9/7 path, bit-exact
  against OpenJPEG. Nothing graded it before: `p0_09` is 9/7 and bit-exact but
  never exercises the case, and `eta_lambert_irreversible`'s tolerance absorbs an
  error of half a quantization step. That is how a double mid-point
  reconstruction bias survived (#101).
  `irreversible_flat_lossless` has **one resolution**, so the inverse DWT is the
  identity and it grades dequantization and the double-scale halving alone. It is
  coded at `-r 1`, so every bit-plane is present and every double-scale magnitude
  is odd — which is exactly the case where the old double bias *canceled itself*
  and looked correct. `irreversible_lossy` has two levels and a truncated rate,
  producing even magnitudes, where the bias does not cancel; it is the fixture
  the pre-#101 decoder actually fails. Generated by
  `scripts/gen-irreversible-fixtures.py`.
- `irreversible_16bit` — 9/7 at 16 bits: two 32x32 components of 3x5 blocks,
  every bit-plane coded, five decomposition levels. Component 0 swings over the
  full range, component 1 from a quarter to three quarters of it. Graded within
  5 of OpenJPEG, the largest difference measured on this fixture; its
  `two_invK` causes it (`docs/correctness.md`, "How close a decode must be"). Other
  content reaches more, depending on how its edges line up with the transform
  grid: a 256x256 checkerboard of 5x5 blocks reaches 7 at 16 bits. `tests/irreversible_16bit.rs`
  also checks that each component differs from OpenJPEG by more than 1, and
  that this decoder is the closer of the two to the encoder's input. On
  component 0 the output clamp hides any overshoot past 0 or 65535, so that
  check is weaker there; component 1 never clamps. Generated by
  `scripts/gen-irreversible-16bit-fixture.py`.
- `roi_maxshift_lossless` — the RGN maxshift (Annex H). Two components, only the
  first upshifted, so it also pins that the shift is per component. Reversible
  5/3 and one lossless layer, so the decode is bit-exact.
  **It does not grade the maxshift threshold.** Its coefficients are all at or
  above it, and none below; a decoder that dropped the threshold test and
  shifted unconditionally still passes. Measured: 337 coefficients above the
  threshold, 47 zero, none between. `p0_06` is the codestream with a real
  foreground and background, and it has none in the band where the rules
  differ. `roi_h1_band/` grades the threshold. Generated by
  `scripts/gen-roi-fixture.py`.
- `roi_h1_band/` — a lossless 8x8 maxshift stream (`SPrgn = 7`, no region, and a second file with `SPrgn = 31`)
  whose background coefficients sit in the band `2^(s-1)..2^s-1`, graded
  against the source image by `tests/roi_h1_band.rs`, not against
  `opj_decompress`, which zeroes the band. Provenance in
  `roi_h1_band/README.md`; generated by `scripts/gen-roi-h1-band-fixture.py`.
- `roi_bypass/` — the RGN maxshift with four bypass code-block styles, graded
  against the source image by `tests/roi_bypass.rs`, not against
  `opj_decompress`, which decodes them wrong. Provenance in
  `roi_bypass/README.md`; generated by `scripts/gen-roi-bypass-fixtures.py`.
- `poc_precedence/` — a POC in the main header and one in the tile-part
  headers, graded against the source image by `tests/poc_precedence.rs`, not
  against `opj_decompress`, which decodes two of them wrong. Provenance in
  `poc_precedence/README.md`; generated by
  `scripts/gen-poc-precedence-fixtures.py`.
- `poc_coverage/` — `opj_compress -POC` encodes whose volumes leave out
  packets of the tile, refused by `tests/poc_coverage.rs`, and covered
  controls graded against their sources. Provenance in
  `poc_coverage/README.md`; generated by `scripts/gen-poc-coverage-fixtures.py`.
- `layers/` — multi-layer codestreams decoded under
  `DecodeOptions::max_quality_layers` by `tests/max_quality_layers.rs`, graded
  against `opj_decompress -l N` at each cap, with SHA-256 oracles for five
  ISO/IEC 15444-4 codestreams. Provenance in `layers/README.md`; generated by
  `scripts/gen-layer-fixtures.py`.
- `quirk_tpsot_tnsot_lossless`, `quirk_restated_cod_qcd_lossless`,
  `quirk_mct_one_component_lossless` — three encoder mistakes OpenJPEG
  corrects: each tile's last tile-part with `TPsot == TNsot`, the main COD and
  QCD repeated in every later tile-part, and the color transform flag on one
  component. Each is an `opj_compress` encode rewritten to carry the mistake;
  the snapshot is `opj_decompress` 2.5.0's output, which equals the source
  image. Generated by `scripts/gen-encoder-quirk-fixtures.py`; see
  "Encoder mistakes OpenJPEG corrects" in `docs/openjpeg-departures.md`.
- `delimiters_sop_lossless`, `delimiters_eph_lossless`,
  `delimiters_sop_eph_lossless` — the SOP and EPH packet delimiters, alone and
  together, over two components in two quality layers. Generated by
  `scripts/gen-delimiter-fixtures.py`.
- `progression_rlcp_lossless`, `progression_rpcl_lossless`,
  `progression_pcrl_lossless`, `progression_cprl_lossless` — three components in
  three layers under each non-LRCP progression order, with one precinct per
  resolution. Generated by `scripts/gen-progression-fixtures.py`; the script
  explains why three of each axis is the minimum that separates the orders.
  PCRL and CPRL coincide here — separating them needs a precinct partition, which
  is what the fixtures below add.
- `precincts_lrcp_lossless` and its `_rlcp_`, `_rpcl_`, `_pcrl_`, `_cprl_`
  siblings — the precinct partition under each progression order: three
  components, three layers, three resolutions, 17 precincts per component. With
  more than one precinct the *position* axis stops being a degenerate counter, so
  RPCL, PCRL and CPRL have to enumerate precincts by sweeping the reference grid
  rather than counting. These are what separate PCRL from CPRL for the first time.
- `precincts_varied_sizes_lossless` — a different precinct size at each
  resolution (2^4, 2^4, 2^5, 2^6), the shape `p1_02` has. A decoder that reads one
  precinct size and applies it to every resolution passes all five above and fails
  this. It is also the **only** fixture that grades the code-block's cap to its
  precinct.
- `precincts_tiled_cprl_lossless` — a 3×2 grid of 48×48 tiles under a 32×32
  precinct lattice. The lattice is anchored at the canvas origin, not at the tile,
  so every tile but the first begins off-lattice and its **leading precinct is
  partial**. That is a separate branch in the positional sweep, and this is the
  only fixture that reaches it.

  All seven are generated by `scripts/gen-precinct-fixtures.py`. They carry the
  partition because the corpus cannot: `p0_11` is the one entry the partition
  alone unblocks, and it has a single resolution and a single precinct, so it
  grades only that `SPcod`'s precinct bytes are parsed. Every other corpus entry
  with precincts is held up by something else. See `docs/correctness.md`.
- `offset_odd_lossless` — a single component at an odd image origin (5, 3),
  isolating the inverse-DWT interleave parity: a sample is low-pass when its
  absolute reference-grid coordinate is even, so an odd origin flips which lifting
  branch index 0 of each band takes. `p1_01` covers this too but carries five
  layers and precincts on top.
- `offset_tiled_lossless` — an image origin (5, 3) *and* a tile origin (3, 2)
  under a 3×3 tile grid, so the leading tile row and column are partial. This is
  the shape of the corpus's `p1_05`, which is blocked on code-block styles the
  decoder does not have yet — so the offset-under-a-tile-grid case is graded here
  instead.
- `offset_subsampled_lossless` — an image origin (5, 3) on a 2×2 sub-sampled
  component, whose origin `ceil(5/2) = 3` is odd even though the image origin is
  not. That exercises the inverse-DWT parity path the `ceil`-division under
  sub-sampling produces, which the unit-sampled `p1_01`/`p1_07` and the other
  offset fixtures never reach. Its snapshot is the reference decoder's output
  verbatim, since a sub-sampled component's samples are not the input raster. All
  three are generated by `scripts/gen-offset-fixtures.py`.

The two multi-component fixtures are synthetic because the ISO/IEC 15444-4 corpus
cannot grade multi-component decoding on its own: every one of its
multi-component entries also needs a feature that is not decoded yet (a
progression other than LRCP, several quality layers, a tile grid, an image
origin, or the color transform). The generator script records the details,
including which OpenJPEG encoder flags do *not* work.

All sources are Apache-2.0 or public domain, compatible with this repo.

### Synthetic JP2 files

`jp2_*.jp2` files wrap a lossless codestream in hand-built JP2 boxes, each with
an `<name>.expected.json` snapshot graded bit-exactly by `tests/jp2_synthetic.rs`.
All are written by `scripts/gen-jp2-fixtures.py` with OpenJPEG 2.5.0
(`opj_compress`, `opj_decompress`); each snapshot's `provenance` records the
source codestream and the oracle command.

| Fixture | Source codestream | Boxes under test | Oracle |
| ------- | ----------------- | ---------------- | ------ |
| `jp2_palette_8bit` | `multilayer_lossless.j2k` | `pclr` 256 x 3 at 8 bits, identity `cmap` | `opj_decompress -i jp2_palette_8bit.jp2 -o out.pgx` |
| `jp2_palette_12bit` | `multilayer_lossless.j2k` | `pclr` 256 x 3 at 12 bits (two-byte entries) | `opj_decompress` on the `.jp2` |
| `jp2_palette_mixed` | `multicomponent_lossless.j2k` | `cmap` channel 0 direct from component 2, channels 1-2 palette from component 0 | `opj_decompress` on the `.jp2` |
| `jp2_palette_clamp` | a signed 8-bit plane coded by the script (`opj_compress -i signed.raw -o signed.j2k -F 32,24,1,8,s -n 3`) | `pclr` of 100 entries over indices -128..127 (clamped) | `opj_decompress` on the `.jp2` |
| `jp2_palette_corrected` | `multilayer_lossless.j2k` | one component, `cmap` channel 1 direct: OpenJPEG's "Trying to correct" identity mapping | `opj_decompress` on the `.jp2` |
| `jp2_cdef_rgba` | a 16x12 RGBA PNG generated by the script, coded with `opj_compress -i rgba.png -o rgba.jp2 -n 3` | `opj_compress`'s own file: `cdef` of three colors and one opacity channel (nothing moves) | `opj_decompress` on the `.jp2` |
| `jp2_cdef_alpha_first` | the `jp2c` of `jp2_cdef_rgba` | `cdef` with component 0 as opacity and 1..3 as colors 1..3: each color entry swaps, so the output is components 1, 2, 3, 0 | `opj_decompress` on the `.jp2` |
| `jp2_cdef_reversed` | `multicomponent_lossless.j2k` | file2's `cdef` (components 0, 1, 2 are colors 3, 2, 1): the output is reversed | `opj_decompress` on the `.jp2` |
| `jp2_cdef_unspecified` | `multicomponent_lossless.j2k` | `cdef` with every `Typ` and `Asoc` 65535 (the ECW SDK's layout): nothing moves | `opj_decompress` on the `.jp2` |
| `jp2_cdef_palette` | `multilayer_lossless.j2k` | the 8-bit palette with the identity `cmap`, then the reversed `cdef` over the palette's channels | `opj_decompress` on the `.jp2` |
| `jp2_palette_subsampled_big_first`, `jp2_palette_subsampled_small_first` | a two-component pair coded by the script (`opj_compress -i sub.raw -o sub.j2k -F 24,16,2,8,u@1x1:2x2 -n 2`): component 0 full resolution, component 1 sub-sampled 2x2 | one palette and one direct channel from components of different sub-sampling, in both orders | OpenJPEG crashes on the first (`free(): invalid pointer`) and leaves 287 of 384 direct-channel samples unset on the second, so: `opj_decompress` on the bare codestream, then the palette applied by table lookup in the script. Each channel keeps its own grid (a deliberate deviation, #162) |
| `jp2_sycc_420_odd`, `jp2_sycc_422_odd` | three planes coded by the script (`opj_compress -i sycc.raw -o sycc.j2k -F <w>,<h>,3,8,u@1x1:<d>:<d> -d 5,3 -mct 0 -n 2`, `<d>` 2x2 for a 14x12 image, 2x1 for 13x5): luma full resolution, chroma sub-sampled, origin (5, 3) | `colr` sYCC; for `ColorOutput::Srgb`, chroma replication at an odd origin | as coded: `opj_decompress -c K` on the bare codestream, one component at a time, so its sYCC heuristic stays out. A second snapshot, `<name>.srgb.json`, is `opj_decompress` on the `.jp2` (sYCC converted); `tests/jp2_srgb.rs` grades the sRGB output within 1 of it (OpenJPEG's formula differs, a deliberate deviation, #165) and `src/jp2/srgb_tests.rs` matches it exactly with OpenJPEG's formula. A third, `<name>.srgb_r1.json`, is `opj_decompress -r 1` on the `.jp2`; `tests/jp2_sycc_replication.rs` grades the sRGB output at `with_resolution_reduction(1)` within 1 of it |
| `jp2_sycc_420_even`, `jp2_sycc_422_even` | the same script and command with `-d 6,2`, for a 13x11 (4:2:0) and an 11x5 (4:2:2) image | `colr` sYCC; chroma replication at an even origin. The `jp2_boxes` fuzz target pins the 4:2:0 codestream | as for the odd ones: `.expected.json` as coded, `.srgb.json` and `.srgb_r1.json` from `opj_decompress` on the `.jp2` (without and with `-r 1`), graded within 1 by `tests/jp2_sycc_replication.rs` |
| `jp2_sycc_420_tiled` | the same script and command with `-d 5,3 -t 8,8`, for a 14x12 4:2:0 image in six 8x8 tiles | `colr` sYCC; a tile that is not the whole image, for `ColorOutput::Srgb` | as coded: `opj_decompress -c K` on the bare codestream (`.expected.json`); no sRGB snapshot, as `tests/decode_window.rs` checks only that sRGB output refuses a tile and equals the plain decode on the whole-image window |
| `jp2_metadata_boxes` | the `jp2c` of `jp2_cdef_rgba` | the metadata boxes no other file has: `colr` sRGB, then `METH` 3 (ignored), then a misplaced sYCC `colr` at the top level after `jp2h` (counted); `cdef` with channel 3 premultiplied opacity for color 2 (nothing moves); `res ` with `resc` `[300, 7, 254, 3, -1, 3]` and `resd` `[72, 1, 96, 5, 2, -128]`; `ihdr` `IPR` 1, a `jp2i` before and after the codestream, and a `uinf` with two UUIDs and a `url `. Its whole `jp2` section is written by `scripts/gen-jp2-box-snapshots.py` | `opj_decompress` on the `.jp2` |
| `jp2_signed16` | a signed 16-bit plane coded by the script (`opj_compress -i signed16.raw -o signed16.j2k -F 24,16,1,16,s -n 3`), samples from -30000 to 29967 | `colr` gray (`EnumCS` 17); negative 16-bit samples | `opj_decompress` on the `.jp2` |

They are ours, built over the Apache-2.0 / synthetic codestreams above.

### Channel fixture

`channels/third_subsampled.j2k` (302 bytes) is three 8x8 8-bit components with
the third sub-sampled 2x2 (the first two are full size), lossless, from
planes `((x*29 + y*13 + k*71) * 7) mod 256` for component `k`, coded with
`opj_compress -i in.raw -o third_subsampled.j2k -F 8,8,3,8,u@1x1:1x1:2x2 -n 2
-mct 0` (OpenJPEG 2.5.0). `src/jp2/srgb_tests.rs` wraps it in a JP2 file
declared grayscale with no `cdef` and requires `ColorOutput::Srgb` to pass it
through as coded, as `opj_decompress` writes it.

`channels/luma_1x2.j2k` (264 bytes) is the same planes at 8x8 with component
sampling 1x2, 2x1, 2x1, coded with `opj_compress -i in.raw -o luma_1x2.j2k -F
8,8,3,8,u@1x2:2x1:2x1 -n 2 -mct 0`. Its first component has unequal x and y
sampling, so a grayscale file around it passes through (OpenJPEG makes no
sYCC guess).

`channels/fourth_2x1.j2k` (393 bytes), `channels/fourth_1x2.j2k` (392
bytes) and `channels/fifth_subsampled.j2k` (454 bytes) use the same planes
at 8x8 for four and five components, the last sub-sampled (2x1, 1x2, 2x2),
coded with `opj_compress -i in.raw -o <name>.j2k -F 8,8,4,8,u@1x1:1x1:1x1:2x1
-n 2 -mct 0` (`@1x1:1x1:1x1:1x2` for `fourth_1x2`; `-F
8,8,5,8,u@1x1:1x1:1x1:1x1:2x2` for `fifth_subsampled`) with OpenJPEG 2.5.0,
and checked lossless against the planes. `tests/jp2_icc.rs` wraps them in
JP2 files with an RGB ICC profile: a sub-sampled fourth channel makes
OpenJPEG skip the conversion, a sub-sampled fifth does not. The expected
planes are hashes of `opj_decompress` output on those JP2 files.

Six more use the same planes at 8x8 for three components, coded with
`opj_compress -i in.raw -o <name>.j2k -F 8,8,3,8,u@<sampling> -n 2 -mct 0`
(OpenJPEG 2.5.0) and checked lossless against the planes:

| File | Bytes | Sampling |
|------|-------|----------|
| `channels/second_2x1.j2k` | 317 | `1x1:2x1:1x1` |
| `channels/second_2x2.j2k` | 301 | `1x1:2x2:1x1` |
| `channels/chroma_2x1_2x2.j2k` | 275 | `1x1:2x1:2x2` |
| `channels/luma_2x2_chroma_2x1.j2k` | 246 | `2x2:2x1:2x1` |
| `channels/chroma_1x2.j2k` | 288 | `1x1:1x2:1x2` |
| `channels/all_2x2.j2k` | 217 | `2x2:2x2:2x2` |

All but `chroma_1x2` have the shape `opj_decompress` guesses sYCC from, and
none is 4:2:0 or 4:2:2, so OpenJPEG cannot convert them as sYCC and writes
them as coded. `src/jp2/srgb_tests.rs` wraps them declared grayscale and
requires `ColorOutput::Srgb` to pass them through. `tests/jp2_icc.rs`
wraps them with the `gamma22` profile from `icc/profiles.json`: all but
`all_2x2` fail OpenJPEG's AFL test and pass through, `all_2x2` converts. The
expected planes are hashes of `opj_decompress` output on those JP2 files.

### Budget fixture

`budget/flat_1024_lossless.j2k` (328 bytes) is a flat 1024x1024 8-bit
one-component lossless codestream, made with `head -c 1048576 /dev/zero >
flat.raw` and `opj_compress -i flat.raw -o flat_1024_lossless.j2k -F
1024,1024,1,8,u -n 3` (OpenJPEG 2.5.0). `docs/resource-bounds.md` cites its
tier-1 work.

`budget/flat_2048_lossless.j2k` (926 bytes, SHA-256
`0395675bb92fbc2d65f4f09201e148327a42c5b4152bed4f0bcefb426e40b417`) is the
same at 2048x2048: `head -c 4194304 /dev/zero > flat.raw` and `opj_compress
-i flat.raw -o flat_2048_lossless.j2k -F 2048,2048,1,8,u -n 3` (OpenJPEG
2.5.0). `src/jp2/tests/matrix.rs` wraps it in a JP2 file with a 255-column
palette to test the sample limit at full size (255 x 2^22 samples, over
2^29) and at resolution reduction 2.

### Sample-limit fixtures

`scripts/gen-sample-limit-fixtures.py` writes both files in `sample_limit/`
with `opj_compress` 2.5.0; `tests/sample_limit.rs` uses them.

| File | Size | SHA-256 | What |
| ---- | ---- | ------- | ---- |
| `rgb_6000x4000.jp2` | 148291 | `511e18920ffee1d1e5786fdcb85f42ec6bf4256cf0e76d5ec354656c99d08b01` | 6000x4000 8-bit RGB, lossless at `opj_compress` defaults: flat blocks (72 M samples) |
| `flat_32769x16385_tiled_lossless.j2k` | 27164 | `db4c24c913fd605c2897904eb1261e4644322f5b8d102aea7af56681893eaac7` | 32769x16385 8-bit gray, all zero, `-n 4 -t 4096,4096` (536920065 samples, just over 2^29; a 9x5 tile grid) |

The script checks that `opj_decompress` decodes `rgb_6000x4000.jp2` to exactly
the picture it was coded from, so the test compares the decode with the
picture's formula. The `opj_decompress` planes, one byte a sample in raster
order, hash to `bb6fd7344cded7db2f53589600d9aeb752d1df3783d6e196cb5f00c1127cf32a`
(red), `829c2fd355e456f0914c2e5dd8be6fb4dba3e4542e69395706b3a95dee320d42`
(green) and `d797e5ddae6828e3152f0fb641999827388e2f165f70e2cd9cebd9b3d5bd7b58`
(blue). `opj_decompress -r 3` decodes the flat file to 4097x2049 zeros
(checked: every sample is 0).

### ICC to sRGB oracles

`scripts/gen-icc-oracles.py` records what `opj_decompress -force-rgb`
(OpenJPEG 2.5.0 with LittleCMS 2.14; `-p 8S` for 8-bit input) gives for
ICC-tagged files, as one SHA-256 per output plane (the samples in raster
order, each a 4-byte big-endian signed integer). `tests/jp2_icc.rs` rebuilds
each file and requires the same hashes from `ColorOutput::Srgb`.

| Snapshot | Files | Cases |
| -------- | ----- | ----- |
| `conformance/jp2/icc_variants.json` | ISO/IEC 15444-4 files 5, 7 and 8, the ICC profile patched in place | rendering intents 0 to 3; intent 3 with `wtpt` D65, (1.2, 1.1, 1.0) and (0.9, 0.95, 0.7). 8-bit output for files 5 and 8, 16-bit for file7 |
| `icc/profiles.json` | `icc/rgb8_lossless.j2k` (256x256, R = x, G = y, B = (37x + 91y) mod 256), `icc/rgb16_lossless.j2k` (64x64 random), `icc/gray8_lossless.j2k` (64x64, every gray level then random), each holding black and white, coded by the script with `opj_compress -F <size>,<size>,<n>,<depth>,u -n 3` and checked lossless against its source samples | 161 RGB profiles, each at 8 and 16 bits, and 27 monochrome ones, all built by `scripts/gen-icc-oracles.py`, each in its own `colr` box: 63 RGB and 10 monochrome edge cases (degenerate, descending and noisy curves, every parametric type, extreme matrices and white points, black point compensation, the gamma 1.6 threshold, and the PR #184 review's 24 malformed profiles); 15 RGB on either side of LittleCMS's header checks (signature, version, tag count, duplicate and skipped tags, declared size); 25 RGB and 4 monochrome on how LittleCMS reads intents, legacy tag types and tags that run short; and 58 RGB and 13 monochrome on device classes, connection and color spaces, lookup tables and missing tags. `docs/srgb-output.md` lists them. A case LittleCMS refuses is recorded as `unconverted` |

The snapshots hold hashes only, so a mismatch names the plane but not the
samples; regenerate the converted output with the script's command to see
them.

#### Exhaustive check against LittleCMS

`scripts/check-icc-exhaustive.py` is an optional check that is not part of
`cargo test`. It runs the same cases as the snapshots through the decoder and
through the system LittleCMS (`liblcms2.so.2`, by ctypes), with the arguments
OpenJPEG passes, and counts the pixels where they differ: the full 8-bit RGB
cube (2^24 colors) for each RGB profile on an 8-bit codestream, a fixed-seed
sample of 2^20 pixels for each on a 16-bit one, and all 256 levels for each
monochrome profile, in the format OpenJPEG picks from the profile's color
space. The bar is 0 mismatches on every case. Where OpenJPEG leaves the
samples as coded, the script checks the decoder gives them back unchanged;
where the decoder declines with `Error::Unsupported`, the case must be one of
`REFUSED_BY_US` in `tests/jp2_icc.rs`. Any other result is a SURPRISE and the
script exits non-zero.

```sh
nice -n 19 python3 scripts/check-icc-exhaustive.py [--only SUBSTRING] [--work DIR]
```

It builds `examples/icc_convert.rs` with `cargo build --release`, makes its
three input codestreams with `opj_compress`, and needs the conformance corpus
for the `icc_variants.json` cases. A full run decodes a 16-million-pixel image
for each 8-bit case, so expect around twenty minutes of one core; `--work`
keeps the generated codestreams between runs. The oracles assume LittleCMS
2.14 (Ubuntu 24.04's `liblcms2-2`) and OpenJPEG 2.5.0 on x86-64; the script
prints the library version it loaded.

### Benchmark fixtures

`bench/grib2_shape_1440x721_u16.j2k` is a synthetic codestream for
`benches/decode.rs`, the size and shape of an operational GRIB2 template 5.40
message: 1440x721, one 16-bit component, one tile, 5/3, 5 levels, 1 layer,
64x64 code-blocks, LRCP, maximum precincts, no SOP, EPH, TLM or PLT. The
samples are a smooth field plus Gaussian noise from a fixed seed (20261113);
no external data. It is 692 KB.

```sh
python3 scripts/gen-grib2-shape-bench-fixture.py
# which runs, on the raw big-endian u16 samples:
#   opj_compress -F 1440,721,1,16,u -n 6 -b 64,64 -p LRCP -r 3
#   opj_decompress -i grib2_shape_1440x721_u16.j2k -o out.pgx
```

Both tools are OpenJPEG 2.5.0. `grib2_shape_1440x721_u16.expected.json` holds
the FNV-1a digest of `opj_decompress`'s samples (little-endian `u16`, raster
order). `tests/bench_fixture.rs` requires our decode to match it exactly.

`bench/small_blocks_128_b4.j2k` (17095 bytes, SHA-256
`f0f7e7559233de0855122c6ba01e986b982464d5583959151c795d9e227d8993`) is a
decode of many small code-blocks for `tests/tier1_alloc.rs`: 128x128, one
8-bit component, one tile, 5/3, 5 levels, 1 layer, 4x4 code-blocks, LRCP,
lossless. The samples are a diagonal ramp plus uniform noise from a fixed
seed (20261115); no external data.

```sh
python3 scripts/gen-small-block-fixture.py
# which runs, on the samples as a PGM:
#   opj_compress -n 6 -b 4,4 -p LRCP
#   opj_decompress -i small_blocks_128_b4.j2k -o out.pgm
```

Both tools are OpenJPEG 2.5.0. `small_blocks_128_b4.expected.json` holds the
FNV-1a digest of `opj_decompress`'s samples (bytes, raster order), which the
test requires our decode to match.

### Packed header overrun fixtures

`scripts/gen-packed-overrun-fixtures.py` writes the files in
`packed_overrun/` byte by byte; `tests/packed_overrun.rs` uses them. Each is
a 1x1 image, one 8-bit component, no decomposition level, one packet whose
header is packed in a `PPT` or `PPM` marker, and a body of seven 0x00
bytes. `ppm_two_parts.j2k` adds an empty second layer in a second
tile-part, and a `COM` that sizes the first so that the prefixes
`rust_j2k::fuzz::tile_parts_walks_agree` cuts end in it and just after it
(`tests/tile_parts_walks_seeds.rs` runs that check on it). The script runs
`opj_decompress` (OpenJPEG 2.5.0) on each file and checks the result in the
table.

| File | Size | SHA-256 | Packed header | `opj_decompress` |
| ---- | ---- | ------- | ------------- | ---------------- |
| `ppt_short.j2k` | 94 | `a4f4d3a6d07773bd20a0eeb965b3ee5062be5209e9c04265ad974e345dee6b25` | `PPT`: `C4`, one bit short | 128, no error |
| `ppt_ff_nostuff.j2k` | 95 | `dbae16d2e6e86a450adc45bd98eb44539d845775535c1bcd97cc9ca0b47515f9` | `PPT`: `C4 FF`, no stuffed byte after the 0xFF | refused |
| `ppm_short.j2k` | 98 | `9afba12af3b58b11ace07073ce14d295ca69718b80265e297914a091da4b709f` | `PPM`: `C4`, one bit short | 128, no error |
| `ppt_whole.j2k` | 96 | `92405a6bd86b72739596d19973d334283281ee4549f781195f8d8c55db2e9eaa` | `PPT`: `C4 FF 00`, whole | 176 |
| `ppm_whole.j2k` | 100 | `9a07d9e65b8af391ac0bd3231e9efc8a60bcd5c2f883059b818e4790386d7639` | `PPM`: `C4 FF 00`, whole | 176 |
| `ppm_two_parts.j2k` | 129 | `3497fa81a3f244deb3953a351b044e63ab0547efa559ee7e8a4b3f1c57a4814d` | `PPM`: `C4 FF 00` and `00`, two layers in two tile-parts | 176 |

The first three are refused with `strict` on and off. The two whole files
are the controls and decode to 176, as OpenJPEG gives.

### HT VLC stuffing-bit fixtures

`ht_vlc_stuffing/` holds 16 pairs of one-block HT codestreams, cleanup pass
only: each block with a set VLC stuffing bit, and with the bit cleared.
`tests/ht_vlc_stuffing.rs` checks them against OpenJPEG's digests in
`manifest.tsv`. Provenance, and the differential they come from, in
[`ht_vlc_stuffing/README.md`](ht_vlc_stuffing/README.md); generated by
`scripts/gen-ht-vlc-stuffing-fixtures.py`.

### Override order fixtures

`override_order/` holds four streams with a COC and QCC per component placed
before or after a wrong COD and QCD, in the main header and in a tile-part
header. See [`override_order/README.md`](override_order/README.md).

### Incremental decoder fixtures

`incremental/spent_bound_tier1_then_eph.j2k` (3211 bytes) and
`spent_bound_tier1_then_eph_2.j2k` (1872 bytes) are inputs the CI fuzz
smoke of the `incremental_decode` target saved on 2026-11-19
(`crash-2d21f1a434324b7664573dfd24881ce43a42c563` and
`crash-d6b0adb60e7e6ad3d4b2a1fcb33a4c1bdadc5fa1`, each the SHA-1 of its
file), committed as found (#485). Each is a 12 x 12 HTJ2K codestream of
three components in 3 x 3 tiles, made by the fuzzer, so it has no license
restriction. `decode_with` fails in the block decoding of one tile (an HT
suffix length `Scup` out of range); a later tile's packets lack the `EPH`
its `COD` signals. `tests/incremental.rs` checks that a refine refused for
what earlier refines spent waits on `BoundSpent` rather than give the later
tile's error. Each file starts with `SOC`, so the fuzz target reads it with
all-zero control bytes: `cargo +nightly fuzz run incremental_decode
tests/fixtures/incremental/spent_bound_tier1_then_eph.j2k` replays it.

`incremental/segsym_tiles_flipped.j2k` (4141 bytes) is a 64 x 64 gray
image in 16 x 16 tiles with segmentation symbols, with one byte of tile 0
flipped (#487). The input is an 8-bit PGM written by Python, the sample at
`(x, y)` being
`int(128 + 60*sin(0.37*x)*cos(0.23*y) + (x*y) % 37) & 255`, encoded with
`opj_compress -i in.pgm -o segsym_tiles.j2k -t 16,16 -M 32 -n 3`
(OpenJPEG 2.5.0). Byte 200 is then XORed with `0xFF`; `tests/incremental.rs`
flips it back for the file as encoded. `decode_with` fails on the
segmentation symbol of a block of tile 0. Under a code-block cap set by
hand, the tests check that a refine refused for what earlier refines
spent, whose count of the later tiles passes the whole code-block bound,
waits on `BoundSpent` rather than a `WorkLimit` that would stand in for
that error, and that a refine not cut before tile 1 waits on the one-shot
`WorkLimit` once the byte is flipped back.

`incremental/segsym_wide_flipped.j2k` (5914 bytes) is a seed of the
`incremental_decode` fuzz target that reaches the same path under the
target's own cap of 2^22 sample-passes, which the tests above set by hand;
`src/incremental/tests.rs` also uses it for a count past the cut that
spans several tiles. The input is a 4096 x 1152 8-bit PGM of the value 100, whose
top-left 64 x 64 samples are `100 + (7*x + 13*y) % 23`, encoded with
`opj_compress -i wide.pgm -o segsym_wide.j2k -t 256,288 -M 32 -n 4`
(OpenJPEG 2.5.0). Its 64 flat tiles each cost little to code but over
2^18 sample-passes under the cap, which charges 4 for each sample, so a
decode of the file as encoded is refused under the cap at tile 12. Byte
3028, in tile 11, is then XORed with `0xFF`, so `decode_with` fails in
tile 11 first. The byte was picked so that no checked refine of the
target falls between the error and tile 12, which would stop the run at
the error, and one due refine past tile 12 draws a probe cut at the
erroring tile. Replay it with
`cargo +nightly fuzz run -O incremental_decode tests/fixtures/incremental/ -- -runs=0`.
The file as encoded is not committed: decoding its 4.7 million samples
would slow every sweep of the fixtures. Both files are made here, so they
have no license restriction.

`incremental/bound_spent_stands_in_for_a_tile_cap_refusal.seed` (383 bytes,
SHA-256 `e9c5b2646f952ccb18178bf91707a2d1b25f1718ed194bb7b814fac1e1ba3077`)
is a seed of the `incremental_decode` fuzz target for a refine that waits on
`BoundSpent` where `decode_with` is refused by a tile's precinct cap (#508).
It is a 1036 x 1024 gray codestream, flat at 100, from
`opj_compress -i flat.pgm -o base.j2k -d 4,0 -t 520,1024 -n 4 -r 1`
(OpenJPEG 2.5.0; the PGM is `P5 1036 1024 255` with every sample 100). Tile 0
is 516 samples wide, and under the target's cap its samples cost 4
sample-passes each, over half the bound of 2^22. Tile 1's tile-part gets a
`COD` of its own (no wavelet levels, 1 x 1 precincts) in front of `SOD`, with
`Psot` raised to match, so it holds 532480 precincts (520 x 1024), over the
2^18 a tile may. The main header's checks pass it, so `decode_with` decodes
tile 0 and is refused at tile 1 as `LimitKind::TilePrecincts`. The
`BoundSpent` arm is not reached by the decoder's own next refine, which waits
on the sample limit for those precincts, but through the probe decoder
(`incremental_with_code_block_work_spent`, by the target's `hash % 8` draw of
the work spent): with the bound spent, its refine is cut at tile 0 and counts
on to tile 1. A `COM` of 47 bytes (`(7 * i + 47) & 0x7F | 0x20` for byte `i`)
in front of the first `SOT` picks the pieces. It pins that `BoundSpent` may
stand in for the cap kinds of a tile, as for any error of a later tile: an arm
that excluded them would panic on it. `tests/incremental.rs` replays it with
the bound spent. It starts with `SOC`, so the target reads it with all-zero
control bytes. Replay it with the
command above. It has no license restriction.

`incremental/at_most_samples_sticky.seed` (94 bytes, SHA-256
`fcb2b633e51d7e1755ccd7a0597008d17b2d9e5666f95fdc8db41af3fde2d269`) is a seed
of the `incremental_decode` fuzz target for the `AtMost` control bit (#494): six
control bytes (`80 00 00 00 00 00`) in front of a 1024 x 1024 one-tile
codestream with no packets, made by `synth::empty_tiles` here, so it has no
license restriction. A 7-byte `COM` segment (`FF 64 00 05 00 01 14`, `Lcom` 5)
in front of the `SOT` changes its hash, which picks the ceiling and the pieces.
The hash of the file mod 4 is 3, so the target draws half of the 1048576
samples the file declares. The due refines are at 16, 37 and 79 bytes: the one
at 79 has the header and gives the sticky `Samples` error, equal to the
one-shot decode's, and the one at 88 repeats it. (A `COM` data byte of `00`
also draws 3, but its due refines at 22 and 48 both fall before the `SOT`, so
none gives the error.) Any other draw is the whole image, which it admits, so the
seed is a refusal only for this hash (#508 added the `COM`; the 87-byte seed
drew a refusing ceiling under the old draw). The file has no `.j2k`
extension, so the sweeps of the fixtures skip it. Replay it with
`cargo +nightly fuzz run -O incremental_decode tests/fixtures/incremental/ -- -runs=0`.

`incremental/at_most_work_bound_waits.seed` (604 bytes) is the control bytes
`A0 00 00 00 00 00` (`AtMost` and a reduction of 1) in front of
`work_bound/cprl_tall.j2k`. Under `AtMost` a refusal by a work bound still
grows with the bytes, so the target must expect the refine to wait where the
one-shot decode is refused, as under any other sample limit. Each of its
decodes is refused at the packet work bound, so the target's packet ledger
(`fuzz/common/budget.rs`) ends the execution after that first refine and its
one-shot decode: about 2 to 6 s to replay under ASan, where it took 17 to
31 s before the ledger. Replay it with the command above.

`incremental/t1bomb_row_128_8.seed` (331 bytes, SHA-256
`8ee10b73fa03f98398a482e04bd39b8d706388f625a301012ab8b539aa5f7482`) is a
seed of the `incremental_decode` fuzz target for a count past the whole
code-block bound where `decode_with` is refused by that bound too (#493),
which `segsym_wide_flipped.j2k` does not reach: there `decode_with` fails in
tile 11 first. It is written from scratch by
`scripts/gen-tier1-work-bound-fixtures.py` as `t1bomb(128, 6, 88, 8)` (no
encoder, no input image, so no license restriction): a row of eight
128 x 128 tiles of one 8-bit component, each the tile of
`tier1_work_bound/t1bomb_1024_64_88.j2k` in small. Under the target's cap
each tile costs 1507328 sample-passes, so tiles 0 and 1 fit the bound of
2^22 and tile 2 passes it. A probe with the bound spent is cut at tile 0
and counts to tile 2; the target and `tests/incremental.rs` check that
its wait gives the tile and work `decode_with` gives. It starts with `SOC`,
so the target reads it with all-zero control bytes, and its extension keeps
it out of the sweeps of the fixtures. Replay it with the command above.

`incremental/tile_error_*.seed` are six seeds of the `incremental_decode`
fuzz target: four for an error in a tile that a later tile-part's fault
replaces (#605), and two (`*_jp2_then_jp2c`, `*_jp2_then_xlbox`) for a
tile's error in a JP2 file that is final once no box after the codestream
can be read (#654). `incremental/held_box_cut_colr_jp2.seed` is one for
a box after the codestream that the input ends inside (#642). They are
built by `seeds()` in `tests/tile_error_final.rs` (no encoder, no input image, so
no license restriction), which checks that the committed bytes are its
output; `J2K_BLESS_TILE_ERROR_SEEDS=1` writes them again. Without them the
target did not catch a refine that never waits on such an error in 1.1M
runs. The target decodes only on a refine due under its doubling rule, so
each seed puts 512 bytes of comment (a `COM` segment, or a `free` box or
a long `colr`) between where the error comes and where it is replaced,
which a due refine cannot step over. With `Ledger::hold` made to hold
nothing, each #605 and #642 seed fails on its first replay (#645); without
that padding, only `tile_error_lblock_then_coc.seed` did. The two #654
seeds put their 512 bytes after the box that settles the file instead.

- `tile_error_reduction_then_cod.seed` (649 bytes, SHA-256
  `f3f915ac8bccd8133cb15b43e4c3b691b619968df64af6fb7ee227ab712b2720`):
  the control bytes `20 00 00 00 00 00` (a reduction of 1) in front of a
  64 x 64 tile whose own `COD` gives no levels, with a later tile-part's
  `COM`, then a `COD` that differs.
- `tile_error_lblock_then_coc.seed` (636 bytes, SHA-256
  `f9edf66fd5ff052bd6e0b86b11046c32a4cf537412d30ae5dd9c66d45947ad03`): a
  4 x 4 tile whose packet header has an Lblock indicator longer than any
  Lblock, with a later tile-part's `COM`, then a `COC` for component 5 of
  1.
- `tile_error_lblock_then_cod.seed` (639 bytes, SHA-256
  `09695eba7435cf107cf5ddc122fc8ea4fdda482816521a3da9628e6034a3ad2b`): the
  same packet, with a later tile-part's `COM`, then a `COD` that differs.
- `tile_error_lblock_jp2_then_colr.seed` (724 bytes, SHA-256
  `6b836027e224c1b10e1184376735fec3819613aadeb6d1509e926bb781dc0660`): the
  packet's codestream in a JP2 file, followed by a `free` box and a `colr`
  box too short to hold its fields.
- `tile_error_lblock_jp2_then_jp2c.seed` (714 bytes, SHA-256
  `88798e159c18228df83e84ace31ddeb78aa7bf3d708c9eb4fec05f6f60337b6c`):
  the packet's codestream in a JP2 file, followed by a second `jp2c` box of
  512 zero bytes. OpenJPEG's walk after the codestream stops at its header,
  so the tile's error is final from there; the bytes after it are padding
  for a due refine. With the JP2 file never settled
  (`Jp2Prefix::settled` always false), it fails on its first replay.
- `tile_error_lblock_jp2_then_xlbox.seed` (722 bytes, SHA-256
  `ece98fc4f97ad3a8f8a76ee7cb5d4b04fd3aeb6470f22bfbfef00e3b1a387934`):
  the same, followed by an `xml ` box whose `XLBox` is 2^32, then 512 zero
  bytes. It fails on its first replay under the same mutation.
- `held_box_cut_colr_jp2.seed` (708 bytes, SHA-256
  `474281b3d7c6475836e45b2adfc9cbfa9309f9be384cb67495a29ddf03fada36`): a
  4 x 4 tile of one empty packet in a JP2 file, followed by a `colr` of
  519 bytes. OpenJPEG ignores its contents, as the header's `colr` was
  read, so the file decodes; a prefix that ends inside it is refused as
  cut short.

All but the first start with `SOC` or the JP2 signature, so the target
reads them with all-zero control bytes. Their extension keeps them out of
the sweeps of the fixtures. Replay them with the command above.

### Retry ladder fixtures

`retry_ladder/tile_precincts.j2k` (98 bytes, SHA-256
`375d754c986a77896de2c9b6926d365856b2a848451fe3e07613804c8593de94`) is
the input that panicked the `retry_ladder` target, written out from the
hex in #506 (found by batch critique 10, committed as reported). It is a
1040 x 512 gray codestream of two 520 x 512 tiles whose
`COD` gives 1 x 1 precincts, so each tile holds 266240 precincts, over the
per-tile cap of 2^18 at the default sample limit, and the tiles carry
empty data. The default decode refuses it as `LimitKind::TilePrecincts`
with the retry `Raised(2)`. That retry passes the cap, and the decode
then ends in the truncation error `the data of tile 0 ends before the last
packet`, as the options with no limits do. `tests/work_limit.rs` checks
it. It starts with `SOC`, so the target reads it with all-zero control
bytes. Replay it with
`cargo +nightly fuzz run -O retry_ladder tests/fixtures/retry_ladder/ -- -runs=0`.
It was written from the hex in the issue, so it has no license restriction.

`retry_ladder/tile_code_blocks.j2k` (82 bytes, SHA-256
`79a4087ba958c33646c6118c84745f2506f482e95ecb94cbfc5e252187186dfb`) is its
twin for the code-block cap, written by `synth::empty_tiles` (the test in
`tests/work_limit.rs` rebuilds it byte for byte): one 4097 x 4096 gray tile,
no wavelet levels, 4 x 4 code-blocks (the smallest) in one precinct, so 1049600
code-blocks, over the cap of 2^20, and one empty packet. The default decode
refuses it as `LimitKind::TileCodeBlocks` with the retry `Raised(2)`, which
decodes the image. Without it no seed reached that kind through the target.
It has no license restriction. Replay both with the command above.

### Header fixtures

`header/rescue_cut_sot.j2k` (68 bytes, SHA-256
`e7821b0144fc26fe0908cd5fdd3cc8886a436e7e05dfe4d0093957cfe3dab4bf`) is
an input a `header` fuzz run found during #495, committed as found
(#498). It is a 1560 x 1536 gray codestream of 3 x 3 tiles of 520 x 512,
whose `COD` gives 1 x 1 precincts, so each tile is over the precinct cap
under the main header; the input ends two bytes into the first `SOT`.
With `strict` on, `decode_with` refuses it for the cut `SOT` ("truncated
marker segment"), and `read_header_with` now does too (it gave the cap's
`Limit`); with `strict` off both admit it. `opj_decompress` 2.5.0 reads
its main header and fails to decode component 0.
`tests/tile_geometry_cap.rs` and `tests/read_header.rs` check it. Replay it
with `cargo +nightly fuzz run -O header tests/fixtures/header/ -- -runs=0`.
It was made by the fuzzer, so it has no license restriction.

`header/own_cod_lower_tile.j2k` (112 bytes, SHA-256
`97c368959ec300dbc2a233adaea66a9ac6ae70b49277977baea17e340cf30078`) is the
repro of #501, a `header` fuzz find from the batch review of `f709e18..4b25890`,
committed as filed. It is an 8400 x 4096 gray codestream with `XOsiz` 3600
and tiles of 4200 x 4096, so tile 0 is 600 wide. The main `COD` gives 4x4
code-blocks and the maximal precincts, which puts tile 1 over the
code-block cap; tile 0's tile-part `COD` gives it 1x1 precincts, over the
precinct cap. Neither tile-part has data. `decode_with` refuses tile 0 for
its precincts with `Raised(10)`, and `read_header_with` now does too (it
refused tile 1 for its code-blocks with `Raised(2)`), with `strict` on and
off. `opj_decompress` 2.5.0, which has no such caps, decodes it.
`tests/tile_geometry_cap.rs` checks it. It was made by the fuzzer, so it
has no license restriction.

`header/lower_tile_data_long.j2k` (128 bytes, SHA-256
`b8df2d69f3bedca7a0acbf46453aeebfd39d2ece02444878e3530fcefb1fda1f`) and
`header/lower_tile_data_short.j2k` (126 bytes, SHA-256
`92d9e10743542bcedfdc73ced11544991009080279c863d84a80858e4b3e9913`) are
written by the first case of
`what_only_decoding_finds_in_a_lower_tile_comes_first_in_the_decode` in
`tests/tile_geometry_cap.rs` (#501): an 8400 x 8192 gray codestream with
`YOsiz` 3584 and tiles of 4200 x 4096, so the first row of tiles is 512
tall and within the caps, and tile 2 is over the code-block cap under the
main header. Tile 0's tile-part has one byte after its packet in the first
and no packet data in the second, with the bytes changed by hand. With
`strict` on, `read_header_with` refuses tile 2 for its code-blocks while
`decode_with` refuses the file for tile 0's data ("tile 0 has 1 byte(s)
left after the last packet", "the data of tile 0 ends before the last
packet"), which only decoding finds. The `header` target accepts that pair
and must not crash on either. `opj_decompress` 2.5.0 decodes the first and
fails to decode tile 0 of the second. They have no license restriction.

`header/strict_missing_over_cap.j2k` (101 bytes, SHA-256
`bbca3381eba2367830c220c7810f8d6672ed1006bc0d88b3e908b34dee4b73e3`) is an
input from the review of #501, committed as found. It is a 2000 x 1000 gray
codestream with `XOsiz` 992 and tiles of 1000 x 1000, so tile 0 is 8 wide,
whose `COD` gives one level and 1x1 precincts at resolution 0, which puts
tile 1 over the precinct cap. Tile 0's tile-part `COD` gives it no levels,
and tile 1 has no tile-part. At resolution reduction 1 with `strict` on,
`decode_with` refuses the file for the missing tile ("the codestream
carries no tile-part for tile 1") before it counts any tile, and
`read_header_with` does too; with `strict` off both give tile 0's
reduction refusal. `opj_decompress` 2.5.0, which has no such caps,
decodes it. `tests/tile_geometry_cap.rs` checks it. It has no license
restriction.

`header/later_missing_over_cap.j2k` (131 bytes, SHA-256
`abb87f5f4fbbf392865dce5e5377e7dd0cbe15649af66fd5061b7ad2c0ab2993`) is an
input from the review of #501, committed as found. It is a 3000 x 1000 gray
codestream with `XOsiz` 992 and tiles of 1000 x 1000, so tile 0 is 8 wide,
whose main `COD` gives one level and 1x1 precincts at resolution 0, which
puts tiles 1 and 2 over the precinct cap. Tile 0's tile-part `COD` gives it no levels,
tile 1's tile-part `COD` admits it, and tile 2 has no tile-part. At
resolution reduction 1 with `strict` on, `decode_with` refuses the file for
the missing tile ("the codestream carries no tile-part for tile 2") and
`read_header_with` does too; with `strict` off both give tile 0's reduction
refusal. `opj_decompress` 2.5.0 fails to read tile 1's `COD`.
`tests/tile_geometry_cap.rs` checks it. It has no license restriction.

`header/own_reduction_then_missing.j2k` (117 bytes, SHA-256
`3e693058fe20389ec268ec92a1c282c5e6bc1b3d9b6931ec375a165277050bbb`) and
`header/own_reduction_after_lower_data_fault.j2k` (117 bytes, SHA-256
`ec27f69e8de7b42f43c36da4d3dc4d5dc4b6effbd2fd224d3bedab4f9ee9d773`) are
inputs from the review of #501, committed as found. Each is a gray
codestream with `XOsiz` 992 and tiles of 1000 x 1000, so tile 0 is 8 wide,
whose main `COD` gives one level and 1x1 precincts at resolution 0, which
puts the tiles 1000 wide over the precinct cap. Tile 0 has no tile-part
`COD` and too little data for its packets; tile 1's tile-part `COD` gives
it no levels. The first is 3000 wide and has no tile-part for tile 2: at
resolution reduction 1 with `strict` on, `decode_with` refuses the file
for the missing tile ("the codestream carries no tile-part for tile 2")
and `read_header_with` does too, where it once gave tile 1's reduction
refusal; with `strict` off both give that refusal. The second is 2000
wide: at reduction 1 `read_header_with` gives tile 1's reduction refusal
while `decode_with` first refuses tile 0's data ("the data of tile 0 ends
before the last packet"), a difference the `read_header_with` docs list,
and the `header` fuzz target accepts it. `opj_decompress` 2.5.0 fails to
decode component 0 of both. `tests/tile_geometry_cap.rs` and
`tests/read_header.rs` check them. They have no license restriction.

`header/rescued_lower_tile_then_data_fault.j2k` (127 bytes, SHA-256
`9324472b7a8c2efeba187be5949697818894ad1aa36d088235728965515c4980`) is the
repro of #532, written by hand (a short Python script over the marker
layouts in ISO/IEC 15444-1, Annex A). It is an 8500 x 4096 gray codestream
with `XOsiz` 3600 and tiles of 4200 x 4096, so tile 0 is 600 wide and tile 2
is 100 wide, both within the caps, and tile 1 is 4200 wide. The main `COD`
gives 4x4 code-blocks and no levels, which puts tile 1 over the code-block
cap; its tile-part `COD` gives 64x64 code-blocks, within it. Each tile has
one empty packet, and tile 2 has one byte after it. `read_header_with`
reads it and `decode_with` refuses it for tile 2 ("tile 2 has 1 byte(s) left
after the last packet"), which only decoding finds. A header that ignored
tile 1's `COD` would refuse tile 1 for its code-blocks instead; the `header`
target must catch that, because tile 1 decodes alone. Mutating
`Rescue::count` that way makes the target panic on this file.
`opj_decompress` 2.5.0, which has no such caps, decodes it.
`tests/read_header.rs` checks it. It has no license restriction.

`header/missing_after_over_cap.j2k` (83 bytes, SHA-256
`6c6b7f0d01aa2ba90207c5d3b16e1d48302f4dc6792811bea16ebd4f2bd1efb2`) is the
repro of batch critique 10 on #504, written out from the hex in the issue.
It is a 1040 x 512 gray codestream of two 520 x 512 tiles whose `COD` gives
1 x 1 precincts, so tile 0 is over the precinct cap; tile 0 has a
tile-part of one data byte and tile 1 has none. With `strict` on, `decode_with` refuses it
for the missing tile ("the codestream carries no tile-part for tile 1")
and `read_header_with` now does too (it once refused tile 0); with `strict`
off both refuse tile 0. `opj_decompress` 2.5.0 decodes tile 0 and writes
the image. `tests/header_tile_parts_fault.rs` checks it. It has no license
restriction.

`header/own_cap_then_missing.j2k` (130 bytes, SHA-256
`7edc4bb4df21b8bfc9dc4352e27303cbe8618cc33f6bb79f0108a7709d9677f3`) and
`header/strict_off_chain_fault.j2k` (115 bytes, SHA-256
`588ca26c6ab2896341dac88ccfec0dc4f7856ec91b08a5743a70d77b9f971081`) are
written by `row` in `tests/header_tile_parts_fault.rs` (#504), whose tests
rebuild them byte for byte. Each is a gray codestream with `XOsiz` 992 and
tiles of 1000 x 1000, so tile 0 is 8 wide, whose main `COD` gives one level
and 1x1 precincts at resolution 0, which puts the tiles 1000 wide over the
precinct cap. Tile 0 has a tile-part `COD` of no levels and one empty
packet; tile 1 has no data. The first is 3000 wide: tile 1's tile-part
`COD` of 1x1 precincts and no levels keeps it over the cap, and tile 2 has
no tile-part. With `strict` on, `decode_with` refuses it for the missing
tile, and `read_header_with`, which refuses tile 1 first, gives that error
too; with `strict` off both refuse tile 1. The second is 2000 wide and ends
in `00 00` where the `EOC` should be. `read_header_with` and `decode_with`
both refuse it for the chain ("expected `SOT` or `EOC` after a tile-part,
found 0x0000"), with `strict` on and off. Before #545, with `strict` off,
`read_header_with` refused tile 1 for its precincts instead, a difference
the `header` target then excused.
`opj_decompress` 2.5.0 fails to decode tile 1 of both, with
`-allow-partial` too. They have no license restriction.

`header/strict_off_one_tile_chain_fault.j2k` (82 bytes, SHA-256
`7b0e6eede24a5aff468a3b4cd168418580fcddfcb7cd12e03efe3ed7c7f68e2b`) is
written by `one_tile` in the same file, and its test rebuilds it byte for
byte: one 1000 x 1000 gray tile whose main `COD` gives no levels and 1x1
precincts, over the precinct cap, with an empty tile-part and `00 00` where
the `EOC` should be. It is the one-tile case of
`strict_off_chain_fault.j2k`: both reads refuse the chain, with `strict`
on and off (before #545 the strict-off header read refused tile 0).
`opj_decompress` 2.5.0 fails to decode
the tile, with `-allow-partial` too. It has no license restriction.

`header/strict_off_tnsot_then_chain.j2k` (129 bytes, SHA-256
`40d127c1d7ae06dcaf8b413c0e9f7f362d23553b8b8916cd042edf59a31dd94a`) was
written by hand in the review of #504 (PR #542), from the main header `row`
writes for two tiles. Tile 1's one tile-part, with no data, comes first;
then tile 0's two tile-parts, each with `TNsot` 1, the first carrying a
`COD` of no levels and one empty packet; then `00 00` where the `EOC` should
be. With `strict` off, `read_header_with` and `decode_with` refuse the
chain. With `strict` on both refuse tile 0's second tile-part, which a
strict walk finds first.
`a_strict_only_fault_can_come_before_the_tile_parts_fault` in
`tests/header_tile_parts_fault.rs` pins these results. `opj_decompress`
2.5.0 fails to decode tile 2 of 2, with `-allow-partial` too. It has no
license restriction.

`header/s_three_or_hole.j2k` (153 bytes, SHA-256
`f7691268d8429084efa0d9ab31ae0dc883103b9b7b20475584f142aee329adf9`) was
written by hand in the confirm round of PR #542 (#504), as a probe of the
`header` target's strict-off excuse, and committed as a seed in #545. It is a
gray 3000 x 1000 codestream of three 1000 x 1000 tiles, each over the
precinct cap under the main `COD`. Tiles 0 and 1 each have a tile-part
`COD` that brings them within and turns on `SOP`, and data that opens with
an `SOP` of `Lsop` 5; tile 2 has none. Tile 0 declares two tile-parts and
carries one. With `strict` on, `read_header_with` and `decode_with` refuse
the count ("tile 0 declares 2 tile-parts but carries 1"). With `strict`
off, `read_header_with` refuses tile 2 for its precincts and `decode_with`
refuses the `SOP` in tile 0's data first, which only decoding reads.
`a_count_fault_with_strict_on_and_a_data_fault_with_it_off` in
`tests/header_tile_parts_fault.rs` pins these results. `opj_decompress`
2.5.0 decodes all three tiles with no warning. It has no license
restriction.

`header/sycc_422_lower_tile_data_fault.jp2` (195 bytes, SHA-256
`c932fbb4d4785a25b1b925e2ae8a2bef87dab34ce29ac30da46e13099e676bb0`) is the
repro of #538, built by hand (a Python script over the same 8400 x 8192
layout as `lower_tile_data_long.j2k`, in a JP2 wrapper with `EnumCS` 18).
It has 3 components with `XRsiz` 1, 2, 2 (sYCC 4:2:2), `YOsiz` 3584 and
tiles of 8400 x 4096, so tile 0 is 512 tall and within the caps and tile 1
is over the code-block cap. Tile 0's tile-part has one byte after its three
empty packets, and tile 1's has its three empty packets. `read_header_with`
refuses tile 1 for its code-blocks and `decode_with` refuses the file for
tile 0's byte ("tile 0 has 1 byte(s) left after the last packet"). Under
sRGB output every `Region::Tile` decode of this file refuses sub-sampled
chroma on an area smaller than the image, so the `header` target must
decode that tile alone as coded for the excuse to hold. `opj_decompress`
2.5.0 decodes it (exit 0, 8400 x 4608). It has no license restriction.

### Regenerating the corpus

The oracle toolchain (`scripts/install-oracle-tools.sh`; eccodes + OpenJPEG) and
the sibling `fieldglass` checkout are the only inputs. `FG` points at
`fieldglass/crates/fieldglass-grib2/tests/fixtures`.

```sh
FG=../fieldglass/crates/fieldglass-grib2/tests/fixtures

# 1. real lossless 5/3: extract the codestream straight from the §5.40 message.
scripts/extract-grib2-codestream.py "$FG/jpeg2000_regular_latlon.grib2" \
    -o tests/fixtures/jpeg2000_regular_latlon.j2k
scripts/gen-oracle.sh tests/fixtures/jpeg2000_regular_latlon.j2k \
    --source "fieldglass jpeg2000_regular_latlon.grib2 (eccodes grid_jpeg repack of ECMWF regular_latlon_surface.grib2)"

# 2. lossless 5/3 at a second geometry: repack a different base to grid_jpeg.
grib_set -r -s packingType=grid_jpeg "$FG/eta_lambert_msg0.grib2" /tmp/eta.grib2
scripts/extract-grib2-codestream.py /tmp/eta.grib2 -o tests/fixtures/eta_lambert_lossless.j2k
scripts/gen-oracle.sh tests/fixtures/eta_lambert_lossless.j2k \
    --source "eccodes grid_jpeg repack of fieldglass eta_lambert_msg0.grib2 (NOAA Eta, Lambert 93x65)"

# 3. lossy 5/3: repack with typeOfCompressionUsed=1. This truncates the
#    bit-stream (lossy codestream) but stays on the reversible 5/3 transform,
#    so the decode is still bit-exact against the oracle.
grib_set -r -s packingType=grid_jpeg,typeOfCompressionUsed=1,targetCompressionRatio=20 \
    "$FG/regular_latlon_surface.grib2" /tmp/lossy.grib2
scripts/extract-grib2-codestream.py /tmp/lossy.grib2 -o tests/fixtures/regular_latlon_lossy.j2k
scripts/gen-oracle.sh tests/fixtures/regular_latlon_lossy.j2k \
    --source "eccodes grid_jpeg lossy (typeOfCompressionUsed=1, ratio 20) repack of ECMWF regular_latlon_surface.grib2" \
    --notes "5/3 reversible (qmfbid=1) made lossy by rate truncation: eccodes' grid_jpeg encoder always uses the reversible 5/3 transform, never 9/7. The inverse is exact integer lifting, so our decode agrees with OpenJPEG bit-exactly."

# 4. irreversible 9/7: no GRIB2 producer ships lossy 9/7, so re-encode the Eta
#    grid's samples (from step 2) with OpenJPEG's irreversible transform.
scripts/gen-irreversible-fixture.sh   # checks the OpenJPEG version first
```

Each `gen-oracle.sh` run records the exact `opj_decompress` command it ran into the
snapshot's `provenance.oracle_command`, so the corpus regrows from these inputs
alone. `cargo test` never invokes any of this — it reads the committed snapshots.

## `<name>.expected.json` schema

The schema is the `Expected` type in `tests/support/mod.rs`; it deserializes the
fields below. Unknown fields are rejected, so a typo'd key fails the parse rather
than silently dropping data. Keep this document and that type in step.

| Field        | Type     | Meaning                                                        |
| ------------ | -------- | ------------------------------------------------------------- |
| `image`      | object   | The reference-grid image area (below).                        |
| `tolerance`  | object   | How close our decode must be to pass (below).                 |
| `components` | object[] | One entry per component, in SIZ order (below).                |
| `provenance` | object   | Where the fixture came from and how to regenerate (below).    |
| `jp2`        | object, optional | For a JP2 fixture, what its header says (below).      |
| `jp2_boxes`  | object, optional | For a JP2 fixture without `jp2`, its `ftyp`, `ihdr`, `colr` and `rreq` fields (below). |

### `image`

The reference-grid image area, `Xsiz - XOsiz` by `Ysiz - YOsiz`. A sub-sampled
component covers this same area with fewer samples, so it is not in general the
shape of any component.

| Field    | Type | Meaning                                |
| -------- | ---- | -------------------------------------- |
| `width`  | u32  | Image area width, in grid points.      |
| `height` | u32  | Image area height, in grid points.     |

### `components[]`

| Field        | Type   | Meaning                                              |
| ------------ | ------ | ---------------------------------------------------- |
| `width`      | u32    | Component width in samples.                          |
| `height`     | u32    | Component height in samples.                         |
| `bit_depth`  | u8     | Bits per sample as declared in SIZ (1–32).           |
| `signed`     | bool   | Whether samples are signed (SIZ component sign).     |
| `x_sampling` | u8     | Horizontal sub-sampling factor (SIZ `XRsiz`).        |
| `y_sampling` | u8     | Vertical sub-sampling factor (SIZ `YRsiz`).          |
| `samples`    | int[]  | `width * height` reference samples, row-major.       |

### `tolerance`

Tagged by `mode`, so both arms read the same in JSON:

- **Reversible (5/3, lossless)** — bit-exact agreement:

  ```json
  "tolerance": { "mode": "exact" }
  ```

- **Irreversible (9/7, lossy)** — each sample within `max_abs_error` (absolute):

  ```json
  "tolerance": { "mode": "absolute", "max_abs_error": 1.0 }
  ```

### `provenance`

| Field            | Type            | Meaning                                                  |
| ---------------- | --------------- | -------------------------------------------------------- |
| `source`         | string          | Where the codestream came from.                          |
| `oracle_command` | string          | Exact command that regenerates this snapshot.            |
| `notes`          | string, optional | Free-form note (how the source was obtained, caveats).  |

### `jp2`

Optional; present on the synthetic JP2 fixtures, and the schema of each record
in `jp2_real/metadata.json`. Checked against `Image::jp2` when present. It is
the `Jp2Snapshot` type in `tests/support/mod.rs`, written so a box dump can be
checked against it by hand.

| Field             | Type     | Meaning                                                              |
| ----------------- | -------- | -------------------------------------------------------------------- |
| `color`          | string   | `srgb`, `grayscale`, `sycc`, `icc:<profile bytes>`, `enumerated:<n>`, `unknown`. |
| `channels`        | string[] | One `<type>/<association>` per output channel, e.g. `color/1`.      |
| `palette_applied` | bool     | Whether the palette was applied.                                     |
| `resolution`      | object   | `capture` and `display`: `[VRN, VRD, HRN, HRD, VRE, HRE]` in box order, or `null`. |
| `xml`             | int[]    | Length of the contents of each top-level `xml ` box, in file order. |
| `uuid`            | array[]  | `[id as lowercase hex, payload length]` per top-level `uuid` box.    |
| `ipr`             | int[], optional | Length of the contents of each top-level `jp2i` box; left out when none. |
| `uuid_info`       | array[], optional | `[[ids as lowercase hex], LOC length (the `url ` payload minus 4 bytes for VERS and FLAG)]` per well-formed top-level `uinf` box; left out when none. |
| `file_type`, `image_header`, `color_specifications`, `reader_requirements` | objects | The box fields below. |

### `jp2_boxes` and the box fields

The `ftyp`, `ihdr`, `colr` and `rreq` fields of a JP2 file. They are the last
four fields of `jp2`, and the whole of `jp2_boxes`, which the synthetic
fixtures without a `jp2` section carry. The same four fields, for ISO/IEC
15444-4 files 1 to 9, are in `conformance/jp2/boxes.json`
(`{"files": {"file1.jp2": {...}}}`).

| Field | Content |
| ----- | ------- |
| `file_type` | `brand`, `minor_version`, `compatibility[]`. Each 4-byte code is text, with `\xNN` for a byte outside printable ASCII. |
| `image_header` | `compression` (`C`), `unknown_color_space` (`UnkC`), `ipr_flag` (`IPR`). |
| `color_specifications[]` | One per `colr` box before the codestream, in file order: `method`, `precedence`, `approximation`, `payload_len`, `payload_sha256` (SHA-256 of the bytes after `APPROX`, so a profile is pinned), and `enum_cs` for `METH` 1 with a 4-byte value. |
| `reader_requirements` | `null`, or `mask_length` (`ML`), `fully_understand`, `decode_completely` (hex), `standard_features[]` as `[SF, SM hex]`, `vendor_features[]` as `[VF hex, VM hex]`. |

Source of truth: `scripts/gen-jp2-box-snapshots.py`, a standalone box walker
(standard library only; `python3 -I scripts/gen-jp2-box-snapshots.py`). It
reads the JP2 box layer of each file and never uses the crate. It writes every
field of `jp2_real/metadata.json` (from scratch) and the four box fields of
the other snapshots. It counts the `colr` boxes of every `jp2h` and those
misplaced at the top level after the first `jp2h`, before the codestream,
and the first top-level `rreq` (`docs/openjpeg-departures.md`).
`scripts/gen-jp2-fixtures.py` runs it as its last step, because it rewrites
the synthetic snapshots without these fields. It was cross-checked
by hand on `file5.jp2` and `file7.jp2` against `xxd` of the box bytes and
`sha256sum` of the profile (`opj_dump` does not print `colr` or `rreq`
fields): `ftyp` `jpx `, version 0, compatibility `00000003`, `jp2 `, `jpx `,
`jpxb`; `ihdr` `C` 7, `UnkC` 0, `IPR` 0; `rreq` `ML` 1, `FUAM` a0, `DCM` c0; `colr`
`METH` 2 (546 and 13332 profile bytes) then `METH` 1 `EnumCS` 21 and 20.
For `jp2_metadata_boxes` it writes the whole `jp2` section, with the checks
it makes for `jp2_real/metadata.json`.

### Box-layer edge cases (`conformance/jp2/edge_cases.json`)

Cases no committed file has, each made by patching `file5.jp2` (a vendor
feature in `rreq`; a top-level `colr` before the `jp2h`, between the `jp2h`
and the `jp2c`, and after the `jp2c`; a second `jp2h`; a malformed first
`rreq` before a well-formed one). The walker defines each case's edits in
`edge_cases()` and writes them with the four box fields it reads from the
patched bytes: `{"cases": {"<name>": {"note", "edits": [{"at", "remove",
"insert" (hex)}], "boxes": {...}}}}`. `tests/jp2_header_fields.rs` applies
the same edits to `file5.jp2` and checks `read_header` and `decode` against
`boxes`. OpenJPEG 2.5.0 decodes every patched file to the same samples as
`file5.jp2`, and ignores the early `colr` ("JPEG2000 Header box not read
yet"). The early `colr` sits directly after the `ftyp` on purpose: that is
the only place before the `jp2h` where OpenJPEG 2.5.0 ignores a header box.
It ORs its unknown-box state (0x7fffffff, which includes the header-read bit)
into its state both after a box it has no handler for (`jp2.c` around line
2396, such as `rreq`) and after a misplaced header box it ignores (around line
2335). So any later top-level header box before the `jp2h` is read as if the
`jp2h` had been: a `colr` after file5's `rreq`, or the second of two `colr`
boxes after the `ftyp`, becomes its color space and changes its output. This
decoder follows the standard and ignores every header box at the top level
before the `jp2h` (#274, 2026-11-11; OpenJPEG's
`JP2_STATE_UNKNOWN` sets every state bit, a mistake rather than a rule), so
the decoders differ on those cases and they are not pinned. The edits hold
only bytes written by the walker (the second `jp2h` copies file5's 14-byte
`ihdr`); the cases need `file5.jp2`, so they live beside it and are left out
of the crate with it.

## Example

```json
{
  "geometry": { "width": 2, "height": 2, "bit_depth": 16, "signed": false },
  "tolerance": { "mode": "exact" },
  "samples": [0, 1, 2, 3],
  "provenance": {
    "source": "fieldglass/jpeg2000_regular_latlon.grib2",
    "oracle_command": "opj_decompress -i sample.j2k -o sample.pgx"
  }
}
```

### Partial decode oracle seeds

`partial/window_rows.j2k` (80 bytes) is an input a 60 s `decode_partial`
fuzz run found on 2026-11-19 while reviewing #491 (#492), committed as
found. It is fuzzer output, 80 bytes of headers, derived from no licensed
file. It declares 65535 tiles (255 x 257 tiles of 1 x 1) and carries one
tile-part. The target's hash of it picks the window 24..55 by 11..34, under
which the decode with `strict` off reports one `MissingTiles` run for each
of the 23 rows of tiles the window reads: 24 entries, past the oracle's old
bound of five per tile-part plus two. `tests/fuzz_partial_oracle.rs` runs
the oracle on it. Replay it with
`cargo +nightly fuzz run -O decode_partial tests/fixtures/partial/window_rows.j2k`.

`partial/header_refused.j2k` (92 bytes) is an input found by the review of
the PR that added the seed above, committed as found. It is fuzzer-shaped
output, 92 bytes of headers, derived from no licensed file. One tile is over
the precinct cap and has no tile-part, so `read_header` refuses it with
`strict` on while the decode with `strict` off admits it. Through the window
22..39 by 9..34 at reduction 1 (the target's hash picks both), the report
holds 25 entries. The oracle reads the grid with `strict` off, and with a
height-based bound if even that fails; the seed checks that. Replay it with
`cargo +nightly fuzz run -O decode_partial tests/fixtures/partial/header_refused.j2k`.
