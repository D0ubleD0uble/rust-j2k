# ISO/IEC 15444-4 Part 1 conformance corpus

The Profile-0 (`p0_*`) and Profile-1 (`p1_*`) JPEG 2000 conformance codestreams
from ITU-T T.803 | ISO/IEC 15444-4, with their `.pgx` reference decodes and the
per-file error bounds. This is the authoritative bar for the Phase 2 decoder:
where the Phase 1 fixtures in the parent directory grade against an OpenJPEG
re-decode, these grade against the standard's own conformance suite.

The corpus is committed so it grades offline, with no reference decoder and no
network fetch at test time. It is **not yet wired into the test harness** — the
grading harness that decodes these and compares against the bounds is a separate
piece of work (issue #55). This directory is the data and its provenance.

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
  manifest.json                    — boxes, colour, features, bounds, flags
```

The references are **display output**, not codestream samples. OpenJPEG's
`jp2_N.tif` baselines are 8-bit RGB after palette expansion, channel
definitions, sYCC to RGB, and ICC to sRGB, then `opj_decompress -p 8S`, which
right-shifts deeper samples to 8 bits. Greyscale files (4, 6, 8) have three
identical TIFF channels; one `.pgx` plane is kept (the script asserts they are
identical before dropping two). That makes 21 reference planes: three for each
of the six colour files, one for each greyscale file.

| File | Size | Channels | Depth | Boxes beyond `ihdr` + `colr` | Colour | Grade |
| ---- | ---- | -------- | ----- | ---------------------------- | ------ | ----- |
| `file1.jp2` | 768×512 | 3 | 8 | two `xml ` (before and after `jp2h`) | sRGB | exact |
| `file2.jp2` | 480×640 | 3 | 8 | `cdef` | sYCC | PAE 4 / MSE 1 |
| `file3.jp2` | 480×640 | 3 | 8 | none (chroma subsampled 2×2) | sYCC | PAE 4 / MSE 1 |
| `file4.jp2` | 768×512 | 1 | 8 | none | greyscale | exact |
| `file5.jp2` | 768×512 | 3 | 8 | `rreq`, second `colr` | ICC (546 bytes) | PAE 4 / MSE 1 |
| `file6.jp2` | 768×512 | 1 | 12 | none | greyscale | exact, shift right 4 |
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
output (sYCC or an ICC profile converted). Until the decoder has an sRGB
output option, those entries report *needs sRGB output* instead of being graded.
`shift_right` is the right shift that takes decoded samples to the reference's
8 bits (file 6: 4, file 7: 8).

Files 3, 5, 7 and 8 also carry an `as_coded` section: one reference per codestream
component (`references/jp2_N_ascoded_K.pgx`), at the component's own size and
depth, with no colour conversion and no shift. While sRGB output is unavailable
the harness grades these bit-exactly and still reports *needs sRGB output*. The
references come from OpenJPEG decoding the `jp2c` payload as a bare codestream,
which keeps ICC handling out. Two OpenJPEG behaviours shape how: it converts
sYCC to RGB by heuristic for any 3-component image with sub-sampled chroma
(`opj_decompress.c:1620-1626`), so file 3 is decoded one component at a time
with `-c K` (its chroma stays 240×320); and `-c K` skips the inverse component
transform, so files 5 and 7 (`mct` 1) are decoded whole instead.

`jp2/manifest.json` has the same top level as the Part 1 manifest, and every
object denies unknown fields. Per entry: `file`, `index`, `graded_components`,
`bit_exact`, `shift_right`, `needs_srgb`, `boxes` (`top` and `jp2h` box types in
file order), `colour` (the first `colr`: `method` with `enum_cs` or
`icc_bytes`), `features` (the Part 1 `features` object for the `jp2c` payload),
`references`, `file_digest` and `reference_digests` (byte size and SHA-256 of each committed file, re-checked by the tests), and `bounds`.

The re-grade was run with OpenJPEG 2.5.0 (the system build). Provenance is the same pinned `uclouvain/openjpeg-data` commit (`39524bd3`):
`input/conformance/file{1..9}.jp2` and `baseline/conformance/jp2_{1..9}.tif`.

## Regenerating

Needs `opj_dump` and `opj_decompress` (OpenJPEG) on `PATH` and network access;
neither is needed to run the committed suite. The JP2 half also needs Python
with Pillow and numpy to read the TIFF references (Pillow decodes their LZW
without extra codecs; the TIFFs are not kept, only the converted `.pgx`).

```sh
python3 regenerate.py          # both sets
python3 regenerate.py part1    # the Part 1 corpus only
python3 regenerate.py jp2      # the JP2 reader files only
```

### Part 1 details

`regenerate.py part1` re-fetches the codestreams and references from the pinned commit (skipping any
already present), reads each codestream's parameters with `opj_dump`, and rewrites
`manifest.json`. The PAE/MSE bound lists are transcribed verbatim from OpenJPEG's
`tests/conformance/CMakeLists.txt` inside the script; re-pin by editing
`DATA_COMMIT` and the bound strings there.
