# ISO/IEC 15444-4 Part 1 conformance corpus

The Profile-0 (`p0_*`) and Profile-1 (`p1_*`) JPEG 2000 conformance codestreams
from ITU-T T.803 | ISO/IEC 15444-4, with their `.pgx` reference decodes and the
per-file error bounds. This is the authoritative bar for the Phase 2 decoder:
where the Phase 1 fixtures in the parent directory grade against an OpenJPEG
re-decode, these grade against the standard's own conformance suite.

The corpus is committed so it grades offline, with no reference decoder and no
network fetch at test time. `tests/conformance_part4.rs` decodes these and
grades them against the bounds; the JP2 and HTJ2K sets below have their own
harnesses (`tests/conformance_jp2.rs`, `tests/conformance_ht.rs`).

## License and redistribution

These files are **not** under this repository's `MIT OR Apache-2.0`. They carry a
narrow conformance-only grant (see [`COPYRIGHT`](COPYRIGHT), retained verbatim as
the grant requires). In its own words it grants use "*for use in hardware or
software products claiming conformance to or testing conformance to the JPEG 2000
Standard*" and states "*No right to these files is granted for non JPEG 2000
Standard uses.*"

Using them to test that this JPEG 2000 decoder conforms to the standard is
squarely within that grant, and OpenJPEG distributes the same files for the same
purpose. Two consequences follow, and both are handled here:

- The grant requires the notice to travel with every copy, so `COPYRIGHT` is
  committed alongside the files and must stay.
- The grant is field-of-use restricted, so it is not an OSI/SPDX license and must
  not be confused with the crate's own terms. To keep the published crate's
  license clean, this whole directory is **excluded from the packaged crate** via
  `exclude` in `Cargo.toml`; it never ships to crates.io. It lives in the git repo
  only, for the test suite.

`cargo deny check` does not inspect committed fixture data (it checks dependency
crates), so it neither sees nor needs to clear these files.

## Layout

```
conformance/
  codestreams/   p{0,1}_{NN}.j2k    — the 23 conformance codestreams
  references/    c{0,1}p{0,1}_*.pgx — reference decodes (class 0 and class 1)
  manifest.json  — per-entry bounds, references, and oracle-derived features
  jp2/           — the nine JP2 reader files and their references (see below)
  ht/            — the HTJ2K codestreams and JPH files (see below)
  COPYRIGHT      — the upstream conformance grant (retain verbatim)
  regenerate.py  — re-fetches and rebuilds the above from the pinned sources
```

## Provenance

| What | Source | Pinned to |
| ---- | ------ | --------- |
| Codestreams | `uclouvain/openjpeg-data` `input/conformance/` | commit `39524bd3` |
| References (`.pgx`) | `uclouvain/openjpeg-data` `baseline/conformance/` | commit `39524bd3` |
| PAE/MSE bounds | `uclouvain/openjpeg` `tests/conformance/CMakeLists.txt` | `C1P*_*_list` |
| Per-file features | `opj_dump -i` plus a direct marker-segment walk over each codestream | — |

Every codestream, reference, and bound in `manifest.json` records where it came
from, so the corpus regrows from these inputs alone.

## Compliance classes

Part 4 grades a decode against a reference image by two bounds: peak absolute
error (PAE) and mean squared error (MSE). A reversible (5/3) entry must match
bit-exactly (PAE = MSE = 0); an irreversible (9/7) entry must stay within the
stated bounds.

- **Class 1** is the active tier and the one `manifest.json` records as the bar:
  bounds are **per component**, and there is one `.pgx` reference per component
  (`c1p0_06_0.pgx … c1p0_06_3.pgx`).
- **Class 0** grades the **first component only**. Its bound lists are commented
  out in the upstream test definitions, so it is the disabled, looser tier. The
  class-0 references and first-component bounds are recorded per entry for
  completeness, but class 1 is the bar.

## Reference format decision

