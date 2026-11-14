# Where this decoder departs from OpenJPEG

This page records every known place where this decoder and OpenJPEG 2.5.0 give
a different answer on the same file, or where this decoder reports something
OpenJPEG does not. Each entry starts with the same summary: what this decoder
does, what OpenJPEG does, what the standard says, which rule decides the case,
the tests and fixtures that pin it, and the issue and date of the decision
where one is recorded. The evidence follows.

The rules are the three in
[correctness.md](correctness.md#when-the-oracle-and-the-standard-disagree):
rule 1 follows the standard, rule 2 gives the error correction a user would
expect (OpenJPEG is usually the guide), and rule 3 returns an error rather than
a wrong image.

"OpenJPEG" means OpenJPEG 2.5.0. Every OpenJPEG source line cited here
(`t1.c:2061`, say) is from tag v2.5.0, commit `a589155`, under
`src/lib/openjp2/` for the library and `src/bin/` for `opj_decompress`. The
oracle binary is Ubuntu's 2.5.0-2ubuntu0.5 build. Where it is known to differ
from the tag, the entry says so ([chroma
replication](srgb-output.md#row-12-sycc-is-converted)).

Three other pages hold departures of their own:
[resource-bounds.md](resource-bounds.md) (files past the sample limit or a work
bound, which OpenJPEG decodes if it can allocate them; each bound has an option
that raises it) and [srgb-output.md](srgb-output.md) (sYCC with the IEC
constants, rounded, color channels picked by their `cdef` labels, and every
refusal under `ColorOutput::Srgb`).

## Index

| Entry | This decoder, against OpenJPEG | Rule |
| ----- | ------------------------------ | ---- |
| [The inverse 9/7's high-pass scale](#the-inverse-97s-high-pass-scale-2k) | exact `2/K`; small gap on 9/7 detail bands | 1 |
| [More than 999 layers](#more-than-999-layers) | decodes; OpenJPEG refuses or returns a wrong image | 1 |
| [A `Psot = 0` codestream followed by a box that ends in `FF 90`](#a-psot--0-codestream-followed-by-a-box-that-ends-in-ff-90) | decodes; OpenJPEG fails | 1 |
| [A header box at the top level before `jp2h`](#a-header-box-at-the-top-level-before-jp2h) | ignored; OpenJPEG reads it after an unknown box | 1 |
| [Bypass under a maxshift](#bypass-under-a-maxshift) | switches to raw coding where D.6 says; OpenJPEG `SPrgn` planes late | 1 |
| [Segmentation symbols](#segmentation-symbols) | checks the symbol; OpenJPEG never does | 3 |
| [A tile-part POC overrides the main header's](#a-tile-part-poc-overrides-the-main-headers) | replaces; OpenJPEG appends | 1 |
| [Code-block contributions of zero bytes](#code-block-contributions-of-zero-bytes) | refuses a wrong symbol from an empty terminated segment | 3 |
| [Stricter than the oracle on non-conforming streams](#stricter-than-the-oracle-on-non-conforming-streams) | refuses; OpenJPEG warns, reads on or invents values | 3 |
| [Packets a POC sequence leaves out](#packets-a-poc-sequence-leaves-out) | refuses; OpenJPEG decodes them empty | 3 |
| [Encoder mistakes OpenJPEG corrects](#encoder-mistakes-openjpeg-corrects) | corrects as OpenJPEG does; refuses mixed counts, differing restatements, and MCT on components that differ in depth or sign | 2 |
| [Partial decode](#partial-decode) | with `strict` off: cut packets, POC gaps and packed-header overruns handled differently | 1, 3 |
| [Signed 32-bit components](#signed-32-bit-components) | decodes; OpenJPEG refuses | 1 |
| [The JP2 box layer](#the-jp2-box-layer) | `ftyp` without `jp2 `, bytes after `Psot = 0`, deep or mixed-sampling palettes, `cdef` labels | 1, 3 |
| [The `jp2i` and `uinf` boxes](#the-jp2i-and-uinf-boxes) | read from the standard; OpenJPEG skips them | 1 |
| [The `rreq` box](#the-rreq-box) | read from the standard; OpenJPEG skips it | 1 |

None of the box entries changes the samples.

## The inverse 9/7's high-pass scale (`2/K`)

| | |
| - | - |
| This decoder | scales high-pass coefficients by the exact `2/K = 1.6257861…` |
| OpenJPEG 2.5.0 | uses `two_invK = 1.625732422f`, 5.4e-5 short |
| Standard | `1/K` (Table F.4) and the factor 2 of the subband gain (Table E-1) |
| Rule | 1, without the better result rule 1 normally needs |
| Tests and fixtures | `tests/irreversible_16bit.rs`; tolerances on the lossy fixtures |
| Decided | #104, 2026-11-13 ([decision](https://github.com/D0ubleD0uble/rust-j2k/issues/104#issuecomment-6869324141)) |

This decoder keeps the standard's exact `2/K`, knowing that OpenJPEG's
constant scores up to 0.72% lower MSE against the ISO references. No option
selects OpenJPEG's constant, so the small gap against OpenJPEG on multi-level
9/7 detail bands is permanent.

The standard scales the high-pass samples by `1/K`, and the subband gain
(Table E-1) adds a factor of two, so a high-pass coefficient is scaled by
`2 · (1/K) = 1.6257861…`. This decoder applies that whole factor in the
inverse 9/7 (`inverse_9_7` in `src/dwt.rs`). OpenJPEG's decoder also folds the
factor of two into the transform, but with an approximate constant:

```c
/* tcd.c:1003-1008 */
/* BUG_WEIRD_TWO_INVK (look for this identifier in dwt.c): */
const OPJ_INT32 log2_gain = (!isEncoder && l_tccp->qmfbid == 0) ? 0 : ...;

/* dwt.c:3149-3151 */
/* Due to using two_invK instead of invK, we have to compensate in tcd.c */
const float two_invK = 1.625732422f;
```

`1.625732422` is 5.4e-5 (3.3e-5 relative) short of `2/K`. The value is a
leftover from OpenJPEG's first, fixed-point decoder. That decoder (0.8, 2004)
ran the 9/7 transform in fixed point with 13 fractional bits and multiplied by
13318, and 13318/8192 = 1.625732421875. The 2007 floating-point rewrite
(dbeebe72b) kept that value as the literal `c13318 = 1.625732422f`, which is
today's `two_invK`. The `BUG_WEIRD_TWO_INVK` comment in dwt.c says that
`invK` failed OpenJPEG's conformance tests, with MSE and peak errors well
above the accepted values, and that because it uses `two_invK` instead,
tcd.c compensates in the step sizes of the non-LL subbands. This decoder's
dequantization is built around the exact `2/K`, so it needs no such
compensation. `two_invK` is not an error correction OpenJPEG makes on
purpose, so rule 2 does not apply to it.

Both decoders use the nearest `f32` to each lifting constant of Table F.4, so
`two_invK` is the only difference between them. With `two_invK` swapped in,
this decoder is bit-exact against `opj_decompress` on
`issue206_image-000.jp2`, `issue208.jp2`, both `issue559-eci-*` files,
`db11217111510058.jp2` and `jpx_disguised_as_jp2.jp2`, and on a synthetic
sweep at 12, 14 and 16 bits.

The measurements don't all point the same way:

- On near-lossless round trips (`opj_compress -I -n 6 -r 1`, whose forward
  transform uses the exact `K`; 64x64 images, nine patterns, two seeds each),
  this decoder returns the encoder's input more closely than `opj_decompress`
  in 18 of 18 cases at 16 bits, 15 of 18 at 14 and 12 of 18 at 12. OpenJPEG is
  closer, by a small margin, on smooth gradients and a random walk. At `-r 4`
  and `-r 20`, where quantization error dominates, the comparison is mixed and
  the margins are negligible. `tests/irreversible_16bit.rs` checks one 16-bit
  case.
- `p0_09` decodes bit-exact against its ISO reference with either constant, so
  it does not decide the question.
- On the lossy conformance entries the ISO references lean slightly the other
  way: OpenJPEG's constant lowers the MSE on 16 of the 26 graded components,
  raises it on 3 and leaves 7 the same. No change exceeds 0.72%, and no peak
  error moves.

The error `two_invK` adds grows with the sample range, so the gap between the
two decoders grows with bit depth.
[How close a decode must be](correctness.md#how-close-a-decode-must-be) gives
the measured gaps, and the lossy fixtures keep their stated tolerances. So a
9/7 fixture graded `tolerance: exact` against OpenJPEG must not carry detail
bands whose values reach a rounding boundary. Grade multi-level 9/7 against the
conformance reference instead, or within a stated tolerance.

## More than 999 layers

| | |
| - | - |
| This decoder | starts a tag-tree node unresolved and reads the inclusion bit, up to the 65535 layers `COD` allows |
| OpenJPEG 2.5.0 | starts every node at 999 (`tgt.c:247`, `opj_tgt_reset`), so at a layer threshold of 1000 or more it skips the bit |
| Standard | B.10.2 |
| Rule | 1 |
| Tests and fixtures | `tests/many_layers.rs` |
| Decided | not recorded |

At a layer threshold of 1000 or more, OpenJPEG takes a code-block not yet
included as included, without reading its inclusion bit, and reads the rest of
the packet header out of step. This decoder starts a node unresolved
(`UNRESOLVED = u32::MAX` in `src/tier2/tagtree.rs`) and reads the bit, as
B.10.2 says.

A file with a code-block first included at layer 999 or later decodes here.
OpenJPEG refuses it ("segment too long (3) with max (0) for codeblock 1998" on
a 2048x2048 file of 1000 layers and two new code-blocks a layer) or returns a
wrong image (a 4x4 image of 1000 layers whose `jp2c` is followed by an `xml `
box gives `[128, 255, 255, 255, 128, 0, ...]` from that box's bytes, where
every sample is 128). Up to 999 layers the two agree bit for bit on the same
files. `tests/many_layers.rs` grades each such file against a twin of the same
code-block data in a few layers, which OpenJPEG decodes, up to the 65535
layers `COD` allows. So OpenJPEG is not an oracle above 999 layers.

## A `Psot = 0` codestream followed by a box that ends in `FF 90`

| | |
| - | - |
| This decoder | decodes: the `jp2c` box holds the codestream |
| OpenJPEG 2.5.0 | reads the tile-part to the end of the file and fails ("failed to decode image") |
| Standard | the `jp2c` box holds the codestream |
| Rule | 1 |
| Tests and fixtures | none specific to this case; `tests/psot_zero.rs` covers the box end |
| Decided | not recorded |

The file is a single-tile codestream whose last tile-part has `Psot = 0`, in a
`jp2c` of the right length, followed by a box whose last two bytes are
`FF 90`. When such a codestream ends at its box is set out under
[`Psot = 0` at the end of `jp2c`](#psot--0-at-the-end-of-jp2c).

## A header box at the top level before `jp2h`

| | |
| - | - |
| This decoder | always ignores it |
| OpenJPEG 2.5.0 | reads it after any unknown box |
| Standard | header boxes belong inside `jp2h` |
| Rule | 1 |
| Tests and fixtures | `header_boxes_misplaced_before_jp2h_are_ignored` in `src/jp2/tests.rs` |
| Decided | 2026-11-11 |

Such a `colr`, or another header box, is always ignored, since the standard
puts them inside `jp2h`. OpenJPEG reads it after any unknown box: its
`JP2_STATE_UNKNOWN` is `0x7fffffff` (`jp2.h:80`), and ORing it into the state
flags (`jp2.c:2335`) marks every state as reached, so the `HEADER` test
passes. The code warns that such a box "will be ignored" (`jp2.c:2332`), and
then reads the next one; it is a mistake, not a rule.

## Bypass under a maxshift

| | |
| - | - |
| This decoder | switches to raw coding where D.6 says, counted from the first coded plane `top` |
| OpenJPEG 2.5.0 | switches `SPrgn` planes late, and so decodes its own encoder's output wrong |
| Standard | D.6, Table D.9, H.2 |
| Rule | 1 |
| Tests and fixtures | `tests/roi_bypass.rs`, `tests/fixtures/roi_bypass/` |
| Decided | #258; recorded 2026-11-11 (#226) |

An RGN marker segment with the maxshift method (`Srgn = 0`) shifts a
component's region-of-interest coefficients up by `SPrgn` bit-planes. Under
the bypass code-block style, a code-block's significance and refinement passes
switch to raw coding from its fifth coded bit-plane on (D.6, Table D.9),
counted from its first coded plane. Under an RGN maxshift the shifted planes
are coded planes (H.2): the first one is `top = Mb + SPrgn - zero_bit_planes`.
Tier-2 splits the segments by pass count (10 passes, then 2, 1, 2, 1, …),
which agrees with that count. This decoder counts from `top`
(`src/tier1/passes.rs`, `decode_block`).

OpenJPEG's decoder starts at the same plane, `bpno_plus_one = roishift +
cblk->numbps` (`t1.c:1990`), but tests `bpno_plus_one <= cblk->numbps - 4`
(`t1.c:2061`, `opj_t1_decode_cblk`). `cblk->numbps` leaves the shift out, so
the switch comes `SPrgn` planes late and the decoder MQ-decodes segments that
were written raw. Its own encoder counts from the first plane it codes
(`bpno < cblk->numbps - 4`, `t1.c:2473` in `opj_t1_encode_cblk`), which is the
plane the decoder calls `top`. So `opj_compress -ROI` with any bypass style
writes a stream that `opj_decompress` 2.5.0 decodes wrong.

The encode is lossless, so the source image is the reference. A sweep of
`opj_compress -i source.pgm -n {1,3,6} -ROI c=0,U={1,3,7,12} -M m` over `m` in
{0, 1, 3, 5, 9, 17, 33, 49, 63} (108 encodes of
`tests/fixtures/roi_bypass/source.pgm`):

| Decoder | Equal to the source |
| ------- | ------------------- |
| `opj_decompress` 2.5.0 | 12 of 108: the `-M 0` encodes only |
| this decoder, counting like OpenJPEG | 12 of 108; 60 wrong, 36 errors |
| this decoder, counting from `top` | 108 of 108 |

Without a maxshift both counts are the same, so no other fixture changes.
`tests/roi_bypass.rs` pins four of these encodes (`-M 1`, `5`, `33`, `17`, at
`U=3`, `-n 3`) against the source; provenance is in
`tests/fixtures/roi_bypass/README.md`.

The decoder rule has been in OpenJPEG since the initial import (2ad6a977,
2003, `libopenjpeg/t1.c`: `bpno = roishift + cblk->numbps - 1` and
`bpno <= (cblk->numbps - 1) - 4`). `git log -L` on the line shows only
renames, reformatting and sign-conversion fixes since, with no commit that
gives a reason. The encoder's rule is from the same import; the 2017 bypass
and termination fixes (81c53117, upstream #674) changed the encoder's
termination and left the decoder's raw decision alone. No upstream issue or
pull request covers ROI with bypass. Upstream ROI has no tests (#1035, "Add
unit tests for ROI feature", open since 2017). Its encoder shifts no
coefficients: `-ROI` only signals `SPrgn`, so the decoder reads every nonzero
coefficient of the component as region of interest (#1036, open). The rule
reads as an oversight, not a choice, so this decoder follows the standard.

## Segmentation symbols

| | |
| - | - |
| This decoder | checks the `1010` symbol where the segment's own bytes decide it, and on every terminated cleanup pass |
| OpenJPEG 2.5.0 | never checks it (the comparison is commented out, `t1.c:1353-1357`) |
| Standard | D.5 makes the symbol an error-detection aid |
| Rule | 3 |
| Tests and fixtures | `cblksty_segsym_lossless`, `cblksty_segsym_truncated`, `roi_bypass_m33` |
| Decided | not recorded |

Under the `segmentation symbols` style each cleanup pass ends with four
decisions that spell `1010` (D.5). This decoder checks the symbol, but not
everywhere:

- A wrong symbol is a `Codestream` error while the MQ decoder has read only
  the segment's own bytes. Those bytes then decide the symbol, so they are
  corrupt.
- On a terminated cleanup pass a wrong symbol is an error even past the end.
  Its length is exact, and the MQ termination makes the codeword decode
  correctly from the fill. The terminated cleanups are the ones
  `opj_t1_enc_is_term_pass` (`t1.c:2374`) terminates: every pass under
  `restart`, the block's last cleanup (plane 1 at this decoder's double scale),
  and under bypass every cleanup from the fourth coded plane down
  (`bpno - 1 <= raw_from` in `decode_block`, so counted from `top`, maxshift
  included). Predictable termination changes how a pass is terminated, not
  which.
- On any other cleanup pass, once the decoder has read past the end of the
  segment, a wrong symbol decodes. Rate control cuts such a pass at an
  estimated length: `opj_compress` uses the bytes written so far plus three
  (`opj_t1_encode_cblk`), which can be a few bytes short. The decoder then
  reads the symbol from the `0xFF` fill and gets `0b1001`.

The last-cleanup rule holds for encoders that shift the region's coefficients
as H.2 describes. `opj_compress -ROI` shifts none, so its last terminated
cleanup lands at plane `SPrgn + 1`, not 1, and the last pass of such a stream
goes unchecked. A lossless `-M 32 -ROI c=0,U=3` stream accepts 134 single-bit
flips as wrong images that checking every symbol refuses; OpenJPEG decodes all
of them too, and no valid stream is refused. Treating plane `SPrgn + 1` as
terminated would refuse a standard maxshift encoder whose output is rate-cut at
that plane, so it is not done.

"Past the end" is `MqDecoder::ran_past_end`: the code register has taken in a
byte the decoder synthesized. It is set by the read that moves the byte
pointer to the segment length, or by the marker test after a final `0xFF`.
That is the first read of fill, and it can come up to about 16 code bits
before any decision depends on fill, so the flag errs early. It is often set
by the end of a segment even in an intact stream, because the MQ decoder
reads ahead. So on an unterminated pass the check mostly catches damage in the
earlier passes of a segment, not in its last one.

The rule rests on 459 random `opj_compress -M 32|m -r ...` encodes, with `m`
from bypass, reset, restart, vertically causal and predictable termination,
layer counts from one to three, and 5/3 and 9/7. Checking every symbol, the
decoder refused 10. With the rule above it refuses none, nine of the ten match
`opj_decompress` exactly, and the tenth (9/7) is within 1. A second run of
1,664 encodes (1,350 with `-r`, 314 with `-ROI`, the same style mix) refuses
none either, and every sample mismatch with `opj_decompress` is explained:
9/7, the sYCC-to-RGB conversion `opj_decompress` applies to three components
with sub-sampled chroma, or OpenJPEG's own maxshift + bypass bug
([Bypass under a maxshift](#bypass-under-a-maxshift)).

Single-bit flips (`0x01`, `0x10`, `0x80`) of every body byte of
`cblksty_segsym_lossless` and `roi_bypass_m33` are refused and accepted
exactly as when every symbol is checked. Tolerating fill on every cleanup
outside `restart` would have accepted 250 more of the 2,580
`cblksty_segsym_lossless` flips, 229 of them decoding to a wrong image. A
narrower rule, which tolerated fill on the block's last decoded pass only,
refused one of the ten: in a two-layer encode the cut pass was followed by
passes with no bytes of their own. `cblksty_segsym_truncated` pins one such
encode. Its provenance is in `tests/fixtures/README.md`.

One case has no fixture: a background code-block (`zero_bit_planes` at least
`SPrgn`) under a maxshift with bypass. `opj_compress -ROI` makes the whole
component the region, and no conformance file has RGN with bypass. That case
rests on the standard and the model of `opj_t1_enc_is_term_pass` only: all 64
`-M` styles over first coded planes 0 to 39 agree with it.

## A tile-part POC overrides the main header's

| | |
| - | - |
| This decoder | a POC in a tile's first tile-part replaces the main header's for that tile |
| OpenJPEG 2.5.0 | appends the tile-part's volumes after the main header's |
| Standard | A.6.6, B.12.3 |
| Rule | 1 |
| Tests and fixtures | `tests/poc_precedence.rs`, `tests/fixtures/poc_precedence/` |
| Decided | #269 |

A.6.6 orders the markers that set a tile's progression, "Tile-part POC > Main
POC > Tile-part COD > Main COD", "where the 'greater than' sign, >, means that
the greater overrides the lessor marker segment". It also says a tile's POC
"must appear in the first tile-part header of that tile". B.12.3 says the
same: "The COD progression order and the main header POC marker segment (if
there is one) are overridden." So a tile whose first tile-part has a POC walks
only its own volumes, accumulated over all its tile-parts in order, and a tile
without one walks the main header's. This decoder does that
(`resolve_tile_header` in `src/codestream/mod.rs`).

OpenJPEG copies the main header's coding parameters into each tile, POC
volumes included (`opj_j2k_copy_default_tcp_and_create_tcd`), and
`opj_j2k_read_poc` then appends a tile-part's volumes after the ones already
there (`l_old_poc_nb = l_tcp->POC ? l_tcp->numpocs + 1 : 0`, `j2k.c:3563`). A
tile with both walks `[main volumes, tile volumes]`, and the walk takes each
packet from the first volume that reaches it, so the main volumes decide the
order of every packet they cover.

`scripts/gen-poc-precedence-fixtures.py` encodes a 24x16, three-component,
three-layer image losslessly with `opj_compress -POC
T1=0,0,3,1,3,CPRL/T1=1,0,3,3,3,RLCP`, which writes the two volumes in the
tile-part header. `opj_decompress` 2.5.0 round-trips that file exactly. The
script then adds a main-header POC of one LRCP volume over resolution 0, every
component and every layer. LRCP and CPRL order resolution 0's nine packets
differently, so only the tile-part POC describes the packets as written.

| Decoder | `poc_main_and_tile.j2k` and `poc_main_and_split_tile.j2k` |
| ------- | ---------------------------------------------------------- |
| `opj_decompress` 2.5.0 | no error; 380, 383 and 384 of 384 samples wrong per component |
| this decoder, appending | `Codestream`: 114 bytes left after the last packet |
| this decoder, overriding | equal to the source |

The split file puts the second tile volume in its own POC in the second
tile-part header. `tests/poc_precedence.rs` grades both against the source;
provenance is in `tests/fixtures/poc_precedence/README.md`. No conformance
file or other committed fixture has a POC in both the main header and a
tile-part header, so none of them changes.

A tile whose first tile-part has no POC, but a later one has, breaks A.6.6.
Its first packets can only have followed the main header's order, so this
decoder reads it as OpenJPEG does: the main volumes, then the tile's. The
32-volume budget counts both. `poc_main_then_later_tile_part.j2k` is such a
file, and both decoders return the source. So this decoder differs from
OpenJPEG only on files that follow A.6.6.

The coverage check ([Packets a POC sequence leaves
out](#packets-a-poc-sequence-leaves-out)) reads the same list the walk does.
`poc_tile_leaves_out_packets.j2k` has a main POC that covers the tile and a
first-tile-part POC that leaves out two resolutions; it is refused as
`Codestream`, where OpenJPEG walks the main volume and returns a wrong image.

OpenJPEG's initial revision (2ad6a977, 2003) already read tile-part POCs, and
it *replaced*: `j2k_read_poc` wrote from index 0 and set
`numpocs = numpchgs`, and every SOT re-copied the default tcp. OpenJPEG 0.8
(4be82998, 2004) added two things in one commit: a `tcp->first` guard, so the
default tcp is copied into a tile once rather than at every SOT, and the
append (`old_poc = tcp->POC ? tcp->numpocs+1 : 0`). The append lets the POCs
of a tile's tile-parts build one list, which B.12.3 allows. The main header's
volumes ending up in that list is a side effect of the copy-once seed. The
append was carried into the v2 reader (1d45d751, 2008) and since then only
renamed, reformatted (563bd849) and given the `J2K_MAX_POCS` constant
(b8c4b450). No commit gives a reason, and no upstream issue covers a main and
a tile-part POC together. OpenJPEG's encoder never writes both kinds: it
writes a main-header POC only for the 4K cinema profile
(`opj_j2k_write_poc`), and a tile-part POC only outside the cinema profiles
(`opj_j2k_write_first_tile_part`). So its own files decode the same under
either rule. The append reads as an oversight, not a choice, so this decoder
follows the standard.

## Code-block contributions of zero bytes

| | |
| - | - |
| This decoder | accepts passes with no bytes and decodes them from the fill; refuses a wrong segmentation symbol read from the fill of an empty terminated segment |
| OpenJPEG 2.5.0 | decodes both (`opj_mqc_init_dec` with a length of 0) |
| Standard | B.10.7, B.10.7.1, B.10.8, D.4.1 |
| Rule | 3 (the refusal); the acceptance matches OpenJPEG |
| Tests and fixtures | `zero_length_contribution`, `zero_length_contribution_bypass`; `tests/work_limit.rs` for the bound |
| Decided | not recorded |

A packet may include a code-block with coding passes and a length of zero.
B.10.7.1 gives the length as a plain binary number ("The length of a codeword
segment is represented by a binary number of length: bits = Lblock +
⌊log2(coding passes added)⌋") and sets no lower bound. B.10.7 also says how a
contribution can end up empty: "in the event that an 0xFF would have appeared
at the end of a code-block's contribution to some packet, the 0xFF may be
safely moved to the subsequent packet which contains contributions from the
code-block, or dropped if there is no such packet." The B.10.8 header order
reads the length for every included block, with no zero case. D.4.1 says how
such passes decode: "the decoder shall extend the input bit stream to the
arithmetic coder with 0xFF bytes, as necessary, until all symbols have been
decoded."

So the decoder accepts passes with no bytes, in one layer or in every layer
that includes the block. The passes count and fill their codeword segment, so
a later layer's passes and lengths go to the right segment, under `restart`
and `bypass` too. A segment that never gets a byte is decoded from the fill,
as OpenJPEG 2.5.0 decodes it. Until this was settled the decoder refused a
block whose passes never got a byte, which stopped openjpeg-data's
`kodak_2layers_lrcp.j2c` (DCP-Werkstatt) on 2153 such blocks. That file now
reaches the [leftover-bytes refusal](#bytes-left-after-the-last-packet).

One case is still refused. Under segmentation symbols, an empty segment whose
cleanup pass counts as terminated (under `restart`, on plane 1, or from the
fourth coded plane down under `bypass`; see [Segmentation
symbols](#segmentation-symbols)) reads its symbol from the fill, gets a wrong
value, and is refused as `Codestream`. OpenJPEG decodes it. No encoder could
have written that segment: a terminated cleanup ends with a correct symbol in
bytes of its own. So the decoder returns an error rather than an image built
on a segment that cannot be right.

A contribution of zero bytes reads its header bits like any other, and its
passes cost the same against the code-block work bound as passes with bytes.

## Stricter than the oracle on non-conforming streams

| | |
| - | - |
| This decoder | refuses each stream below |
| OpenJPEG 2.5.0 | decodes it, with a warning or none, or with invented values |
| Standard | B.11 (only whole packets may be missing), Table A-33 (POC ranges), the step count a decomposition needs |
| Rule | 3 |
| Tests and fixtures | the reject matrix in `src/codestream/tests.rs`; `tests/fixtures/partial/tp_r.j2k` |
| Decided | the interop rules: 2026-11-12, and #269 |

Three tier-2 checks reject streams OpenJPEG decodes with a warning: an `Nsop`
that does not match the packet sequence, a signaled EPH that is missing, and
tile-part bytes left over after the last packet. Two tier-1 checks are of the
same kind: predictable termination on every segment, and a wrong segmentation
symbol read from the fill of an empty terminated segment ([Code-block
contributions of zero bytes](#code-block-contributions-of-zero-bytes)). Each
condition means the packet walk has probably lost its place, and decoding on
risks a wrong image with no error. A typed error never returns a wrong image;
warn-and-continue bets the output on a parse that has already gone wrong.

Three parse-time checks are stricter for a different reason: the field is
malformed, or the codestream is missing data the standard requires and
OpenJPEG fills the gap with invented values. The entries below give each one.

### SOP sequence numbers

An `Nsop` that does not match the packet sequence is refused. OpenJPEG never
checks it. ECW JPEG 2000 SDK 6.0 (ERDAS APOLLO GeoCompressor 2021) numbers its
SOP markers out of sequence: jpylyzer-test-files'
`erdas-nullinput-uint8-rgb-null-2tileparts.jp2` is refused with "SOP sequence
number 50 for packet 0, expected 0", and OpenJPEG decodes it.

### A missing EPH

A signaled EPH that is missing is refused. OpenJPEG continues without
consuming one.

### Bytes left after the last packet

Tile-part bytes left over after the last packet are refused with a
`Codestream` error that gives their count. OpenJPEG ignores them. With
`strict` off they are skipped and reported ([Partial decode](#partial-decode)).

`opj_compress` writes two patterns this check refuses:

- Overlapping POC volumes (`-n 1 -POC T1=0,0,1,1,1,LRCP/T1=0,0,1,1,1,PCRL`).
  OpenJPEG's encoder builds a new packet iterator, and so a new include array,
  for each tile-part (`t2.c:248`, `opj_t2_encode_packets`), so it writes the
  packets twice and the file doubles in size. OpenJPEG's decoder skips the
  repeats, ignores the bytes left over, and matches the source. This decoder
  refuses it with a `Codestream` error that counts the bytes left after the
  last packet. A partial overlap (`-r 4,2,1`, layer ranges 2 then 3) is
  refused with a `Codestream` error saying the data of the tile ends before
  its last packet.
- `-TP R` or `-TP C` with empty resolutions (`-F 2,99,1,1,u -n 6 -t 32,64
  -d 22,14 -TP R`). The encoder writes a stray `0x80` byte into each
  tile-part of an empty resolution. This decoder refuses it
  (`tests/fixtures/partial/tp_r.j2k`, refused as `Codestream` under strict
  decoding). OpenJPEG decodes it silently to a different image (117 of 198
  samples differ on the committed fixture), so refusing is the better outcome
  here.

openjpeg-data's `kodak_2layers_lrcp.j2c` (DCP-Werkstatt) has 975304 bytes left
after its last packet, and this decoder refuses the file with a `Codestream`
error that gives that count; OpenJPEG ignores them. With the check turned off, this
decoder's image is within 1 of OpenJPEG's (it is 9/7).

### A tile-part that ends before its last packet

A tile-part that ends before its last packet is refused ("the data of tile T
ends before the last packet"). That is not a desynchronized walk: B.11 lets "any
number of whole packets (in order)" be dropped from the end of a tile, so this
is the strict decode refusing missing data, which partial decode accepts with
the missing packets reported ([Partial decode](#partial-decode)).
openjpeg-data's `Marrin.jp2` (Kakadu 5.2.1) declares two layers, but its one
tile-part's `Psot` covers only the packets of layer 0. OpenJPEG decodes the
packets it finds, with no warning.

### Predictable termination on every segment

Under predictable termination every MQ segment of a code-block must end as the
termination predicts. OpenJPEG checks only the last segment and only warns
(`decode_block` in `src/tier1/passes.rs`; see also [Segmentation
symbols](#segmentation-symbols)).

### Too few quantization steps

A QCD/QCC carrying fewer than the 3·NL + 1 step entries its decomposition needs
is rejected as `Marker`. OpenJPEG zero-fills the missing step sizes and
decodes, reconstructing those subbands at a scale the encoder never used. Its
output is a deterministic guess, not the encoder's image, and this decoder
does not half-decode. The converse stays oracle-matched: extra step entries
are ignored.

### 9/7 with no quantization

The 9/7 wavelet paired with the no-quantization style is rejected as
`Marker`. OpenJPEG derives steps from the style-0 exponents with mantissa zero,
again a guess rather than the encoder's image. The converse stays
oracle-matched: 5/3 with a scalar style decodes, since the reversible path never
reads the mantissas.

### An empty POC volume

A `POC` volume whose resolution, component or layer range is empty
(`REpoc <= RSpoc`, `CEpoc <= CSpoc`, or `LYEpoc = 0`) is rejected as `Marker`.
Table A-33 requires `RSpoc < REpoc`, so such a volume is malformed. OpenJPEG
does not check it, and that volume's packet loops run zero times. A volume that
covers nothing is a corrupt field, not one an encoder would mean.

### Two faults in one file

Where a file has two faults, the two decoders can name different ones; both
refuse it. A code-block size past the limit in any COD or COC, main or
tile-part header, is `Marker` when that marker is read, as OpenJPEG refuses
it there. When the same COD or COC also has fewer levels than the
`resolution_reduction` asks for, OpenJPEG reports the reduction and this
decoder reports the code-block size. Matching it would mean passing the
reduction into every COD and COC parse.

### When a real file trips one of these

The stricter checks have a cost in interop: an encoder that produces such a
stream decodes under OpenJPEG and is refused here. No conformance file trips
them. The committed fixtures that do were made to, such as
`tests/fixtures/partial/tp_r.j2k`. The two real files above (the ECW SOP file
and `kodak_2layers_lrcp.j2c`) trip them too, and `opj_compress` writes the two
tier-2 patterns.

Two rules (2026-11-12) decide what happens when a real file needs a check
relaxed. Rule 2 covers only OpenJPEG behavior that is deliberate error
correction; leniency that exists because OpenJPEG never wrote a check goes
behind `strict = false`, with an entry in the report, not into the strict
default. The exception (#269): a lenient reading can stay in the strict
default when the existing checks (the full packet walk, coverage, leftover
bytes) would still catch a wrong guess. So such a file is read with
`strict = false`.

## Packets a POC sequence leaves out

| | |
| - | - |
| This decoder | refuses as `Codestream`, before the packets are read |
| OpenJPEG 2.5.0 | decodes the missing packets as empty |
| Standard | A.6.6, B.6, B.9, B.11, B.12.3 |
| Rule | 3 |
| Tests and fixtures | `tests/fixtures/poc_coverage/` |
| Decided | not recorded |

A.6.6 requires the volumes to define every packet: "In the case where a POC
marker segment is used, the progression of every packet in the codestream (or
for that tile of the codestream) shall be defined in one or more POC marker
segments", and B.12.3 adds that "there will never be the case where a
progression order volume is filled and the next one is not defined". The
volumes are the ones the tile walks: its own when its first tile-part has a
POC, which replaces the main header's ([A tile-part POC overrides the main
header's](#a-tile-part-poc-overrides-the-main-headers)), and otherwise the main
header's and then any later tile-part's. A tile has a packet for every layer,
every resolution each component has (its own `COD` or `COC` level count) and
every precinct of it (B.6, B.9). A precinct or resolution with no code-blocks
still has its packets, which B.9 says must appear as empty packets, so a `POC`
that skips it is refused. A resolution with no precincts on the tile has no
packets and needs no volume. "The layer index always starts at zero" (A.6.6),
so a component's resolution has all its packets only when one volume reaching
the last layer covers it; the check costs `components × (volumes +
resolutions)` steps.

Such a file is not out of the standard in every reading, though: B.11 lets
"any number of whole packets (in order)" be dropped from the end of a tile,
and B.12.3 lets "the last progression order volume in each tile" be
incomplete. A tile whose volumes leave packets out is byte for byte a tile
whose covering volumes had those packets dropped. So this is the strict
decode refusing missing data, as it refuses a tile cut short; partial decode
accepts it, with the missing packets reported. OpenJPEG decodes such a tile
with them empty, and `opj_compress` writes such a file when its `-POC` volumes
do not cover the tile: `-n 2 -POC T1=0,0,1,1,1,LRCP` leaves resolution 1 out,
and OpenJPEG's decode of it has every sample wrong
(`tests/fixtures/poc_coverage/`).

The refusal holds under a `resolution_reduction` too, when the packets left
out are only those of resolutions the reduction discards. That is stricter
than `opj_decompress -r`, which gives the exact reduced image, and it is
consistent with refusing a tile cut short (B.11) at a reduction of 1, which
this decoder already does. Partial decode reports such a tile complete at
the reduction that drops the missing resolutions, and its image there is the
strict decode of a file whose volumes cover the tile.

The check is on the volumes' axes, not on each precinct: a positional order's
sweep can pass over a precinct with mixed sub-sampling ([The tier-2 work
bound](resource-bounds.md#the-tier-2-work-bound)), with or without a `POC`, as
OpenJPEG's encoder and decoder both do, and that is not refused.
`read_header`'s walk of a `Psot = 0` tile does not make the check: the packets
the volumes reach end where they end, so for such a tile in a JP2 file the
codestream ends at its box and the boxes after `jp2c` are reported, as for the
raw codestream, which the decode refuses. 381 `opj_compress -POC` encodes whose
volumes do cover the tile (two or three volumes split by resolution or
component, every order, 1 to 4 components, some sub-sampled, 1 to 3 layers)
decode as before.

## Encoder mistakes OpenJPEG corrects

| | |
| - | - |
| This decoder | corrects three encoder mistakes as OpenJPEG does; refuses the cases the correction would read wrongly |
| OpenJPEG 2.5.0 | corrects them on purpose (a named function, a warning, a fixed issue) |
| Standard | says each file is wrong but not what it means |
| Rule | 2 |
| Tests and fixtures | `tests/encoder_quirks.rs`, the `encoder quirks` block of `src/codestream/tests.rs`; `quirk_*_lossless` in `tests/fixtures/` from `scripts/gen-encoder-quirk-fixtures.py` |
| Decided | #286 |

In each case the standard says the file is wrong but not what it means,
OpenJPEG's correction is deliberate, and the checks that follow still refuse a
file the correction reads wrongly. This decoder departs from OpenJPEG only
where the correction would leave a tile's counts or parameters ambiguous.

### `TPsot == TNsot`

A tile's tile-parts are numbered 0 to N with N declared, so its last part has
`TPsot` equal to `TNsot`. Adobe Photoshop's plug-in writes this. OpenJPEG warns
"Non conformant codestream TPsot==TNsot" and adds one to every count
(`opj_j2k_need_nb_tile_parts_correction`, `j2k.c:9410`, called at
`j2k.c:9701`; its issue 254). Its trigger is copied exactly
(`count_is_one_short` in `src/codestream/mod.rs`): once per codestream,
when the first tile has all the parts it declares, the `SOT` chain is
walked onward, stepping over other tiles' parts by `Psot`, to the next part
of that tile. If that part has `TPsot == TNsot`, every count stated so far
and every count stated after is raised by one. The walk finds nothing at a
marker other than `SOT`, at a `Psot` below 14 on another tile's part, or at
a `Psot` past the end. It is safe because the raised counts are held to the
parts that arrive like any other count, and the parts are still read in
`TPsot` order.

The two decoders differ where the counts are mixed. When the correction is
on, a tile whose own count was right now carries one part fewer than its
count, and is refused as a tile with a missing part ("tile 3 declares 4
tile-parts but carries 3"). OpenJPEG decodes it as it decodes a truncated
tile, which here gives the full image. When the first tile to complete is
counted right and a later tile has an extra part, the correction is not
made, and the extra part is refused ("tile 1 tile-part 2 of a declared 2
tile-parts"). OpenJPEG decodes that tile from the parts before it and drops
the last one: 2293 of the 3072 samples then differ from the source. Both
were measured on `quirk_tpsot_tnsot_lossless` with its counts edited.

### A COD, COC, QCD or QCC restated in a later tile-part

A.4.2 puts these in a tile's first tile-part. BAP Image Systems writes the main
header's COD and QCD into every tile-part. OpenJPEG reads them in any tile-part
header (`J2K_STATE_TPH` in its marker table, `j2k.c:1411`) and applies them to
the whole tile; it removed its "one COD per tile" check on purpose so that
files like these decode (`opj_j2k_read_cod`, the `#if 0` at `j2k.c:2688`; its
issue 1043, a BAP bulk-scanner file). Here a later part's marker is accepted
only when it changes nothing in either reading, and is then dropped
(`check_restatement`):

- a COD must equal the tile's COD in force (its own, else the main
  header's), and every component of the tile must already code as it says,
  since OpenJPEG writes a COD over every component. So a tile or main COC
  that differs from the COD makes the restatement a refusal: this decoder
  would keep the COC and OpenJPEG would overwrite it;
- a QCD likewise, against the QCD in force and every QCC;
- a COC or QCC must equal what its component already has.

A marker that differs from the one in force is a `Codestream` error naming
the tile and the marker. OpenJPEG applies it to the whole tile, including the parts read before it; which values the encoder
meant is not in the file, so this decoder does not guess. An RGN in a later
part is still refused, as before. This is safe because an accepted
restatement leaves every value the decode reads as it was.

### The color transform on one or two components

`SGcod`'s MCT byte set on an image of fewer than three components
(openjpeg-data `123.j2c`). The transform is defined over components 0, 1 and 2
(G.2), so it has nothing to apply to. OpenJPEG says "Number of components (1)
is inconsistent with a MCT. Skip the MCT step." and decodes the components as
coded (`opj_tcd_mct_decode`, `tcd.c:2188`), the only image such a file can
describe. This decoder skips it too (`mct::applies`); before #286 it refused
the file as `Marker`. Three or more components whose sub-sampling or sizes
differ are still refused, as OpenJPEG refuses them (`tcd.c:2067-2119`). Three
that differ only in depth or sign are refused too; OpenJPEG decodes those, so
that refusal is this decoder's own stricter choice (`mct::check_geometry`).

### Real files

Real files, decoded with `opj_decompress` 2.5.0 from their bare codestreams
and compared sample by sample (the files are not committed; their licenses
vary):

| File | Quirk | Result |
| ---- | ----- | ------ |
| openjpeg-data `issue235.jp2`, `issue254.jp2`, `text_GBR.jp2` | `TPsot == TNsot` | bit-exact (5/3) |
| openjpeg-data `issue206_image-000.jp2`, `issue208.jp2`, `issue559-eci-091-CIELab.jp2`; jpylyzer `jpx_disguised_as_jp2.jp2` (and `wrong-number-of-tileparts.jpf`, byte-identical to it) | `TPsot == TNsot` | within 1 (9/7) |
| openjpeg-data `issue559-eci-090-CIELab.jp2` | `TPsot == TNsot` | within 2 (9/7, 16-bit) |
| openjpeg-data `issue390.jp2` | `TPsot == TNsot` | both refuse a later fault |
| openjpeg-data `db11217111510058.jp2` | COD and QCD restated | within 1 (9/7) |
| openjpeg-data `123.j2c` | MCT on one component | bit-exact (5/3) |

`issue559-eci-090-CIELab.jp2` decodes the same, in both decoders, with its
counts edited to be right, so its difference of 2 (740 of 89888 samples of
component 0) is the 9/7 difference at 16 bits, not the correction. Every
other committed fixture and conformance file decodes as it did before.

The difference of 2 comes from the [`2/K`
constant](#the-inverse-97s-high-pass-scale-2k). With both decoders dumping
each stage of every tile-component, the dequantized coefficients agree
bit-for-bit (all 269664 of them), and the first inverse-DWT level is the first
stage that differs: 1121 of its 1134 values, by up to 1.08. The gap grows to
1.91 at the last level, before rounding. With OpenJPEG's `two_invK` in place
of `2/K`, every stage agrees bit for bit and the file decodes bit-exact.

## Partial decode

| | |
| - | - |
| This decoder | with `DecodeOptions::strict` off, takes missing data and leftover bytes and reports them on `Image::report` |
| OpenJPEG 2.5.0 | `opj_decompress -allow-partial` |
| Standard | B.11 (whole packets may be dropped from the end of a tile) |
| Rule | 1 for a cut packet (B.11); 3 for a packed-header overrun |
| Tests and fixtures | `tests/partial_decode.rs`, `src/tier2/partial_tests.rs`, `tests/packed_overrun.rs`; `tests/fixtures/partial/`, `tests/fixtures/poc_coverage/`, `tests/fixtures/packed_overrun/` |
| Decided | #214 |

With `DecodeOptions::strict` off the decoder takes missing data and bytes left
after a tile's last packet, as `opj_decompress -allow-partial` does, and
reports them on `Image::report`. Everything else is refused as when strict. A
strict decode is unchanged: its output and error text match the decoder before
this option on every committed fixture and on the public JP2, J2K and GRIB2
files of the #226 testing round, not committed, at reductions 0 to 3.

The oracle is `opj_decompress -allow-partial`, on the fixtures in
`tests/fixtures/partial/` (`scripts/gen-partial-fixtures.py`, provenance in
that directory's README) and checked by `tests/partial_decode.rs`:

- Prefixes. For a one-tile LRCP codestream of five levels, the shortest
  prefix that `opj_decompress -allow-partial -r r` decodes to the whole
  file's image is, for each r from 0 to 5, the shortest prefix this decoder
  reports complete at r, and both decoders give the same samples for it and
  for the prefix one byte shorter. A strict decode refuses that shorter
  prefix; a partial one reports it complete at r + 1, where its image equals
  the whole file's. Every prefix from the first `SOT` marker on decodes, and
  a prefix the report calls complete at r gives the whole file's image at r.
- Missing tiles are 0 after the DC level shift, as OpenJPEG writes them.
- `Psot = 0` without an `EOC` (`tests/fixtures/psot0/`) matches OpenJPEG.
- Overlapping `POC` volumes, whose duplicate packets leave bytes over,
  decode to the source, as OpenJPEG's do, and report `LeftoverBytes`.
- `-TP R` with empty resolutions decodes to OpenJPEG's image, which is
  not the source: on the committed fixture 117 of 198 samples differ from it.
  The phantom bytes sit at tile-part boundaries and nothing tells them from
  packet data, so the report gives `LeftoverBytes` and the tile counts as
  incomplete at every reduction.

On real files from that outside corpus, not committed: openjpeg-data's
`Marrin.jp2` (a `Psot` that covers one of two layers) and
`kodak_2layers_lrcp.j2c` (975304 leftover bytes) are within 1 of OpenJPEG
(both are 9/7), and GDAL's `small_world_truncated.jp2` (cut inside its tile)
matches it exactly.

`TNsot` is advisory: a count that disagrees with the tile-parts found is
reported and the tile decodes from them. The decode needs the main header
through the first `SOT` marker; a shorter prefix is `Error::Incomplete`, in
either mode, so a streaming caller can tell it from a damaged header.

Partial decode bounds its work like a strict one, with two more charges
against the packet work bound: each tile-component of a missing tile costs
one step, and the end of a tile's data costs 32 steps for each of its
(component, resolution) pairs, with one more for each packet after it.
Without them, a header of a few kilobytes that declares 65535 tiles of 16384
components with none present took 38 s, and a run of empty tile-parts, each
making its tile set up its geometry for no data, took 19 s. With them the
first is refused before any tile is decoded and the second is refused in
under a second (`missing_tiles_are_charged_against_the_work_bound` and
`the_end_of_each_tiles_data_is_charged_against_the_work_bound` in
`tests/partial_decode.rs`). The end-of-data charge falls on every tile whose
data ends, so a prefix cut late in a codestream that interleaves the
tile-parts of very many tiles can be refused with `Error::Limit` though the
whole file decodes: each tile whose later parts have not arrived is charged.
`DecodeOptions::packet_work_limit` raises the bound for such a stream. The
report holds at most five entries per tile-part, plus two.

Three differences from OpenJPEG are deliberate.

### A cut packet keeps each code-block's whole layers

When the data ends inside a packet's body, B.11 allows only whole packets to
be dropped, so a cut packet has no meaning of its own. This decoder keeps each
code-block whose contribution to the packet is all there, and for the rest
keeps only the layers before the cut packet, so each code-block decodes as a
codestream truncated after some whole layer would. OpenJPEG keeps the bytes
that are there for every code-block of the cut packet, a block's data cut
mid-layer included. The two images can differ in the code-blocks the cut
packet reaches; the report says the tile is incomplete either way.
`a_cut_packet_keeps_each_code_blocks_whole_layers` in
`src/tier2/partial_tests.rs` checks each code-block of a cut three-layer packet
against layer-limited walks.

### Packets a POC leaves out are zero coefficients

A `POC` sequence that leaves packets out (`tests/fixtures/poc_coverage/`)
decodes with those packets empty and reported. At the reduction that drops
them the image matches OpenJPEG's. At full resolution the two differ (1023 of
1024 samples for `poc_res_missing.j2k`) because they fill the gap differently.
OpenJPEG decodes a tile only up to the highest resolution it read a packet of
(`resno_decoded`, `t2.c:518`) and pastes that smaller image into the top-left
corner of a zeroed full-size frame. This decoder sets the missing coefficients
to zero and runs the whole inverse wavelet, so its even-indexed samples are the
reduction-1 image and its largest error against the source is 193, where
OpenJPEG's is 993. Either way the report says the tile is incomplete, with a
`complete_reduction` of `Some(1)`.

### A packed header that overruns its marker

A packet header packed in a `PPM` or `PPT` marker that reads past the end of
the packed bytes is an `Error::Codestream`, with `strict` on or off. Those
bytes are in the main or tile-part headers, which are there whole or not at
all, so running past them is damage, not the data ending. That includes a
header whose last byte is 0xFF with no stuffed byte after it (B.10.1). OpenJPEG
refuses that case too (`opj_bio_inalign`, `t2.c:1323`), but reads the other
missing bits as 0 and decodes with no error; that is not a deliberate
correction, so this decoder refuses (`tests/packed_overrun.rs`, fixtures in
`tests/fixtures/packed_overrun/`). An inline header whose bits run past the end
of the data is the data ending, and the packet adds nothing.

## Signed 32-bit components

| | |
| - | - |
| This decoder | accepts a signed 32-bit component; refuses unsigned 32 bits or more, and 33 to 38 bits, as `Unsupported` |
| OpenJPEG 2.5.0 | refuses every precision over 31 bits when it reads SIZ (`j2k.c:2289` and `j2k.c:6618`) |
| Standard | ISO/IEC 15444-1 allows `Ssiz` up to 38 bits |
| Rule | 1 (looser than OpenJPEG) |
| Tests and fixtures | `tests/signed_32bit.rs` |
| Decided | 2026-11-01 |

A signed 32-bit sample fits the `i32` container exactly. This decoder refuses
what the container cannot hold, as `Unsupported`, from SIZ and before any tile
is read: an unsigned component of 32 bits or more, and any component of 33 to
38 bits (an `Ssiz` past 38 is a malformed `Marker`). A palette column follows
the same rule. No oracle decodes such a file, so `tests/signed_32bit.rs`
derives the expected samples by hand: a signed 8-bit lossless codestream with
only `Ssiz` patched to 32 bits must decode to the same samples. That shows the
depth is accepted and the sign, level shift and clamp are right for small
values; it does not exercise coefficients near 2^31.

## The JP2 box layer

The JP2 box layer follows OpenJPEG 2.5.0 where a file bends Annex I in a way it
tolerates, and differs from it in both directions in the entries below. It
follows OpenJPEG on:

- `ihdr` anywhere in `jp2h`, a second `jp2h` merged into the first, and a
  header box misplaced at the top level after `jp2h`, applied as if it were
  inside;
- a `jp2c` box whose length is wrong ([A `jp2c` whose length is
  wrong](#a-jp2c-whose-length-is-wrong));
- an `ihdr.NC` that differs from `Csiz`, which decodes, as there: the
  codestream decides the image, `bpcc` is still held to `NC`, and the
  palette's `cmap` is checked against `Csiz`, as OpenJPEG checks it
  (2026-11-01);
- an `ihdr` with a zero width, height or `NC`, or an `NC` over 16384, refused
  as OpenJPEG refuses it;
- the `res ` box, which OpenJPEG never reads, never failing a decode: a
  malformed `resc` or `resd` is reported as absent;
- a malformed header box after the codestream (a second signature or `ftyp`
  box, a `jp2h` without `ihdr`, a `colr` running past the end of the file, a
  bad `cdef`), which is `FileFormat`, as in OpenJPEG, which runs its box
  handlers on what follows `jp2c` once it has decoded (`src/jp2/after.rs`).

### The order of the header checks

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

### `ftyp` without `jp2 `

| | |
| - | - |
| This decoder | refuses as `Unsupported` |
| OpenJPEG 2.5.0 | decodes the first codestream |
| Standard | a file without `jp2 ` compatibility says a JP2 reader cannot present it correctly |
| Rule | 3 (an error over a possibly wrong image) |
| Tests and fixtures | `a_jpx_without_jp2_compatibility_is_unsupported` (`src/jp2/tests.rs`); the "no jp2 in brand or compatibility list" row of `every_robustness_table_row_has_its_outcome` (`src/jp2/tests/matrix.rs`) |
| Decided | 2026-11-01 |

A file whose `ftyp` does not claim `jp2 ` compatibility (pure JPX, JPM or MJ2)
says a JP2 reader cannot present it correctly (composited codestreams, Part 2
extensions, `rreq` features), so its first codestream can be the wrong image
with no error. Refusing it lets a later JPX reader produce the right image
without silently changing output that already "worked".

### `Psot = 0` at the end of `jp2c`

| | |
| - | - |
| This decoder | ends a `Psot = 0` last tile-part at the end of its `jp2c` box when the packet walk ends exactly there; otherwise reads to the end of the file and refuses bytes left after the last packet |
| OpenJPEG 2.5.0 | always reads to the end of the file and ignores bytes left after the last packet |
| Standard | the `jp2c` box holds the codestream |
| Rule | 1 at the box end; 3 for the leftover bytes |
| Tests and fixtures | `tests/psot_zero.rs`, `tests/fixtures/psot0/` (`jp2_t2bomb_*`, `jp2_t2_sweep_*`) |
| Decided | #183, 2026-11-10 |

A `Psot = 0` last tile-part ends at the end of its `jp2c` box only when the
packet walk ends exactly there. Otherwise it runs to the end of the file. The
box end is used when all of these hold:

- the box declares a length (with or without an `EOC`);
- that length ends at least two bytes before the end of the file;
- the box holds every tile-part's `SOT`;
- tier-2 walks the packets of that tile-part's tile to exactly the box's end.

Tier-2 refuses bytes left after the last packet and a tile-part that ends
before its last packet, so the last condition holds only when the packets end
there. OpenJPEG, which reads the tile-part on to the end of the file, then
reads the same packets when the tile has 999 layers or fewer ([More than 999
layers](#more-than-999-layers)). Read to the end of the file, those packets are
followed by bytes left after them, which are refused, so a fault in tier-1 is
the same fault either way.

When the box end is not used (`LBox` 0, or a length that ends inside the
codestream with more bytes after the codestream), the tile-part runs to the
end of the file in both decoders, so the bytes after its last packet are tile
data. OpenJPEG ignores them; the codestream parser rejects them with a
`Codestream` error that counts the bytes left after the last packet, as it
does for a raw codestream.
The packets themselves decode, so this is not the
[leftover-bytes rule](#bytes-left-after-the-last-packet) of a raw stream.

The check is part of reading the file's structure, so `read_header` makes it
too: it walks that one tile's packet headers, without decoding any samples.
Every step of the walk is bounded, since a one-byte packet header can step over
each row of code-blocks of its precinct and a positional progression order can
sweep millions of positions that hold no packet. A codestream of more than 4096
tiles or 1024 components, or of fewer than 14 bytes per tile, is not walked,
nor is a tile of more than 2^14 code-blocks or precincts, and the walk stops
after max(2^22, 8 × the tile's data bytes) steps, counted as the decode's bound
counts them ([The tier-2 work bound](resource-bounds.md#the-tier-2-work-bound)).
Past a bound the check answers no, and the codestream is read to the end of the
file as above (`jp2_t2bomb_*` and `jp2_t2_sweep_*` in `tests/fixtures/psot0/`).

When the box's end is used, the boxes after it are walked by the rules for the
boxes after the codestream and reported as metadata (`xml `, `uuid` and the
like); otherwise the bytes after the box are tile data and nothing is reported
from them. Neither decoder reads header boxes there: OpenJPEG's walk after the
decode starts at the end of the file.

### A `jp2c` whose length is wrong

A `jp2c` whose length is wrong (past the end of the file, `LBox` 2..7, `XLBox`
below 16, shorter than its header, or shorter than its codestream) decodes as
in OpenJPEG, which ignores the length: the codestream is read from the box to
its last tile-part. The boxes after the codestream are walked from where
OpenJPEG's decoder stops, and their header boxes are read again, as OpenJPEG
does once it has decoded, failing where OpenJPEG fails. So a malformed header
box there, or padding inside the `jp2c` box that reads as one, is `FileFormat`
in both.

### Palette

| | |
| - | - |
| This decoder | follows OpenJPEG on what it accepts and on its output samples; refuses a column too deep for `i32`; keeps each channel's own sub-sampling |
| OpenJPEG 2.5.0 | decodes deep columns; sizes every channel by the first one's grid |
| Standard | Annex I (`pclr`, `cmap`) |
| Rule | 3 for deep columns; 1 for mixed sub-sampling (OpenJPEG crashes) |
| Tests and fixtures | `jp2_palette_subsampled_*`; `src/jp2/tests/palette.rs` |
| Decided | 2026-11-01 |

The palette (`pclr` + `cmap`) follows OpenJPEG on what it accepts and on its
output samples, including its quirks: `cmap` holds `NPC` entries whatever its
length, a palette channel `i` must read column `i` (another column is
`Unsupported`, as OpenJPEG refuses it), a direct channel `i` takes column `i`'s
depth, signed entries are not sign-extended, and a single-component file whose
`cmap` leaves a column unused is read as the identity mapping. Two differences:

- Stricter: a palette column too deep for the `i32` sample container (unsigned
  32 bits, or more than 32) is `Unsupported` when the palette is applied, as
  such a codestream component is. OpenJPEG decodes it.
- Looser: palette and direct channels from components of different
  sub-sampling each keep their own component's grid. OpenJPEG sizes every
  channel by the first one's grid and crashes or leaves samples unset, so
  `jp2_palette_subsampled_*` are graded by table lookup over OpenJPEG's
  decode of the bare codestream.

### Channel definitions

| | |
| - | - |
| This decoder | labels each channel with the `cdef` entry that names it, so the label moves with the samples |
| OpenJPEG 2.5.0 | keeps only the type, in `comps[].alpha`, at the position named before the swap, and never writes it out |
| Standard | I.5.3.6 |
| Rule | 1 |
| Tests and fixtures | `every_channel_is_labeled_with_its_entry` in `src/jp2/tests/channels.rs`; the `jp2_cdef_*` fixtures |
| Decided | not recorded |

Channel definitions (`cdef`) follow OpenJPEG on what it accepts and on the
output channel order, including its sequential swap rule: each color entry
swaps its channel to `Asoc - 1`, later entries follow their channels, and an
entry may repeat a channel once every channel is named. The labels on
`Jp2Metadata::channels` have no OpenJPEG output to match: its library keeps
only the type, in `comps[].alpha`, which `opj_decompress` never writes out
(no PGX, PNG or TIFF writer reads it). That field is also written to the
position the entry named before its swap, so after a swap it can sit on the
wrong channel (an ARGB layout loses its opacity flag). Here each channel
carries the entry that names it, so the label moves with the samples, as
I.5.3.6 defines `Cn`.

### Default labels without `cdef`

| | |
| - | - |
| This decoder | the first N channels are colors 1 to N, N being the color space's number of colors; each channel past N is unspecified |
| OpenJPEG 2.5.0 | labels nothing (every `comps[].alpha` is 0); `opj_decompress` decides by component count |
| Standard | I.5.3.6 gives a channel past N no meaning |
| Rule | 1 |
| Tests and fixtures | `a_gray_profile_on_two_channels_without_cdef_is_gray_plus_opacity`, `an_srgb_file_with_a_differently_formatted_fourth_channel_passes_through` |
| Decided | not recorded |

Without a `cdef`, libopenjp2 labels nothing, so the default labels have no
oracle either. `opj_decompress` decides by component count, not by a label:
its writers (`convertpng.c`, `convert.c`) and `color.c` take the last component
of two or four as alpha (PNG caps at four; PGX and raw output have no alpha),
and read two components as gray plus alpha. This decides two
`ColorOutput::Srgb` cases, both in [srgb-output.md](srgb-output.md): two
components with a grayscale ICC profile and no `cdef` convert as gray plus
opacity ([row 20](srgb-output.md#rows-20-and-24-profiles-that-convert)), and
an enumerated sRGB file of four components and no `cdef`, whose fourth differs
in format from the colors, passes through as coded ([row
6](srgb-output.md#rows-6-and-7-srgb-and-grayscale)).

## The `jp2i` and `uinf` boxes

| | |
| - | - |
| This decoder | returns `jp2i` contents as bytes; reports a `uinf` only when it is well formed |
| OpenJPEG 2.5.0 | skips both box types |
| Standard | ISO/IEC 15444-1 I.6, I.7.3, I.8, Figure I.20 |
| Rule | 1; the samples are not affected |
| Tests and fixtures | `src/jp2/tests/uuid_info.rs` |
| Decided | 2026-11-01 |

ISO/IEC 15444-1 is the reference for their contents. Neither affects the
samples. `jp2i` contents are returned as bytes: their format is reserved for
ISO, so nothing about them is checked, and the `ihdr` `IPR` flag is not held to
their presence.

A `uinf` is reported only when it is well formed, and a malformed one is
left out of `Jp2Metadata::uuid_info` as a whole: the decode goes on, as it
does in OpenJPEG, whose `opj_decompress` decodes every malformed variant.
Nothing partial is reported. Well formed means:

- Its contents are whole boxes that fill it exactly. A child box that
  overruns the `uinf`, has length 0, a length of 2..7, an `XLBox` below 16
  or past the `uinf`, or 1..7 bytes left after the last child make it
  malformed. A `uinf` whose own length is bad is a top-level fault, handled
  like any other box's.
- One `ulst` and one `url `, in either order. Figure I.20 shows `ulst`
  first, but I.7.3 does not require it, so the order is not checked. A
  missing or second `ulst` or `url ` makes it malformed. Other boxes inside
  are ignored (I.8).
- The `ulst` holds its 2-byte `NU` and exactly `NU` UUIDs ("the number of
  UUIDi fields shall be the same as the value of the NU field").
- The `url ` holds at least its `VERS` and `FLAG` (4 bytes).

Three requirements are reported rather than checked: `VERS` and `FLAG`
"shall be 0", and `LOC` is "a null terminated string of UTF-8 characters".
`UuidInfo` keeps the version, flags and location bytes as stored, and
`UuidInfo::location_str` returns `None` for a location that is not such a
string.

The boxes are read wherever `xml ` and `uuid` are collected: at the top level
before the codestream, and after it as far as OpenJPEG's walk there goes. A
`uinf` or `jp2i` inside `jp2h` or another superbox is not read. No real file
in the corpus carries either box.

## The `rreq` box

| | |
| - | - |
| This decoder | reports the first top-level `rreq` as `Jp2Metadata::reader_requirements` when it is well formed |
| OpenJPEG 2.5.0 | has no handler for it |
| Standard | ISO/IEC 15444-2 M.11.1 |
| Rule | 1; the samples are not affected |
| Tests and fixtures | `src/jp2/tests/rreq.rs`; files 5 and 7 of the Part 4 set |
| Decided | not recorded |

ISO/IEC 15444-2 M.11.1 is the reference for the Reader Requirements box's
layout: `ML`, then `FUAM` and `DCM` of `ML` bytes each, `NSF` standard features
(`SF` and an `ML`-byte `SM`), and `NVF` vendor features (a 16-byte `VF` and an
`ML`-byte `VM`). The first top-level `rreq`, before or after the codestream, is
reported when it holds exactly those fields. One that is cut short or has bytes
left over gives `None` and the decode goes on, as in OpenJPEG:
`opj_decompress` decodes `file5.jp2` with its `NSF` raised to 65535, or its
`ML` to 255, to the same samples as the unmodified file. The values (an `ML`
outside 1, 2, 4 and 8, an unknown feature) are reported, not checked. Files 5
and 7 of the Part 4 set carry an `rreq`.
