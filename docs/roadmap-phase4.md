# Phase 4 roadmap — HTJ2K decode (Part 15)

The overall [roadmap.md](roadmap.md) sequences the whole project; this file
covers Phase 4 alone. It breaks "decode HTJ2K codestreams" into ordered
steps, each tied to the module it changes, the oracle that checks it and the
gate that lets the next step start.

The feature rows this phase covers are the Part 15 rows in
[scope.md](scope.md), plus the two Part 1 rows that mention HTJ2K (the `CAP`
marker and the code-block style bits). The specification is ISO/IEC 15444-15 |
ITU-T T.814. The conformance authority is ISO/IEC 15444-4 | ITU-T T.803, which
adds HTJ2K codestreams and JPH files to the Part 1 test set.

## Decisions

Settled while planning, on 2026-11-14:

1. **The gate is the Part 4 HT set, graded against the standard.** Every
   ISO/IEC 15444-4 HT codestream (41) and JPH file (20) decodes within its ISO
   bounds, the three MIXED codestreams included. Wherever OpenJPEG 2.5.0
   decodes a file, the output also matches it bit for bit, apart from the
   departures already listed in [openjpeg-departures.md](openjpeg-departures.md).
   Where the standard and OpenJPEG disagree, the standard wins.
2. **All Part 4 HT codestreams and JPH files are vendored**, under the same
   conformance-only grant as the Part 1 corpus and excluded from the published
   crate like `tests/fixtures/conformance/`. So are the 16 OpenJPH test files
   under 100 KB, with their BSD-2-Clause notice. The four openjpeg-data HT
   files stay local and are not committed, like the rest of `nonregression/`
   (it has no license statement).
3. **OpenJPH is a regeneration-only tool**, like `opj_compress`, pinned to a
   release tag. It makes synthetic fixtures and serves as a second oracle. It
   is never a build or test dependency, so the crate stays pure Rust and the
   suite stays offline.
4. **Bounds come from T.803 C.2.** Each HT entry's PAE and MSE bounds are
   transcribed from the HTJ2K tolerances there. Every transcribed number must
   also pass a cross-check: the source Part 1 entry's bound plus the transcode
   error Kakadu recorded for that file (PAE adds; for MSE, square roots add).
   If T.803 can't be obtained, only the highest-bound file of each set is
   graded (it is lossless against its Part 1 decode), and the others are
   recorded without a grade.
5. **MULTIHT is refused with `Error::Unsupported`.** No corpus file uses it,
   so there is no oracle for it. The HT-declared coder set is accepted, since
   the code-block style bits already say which coder each block uses.
6. **The VLC and UVLC tables are generated from the T.814 text** by a
   committed script, and tested against OpenJPEG's behavior through the
   end-to-end fixtures. The BSD-licensed tables in OpenJPEG and OpenJPH are
   not copied, so no third-party notice ships in the crate.
7. **No public API change, and one release.** `decode` and `read_header`
   accept HTJ2K; no type or field is added. The phase ships as 0.5.0 when the
   gate passes, with no interim HT release. Codestreams that were `Unsupported`
   now decode, which is the behavior change that earns the minor bump.

## What Phase 4 decodes

Phase 3 decodes any Part 1 codestream, raw or in a JP2 file. Phase 4 adds the
codestreams whose code-blocks use the HT block coder: all of them, or some of
them with the rest coded as in Part 1 (MIXED), raw or wrapped in a JPH or JP2
file. A Part 15 feature outside the plan (MULTIHT) is refused with
`Error::Unsupported`, as any feature outside the decoded subset is now.

## What HTJ2K changes

Part 15 replaces the Tier-1 block coder and leaves the rest of Part 1 in
place. The codestream syntax, Tier-2 packets, dequantization, DWT and MCT stay
as they are, apart from these changes:

- **Signaling.** SIZ `Rsiz` bit 14 (`0x4000`) says Part 15 capabilities are in
  use; every Part 4 HT file sets it. A `CAP` marker (`0xFF50`) follows
  SIZ: `Pcap` is a 32-bit mask, and its Part 15 bit (`0x0002_0000`) carries one
  16-bit `Ccap15` word. A `CPF` marker (`0xFF59`) may give the profile. It is
  optional: four of the conformance codestreams leave it out. The decoder
  refuses `CAP` in the main header today.