The `.pgx` references are committed **raw**, not pre-converted to the Phase 1
`expected.json` sample-array shape. Two reasons: the class-1 references are
per-component and that schema is single-component, so a multi-component schema
has to be designed first (part of #55, not this corpus); and committing the
upstream bytes verbatim keeps provenance exact. The harness in #55 reads `.pgx`
directly.

## The corpus

Oracle-derived facts (`opj_dump`); "Grade" is `exact` for reversible entries,
`PAE/MSE` for the bounded irreversible ones.

"Comps" is the number of components graded at class 1 (see `graded_components`).

| Codestream | Size | Comps | Tiles | Progression | Layers | Wavelet | Grade |
| ---------- | ---- | ----- | ----- | ----------- | ------ | ------- | ----- |
| `p0_01.j2k` | 128×128 | 1 | 1×1 | RLCP | 1 | 5/3 | exact |
| `p0_02.j2k` | 127×126 | 1 | 1×1 | LRCP | 6 | 5/3 | exact |
| `p0_03.j2k` | 256×256 | 1 | 2×2 | PCRL | 8 | 5/3 | exact |
| `p0_04.j2k` | 640×480 | 3 | 1×1 | RLCP | 20 | 9/7 | PAE/MSE |
| `p0_05.j2k` | 1024×1024 | 4 | 1×1 | PCRL | 7 | 9/7 | PAE/MSE |
| `p0_06.j2k` | 513×129 | 4 | 1×1 | RPCL | 4 | 9/7 | PAE/MSE |
| `p0_07.j2k` | 2048×2048 | 3 | 16×16 | RLCP | 8 | 5/3 | exact |
| `p0_08.j2k` | 513×3072 | 3 | 1×1 | CPRL | 30 | 5/3 | exact |
| `p0_09.j2k` | 17×37 | 1 | 1×1 | LRCP | 1 | 9/7 | exact |
| `p0_10.j2k` | 256×256 | 3 | 2×2 | LRCP | 2 | 5/3 | exact |
| `p0_11.j2k` | 128×1 | 1 | 1×1 | LRCP | 1 | 5/3 | exact |
| `p0_12.j2k` | 3×5 | 1 | 1×1 | LRCP | 1 | 5/3 | exact |
| `p0_13.j2k` | 1×1 | 4 of 257 | 1×1 | RLCP | 1 | 5/3 | exact |
| `p0_14.j2k` | 49×49 | 3 | 1×1 | LRCP | 1 | 5/3 | exact |
| `p0_15.j2k` | 256×256 | 1 | 2×2 | PCRL | 8 | 5/3 | exact |
| `p0_16.j2k` | 128×128 | 1 | 1×1 | RLCP | 3 | 5/3 | exact |
| `p1_01.j2k` | 127×227 | 1 | 1×1 | LRCP | 5 | 5/3 | exact |
| `p1_02.j2k` | 640×480 | 3 | 1×1 | LRCP | 19 | 9/7 | PAE/MSE |
| `p1_03.j2k` | 1024×1024 | 4 | 1×1 | PCRL | 10 | 9/7 | PAE/MSE |
| `p1_04.j2k` | 1024×1024 | 1 | 8×8 | LRCP | 1 | 9/7 | PAE/MSE |
| `p1_05.j2k` | 529×524 | 3 | 15×15 | PCRL | 2 | 9/7 | PAE/MSE |
| `p1_06.j2k` | 12×12 | 3 | 4×4 | PCRL | 1 | 9/7 | PAE/MSE |
| `p1_07.j2k` | 12×12 | 2 | 1×1 | RPCL | 1 | 5/3 | exact |

Between them the 23 entries exercise the Phase 2 breadth: all five progression
orders, single and tiled canvases (up to 16×16), single and multi-component
images with subsampling (up to a 257-component stress image, `p0_13`), single and
multiple quality layers, and both wavelets. Each entry's full parameters live in
`manifest.json`.

### Scope

`codestreams/` holds only the bare Part-1 codestreams (`p0_*`, `p1_*`). The
JP2-wrapped reader files are a separate set under `jp2/` (below).

## `manifest.json` schema

One object with `provenance`, `compliance_class`, and an `entries` array. Each
entry:

| Field | Meaning |
| ----- | ------- |
| `codestream` | Path to the `.j2k`, relative to this directory. |
| `profile`, `index` | Profile (0/1) and number `NN`. |
| `graded_components` | Components graded at class 1 (= number of class-1 references = bound arity). May be fewer than `features.components`: `p0_13` is a 257-component stress image of which only the first 4 are graded. |
| `bit_exact` | True when the class-1 bounds are all zero, i.e. the decode must match the reference exactly. This tracks the bounds, not the wavelet: `p0_09` is 9/7 yet graded bit-exact, so it is decoupled from `features.reversible`. |
| `features` | `opj_dump`-derived parameters (dimensions, subsampling, tiles, progression, layers, MCT flag, resolutions, code-block size, reversibility) plus marker-walk fields: `markers_main`/`markers_tile` (optional markers by header, reserved codes as hex), `tile_parts`, `sop`/`eph`/`precincts` flags, and the six `cblksty` style bits. These map each Phase 2 feature to the entries that exercise it; `tests/conformance_corpus.rs` asserts the coverage, including the known gaps (no entry carries PLM or a tile-part COD/COC/QCC). |
| `references.class0` / `.class1` | Reference `.pgx` paths; class-1 is one per component, in order. |
| `bounds_class1` | Per-component `pae` and `mse` arrays — the grading bar. |
| `bounds_class0_first_component` | First-component `pae`/`mse` for the disabled class-0 tier. |
| `phase2_in_scope` | True for every entry (all are bare Part-1 codestreams). |

## JP2 reader files (`jp2/`)

`file1.jp2` .. `file9.jp2` from ITU-T T.803 | ISO/IEC 15444-4 Annex G, with
OpenJPEG's conformance baselines as references. They exercise the JP2 box layer
around an ordinary codestream, not the codestream itself. They are graded by
`tests/conformance_jp2.rs` (decode, shift, then the same PAE/MSE comparator and
ratchet as the Part 1 set). The same `COPYRIGHT` governs them, and the whole
`tests/fixtures/conformance/` directory is excluded from the crate.

```
jp2/
  files/        fileN.jp2          — the nine reader files
  references/   jp2_N_K.pgx        — channel K of the reference for file N
  manifest.json                    — boxes, color, features, bounds, flags
  boxes.json                       — ftyp, ihdr, colr, rreq fields (box walker)
  edge_cases.json                  — file5 patched into box-layer edge cases
```

The references are **display output**, not codestream samples. OpenJPEG's
`jp2_N.tif` baselines are 8-bit RGB after palette expansion, channel
definitions, sYCC to RGB, and ICC to sRGB, then `opj_decompress -p 8S`, which
right-shifts deeper samples to 8 bits. Grayscale files (4, 6, 8) have three
identical TIFF channels; one `.pgx` plane is kept (the script asserts they are
identical before dropping two). That makes 21 reference planes: three for each
of the six color files, one for each grayscale file.

| File | Size | Channels | Depth | Boxes beyond `ihdr` + `colr` | Color | Grade |
| ---- | ---- | -------- | ----- | ---------------------------- | ------ | ----- |
| `file1.jp2` | 768×512 | 3 | 8 | two `xml ` (before and after `jp2h`) | sRGB | exact |
| `file2.jp2` | 480×640 | 3 | 8 | `cdef` | sYCC | PAE 4 / MSE 1 |
| `file3.jp2` | 480×640 | 3 | 8 | none (chroma subsampled 2×2) | sYCC | PAE 4 / MSE 1 |
| `file4.jp2` | 768×512 | 1 | 8 | none | grayscale | exact |
| `file5.jp2` | 768×512 | 3 | 8 | `rreq`, second `colr` | ICC (546 bytes) | PAE 4 / MSE 1 |
| `file6.jp2` | 768×512 | 1 | 12 | none | grayscale | exact, shift right 4 |
| `file7.jp2` | 480×640 | 3 | 16 | `rreq`, second `colr` | ICC (13332 bytes) | PAE 4 / MSE 1, shift right 8 |
| `file8.jp2` | 700×400 | 1 | 8 | `xml ` after `jp2h` and after `jp2c`, `COM` marker | ICC (414 bytes) | PAE 4 / MSE 1 |
| `file9.jp2` | 768×512 | 1 stored, 3 out | 8 | `pclr`, `cmap` | sRGB | exact |

Bounds are OpenJPEG's JP2 block in `tests/conformance/CMakeLists.txt` (Part 4
Table G.1): PAE 4, MSE 1 on each of three channels. Files 1, 4, 6 and 9 are
graded bit-exact because the OpenJPEG decode equals the reference there
(measured by the script); the manifest still records 4 / 1 for them, and the
harness forces `bit_exact` entries to zero. The other five stay within PAE 1 in
the same measurement.

