# Phase 4 roadmap — HTJ2K decode (Part 15)

The overall [roadmap.md](roadmap.md) sequences the whole project; this file
covers Phase 4 alone. It breaks "decode HTJ2K codestreams" into ordered
steps, each tied to the module it changes, the oracle that checks it and the
gate that lets the next step start. The decisions that shape the plan are in
[Decisions](#decisions), after the background they rely on.

The feature rows this phase covers are the Part 15 rows in
[scope.md](scope.md), plus the two Part 1 rows that mention HTJ2K (the `CAP`
marker and the code-block style bits). The specification is ISO/IEC
15444-15:2019 | ITU-T T.814 (06/2019); the clause numbers below are its. The
conformance authority is ISO/IEC 15444-4 | ITU-T T.803, which adds HTJ2K
codestreams and JPH files to the Part 1 test set.

## What Phase 4 decodes

Phase 3 decodes the Part 1 codestreams listed in [scope.md](scope.md), raw or
in a JP2 file. Phase 4 adds HTJ2K codestreams: those whose code-blocks are
coded with the HT block coder, either all of them or some of them with the
rest coded as in Part 1. They decode raw, in a JP2 file, or in a JPH file, the
HTJ2K variant of JP2. A Part 15 feature outside the plan is refused with
`Error::Unsupported`, as any feature outside the decoded subset is now.

## What HTJ2K changes

Part 15 replaces the Tier-1 block coder and leaves the rest of Part 1 in
place (clause 6.2). The codestream syntax, Tier-2 packets, dequantization,
DWT and MCT stay as they are, apart from the changes below.

### Signaling

SIZ `Rsiz` bit 14 (`0x4000`) says Part 15 capabilities are in use, and a `CAP`
marker (`0xFF50`) must follow SIZ (A.2, A.3). `Pcap` is a 32-bit mask whose
Part 15 bit (`0x0002_0000`) must be set; it carries one 16-bit `Ccap15` word.
A `CPF` marker (`0xFF59`) may name the Part 1 profile the codestream
transcodes to and from (A.6). Every Part 4 HT codestream sets `Rsiz` bit 14
and carries `CAP`; four of them leave out `CPF`. The decoder refuses `CAP` in
the main header today.

`Ccap15` (Table A.2) describes which of the constrained sets of clause 8 the
codestream may belong to, so a decoder can refuse early what it doesn't
support:

- Bits 15–14 give the coder set. `00` is HTONLY: every code-block is an HT
  code-block. `10` is HTDECLARED: each tile-component is coded entirely with
  one coder or the other. `11` allows MIXED: one tile-component can hold both
  kinds of code-block. Every Part 4 HT codestream is `00` except the three
  MIXED files, which are `11`. None is `10`.
- Bit 13 allows MULTIHT: a code-block may carry more than one HT set (see
  below), for example to keep several quality levels of the same block. No
  corpus file sets it.
- Bit 12 says `RGN` markers may be present. It is set on exactly the files
  that carry one.
- Bit 11 says the codestream may be HETEROGENEOUS (8.5): functional markers
  such as COD, COC, QCD or POC appear in tile-part headers, or PPT is used. The
  decoder already reads those, so the bit is recorded and not needed to
  decode. 17 codestreams set it.
- Bit 5 says HT code-blocks may be used with the irreversible transform.
- Bits 4–0 hold `P`, which gives the cleanup magnitude bound `B` (A.3.7): 8
  when `P` is 0, `P + 8` below 20, `4(P − 19) + 27` from 20 to 30, and 74 at
  31. Every `_bNN` in a file name is that file's `B`: 8 to 18 for the HT
  codestreams, up to 19 for the JPH files.

### Code-block style

`SPcod`/`SPcoc` bit 6 (`0x40`) selects the HT block coder for the
tile-component, and bits 6 and 7 together (`0xC0`) say its code-blocks may be
of either kind (A.4). `decode_coding` refuses both today. Vertically causal
context (`0x08`) still applies, and four Part 4 HT codestreams and
`byte_causal.jhc` use it. The other Part 1 style bits have no meaning for an
HT code-block.

### Block coding

An HT code-block is coded in passes, like a Part 1 block, but the passes come
in groups of three called *HT sets* (B.1): a cleanup pass, then optionally a
SigProp (significance propagation) pass and a MagRef (magnitude refinement)
pass. A decoder reads one HT set per block (clause 7). Before the first HT set
can come `3 × P0` *placeholder passes*: passes that are counted in the packet
headers but carry no bytes. A transcoder writes them to keep the quality layer
boundaries of the Part 1 codestream it started from, which is why the Part 4
HT set has them. Each group of three placeholder passes adds one to the
block's count of skipped magnitude bit-planes (B.3).

The cleanup pass works on *quads*, 2×2 groups of samples. Its segment, of
length `Lcup`, holds three streams (7.1.1):

- the MagSgn stream, read forward from the start: the raw magnitude and sign
  bits of each significant sample;
- the MEL stream, an adaptive run-length code read forward from byte
  `Lcup − Scup`: for a quad whose neighbors are all insignificant, whether it
  has any significant sample;
- the VLC stream, read backward from the end: context-coded variable-length
  codewords that give each quad's significance pattern, plus U-VLC codes for
  the residual that bounds how many MagSgn bits each sample takes.

The last two bytes of the segment give `Scup`, the length of the MEL and VLC
part, with `2 <= Scup <= min(Lcup, 4079)`. The VLC codewords come from two
tables, `CxtVLC_table_0` for the first row of quads and `CxtVLC_table_1` for
the rest (Annex C). The SigProp and MagRef passes share a second segment, the
refinement segment: SigProp is read forward from its start and MagRef
backward from its end.

### Tier-2
 An HT block's passes split into segments by the rule of B.2, not by the Part 1
`segment_max_passes` rule. In the packet with a block's first contribution, the
first segment holds the placeholder passes plus the one cleanup pass, `3k + 1`
passes in all, and the second holds up to two refinement passes (B.3). The first
contribution can also hold only placeholder passes (B.3 NOTE 4); the cleanup
pass then arrives in a later layer, and the B.2 rule covers both cases. The
length fields are `Lblock + floor(log2(3k + 1))` bits for the cleanup segment
and `Lblock + floor(log2(n))` bits for `n` refinement passes. The cleanup
segment is under 65535 bytes and the refinement segment under
2047.

### File format
 JPH (Annex D) is the JP2 format with the `jph ` brand, which `ftyp` must give
as its brand and in its compatibility list (D.3). Its `colr` box may be absent
when `ihdr`'s `UnkC` is set, and then no channel may be `Typ` 0 (D.2). Its
`cdef` box allows a channel type `Typ` 3, a color the application defines, with
an `Asoc` value the application defines too (D.6.3). Every Part 4 JPH file lists
only `jph ` in `ftyp`, so the decoder's `jp2 `
compatibility check refuses them today.

## What OpenJPEG 2.5.0 decodes

`/usr/bin/opj_decompress` (Ubuntu `2.5.0-2ubuntu0.5`) decodes HTJ2K, but not
all of Part 15. The limits, from the source at tag `v2.5.0` (`a5891555`) and
confirmed by running it on the corpora below:

| Feature | OpenJPEG 2.5.0 | Where |
| ------- | -------------- | ----- |
| `CAP` and `CPF` | read and ignored (empty handlers, main header only) | `j2k.c` `opj_j2k_read_cap`, `opj_j2k_read_cpf` |
| MIXED (code-block style `0x80`) | header refused: "Unsupported Mixed HT code-block style found" | `j2k.c` `opj_j2k_read_SPCod_SPCoc` |
| More than 3 passes in a block (placeholder passes) | refused: "We do not support more than 3 coding passes in an HT codeblock" | `ht_dec.c` `opj_t1_ht_decode_cblk` |
| Placeholder passes in Tier-2 | the first segment always gets one pass, so the length field is read at the wrong width and the packet parse loses sync ("segment too long", "Expected EPH marker") | `t2.c` `opj_t2_read_packet_header` |
| ROI (maxshift) on HT blocks | refused: "We do not support ROI in decoding HT codeblocks" | `ht_dec.c` |
| More than 30 magnitude bit-planes (`Mb > 30`) | refused | `ht_dec.c` |
| Refinement segment of zero length with more than one pass | warns, then decodes the cleanup pass only | `ht_dec.c` |
| SigProp and MagRef in one HT set | decoded | `ht_dec.c` |

OpenJPEG's own test suite runs only the four openjpeg-data HT files
(`tests/nonregression/test_suite.ctest.in`), none of the Part 4 set.

The Part 4 HT codestreams were made by transcoding the Part 1 conformance
codestreams, so most of them have several quality layers, and placeholder
passes with them. OpenJPEG decodes 10 of the 41: the 9 single-layer
codestreams other than `ds0_ht_13` (ROI), and `ds1_ht_05` (2 layers). The nine
of those with Part 1 references are within the Part 1 class-1 bounds, and six
are bit-exact (`ds0_ht_01, 09, 11, 12, 14` and `ds1_ht_07`). It refuses the 3
MIXED files at the header, `ds0_ht_13`, and the other 27 multi-layer
codestreams.

## The corpora

Measured on 2026-11-14 by running `opj_decompress` 2.5.0 on every file and,
where it decoded, grading its output against the ISO reference.

| Corpus | Files | License | What it covers | `opj_decompress` 2.5.0 |
| ------ | ----- | ------- | -------------- | ---------------------- |
| Part 4 HT codestreams: `gitlab.com/wg1/htj2k-codestreams` (`htj2k_bsets_profile0`, `htj2k_bsets_profile1`, `codestreams_hifi`), commit `f6b9ede` | 41, 40 MB: 27 `ds0_ht_*`, 3 `ds0_hm_*` (MIXED), 10 `ds1_ht_*`, `hifi_ht1_02`. 32 MB of it is the `p0_07` and `p0_08` sets | the conformance-only grant (`COPYRIGHT.txt`, "Copyright (c) 2002, 2021") | Kakadu `kdu_transcode` of each Part 1 `p0_*`/`p1_*` codestream at one to three magnitude bounds; HTONLY and MIXED; RGN; 1 to 30 layers; 5/3 and 9/7; vertically causal; the Part 1 sets' tiling, sampling, progression and marker variety | decodes 10 (see above) |
| Part 4 JPH files: same repository, `testfiles_jph` | 20, 9 MB: `file1`..`file9` at one to three bounds, plus `file10` at two | same grant | HT transcodes of the nine Part 4 JP2 files (`colr` enumerated and restricted ICC, `pclr`/`cmap`, `cdef`, `xml `); `file10`: five components, no `colr`, a `cdef` with four `Typ` 3 channels and one opacity channel | decodes 18. The highest-bound file of each set matches the `fileN.jp2` decode bit for bit. Refuses `file10_b11` and `file10_b12` ("Invalid component index"); their references are five `.pgm` files in `reference_jph/` |
| openjpeg-data `input/nonregression/htj2k/`, commit `39524bd3` (the commit already pinned for the Part 1 corpus) | 4: `Bretagne1_ht.j2k`, `Bretagne1_ht_lossy.j2k`, `byte.jph`, `byte_causal.jhc` | no license statement | single-layer HTONLY: RGB 5/3 and 9/7, gray in JPH, vertically causal | decodes all 4 |
| OpenJPH test codestreams: `github.com/aous72/jp2k_test_codestreams` `openjph/`, commit `1ccd349` | 70, 39 MB; 16 under 100 KB (420 KB in all) | BSD-2-Clause | OpenJPH encoder output: single layer, HTONLY; code-block sizes from 4×4 to 1024×4; all five progression orders; tiles; YUV with sub-sampled chroma; 16-bit; gray; tall and narrow images | decodes 69. Refuses `simple_dec_irv53_bhvhb_low_latency.jph`, a Part 2 codestream (`Rsiz` `0xC0A0`) |

Two facts shape the grading. First, the HT references are mostly the Part 1
references. Each transcode script names its Part 1 source, so the class-1
`.pgx` references already in `tests/fixtures/conformance/references/` grade the
`ds0` and `ds1` codestreams, and the vendored `jp2/` references grade
`file1`..`file9`. Only `file10` and `hifi_ht1_02` need new references
(`reference_jph/` and `reference_class1HF_profile1/`, 426 KB together).

Second, a lower bound is lossy on purpose. Each set's `stats.txt` records
Kakadu's PAE and MSE between the transcode and the Part 1 decode. At the
highest bound it is 0. At `B = 11` it reaches PAE 64 on `ds0_ht_03` and PAE
2198 on `ds0_ht_06`. So a low-bound entry can't reuse its Part 1 entry's
bounds, which is why decision 4 takes them from T.803.

## Decisions

Settled while planning, on 2026-11-14. Below, *HT codestream* means one of the
41 Part 4 HT codestreams, *JPH file* one of the 20 Part 4 JPH files, and
*entry* either.

1. **The gate is the Part 4 HT set, graded against the standard.** Every HT
   codestream and JPH file decodes within its ISO bounds, the three MIXED
   codestreams included. Wherever OpenJPEG 2.5.0 decodes a file, the output
   also matches it bit for bit, apart from the departures listed in
   [openjpeg-departures.md](openjpeg-departures.md). That list includes any
   departure this phase adds, each with the spec text that justifies it.
   Where the standard and OpenJPEG disagree, the standard wins.
2. **All HT codestreams and JPH files are vendored**, under the same
   conformance-only grant as the Part 1 corpus and excluded from the published
   crate like `tests/fixtures/conformance/`. So are the 16 OpenJPH test files
   under 100 KB, with their BSD-2-Clause notice. The four openjpeg-data HT
   files stay local and are not committed, like the rest of `nonregression/`.
3. **OpenJPH is a regeneration-only tool**, like `opj_compress`, pinned to a
   release tag chosen in P4.0. It has two jobs: it generates synthetic
   fixtures, and it is a second oracle for single-layer HT codestreams. Its
   decoder reads one quality layer only and skips `RGN`, so placeholder passes
   and multi-layer files are graded only against the ISO references. It is
   never a build or test dependency, so the crate stays pure Rust and the
   suite stays offline.
4. **Bounds come from T.803 C.2.** Each entry's PAE and MSE bounds are
   transcribed from the HTJ2K tolerances there. Every transcribed number must
   also pass a cross-check: the source Part 1 entry's bound plus the transcode
   error Kakadu recorded for that file (PAE adds; for MSE, square roots add).
   If T.803 can't be obtained, only the highest-bound file of each set is
   graded (it is lossless against its Part 1 decode), and every other entry is
   recorded with the reason it is not graded.
5. **MULTIHT is refused with `Error::Unsupported`.** No corpus file sets it,
   so there is no oracle for it. HTDECLARED codestreams are accepted: the
   code-block style bits already say which coder each tile-component uses.
6. **The VLC and UVLC tables are generated from the T.814 text** (Annex C) by
   a committed script, and tested against OpenJPEG's behavior through the
   end-to-end fixtures. The BSD-licensed tables in OpenJPEG and OpenJPH are
   not copied, so no third-party notice ships in the crate.
7. **One release.** The phase ships as 0.5.0 when the gate passes, with no
   interim HT release. Codestreams that were `Unsupported` now decode, which
   is a behavior change. The HTJ2K steps add two enum variants for JPH's
   application-defined channels (P4.7), `ChannelType::ApplicationDefined` and
   `Association::ApplicationDefined(u16)`, and no other API.

## What else ships in 0.5.0

0.5.0 also carries the streaming and partial-decode work in the same
milestone, which does change the public API: a signal that tells a cut-short
main header apart from a damaged one (#305), `DecodeOptions::max_quality_layers`
to decode only the first layers (#215), decoding a region, a tile or a subset
of components (#216), and a stateful incremental decoder (#217), which is
correct when its output equals a one-shot decode of the same prefix. These
run as their own chain beside the HTJ2K steps and are planned in their
issues.

## Build order and why

```text
P4.0 corpus + harness ─ P4.1 CAP/CPF + refusals ─ P4.2 tier-1 seam + HT cleanup ─ P4.3 tier-2 HT segments ─ P4.4 SigProp + MagRef ─┬─ P4.5 ROI on HT blocks ─ P4.6 MIXED
                                                                                                                                    └─ P4.7 JPH file format
P4.8 fuzz targets + reject matrix (starts after P4.2, runs throughout) ─────────────────────────────────────────────────────────────── P4.9 integration + gate
```

P4.0 comes first, as in Phases 2 and 3. The Tier-1 seam lands in P4.2 with the
first HT decoder, so the second implementation shapes it (#137). Moving EBCOT
behind the seam touches only `src/tier1/` and can start beside P4.1; the HT half
of P4.2 needs P4.1's parsing. P4.3 comes before the refinement passes because
Tier-2 must hand Tier-1 both segments before a refinement pass can be read. P4.5
and P4.7 each need only P4.4 and can run in parallel. P4.6 needs P4.5 as well,
because all three MIXED codestreams carry an `RGN` marker.

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
  `opj_decompress` snapshots, excluded from the crate the same way. Choose and
  record the OpenJPH release tag (decision 3).
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
  in `decode_coding`. The refusal moves to one guard that later steps relax
  in turn: P4.2 for blocks with only a cleanup pass, P4.4 for HTONLY
  codestreams without `RGN`, P4.5 for those with `RGN`, and P4.6 for MIXED.
  Until P4.2, any code-block style with `0x40` is
  `Unsupported`.
- Refuse MULTIHT (decision 5). The code-block style bits decide the coder of
  each tile-component, or that it may mix both (MIXED); `CAP` is cross-checked
  but does not override them.
- T.814 requires `Rsiz` bit 14 and `CAP` in every HTJ2K codestream. For HT
  code-block style without one of them, measure OpenJPEG on each shape and
  follow correctness.md's rule order.
- The reject matrix in `src/codestream/tests.rs`: the `CAP` row moves from
  "outside the decoded subset" to the new rows.

**Oracle:** the marker bytes of the HT codestreams against their
`jp2codestream.py` logs (`descriptions_htj2k_profile0/1`), checked by a test
that decodes `Ccap15` for every file. There are 40 logs for the 41
codestreams: `ds0_ht_06_b15` has none, so its `Ccap15` is checked against its
`_b15` name and its `_b11` and `_b18` siblings.

**Gate:** each of the 41 HT codestreams reaches the new `Unsupported` guard,
not a parse error; the Part 1 and JP2 corpora are unchanged; the reject matrix
passes.

### P4.2 — Tier-1 seam and cleanup-only HT blocks (`src/tier1/`, #137)

**Goal:** assembly dispatches through a block-coder seam, and an HT block with
one cleanup pass decodes through it.

**Work:**
- Move `passes.rs` and `mq.rs` behind an `ebcot` submodule. Give `assemble`
  and `decode_subband` a seam (an enum over the two coders is enough; a trait
  only if it earns its keep) that takes a code-block's segments, geometry,
  `numbps`, zero bit-planes and `BlockParams`, and fills a coefficient buffer
  at the scale the EBCOT path uses. `tier1::work` asks the seam for a block's
  cost. The HT decoder below is the seam's second implementation, so the two
  land together, as #137 requires.
- In `src/tier1/ht/`: the cleanup segment layout (`Lcup`, and `Scup` from the
  last two bytes, within its bounds); the MEL decoder; the backward VLC stream
  with its two context tables and the U-VLC prefix and suffix; the forward
  MagSgn stream; quad-pair decoding, significance and exponent prediction. The
  tables come from the script of decision 6, with a test that pins their
  checksum. The cleanup pass honors vertically causal context (`0x08`).

**Oracle:** the existing suite, bit for bit, for the move. For the HT decoder,
`opj_decompress` 2.5.0 and OpenJPH on the single-layer files in the corpora
whose blocks carry only a cleanup pass (listed from P4.0's manifest), with the
`.jph` files (`byte.jph` and the OpenJPH `simple_dec_*` files) read as raw
`jp2c` payloads until P4.7. Per-stage golden vectors for MEL and VLC slice
code-blocks out of these files and take the coefficients from OpenJPEG's
decode; OpenJPH makes small synthetic blocks for the edge cases.

**Gate:** `cargo test` unchanged; 23 of 23 Part 1 and 9 of 9 JP2 entries in
class; the `tier1_block` and `mq` fuzz targets build unchanged or with renamed
paths only; the HT files above match OpenJPEG, and the Part 4 ones among them
are in `IN_CLASS`.

### P4.3 — Tier-2 for HT code-blocks (`src/tier2/`)

**Goal:** packet headers hand Tier-1 the cleanup and refinement segments of
every HT block, across any number of layers.

**Work:** an HT branch beside `segment_max_passes`. It splits the first
contribution into placeholder passes and one cleanup pass, adds one to the
skipped bit-plane count for every three placeholders, reads the two length
fields at their widths and enforces the segment limits from
[Tier-2](#tier-2). It segments passes that arrive in later layers by the B.2
rule. Under `with_strict(false)`, a cut HT segment is reported the way a
Part 1 segment is. Update the packet work count.

**Oracle:** the Part 4 references for the multi-layer entries whose blocks
need only cleanup passes once this step lands (listed from P4.0's manifest),
and the packet-header bytes of `ds0_ht_02` and `ds0_ht_05` decoded by hand in
unit tests. Neither OpenJPEG nor OpenJPH can grade this step: OpenJPEG refuses
placeholder passes, and OpenJPH reads one quality layer only.

**Gate:** no HT codestream fails with a Tier-2 error; every entry that needs
only cleanup passes is in class; the Part 1 Tier-2 fixtures are unchanged.

### P4.4 — SigProp and MagRef passes (`src/tier1/ht/`)

**Goal:** the refinement segment decodes, so a full HT set decodes.

**Work:** the SigProp pass (forward stream, neighbors from the cleanup pass's
significance) and the MagRef pass (backward stream), each run only when its
pass is present. A refinement segment of zero length with passes declared
gives `Z_blk = 1` (B.3), so only the cleanup pass is decoded. OpenJPEG does
the same for `ds1_ht_05` and `ds1_ht_06`, though it warns that the block is
malformed. Lift the `0x40` guard for HTONLY codestreams without
`RGN`.

**Oracle:** `opj_decompress` on every file it decodes, and OpenJPH on the
single-layer files without `RGN`; the Part 4 references for the rest.

**Gate:** every HTONLY HT codestream without `RGN` is in class, and every
codestream in the corpora that OpenJPEG decodes, with `.jph` files read as raw
`jp2c` payloads, matches it.

### P4.5 — ROI on HT blocks (`src/tier1/`)

**Goal:** HT blocks honor a maxshift `RGN`.

**Work:** apply `roi_shift` to HT output the way `undo_maxshift` does for
EBCOT; check `top_coded_plane` against the `B` bound; cross-check the
`Ccap15` RGN bit against the `RGN` markers; lift the guard for HTONLY
codestreams with `RGN`.

**Oracle:** the Part 4 references for `ds0_ht_03`, `06`, `13` and `15` (eight
files). OpenJPEG refuses ROI on HT blocks.

**Gate:** those entries in class.

### P4.6 — MIXED code-blocks (`src/tier1/`, `src/tier2/`)

**Goal:** a tile-component with code-block style `0xC0` decodes, each
code-block with its own coder.

**Work:** the A.4 rules for a MIXED tile-component. In an HT code-block, the
first non-empty codeword segment's length starts with a 0 bit and `Lblock` is
above 3; the Part 1 code-blocks use neither bypass nor termination on each
pass. A.4's note says a decoder tells the two apart by processing the block as
an HT block and falling back to Part 1 when that fails. Implement that choice
and each kind's pass and segment accounting through the seam, measured
against the three files; lift the `0x80` guard.

**Oracle:** the Part 4 references for `ds0_hm_06_b11`, `ds0_hm_06_b18` and
`ds0_hm_15_b8`. Neither OpenJPEG nor OpenJPH decodes MIXED.

**Gate:** the three entries in class.

### P4.7 — JPH file format (`src/jp2/`)

**Goal:** a `.jph` file decodes through `decode` like a `.jp2`.

**Work:** accept `jph ` as brand or compatible brand. Apply the Annex D
relaxations: the `cdef` `Asoc` range check skips `Typ` 3 channels (the color
reorder already moves only `Typ` 0). A missing
`colr` already gives `ColorSpace::Unknown`; `file10` adds the test. Add the
reject-matrix rows in `src/jp2/tests.rs` for the remaining JPX, JPM and MJ2
brands.

A `Typ` 3 channel reports as a new `ChannelType::ApplicationDefined`, and its
`Asoc` as a new `Association::ApplicationDefined(u16)` carrying the value as
stored. Neither existing variant fits: `Other(3)` reads as a reserved type, and
`Color(n)` means color `n` of the color space. Both enums stay exhaustive, so
adding the variants is a breaking change for callers that match on them. A
0.x minor release allows that, and P4.7 lists both in the 0.5.0 CHANGELOG's
Migrating section.

**Oracle:** the Part 4 JP2 references for `file1`..`file9` (sRGB conversion as
in `tests/conformance_jp2.rs`); `opj_decompress` on the 18 it decodes; the
`reference_jph/` images for `file10`.

**Gate:** all 20 JPH files in class; `byte.jph` matches OpenJPEG.

### P4.8 — Fuzz targets and the reject matrix (`fuzz/`)

**Goal:** the HT block decoder and the HT Tier-2 branch are fuzzed from the
step that adds them.

**Work:** a `tier1_ht_block` target that feeds bytes straight to the HT block
decoder with fuzzed geometry, `Lcup`/`Scup` split and pass count; HT files as
seeds for `decode`, `tile_body` and `decode_partial`; `CAP`, `CPF` and
code-block style entries in the dictionary; the 60-second CI smoke for the new
target; a work bound for an HT block in `tier1::work` and
[resource-bounds.md](resource-bounds.md).

**Oracle:** no panic, bounded allocation, every reject a typed `Error`.

**Gate:** a documented session of up to 60 seconds per target (the shared-CPU
budget) finds nothing; the reject matrices pass.

### P4.9 — Integration and the phase gate
 **Work:** the `scope.md` Part 15 rows and the two Part 1 rows; `roadmap.md`;
`correctness.md` gains the HT corpus; `openjpeg-departures.md` lists each place
this decoder decodes what OpenJPEG refuses (placeholder passes, ROI, MIXED,
`file10`) and any departure found on files OpenJPEG decodes, each with its spec
text; `resource-bounds.md`; `tests/fixtures/README.md` provenance; `lib.rs`
docs, README and CHANGELOG; the `RELEASING.md` pre-release review.

**Gate (the Phase 4 exit):**
- every graded entry decodes within its bounds (decision 4), tracked by
  `IN_CLASS` in `tests/conformance_ht.rs`. With T.803's bounds that is all 41
  HT codestreams and 20 JPH files. Under decision 4's fallback it is the
  highest-bound file of each set, and every other entry is recorded with the
  reason it is not graded;
- every HT file in the corpora that OpenJPEG 2.5.0 decodes matches it bit for
  bit, apart from the departures listed in openjpeg-departures.md;
- the 23 Part 1 entries, the nine JP2 files and every earlier fixture are
  unchanged;
- fuzzing is clean and the quality gates are green.

## Out of scope for Phase 4

Refused cleanly, never attempted: MULTIHT; Part 2 codestreams, HT or not
(Phase 6); `CAP` bits for parts other than 15. JPH's extra `colr` methods
(`METH` 3, any ICC profile, and `METH` 5, H.273 parameters; D.4) are skipped as
JP2 skips an unknown `METH` today, so such a file reports its color space as
unknown; no corpus file uses them. Annex D's other `cdef` rules are not
checked or acted on: at most one opacity channel, with `Asoc` 0 (D.6.1);
several channels sharing one `Typ` and `Asoc` (D.6.2), which JPH allows; and
no `Typ` 0 channel when `colr` is absent (D.2). Shared pairs decode as in JP2
today: nothing is refused, and `Typ` 0 entries are swapped to position
`Asoc − 1` in file order, so the later of two that share a color ends there
and the earlier one takes the later one's old position. Other types stay where
the file put them. No corpus file needs the first or last of these rules;
`file10` shares an `Asoc` between two `Typ` 3 channels, which the reorder
leaves in place. The HTJ2K encoder is Phase 6.
`hifi_p1_02.j2k`, the Part 1 codestream beside `hifi_ht1_02`, is not part of
the HT set. See [roadmap.md](roadmap.md) and [scope.md](scope.md).

## Sources

- ITU-T T.814 (06/2019) | ISO/IEC 15444-15:2019,
  `https://www.itu.int/rec/T-REC-T.814`: clauses 6 to 8 and Annexes A to D.
- ITU-T T.803, `https://www.itu.int/rec/T-REC-T.803`: the HTJ2K tolerances in
  C.2.
- OpenJPEG 2.5.0 source, tag `v2.5.0` (`a5891555`): `src/lib/openjp2/ht_dec.c`,
  `j2k.c` (`opj_j2k_read_cap`, `opj_j2k_read_cpf`, `opj_j2k_read_SPCod_SPCoc`),
  `t2.c` (`opj_t2_read_packet_header`),
  `tests/nonregression/test_suite.ctest.in`.
- `https://gitlab.com/wg1/htj2k-codestreams`, commit `f6b9ede`: codestreams,
  `transcode` scripts, `stats.txt`, syntax logs, `COPYRIGHT.txt`.
- `https://github.com/uclouvain/openjpeg-data`, commit `39524bd3`:
  `input/nonregression/htj2k/`.
- `https://github.com/aous72/jp2k_test_codestreams`, commit `1ccd349`.
- `https://github.com/aous72/OpenJPH`, read at commit `1d155a0` (version
  0.33.0, untagged): `Ccap15` writing in
  `src/core/codestream/ojph_params_local.h`, placeholder-pass parsing in
  `ojph_precinct.cpp`, the COD check in `ojph_params.cpp` that refuses MIXED,
  and the one-layer limit in `ojph_codestream_local.cpp`.