- **`Ccap15`.** Bits 4–0 hold the magnitude bound `B` as `Bp`, where
  `B = Bp + 8` for `B` from 9 to 27 and `Bp = 0` for `B` up to 8 (OpenJPH's
  writer; each file's `_bNN` name agrees). Bit 5 is set for the irreversible
  transform. Bits 15–14 give the block coder set: `00` on every HT-only file
  and `11` on the three MIXED files. Bit 12 is set on exactly the files that
  carry an `RGN` marker, and bit 11 on the 17 files Kakadu transcoded with its
  `HET` option. T.814 also defines the MULTIHT bit and the HT-declared set,
  which no corpus file uses; P4.1 takes their positions from T.814.
- **Code-block style.** `SPcod`/`SPcoc` bit `0x40` selects the HT block coder
  and bit `0x80` selects MIXED (the MIXED files carry `0xC0`). `decode_coding`
  refuses both today. Vertically causal (`0x08`) still applies; four conformance
  codestreams and `byte_causal.jhc` use it.
- **Block coding.** An HT code-block holds one *HT set*: a cleanup pass, whose
  segment holds the MEL, VLC and MagSgn streams, and optionally a SigProp pass
  and a MagRef pass in a second, *refinement* segment. The cleanup segment
  ends with `Scup`, the MEL and VLC length, in its last two bytes, and
  `2 <= Scup <= min(Lcup, 4079)`. MagSgn and SigProp are read forward, VLC and
  MagRef backward.
- **Tier-2.** An HT block's passes split into the two segments, not by the
  Part 1 `segment_max_passes` rule. The first contribution's passes are
  placeholders plus one cleanup pass, `3k + 1` in all, and up to two
  refinement passes. Each group of three placeholder passes raises the block's
  missing MSB count by one. The length fields are `Lblock + floor(log2(3k + 1))`
  bits for the cleanup segment and `Lblock + floor(log2(n))` bits for `n`
  refinement passes. The cleanup segment is under 65535 bytes and the
  refinement segment under 2047.
- **File format.** JPH (Part 15 Annex D) is the JP2 format with the `jph `
  brand. Its `colr` box may be absent, and `cdef` allows a channel type (`Typ`
  3) with application-defined `Asoc` values. Every Part 4 JPH file lists only
  `jph ` in `ftyp`, so the decoder's `jp2 ` compatibility check refuses them
  today.

## What OpenJPEG 2.5.0 decodes

`/usr/bin/opj_decompress` (Ubuntu `2.5.0-2ubuntu0.5`) decodes HTJ2K, but not
all of Part 15. The limits, from the source at tag `v2.5.0` (`a5891555`) and
confirmed by running it on the corpora below:

| Feature | OpenJPEG 2.5.0 | Where |
| ------- | -------------- | ----- |
| `CAP` and `CPF` | read and ignored (empty handlers, main header only) | `j2k.c` `opj_j2k_read_cap`, `opj_j2k_read_cpf` |
| MIXED (`cblksty & 0x80`) | header refused: "Unsupported Mixed HT code-block style found" | `j2k.c` `opj_j2k_read_SPCod_SPCoc` |
| More than 3 passes in a block (placeholder passes) | refused: "We do not support more than 3 coding passes in an HT codeblock" | `ht_dec.c` `opj_t1_ht_decode_cblk` |
| Placeholder passes in Tier-2 | the first segment always gets one pass, so the length field is read at the wrong width and the packet parse loses sync ("segment too long", "Expected EPH marker") | `t2.c` `opj_t2_read_packet_header` |
| ROI (maxshift) on HT blocks | refused: "We do not support ROI in decoding HT codeblocks" | `ht_dec.c` |
| `Mb > 30` | refused | `ht_dec.c` |
| Refinement segment of zero length with more than one pass | warns, then decodes the cleanup pass only | `ht_dec.c` |
| SigProp and MagRef in one HT set | decoded | `ht_dec.c` |

OpenJPEG's own test suite runs only the four openjpeg-data HT files
(`tests/nonregression/test_suite.ctest.in`), none of the Part 4 set.

The Part 4 HT codestreams were made by transcoding the Part 1 conformance
codestreams, so most of them have several quality layers, and placeholder
passes with them. OpenJPEG decodes 10 of the 41: the 9 single-layer files other
than `ds0_ht_13` (ROI), and `ds1_ht_05` (2 layers). That is why the gate grades
against the ISO references (decision 1), with OpenJPEG as the bit-exact check
where it decodes.

## The corpora

Measured on 2026-11-14 by running `opj_decompress` 2.5.0 on every file and,
where it decoded, grading its output against the ISO reference.

| Corpus | Files | License | What it covers | `opj_decompress` 2.5.0 |
| ------ | ----- | ------- | -------------- | ---------------------- |
| **Part 4 HT codestreams**: `gitlab.com/wg1/htj2k-codestreams` (`htj2k_bsets_profile0`, `htj2k_bsets_profile1`, `codestreams_hifi`), commit `f6b9ede` | 41, 40 MB: 27 `ds0_ht_*`, 3 `ds0_hm_*` (MIXED), 10 `ds1_ht_*`, `hifi_ht1_02`. 32 MB of it is the `p0_07` and `p0_08` sets | the conformance-only grant (`COPYRIGHT.txt`, "Copyright (c) 2002, 2021") | Kakadu `kdu_transcode` of each Part 1 `p0_*`/`p1_*` codestream at one to three magnitude bounds (`_bNN` is `B`, from 8 to 18); HT only and MIXED; RGN; 1 to 30 layers; 5/3 and 9/7; vertically causal; the Part 1 sets' tiling, sampling, progression and marker variety | decodes 10: `ds0_ht_01, 09, 11, 12, 14`, `ds1_ht_04, 05, 06, 07`, `hifi_ht1_02`. The nine with Part 1 references are within the Part 1 class-1 bounds; the five `ds0` files and `ds1_ht_07` are bit-exact. Refuses the 3 MIXED files at the header, `ds0_ht_13` (ROI), and the other 27 multi-layer files |
| **Part 4 JPH files**: same repository, `testfiles_jph` | 20, 9 MB: `file1`..`file9` at two or three bounds, plus `file10` at two | same grant | HT transcodes of the nine Part 4 JP2 files (`colr` enumerated and restricted ICC, `pclr`/`cmap`, `cdef`, `xml `); `file10`: five components, no `colr`, a `cdef` with four `Typ` 3 channels and one opacity channel | decodes 18. The highest-bound file of each set matches the `fileN.jp2` decode bit for bit. Refuses `file10_b11` and `file10_b12` ("Invalid component index"); their references are five `.pgm` files in `reference_jph/` |
| **openjpeg-data** `input/nonregression/htj2k/`, commit `39524bd3` (the commit already pinned for the Part 1 corpus) | 4: `Bretagne1_ht.j2k`, `Bretagne1_ht_lossy.j2k`, `byte.jph`, `byte_causal.jhc` | no license statement | single-layer HT: RGB 5/3 and 9/7, gray in JPH, vertically causal (`cblksty 0x48`) | decodes all 4 |
| **OpenJPH test codestreams**: `github.com/aous72/jp2k_test_codestreams` `openjph/`, commit `1ccd349` | 70, 39 MB; 16 under 100 KB (420 KB in all) | BSD-2-Clause | OpenJPH encoder output: single layer, HT only; code-block sizes from 4×4 to 1024×4; all five progression orders; tiles; YUV with sub-sampled chroma; 16-bit; gray; tall and narrow images | decodes 69. Refuses `simple_dec_irv53_bhvhb_low_latency.jph`, a Part 2 codestream (`Rsiz` `0xC0A0`) |

Two facts shape the grading:

- **The HT references are mostly the Part 1 references.** Each transcode
  script names its Part 1 source, so the class-1 `.pgx` references already in
  `tests/fixtures/conformance/references/` grade the `ds0` and `ds1` files, and
  the vendored `jp2/` references grade `file1`..`file9`. Only `file10` and
  `hifi_ht1_02` need new references (`reference_jph/`,
  `reference_class1HF_profile1/`, 426 KB together).
- **A lower bound is lossy on purpose.** Each set's `stats.txt` records
  Kakadu's PAE and MSE between the transcode and the Part 1 decode. At the
  highest bound it is 0. At `B = 11` it reaches PAE 64 on `ds0_ht_03` and PAE
  2198 on `ds0_ht_06`. So a low-bound entry can't reuse its Part 1 entry's
  bounds, which is why decision 4 takes them from T.803.

## Build order and why

```text
P4.0 corpus + harness
P4.1 CAP/CPF parse + refusals ─┐
P4.2 tier-1 seam (#137) ───────┴─ P4.3 HT cleanup pass ─ P4.4 tier-2 HT segments ─ P4.5 SigProp + MagRef ─┬─ P4.6 ROI on HT blocks
                                                                                                          ├─ P4.7 MIXED
                                                                                                          └─ P4.8 JPH file format
P4.9 fuzz targets + reject matrix (starts after P4.3, runs throughout) ────────────────────────────────────── P4.10 integration + gate
```

P4.1 and P4.2 touch different modules and can run in parallel. P4.3 needs
both. P4.4 comes before the refinement passes because Tier-2 must hand Tier-1
both segments before a refinement pass can be read. P4.6, P4.7 and P4.8 each
need only P4.5 and can run in parallel worktrees.

## Steps

### P4.0 — HT conformance corpus and harness

**Goal:** the grading scaffold exists before the features it grades, as P2.0
and P3.0 did.

**Work:**
- Vendor the 41 HT codestreams and 20 JPH files into
  `tests/fixtures/conformance/ht/` from the pinned `wg1/htj2k-codestreams`
  commit, with its `COPYRIGHT.txt` copied verbatim, under the existing
  `exclude`. Extend `regenerate.py` to fetch them and to convert
  `reference_jph/*.pgm` and `reference_class1HF_profile1/*.pgx` for the two
  entries without a Part 1 reference.
- Vendor the 16 OpenJPH files under 100 KB with their license notice and
  `opj_decompress` snapshots, excluded from the crate the same way.
- A manifest entry per file: source Part 1 entry, the `Ccap15` fields, whether
  `CPF` is present, layers, whether any block has placeholder or refinement
  passes, references, bounds (decision 4), `bit_exact`, and Kakadu's
  `stats.txt` PAE and MSE as the recorded cross-check.
- `tests/conformance_ht.rs`: the same grading and `IN_CLASS` ratchet as
  `tests/conformance_part4.rs`, reusing `tests/support/part4.rs`. Every entry
  reports *not yet decoded*.

**Oracle:** the ISO references; this step is the harness.

**Gate:** every reference parses; every bound passes its cross-check; all
entries report *not yet decoded*; `cargo publish --dry-run` lists no
conformance or OpenJPH file.

### P4.1 — `CAP` and `CPF` parsing, and the HT refusals (`src/codestream/`)

**Goal:** the main header reads the Part 15 signaling and refuses exactly what
the decoder can't read yet.

**Work:**
- Parse `CAP`: `Lcap`, `Pcap`, then one `Ccap` word per set `Pcap` bit, with
  the segment length checked against the bit count. Keep `Ccap15` on
  `MainHeader` as a small struct. A `Pcap` bit other than Part 15's is
  `Unsupported`, naming the part. Parse `CPF` and keep it. Refuse a second
  `CAP` or `CPF`, and either one in a tile-part header (OpenJPEG reads both in
  the main header only).
- Record the `0x40` and `0x80` style bits on `Coding` instead of refusing them
  in `decode_coding`. The refusal moves to one guard that P4.3, P4.5 and P4.7
  relax in turn. Until P4.3, any code-block style with `0x40` is `Unsupported`.
- Refuse MULTIHT (decision 5). The code-block style bits decide which coder a
  component uses; `CAP` is cross-checked but does not override them.
- The reject matrix in `src/codestream/tests.rs`: the `CAP` row moves from
  "outside the decoded subset" to the new rows.

**Oracle:** the marker bytes of the 41 conformance codestreams against their
`jp2codestream.py` logs (`descriptions_htj2k_profile0/1`), checked by a test
that decodes `Ccap15` for every file.

**Gate:** every Part 4 HT file reaches the new `Unsupported` guard, not a
parse error; the Part 1 and JP2 corpora are unchanged; the reject matrix
passes.

### P4.2 — Tier-1 dispatch seam (`src/tier1/`, #137)

**Goal:** assembly dispatches through a block-coder seam, so an `ht` module
can sit beside EBCOT.

**Work:** move `passes.rs` and `mq.rs` behind an `ebcot` submodule. Give
`assemble` and `decode_subband` a seam (an enum over the two coders is enough;
a trait only if it earns its keep) that takes a code-block's segments,
geometry, `numbps`, zero bit-planes and `BlockParams`, and fills a coefficient
buffer at the scale the EBCOT path uses. The `ht` stub returns `Unsupported`.
`tier1::work` asks the seam for a block's cost.

**Oracle:** the existing suite, bit for bit.

**Gate:** `cargo test` unchanged; 23 of 23 Part 1 and 9 of 9 JP2 entries in
class; the `tier1_block` and `mq` fuzz targets build unchanged or with renamed
paths only.

### P4.3 — HT cleanup pass: MEL, VLC, UVLC and MagSgn (`src/tier1/ht/`)

**Goal:** a code-block with one cleanup pass decodes.

**Work:** the cleanup segment layout (`Lcup`, and `Scup` from the last two
bytes, within its bounds); the MEL decoder; the backward VLC stream with its
two context tables (the first quad row and the rest) and the UVLC prefix and
suffix; the forward MagSgn stream; quad-pair decoding, significance and
exponent prediction. The tables come from the script of decision 6, with a
test that pins their checksum. Vertically causal mode applies.

**Oracle:** `opj_decompress` 2.5.0 on the single-layer files in the corpora
whose blocks carry only a cleanup pass (listed from P4.0's manifest), with
`byte.jph` read as a raw `jp2c` payload until P4.8. Per-stage golden vectors
for MEL and VLC slice code-blocks out of these files and take the coefficients
from OpenJPEG's decode; OpenJPH (decision 3) makes small synthetic blocks for
the edge cases.

**Gate:** those files match OpenJPEG; the Part 4 ones among them are in
`IN_CLASS`.

### P4.4 — Tier-2 for HT code-blocks (`src/tier2/`)

**Goal:** packet headers hand Tier-1 the cleanup and refinement segments of
every HT block, across any number of layers.

**Work:** an HT branch beside `segment_max_passes`: the placeholder and
cleanup split, the missing-MSB increment per placeholder triple, the two
length-field widths and the segment length limits from "What HTJ2K changes",
and the T.814 rules for passes that arrive in later layers. Under
`with_strict(false)`, a cut HT segment is reported the way a Part 1 segment
is. Update the packet work count.

**Oracle:** the Part 4 references for the multi-layer entries whose blocks
need only cleanup passes once this step lands (listed from P4.0's manifest);
OpenJPH's decoder, which parses placeholder passes, wherever it decodes the
file; and the packet-header bytes of `ds0_ht_02` and `ds0_ht_05` decoded by
hand in unit tests. OpenJPEG can't grade this step.

**Gate:** no Part 4 HT entry fails with a Tier-2 error; every entry that needs
only cleanup passes is in class; the Part 1 Tier-2 fixtures are unchanged.

### P4.5 — SigProp and MagRef passes (`src/tier1/ht/`)

**Goal:** the refinement segment decodes, so a full HT set decodes.

**Work:** the SigProp pass (forward stream, neighbors from the cleanup pass's
significance) and the MagRef pass (backward stream), each run only when its
pass is present. A zero-length refinement segment with passes declared:
measure what T.814 gives against OpenJPEG's cleanup-only fallback, which it
applies to `ds1_ht_05` and `ds1_ht_06`, and follow the standard where they
differ (decision 1). Lift the `0x40` guard for HT-only codestreams
without RGN.

**Oracle:** `opj_decompress` on every file it decodes; the Part 4 references
for the rest.

**Gate:** every HT-only Part 4 entry without RGN is in class, and every file in
the corpora that OpenJPEG decodes matches it.

### P4.6 — ROI on HT blocks (`src/tier1/`)

**Goal:** HT blocks honor a maxshift `RGN`.

**Work:** apply `roi_shift` to HT output the way `undo_maxshift` does for
EBCOT; check `top_coded_plane` against the `B` bound; cross-check the
`Ccap15` RGN bit against the `RGN` markers.

**Oracle:** the Part 4 references for `ds0_ht_03`, `06`, `13` and `15` (eight
files). OpenJPEG refuses ROI on HT blocks.

**Gate:** those entries in class.

### P4.7 — MIXED code-blocks (`src/tier1/`, `src/tier2/`)

**Goal:** a codestream with `cblksty & 0x80` decodes, each code-block with the
coder the standard selects for it.

**Work:** the Part 15 rules for choosing a MIXED block's coder and for its pass
and segment accounting, read from T.814 and implemented through the seam; lift
the `0x80` guard.

**Oracle:** the Part 4 references for `ds0_hm_06_b11`, `ds0_hm_06_b18` and
`ds0_hm_15_b8`. Neither OpenJPEG nor OpenJPH decodes MIXED.

**Gate:** the three entries in class.

### P4.8 — JPH file format (`src/jp2/`)

**Goal:** a `.jph` file decodes through `decode` like a `.jp2`.

**Work:** accept `jph ` as brand or compatible brand; apply the Annex D
relaxations, so a missing `colr` gives `ColorSpace::Unknown`, the `cdef`
`Asoc` range check skips `Typ` 3 channels, and such a channel is reported
through the existing `ChannelType::Other(3)` and `Association` values (no new
API, decision 7); the reject-matrix rows in `src/jp2/tests.rs` for the remaining
JPX, JPM and MJ2 brands.

**Oracle:** the Part 4 JP2 references for `file1`..`file9` (sRGB conversion as
in `tests/conformance_jp2.rs`); `opj_decompress` on the 18 it decodes; the
`reference_jph/` images for `file10`.

**Gate:** all 20 JPH files in class; `byte.jph` matches OpenJPEG.

### P4.9 — Fuzz targets and the reject matrix (`fuzz/`)

**Goal:** the HT block decoder and the HT Tier-2 branch are fuzzed from the
step that adds them.

**Work:** a `tier1_ht_block` target that feeds bytes straight to the HT block
decoder with fuzzed geometry, `Lcup`/`Scup` split and pass count; HT files as
seeds for `decode`, `tile_body` and `decode_partial`; `CAP`, `CPF` and
`cblksty` entries in the dictionary; the 60-second CI smoke for the new
target; a work bound for an HT block in `tier1::work` and
[resource-bounds.md](resource-bounds.md).

**Oracle:** no panic, bounded allocation, every reject a typed `Error`.

**Gate:** a documented session of up to 60 seconds per target (the shared-CPU
budget) finds nothing; the reject matrices pass.

### P4.10 — Integration and the phase gate

**Work:** the `scope.md` Part 15 rows and the two Part 1 rows; `roadmap.md`;
`correctness.md` gains the HT corpus; `openjpeg-departures.md` lists each place
this decoder decodes what OpenJPEG refuses (placeholder passes, ROI, MIXED,
`file10`) and the zero-length refinement result; `resource-bounds.md`;
`tests/fixtures/README.md` provenance; `lib.rs` docs, README and CHANGELOG;
the `RELEASING.md` pre-release review.

**Gate (the Phase 4 exit):**
- all 41 Part 4 HT codestreams and 20 JPH files decode within their bounds
  (decision 4), tracked by `IN_CLASS` in `tests/conformance_ht.rs`;
- every HT file in the corpora that OpenJPEG 2.5.0 decodes matches it bit for
  bit, apart from the listed departures;
- the 23 Part 1 entries, the nine JP2 files and every earlier fixture are
  unchanged;
- fuzzing is clean and the quality gates are green.

## Out of scope for Phase 4

Refused cleanly, never attempted: MULTIHT; Part 2 codestreams, HT or not
(`DFS`, `ATK` and the rest stay Phase 6); `CAP` bits for parts other than 15.
The HTJ2K encoder is Phase 6. `hifi_p1_02.j2k`, the Part 1 codestream beside
`hifi_ht1_02`, is not part of the HT set. See [roadmap.md](roadmap.md) and
[scope.md](scope.md).

## Sources

- OpenJPEG 2.5.0 source, tag `v2.5.0` (`a5891555`): `src/lib/openjp2/ht_dec.c`,
  `j2k.c` (`opj_j2k_read_cap`, `opj_j2k_read_cpf`, `opj_j2k_read_SPCod_SPCoc`),
  `t2.c` (`opj_t2_read_packet_header`), `tests/nonregression/test_suite.ctest.in`.
- `https://gitlab.com/wg1/htj2k-codestreams`, commit `f6b9ede`: codestreams,
  `transcode` scripts, `stats.txt`, syntax logs, `COPYRIGHT.txt`.
- `https://github.com/uclouvain/openjpeg-data`, commit `39524bd3`:
  `input/nonregression/htj2k/`.
- `https://github.com/aous72/jp2k_test_codestreams`, commit `1ccd349`, and
  `https://github.com/aous72/OpenJPH` (`Ccap15` writing in
  `src/core/codestream/ojph_params_local.h`, placeholder-pass parsing in
  `ojph_precinct.cpp`, the COD check in `ojph_params.cpp` that refuses MIXED).
- ITU-T T.814 and T.803: `https://www.itu.int/rec/T-REC-T.814`,
  `https://www.itu.int/rec/T-REC-T.803`.