`needs_srgb` marks files 2, 3, 5, 7 and 8, whose references are sRGB display
output (sYCC or an ICC profile converted). The harness decodes those with
`ColorOutput::Srgb` and grades the result against the display reference,
after grading the default decode as coded (below). `shift_right` is the right
shift that takes decoded samples to the reference's 8 bits (file 6: 4, file
7: 8); file 7's sRGB output stays 16-bit and takes the same shift. File 8's
sRGB output is one gray channel, graded against the one kept plane.

Measured on the converted files (PAE / MSE per channel, R/G/B or gray; the
OpenJPEG column is `opj_decompress -p 8S -force-rgb` on the `.jp2`, OpenJPEG
2.5.0 with LittleCMS 2.14; file7's ours-vs-OpenJPEG is at its native 16
bits, the ISO columns after the shift):

| File | Ours vs ISO | OpenJPEG vs ISO | Ours vs OpenJPEG |
| ---- | ----------- | --------------- | ---------------- |
| file2 | 1 / 0.0003, 1 / 0.0155, 1 / 0.0242 | 1 / 0.3853, 1 / 0.4748, 1 / 0.5135 | 1 / 0.3850, 1 / 0.4803, 1 / 0.4895 |
| file3 | 1 / 0.0002, 1 / 0.0151, 1 / 0.0239 | 1 / 0.3862, 1 / 0.4724, 1 / 0.5109 | 1 / 0.3860, 1 / 0.4774, 1 / 0.4872 |
| file5 | 1 / 0.0897, 1 / 0.1750, 1 / 0.2050 | 1 / 0.0897, 1 / 0.1750, 1 / 0.2050 | 0 / 0, 0 / 0, 0 / 0 |
| file7 | 1 / 0.1442, 1 / 0.1770, 1 / 0.1900 | 1 / 0.1442, 1 / 0.1770, 1 / 0.1900 | 0 / 0, 0 / 0, 0 / 0 |
| file8 | 1 / 0.2739 | 1 / 0.2739 | 0 / 0 |

All five are in class (PAE 4, MSE 1). The sYCC files differ from OpenJPEG by
its formula (shortened constants, truncation), a deliberate deviation
(#165) written up in `docs/srgb-output.md`. The ICC files match OpenJPEG exactly:
the decoder ports LittleCMS's pipeline (`tests/jp2_icc.rs` also checks them
at every rendering intent).

Files 2, 3, 5, 7 and 8 also carry an `as_coded` section: one reference per codestream
component (`references/jp2_N_ascoded_K.pgx`), at the component's own size and
depth, with no color conversion and no shift. File 2's are in the order its
`cdef` gives (components 2, 1, 0), and that order is checked against
`opj_decompress` on `file2.jp2` with its `colr` rewritten to sRGB, so OpenJPEG
applies the `cdef` without converting sYCC. The harness grades these
bit-exactly against the default decode, so the as-coded output cannot drift
while the sRGB output is graded. The
references come from OpenJPEG decoding the `jp2c` payload as a bare codestream,
which keeps ICC handling out. Two OpenJPEG behaviors shape how: it converts
sYCC to RGB by heuristic for any 3-component image with sub-sampled chroma
(`opj_decompress.c:1620-1626`), so file 3 is decoded one component at a time
with `-c K` (its chroma stays 240×320); and `-c K` skips the inverse component
transform, so files 5 and 7 (`mct` 1) are decoded whole instead.

`jp2/manifest.json` has the same top level as the Part 1 manifest, and every
object denies unknown fields. Per entry: `file`, `index`, `graded_components`,
`bit_exact`, `shift_right`, `needs_srgb`, `boxes` (`top` and `jp2h` box types in
file order), `color` (the first `colr`: `method` with `enum_cs` or
`icc_bytes`), `features` (the Part 1 `features` object for the `jp2c` payload),
`references`, `file_digest` and `reference_digests` (byte size and SHA-256 of each committed file, re-checked by the tests), `bounds`, and optionally `as_coded` (`references` and `reference_digests`, as above).

The re-grade was run with OpenJPEG 2.5.0 (the system build). Provenance is the same pinned `uclouvain/openjpeg-data` commit (`39524bd3`):
`input/conformance/file{1..9}.jp2` and `baseline/conformance/jp2_{1..9}.tif`.

## HTJ2K files (`ht/`)

The High-Throughput (ISO/IEC 15444-15) conformance files of ITU-T T.803
(V3, 02/2024) | ISO/IEC 15444-4:2024: 41 codestreams and 20 JPH files. They are
graded by `tests/conformance_ht.rs` with the Part 1 comparator and ratchet.

| What | Source | Pinned to |
| ---- | ------ | --------- |
| Codestreams, JPH files, `COPYRIGHT.txt` | `gitlab.com/wg1/htj2k-codestreams` | commit `f6b9ede0` |
| `hifi_ht1_02` references | the same repo, `reference_class1HF_profile1/`, copied verbatim | commit `f6b9ede0` |
| `file10` references | the same repo, `reference_jph/jph_10_*.pgm`, rewritten as 8-bit PGX | commit `f6b9ede0` |
| Bounds | T.803 Annex C, Tables C.1bis, C.4bis, C.6, C.7 and C.8 | V3 (02/2024) |
| Kakadu error, pass counts | the repo's `stats.txt`; OpenJPEG 2.5.0 with a pass-count print | — |

**License.** `ht/COPYRIGHT.txt` is the same conformance-only grant as
`COPYRIGHT`, kept verbatim. The files sit under `tests/fixtures/conformance/`,
so the existing `exclude` keeps them out of the published crate.

```
ht/
  codestreams/  ds{0,1}_h{t,m}_NN_bB.j2k, hifi_ht1_02.j2k — the 41 codestreams
  jph/          fileN_bB.jph          — the 20 JPH files
  references/   hifi_02-K.pgx, jph_10_K.pgx — references not in the Part 1 set
  oracle/       <name>_K.pgx          — OpenJPEG's output where it differs from the references
  manifest.json                       — bounds and their sources, signaling, passes, digests
  COPYRIGHT.txt                       — the upstream grant (retain verbatim)
```

**Codestreams.** `ds0_*` and `ds1_*` are Kakadu transcodes of the Part 1
profile-0 and profile-1 codestreams to HT code-blocks, at one or more magnitude
bounds `B` (the `_bB` suffix). `ds0_hm_*` are MIXED: both block coders in one
codestream. Each is graded against the Part 1 class-1 references of the
codestream it came from (`../references/`), at that entry's reduction.
`hifi_ht1_02` is a transcode of a high-fidelity profile-1 file and is graded
against its own references.

**Bounds.** T.803 C.2 gives the bound for each HT codestream as the Part 1
class-1 tolerance (Tables C.6, C.7) plus an additional tolerance (Table C.1bis
for `ds0`, C.4bis for `ds1`) quoted at a reference depth `Dref`. At depth `D`
the additional peak scales by `2^(D - Dref)`, rounded up, and the additional
MSE by `4^(D - Dref)`. `hifi_ht1_02` takes its bounds from Table C.8 whole. The
Part 1 tolerances in T.803 differ from OpenJPEG's lists (used for the Part 1
set) in three places, and the HT bounds follow T.803:

| Source | Component | T.803 MSE | OpenJPEG MSE |
| ------ | --------- | --------- | ------------ |
| `p0_05` | 0, 1, 2 | 0.319, 0.323, 0.317 | 0.302, 0.307, 0.269 |
| `p1_03` | 0, 1, 2 | 0.311, 0.280, 0.267 | 0.3, 0.21, 0.2 |
| `p1_05` | 1 | 9.716 | 9.816 |

Each bound is cross-checked when the manifest is written, and again by the
tests:

- It is at least the Part 1 tolerance plus the error Kakadu recorded between
  the transcode and its source (`stats.txt`), PAE and MSE each added at the
  component's own depth. Kakadu quotes the 4-bit `p0_03` and `p0_15` sets at
  8 bits, so their figures are divided by 16 and 256.
- Where `opj_decompress` 2.5.0 decodes a codestream, its error against the
  references is within the bound. The tests recompute that error from the
  snapshot in `oracle/` for the four codestreams that have one; for the six
  that matched the references they check that the recorded error is zero.

The tests also hold the manifest's additional tolerances, `Dref` values and
C.8 bounds to their own transcription of the T.803 tables
(`T803_ALLOWANCE`, `T803_C8` in `tests/conformance_ht.rs`).

The MSE parts add linearly: the bound is the Part 1 MSE plus the allowance,
so the cross-check holds exactly when the allowance is at least Kakadu's
recorded MSE, as every T.803 allowance is. Adding them as square roots instead (`(√a + √b)²`) gives a larger figure than the
T.803 bound for 11 components (`ds0_ht_04_b11`, `ds0_ht_05_b11`,
`ds0_ht_06_b11`, `ds1_ht_02_b11` and `ds1_ht_03_b11`).

Two oddities in the tables are transcribed as printed:

- `ds0_ht_03_b11` and `ds0_ht_15_b11` have an additional peak of 17 on 4-bit
  data, where no error can exceed 15, so their PAE bound cannot fail. It is
  likely a typo in T.803.
- The note under Table C.4bis says the allowances apply to Table C.6, but
  `ds1` is profile 1, whose tolerances are in Table C.7. The bounds here use
  C.7.

**OpenJPEG.** `opj_decompress` 2.5.0 decodes 10 of the 41 codestreams. Six
match the references exactly. For the other four (`ds1_ht_04_b9`,
`ds1_ht_05_b11`, `ds1_ht_06_b11`, `hifi_ht1_02`) its output is kept in
`oracle/`, and a second ratchet grades our decode against it, exact for 5/3 and
within PAE 1, MSE 1 for 9/7. It warns of zero-length refinement passes on
`ds1_ht_05_b11` and `ds1_ht_06_b11`. It refuses the other 31; each refusal is
recorded with its message.

**JPH files.** T.803 G.5 gives no tolerances for JPH files, and B.2 tests a
decoder on the highest-bound file of each set, so 10 of the 20 are graded:

- `file1` .. `file9` at their highest bound decode, under OpenJPEG, to the same
  samples as `fileN.jp2`. They are graded as `fileN.jp2` is, against
  `jp2/references/` and with its bounds and color handling.
- `file10_b12` (five components) is graded bit-exactly against
  `references/jph_10_K.pgx`. OpenJPEG decodes its codestream to those samples
  but refuses the file.

The other 10 record why they are not graded.

**Passes.** For the codestreams OpenJPEG decodes in full, the manifest records
the pass count of every HT code-block, measured with a print added to
`opj_t1_ht_decode_cblk`. None has more than three passes, so none has
placeholder passes. T.803 Table C.5bis describes `ds1` set 5 as having
placeholder passes, but `ds1_ht_05_b11` measures at most three per block. The
other files record `placeholder` and `refinement` as unknown.

`ht/manifest.json` denies unknown fields throughout. Each codestream records
its `source` Part 1 entry, `references`, `bounds`, `bit_exact`, `reduction`,
`tolerance` (the table, the Part 1 and additional parts, `Dref` and each
component's `D`), `kakadu_stats`, `features` (as for Part 1, plus every
code-block style byte), `ht` (`Pcap`, the `Ccap15` fields and `CPFnum`),
`passes`, digests, and `openjpeg` (whether it decodes, its message or error,
the agreement bound and any snapshot). Each JPH file records its box list, the
`jp2c` byte range, the same `features`, `ht` and `passes`, Kakadu's figures,
and `graded_as` or `not_graded`.

## OpenJPH test codestreams (`../openjph/`)

The 16 OpenJPH test codestreams under 100 KB, with OpenJPEG's decode as their
references. See [`../openjph/README.md`](../openjph/README.md).

## Regenerating

Needs `opj_dump` and `opj_decompress` (OpenJPEG) on `PATH` and network access;
neither is needed to run the committed suite. The JP2 half also needs Python
with Pillow and numpy to read the TIFF references (Pillow decodes their LZW
without extra codecs; the TIFFs are not kept, only the converted `.pgx`).

```sh
python3 regenerate.py          # the Part 1 and JP2 sets
python3 regenerate.py part1    # the Part 1 corpus only
python3 regenerate.py jp2      # the JP2 reader files only
python3 regenerate.py ht       # the HTJ2K files (ht/)
python3 regenerate.py openjph  # the OpenJPH codestreams (../openjph/)
```

The HT set also needs numpy, `opj_decompress` 2.5.0 and LittleCMS, which it
checks for.
It downloads from the pinned commit, or copies from a local clone of
`htj2k-codestreams` at that commit when `HTJ2K_CODESTREAMS` names one (the
script checks the clone's `HEAD`). The T.803 tables and the measured pass
counts are transcribed in the script (`T803_*`, `HT_PASSES`); re-pin by editing
`HT_COMMIT` there. Files already present must match what is fetched.

### Part 1 details

`regenerate.py part1` re-fetches the codestreams and references from the pinned commit (skipping any
already present), reads each codestream's parameters with `opj_dump`, and rewrites
`manifest.json`. The PAE/MSE bound lists are transcribed verbatim from OpenJPEG's
`tests/conformance/CMakeLists.txt` inside the script; re-pin by editing
`DATA_COMMIT` and the bound strings there.
