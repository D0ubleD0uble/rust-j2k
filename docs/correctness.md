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

That is a narrow license, not a general one. **Match OpenJPEG by default.**
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
that does not match the packet sequence (OpenJPEG never checks it), a signaled
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
- a last tile-part with `Psot = 0` followed by more bytes, when the `jp2c`
  box does not end where its codestream does: `LBox` 0, or a length that
  ends inside the codestream with more bytes after the codestream. The
  tile-part then runs to the end of the file in both decoders, so the bytes
  after its last packet are tile data. OpenJPEG ignores them; the codestream
  parser rejects them as `Codestream` ("left after the last packet"), as it
  does for a raw codestream (owner decision on #183, 2026-11-10). The
  packets themselves decode, so this is not the tier-2 rule above.

A `Psot = 0` last tile-part ends at the end of its `jp2c` box, with or
without an `EOC`, when the box declares a length that ends at least two
bytes before the end of the file and holds every tile-part's `SOT`, and
tier-2 walks the packets of that tile-part's tile to exactly the box's end.
Tier-2 refuses bytes left after the last packet and a tile-part that ends
before its last packet, so this holds only when the packets end there, and
OpenJPEG, which reads the tile-part on to the end of the file, reads the same
packets. Read to the end of the file, those packets are followed by bytes
left after them, which are refused, so a fault in tier-1 is the same fault
either way. When tier-2 does not reach the box's end (the box ends inside the
codestream), the codestream is read to the end of the file instead, with the
result above. The check is part of reading the file's structure, so
`read_header` makes it too: it walks that one tile's packet headers, without
decoding any samples. Every step of the walk is bounded, since a one-byte
packet header can step over each row of code-blocks of its precinct and a positional
progression order can sweep millions of positions that hold no packet: a
codestream of more than 4096 tiles or 1024 components, or of fewer than 14
bytes per tile, is not walked, nor is a tile of more than 2^14 code-blocks or
precincts, and the walk stops after max(2^22, 8 × the tile's data bytes)
steps, counted as the decode's bound counts them (see below).
Past a bound the check answers no, and the codestream is read to the end of
the file as above (`jp2_t2bomb_*` and `jp2_t2_sweep_*` in
`tests/fixtures/psot0/`). When the box's end is used, the boxes after it are
walked by the rules for the boxes after the codestream and reported as
metadata (`xml `, `uuid` and the like); otherwise the bytes after the box are
tile data and nothing is reported from them. Neither decoder reads header
boxes there: OpenJPEG's walk after the decode starts at the end of the file.

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

Without a `cdef`, libopenjp2 labels nothing (every `comps[].alpha` is 0),
so the default labels have no oracle either. `opj_decompress` decides by
component count, not by a label: its writers (`convertpng.c`, `convert.c`)
and `color.c` take the last component of two or four as alpha (PNG caps at
four; PGX and raw output have no alpha), and read two components as gray plus
alpha. That is why the gray ICC case below matches. Here the first N channels
are colors 1 to N, N being the color space's number of colors, and each
channel past N is unspecified, since I.5.3.6 gives it no meaning. This
decides one `ColorOutput::Srgb` case: two components with a grayscale ICC
profile and no `cdef` convert as gray plus opacity, and match
`opj_decompress -force-rgb` sample for sample (test
`a_gray_profile_on_two_channels_without_cdef_is_gray_plus_opacity`).
It decides a second case: an enumerated sRGB file of four components and no
`cdef`, whose fourth differs in format from the colors (signed 8-bit, say),
passes through as coded, as with a `cdef` and as in OpenJPEG (test
`an_srgb_file_with_a_differently_formatted_fourth_channel_passes_through`).
One check stays on every channel: a file declared sRGB or grayscale on
three components laid out 4:2:0 or 4:2:2 (luma at 1x1, both chroma
channels at 2x2 or both at 2x1) is refused, whatever its `cdef` labels or
without one. Checked on such sRGB files with a `cdef` of color 1 and two
opacity or unspecified channels, of no color, and of colors 2 and 3
only: `opj_decompress` 2.5.0, with and without `-force-rgb`, writes three
full-size planes byte-identical to those of the file with no `cdef` (test
`srgb_4_2_0_or_4_2_2_is_refused_whatever_cdef_labels`). `opj_decompress`
guesses sYCC whenever the second component is sub-sampled in x and the
first has equal x and y sampling, whatever the file declares
(`opj_decompress.c:1620-1623`), but `color_sycc_to_rgb` converts only those
two layouts (`color.c:403-436`); this decoder does not make the guess. Any
other three-component grayscale file passes through as coded, as OpenJPEG
writes it: one whose third channel is signed or the only one sub-sampled,
whose luma is sampled 1x2 with chroma 2x1, or whose layout is in the guess
shape but not one of the two (1x1, 2x1, 1x1, say), where OpenJPEG prints
"CAN NOT CONVERT".

Checked with `opj_decompress` on 1x1:2x1:1x1, 1x1:2x2:1x1, 1x1:2x1:2x2,
1x1:2x2:2x1 and 2x2:2x2:2x2, as grayscale and under an RGB profile: each
is written as coded, except 2x2:2x2:2x2 under the profile, which the
profile converts. A grayscale palette of three columns over a 2x2 index is
written as coded too.

Looser here, where OpenJPEG fails: a malformed header box after the
codestream (a second signature or `ftyp` box, a `jp2h` without `ihdr`, a
`colr` running past the end of the file, a bad `cdef`). OpenJPEG runs its box
handlers on what follows `jp2c`; this decoder only checks lengths there.

The cost is interop: a real-world encoder that produces such a stream decodes
under OpenJPEG and rejects here. No corpus or conformance file trips any of
these today. If one ever does, relax that check to match the oracle — the
protective argument loses to a demonstrated real file.

### The tier-2 work bound

A packet header names every code-block of its precinct, in raster order,
and reads tag-tree bits for each one not yet included. Once a node of the
inclusion tree is known to lie past the current layer, every code-block
under it is excluded from that packet without reading a bit. The decode
steps over them in one go: the rest of the node's span on the current row,
or all of its rows when it spans the precinct's width. It keeps the spans it
has found for the rows below. This yields the same inclusion and tag-tree
state as reading each code-block in turn
(`stepping_over_unread_spans_reads_as_every_leaf_does` checks this over
3000 random precincts and layer sequences). The work of a header is then
proportional to the bits it reads plus the code-blocks it includes, plus
one step for each span it steps over.

That matters for real files: OpenJPEG's encoder sets every packet's
"present" bit, so each layer's header names all of its code-blocks even
when almost none are included. `opj_compress -b 4,4` with 64 or 100 layers
on a nearly flat 2048^2 image (`tests/fixtures/work_bound/opj_*`) used to
visit 262144 code-blocks per layer; it now decodes in 0.15 s.

What remains is still bounded (owner decision on #235, 2026-11-11). Walking
every tile's packets may take at most max(2^24, 32 × the tiles' data
bytes) steps. A step is a packet read, a span stepped over, or a sweep
position, once for each (component, resolution) checked there. Code-block
entries that read a bit are not counted, since the header's bytes already
pay for them. Past the bound the decode is `Limit`. It is one bound for the
whole decode, not one per tile, and there is no option to change it.

A header of one byte can still step over each row of code-blocks in its
precinct, and a positional order can sweep millions of positions that hold
no packet. Measured on release builds (`scripts/gen-work-bound-fixtures.py`;
times are this decoder before this change, then after, then
`opj_decompress` 2.5.0):

| Shape | Size | Before | After | OpenJPEG |
| ----- | ---- | ------ | ----- | -------- |
| LRCP, 2^17 4x4 code-blocks, 1000 one-byte packets | 1.1 KB | 4.6 s | decodes, 14 ms | 6.8 s |
| LRCP, 2^14 code-blocks, 65535 layers | 66 KB | 28.7 s | decodes, 8 ms | 0.3 s |
| LRCP, 2^19 code-blocks, 65535 layers | 66 KB | | decodes, 0.09 s | 26.5 s |
| As above, first packet resolves the root (`t2skip`), 2^19 blocks | 66 KB | | `Limit`, 0.5 s | 37.7 s |
| `t2skip`, 512 x 256 code-blocks, 40000 layers | 40 KB | | `Limit`, 0.4 s | 6.1 s |
| PCRL sweep, 1024 components, step 2 on a 2048^2 tile | 9.3 KB | 28.9 s | `Limit`, 0.3 s | 37.4 s |
| CPRL, one 2^24-row column, 31 POC volumes | 819 B | 6.5 s | `Limit`, 0.1 s | 20.6 s |

The bound scales with size, so a large file of the same shape still takes
as long as its bytes allow: `t2skip` in 153 tiles, 10 MB, is refused after
4.5 s, about what 10 MB of real data takes to decode (about 0.45 µs a
byte). A step costs 10 to 50 ns.

A POC marker's volumes may reach the same packet more than once; the
first reads it and the rest skip it (OpenJPEG's `include` array). The
decode keeps one bit per packet the volumes reach, and skips a packet
already read from that bitmap in about 4 ns, without a step: there are at
most 31 skips per packet read, and charging them would refuse the 1 MB
file below. The bitmap is used when it has no more bits than the steps
left, since a walk that reads every packet takes a step for each; that
keeps it under 2 MB below the floor and 4 bytes per byte of data above it
(`read_header`: 512 KB and 1 byte). With more packets than that, a walk in
layer orders cannot finish and is `Limit` before it starts. One with a
positional order can pass over precincts, so the decode keeps the packets
read in a set, at most one per byte of packet headers and about 24 bytes
each. A skip from the set is a lookup that misses the cache, 80 to 180 ns
against about 10 ns for a step, so each one is charged 12 steps.
`read_header`'s walk keeps no set: past its bitmap it gives up, and the
codestream runs to the end of the file. Measured on
release builds (`pocdup` in `scripts/gen-work-bound-fixtures.py`; times
and peak memory are before this change, then after, then `opj_decompress`
2.5.0; `read_header` is on the `Psot = 0` JP2 form):

| Shape | Size | `decode` before | `decode` after | `read_header` before | `read_header` after | OpenJPEG |
| ----- | ---- | --------------- | -------------- | -------------------- | ------------------- | -------- |
| 16 precincts, 65535 layers, 31 identical volumes | 1 MB | 5.7 s, 105 MB | 0.15 s, 3.5 MB | 5.3 s, 105 MB | 0.15 s, 3.5 MB | 0.10 s, 9 MB |
| 160 components, 65535 layers, 2 volumes | 10.5 MB | 4.5 s, 824 MB | 0.32 s, 14 MB | 4.2 s, 824 MB | 0.33 s, 14 MB | 0.57 s, 37 MB |
| 160 precincts, 65535 layers, 32 volumes | 10.5 MB | 65 s, 824 MB | 1.5 s, 14 MB | 71 s, 824 MB | 1.5 s, 14 MB | refused, 32 POCs |
| 2^14 precincts, 65535 layers, 2 volumes, 10^7 packets in the data | 10 MB | `Codestream`, 1.7 s, 828 MB | `Limit`, 0.003 s, 16 MB | 1.8 s, 828 MB | 0.003 s, 16 MB | 53 s, 2.1 GB |
| The same in RPCL | 10 MB | `Codestream`, 1.9 s, 828 MB | `Codestream`, 1.2 s, 238 MB | 2.0 s, 828 MB | not walked, 0.004 s, 16 MB | 153 s, 2.1 GB |
| `setskip(65535, 31)`: RPCL, 6.5e8 packets reached, 1.4e7 read | 14 MB | decodes, 46.7 s, 240 MB | `Limit`, 5.5 s, 240 MB | 44 s | not walked, 0.003 s, 19 MB | |

Encoder output stays far below the bound. Every conformance file spends
under 0.5% of it (`p1_05` the most, at 0.29 steps per byte; `p0_13` takes
0.76). `opj_compress` output takes 0.04 to 88 steps per byte; the high end
is files of a few hundred bytes with 4x4 code-blocks and 64 or 100 layers,
which stay under the floor (the committed ones use under 0.1% of it). The
largest share seen is 12.8%: a sparse 2048^2 image in 4x4 code-blocks with
64 layers, 469 KB at 4.6 steps per byte.
`every_committed_file_walks_far_below_the_work_bound` prints the table for
the committed files and asserts each is under 1/64.

The bound does refuse some valid files. Take one tile, no wavelet levels,
4x4 code-blocks in one precinct, LRCP, and many layers that each include
one new code-block at a scattered place (one pass, one byte), with the
blocks included earlier reading a "no new passes" bit. The new blocks keep
the inclusion tree from settling high up, so each layer steps over many
spans narrower than the precinct, one step per row each: about W·√n steps
a layer for a precinct W blocks wide after n layers. The steps per byte
grow with the layer count, so no constant per byte admits the whole class.
At 2048^2 or more it passes the bound from about 600 to 800 layers:

| Image | Layers | Size | Refused after | OpenJPEG |
| ----- | ------ | ---- | ------------- | -------- |
| 2048^2 | 800 | 527 KB | 1.1 s | 6.9 s |
| 2896^2 | 600 | 343 KB | 1.2 s | 10.5 s |
| 2896^2 | 800 | 585 KB | 1.5 s | 15.4 s |

OpenJPEG decodes all three. No encoder seen writes this shape; the files
were built by hand for the review of #235.

The charge on skips from a POC sequence's set refuses another class. Take
one 38608 x 1 tile in RPCL, no wavelet levels, 1 x 1 precincts, one
component at `XRsiz` 254 and 64 at 255. The first sets the sweep's step to
254, which meets only the first of each other component's 152 precincts,
so the volumes reach 9880 packets a layer but read 216. That is more than
the bitmap may hold, so the packets read go in a set, and every volume
after the first skips each of them at 12 steps. With 2048 layers (442 KB)
one volume decodes and 31 are `Limit` after 0.05 s; OpenJPEG decodes both
in 0.14 s or less. `setskip` in `scripts/gen-work-bound-fixtures.py`
writes the shape, and `skips_from_the_poc_set_are_bounded` in
`tests/work_bound.rs` builds it. It came from the review of PR #261.

The bound covers the walk `read_header` makes for a `Psot = 0` tile-part
(see above). That walk counts the same steps under its own bound,
max(2^22, 8 × the tile's data bytes), no larger at any size, so for a
single-tile codestream a tile that walk finishes is one the decode's walk
finishes too. The decode's bound does not replace that walk's geometry
bound of 2^14 code-blocks: `read_header` allocates for its walk, and a
geometry bound at the decode's 2^19 code-blocks would let `read_header`
hold about 75 MB for a 66 KB file.

### The `jp2i` and `uinf` boxes

OpenJPEG 2.5.0 skips both box types, so ISO/IEC 15444-1 (I.6, I.7.3) is the
reference for their contents. Neither affects the samples. `jp2i` contents
are returned as bytes: their format is reserved for ISO, so nothing about
them is checked, and the `ihdr` `IPR` flag is not held to their presence.

A `uinf` is reported only when it is well formed, and a malformed one is
left out of `Jp2Metadata::uuid_info` as a whole: the decode goes on, as it
does in OpenJPEG, whose `opj_decompress` decodes every malformed variant
(owner decision, 2026-11-01). Nothing partial is reported. Well formed means:

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

### The `rreq` box

OpenJPEG 2.5.0 has no handler for the Reader Requirements box, so ISO/IEC
15444-2 M.11.1 is the reference for its layout: `ML`, then `FUAM` and `DCM`
of `ML` bytes each, `NSF` standard features (`SF` and an `ML`-byte `SM`), and
`NVF` vendor features (a 16-byte `VF` and an `ML`-byte `VM`). The first
top-level `rreq`, before or after the codestream, is reported as
`Jp2Metadata::reader_requirements` when it holds exactly those fields. One
that is cut short or has bytes left over gives `None` and the decode goes on,
as in OpenJPEG: `opj_decompress` decodes `file5.jp2` with its `NSF` raised to
65535, or its `ML` to 255, to the same samples as the unmodified file. The
values (an `ML` outside 1, 2, 4 and 8, an unknown feature) are reported, not
checked. Files 5 and 7 of the Part 4 set carry an `rreq`.

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

`ColorOutput::Srgb` converts sYCC and the two restricted ICC profile forms
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
`opj_decompress` sample for sample, and `tests/jp2_sycc_replication.rs`
grades them and the even-origin `jp2_sycc_420_even` and `jp2_sycc_422_even`
against `opj_decompress -r 1` too: there the reduced origin's parity
differs from the full one's in y (and in x for the even fixtures), so a
replication keyed on the reduced origin fails. The only change is where
OpenJPEG would read past the chroma plane: that sample takes chroma zero
here.

**ICC profiles are converted as LittleCMS converts them.** OpenJPEG hands
the profile to LittleCMS 2 (2.14 in the oracle build) with the profile's
rendering intent. `src/jp2/lcms.rs` ports the part of lcms these profiles
reach: the pipeline it builds (the tone curves and matrix into XYZ, the
white-point scaling of absolute colorimetric, black point compensation under
perceptual and saturation, since lcms's sRGB profile is ICC v4, then the
inverse sRGB matrix and curves), the optimization it picks for that pipeline
(the 8-bit matrix-shaper in 1.14 fixed point, prelinearization curves with a
33-point table, or resampling into a 33-point table, with the white fix-up;
for 16-bit RGB on a profile of gamma below 1.6, no optimization), and its
16-bit interpolation, step for step at lcms's precision. It also applies
the checks lcms makes when it opens a profile (`_cmsReadHeader`): the `acsp`
signature, a version of at most 5.0, a device class lcms knows, at most 100
tags, no two kept entries with the same signature. Like lcms, it first skips
a tag entry with a zero offset or size, or one that ends past the size the
header declares (capped at the profile's length), so a tag it needs is then
missing, and a skipped entry does not count as a duplicate. When building
the transform it makes lcms's checks too: a named color profile, a device
link or abstract profile without a lookup table, or a connection space
other than XYZ or Lab builds none, and of the lookup tables lcms would read
for the intent (`D2Bx`, then `A2Bx`, then `A2B0`) it checks the type, the
channel counts, the grid and the table's length as lcms's reader does. It reads tags as `cmsReadTag`
does: a tag under 8 bytes or of a type lcms does not allow for it fails,
lcms's legacy types are read (an old Monaco tool's curve type for `rTRC`,
`gTRC` and `bTRC`, an old Corbis tool's XYZ type), and a value is read
without checking the tag's size, so one that runs past its tag takes the
bytes after it and fails only past the end of the profile. A media white
point lcms can't read is D50 (`_cmsReadMediaWhitePoint`). lcms's
black-preserving intents 10 to 15 on a profile that is not CMYK become
perceptual, relative colorimetric or saturation without the black point
compensation lcms forces for intents 0 and 2, which for these profiles is
relative colorimetric in every case; other intents above 3 build no
transform.

The output matches `opj_decompress -force-rgb` sample for sample
(`tests/jp2_icc.rs`). The reference is OpenJPEG 2.5.0 linked against
LittleCMS 2.14 on x86-64 Linux (glibc 2.39, on a CPU with FMA and AVX2).

LittleCMS calls the C library's `pow` and `log`, and a platform's own can
differ in the last bit, which moves an output level. So `src/jp2/math.rs`
ports glibc 2.39's `pow` and `log` (Arm's optimized-routines code), and the
output is the same on every platform whose `fma` is correctly rounded.
MinGW's (`*-pc-windows-gnu`) may round twice (rust-lang/rust#140515), so
the output there can differ. On x86-64, glibc picks a variant built with
FMA when the CPU has FMA and AVX2, and a plain SSE2 one otherwise. The port
follows the FMA variant, the one the reference machine runs: each
multiply-add GCC fused there is a `mul_add` in the port. The two variants differ
in the last bit on 296 of 5.2 million `log` inputs and 3,251 of 3.7 million
`pow` inputs of an earlier test sweep, so oracles regenerated on an x86-64 CPU
without FMA could differ. The current ones do not: regenerating them with
glibc held to its SSE2 variant (`GLIBC_TUNABLES=glibc.cpu.hwcaps=-AVX2,-FMA`)
gives the same hashes. The default test (`src/jp2/math_tests.rs`) checks
the port's results on a sweep of inputs against digests recorded from
glibc, on every platform. An ignored, larger sweep compares the port bit
for bit with the platform's `f64::ln` and `f64::powf` on x86-64 Linux with
glibc and FMA.

The cases:

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
  0, 140 and 0xFFFFFFFF, and ones that cut off `wtpt` or an `A2B0` table);
- 25 RGB profiles at 8 and 16 bits and 4 monochrome ones on how lcms reads
  intents, legacy tag types and tags that run short (the table below);
- 58 RGB profiles at 8 and 16 bits and 13 monochrome ones on device
  classes, connection and color spaces, lookup tables and missing tags
  (the table below).

During development the port also matched lcms 2.14 itself on every 8-bit
RGB input (2^24) and a million random 16-bit pixels for each of these
profiles. Against the ISO references the result is OpenJPEG's, inside the
class bounds (PAE 4, MSE 1):

| File | Ours vs ISO (MSE) | Ours vs OpenJPEG |
| ---- | ----------------- | ---------------- |
| file5 (RGB, gamma 1.8) | 0.0897 / 0.1750 / 0.2050 | identical |
| file7 (RGB, 16-bit, 2048-entry curves; after `>> 8`) | 0.1442 / 0.1770 / 0.1900 | identical at 16 bits |
| file8 (gray, `kTRC`) | 0.2739 | identical |

Peak error against ISO is 1 everywhere. The floating-point evaluation this
replaced was slightly closer to ISO on files 5 and 8 (MSE 0.0846 / 0.1723 /
0.2025 and 0.2115). OpenJPEG is the reference decoder, and nothing in the
standard asks for a closer match to these display references, so the
evidence rule above keeps OpenJPEG's output.

Two output shapes differ from `opj_decompress`, by choice of the grading
harness and not of the samples: file7 stays 16-bit, and the harness shifts
it down by 8 to compare with the 8-bit reference (its `shift_right`, as for
the as-coded grade), and file8 stays one gray channel where OpenJPEG writes
three identical ones (graded on that one channel).

**A profile lcms refuses passes through, as in OpenJPEG (owner decision,
2026-11-01, on #199).** When lcms will not open the profile or build a
transform from it, `color_apply_icc_profile` returns without converting and
`opj_decompress` writes the samples as coded. `Srgb` does the same, and
`Jp2Metadata::color` stays `RestrictedIcc`, so a caller can tell. Each
case was checked with `opj_decompress` 2.5.0 on a crafted file, and is a
case of `icc/profiles.json` and a test in `src/jp2/srgb_tests.rs`:

| Case | OpenJPEG + lcms 2.14 | `Srgb` here |
| ---- | -------------------- | ----------- |
| profile under 132 bytes, no `acsp`, version above 5.0, unknown class (`zzzz`), more than 100 tags, tag table past the end, two kept entries with one signature | unconverted (lcms won't open it) | as coded |
| a duplicate whose first copy lcms skips | converted from the second | the same |
| class `mntr`, `prtr`, `spac` or 0 with a matrix and curves | converted as `scnr` (a version 2 `mntr` takes D50 as its white point) | the same |
| class `nmcl`; class `link` or `abst` with no lookup table | unconverted | as coded |
| intent 4 to 9 or above 15 | unconverted | as coded |
| connection space other than XYZ or Lab | unconverted | as coded |
| color space other than RGB, gray or YCbCr | unconverted (no transform for it) | as coded |
| lookup table lcms would read of the wrong type, wrong channel counts, a grid of 1, or cut short | unconverted | as coded |
| lookup table for another intent only | converted from the matrix | the same |
| lookup table lcms reads | converted through it | `Unsupported` (below) |
| Lab connection space with a matrix and curves | converted | `Unsupported` (below) |
| intent 10 to 15 | converted as intent 1 | converted as intent 1 |
| needed tag missing or skipped, under 8 bytes, of the wrong type, or with a value past the profile's end; `para` function above 4; `curv` of more than 32767 entries | unconverted | as coded |
| value past its tag but inside the profile | converted from the bytes after it | the same |
| Monaco curve type, Corbis XYZ type | converted | converted |
| `wtpt` unreadable at intent 3 | converted with D50 | converted with D50 |

The order follows OpenJPEG's too (owner decision, 2026-11-02, on #199):
every refusal above, whether lcms makes it opening the profile or building
the transform, comes before this decoder's own checks below. So a profile
lcms refuses passes through whatever the samples are (bar the 4:2:0 and
4:2:2 exception below): an intent of 4 on
12-bit RGB, 16-bit gray, or a monochrome profile on three channels gives
the samples as coded, as `opj_decompress` does. The refusals below (#165,
#197) apply only where lcms would build the transform. Test:
`refusals_are_ordered_as_in_openjpeg`.

One exception comes before all of it: a three-channel 4:2:0 or 4:2:2 file.
`opj_decompress` guesses sYCC and converts it before `color_apply_icc_profile`
runs (`opj_decompress.c:1620-1635`), so a profile lcms refuses does not
leave it as coded. Checked on `jp2_sycc_420_odd` and `jp2_sycc_422_odd`
under `lcms_intent_4`, a refused gray profile (`gray_lcms_intent_4`), a
profile cut short (`bad_short_header`) and a CMYK profile (`space_cmyk`):
`-force-rgb` writes the sYCC original's planes each time. `Srgb` makes no
sYCC guess, so it refuses all of them, before the profile is read, with the
sub-sampling message. Tests:
`an_lcms_refused_profile_on_4_2_0_or_4_2_2_is_refused`, and
`an_lcms_refused_profile_passes_through_outside_the_sycc_layouts` for the
layouts that stay as coded.

**An RGB profile is skipped when the first four channels differ, as in
OpenJPEG (owner decision, 2026-11-02, on #206).** In
`color_apply_icc_profile` (OpenJPEG 2.5.0, `src/bin/common/color.c`), lcms
opens the profile first (`cmsOpenProfileFromMem`, :468-477). For an RGB
profile OpenJPEG then returns on fewer than three components (:491-494),
and otherwise runs its "AFL test" (:495-519): it compares
`comps[1..min(numcomps, 4)]` with `comps[0]` by `dx`, `dy`, `prec` and
`sgnd`, and returns without converting if any differs. Only then does lcms
build the transform (`cmsCreateTransform`, :586). The gray (:530-534) and
YCbCr (:535-543) branches have no such test. The components are the image
after the palette and `cdef`, by position, so a differing opacity channel
counts and a fifth channel does not.

`Srgb` runs the same test in the same place: after the profile is opened
and before the transform is built, so before every check of this
decoder's own (a lookup table, the Lab connection space, a `cdef` without
colors 1 to 3, an unsupported depth). One case is refused before the
test: a 4:2:0 or 4:2:2 image, which `opj_decompress` converts
as sYCC before it reads the profile (`opj_decompress.c:1620-1630`). Other
layouts in its sYCC-guess shape stay as coded there, so the test applies
to them: 1x1:2x1:1x1 and 1x1:2x2:1x1 pass through, and 2x2:2x2:2x2
matches and converts.

Checked against `opj_decompress` 2.5.0 with lcms 2.14. With and without a
`cdef`, a signed, a 12-bit, a 2x1 and a 1x2 sub-sampled fourth channel,
and a signed or sub-sampled third color, pass through as coded
(`-force-rgb` fails, as nothing was converted). A lookup-table or Lab
profile, or a `cdef` of two colors, passes through with a signed fourth
channel. A matching fourth channel, a sub-sampled fifth, and a gray
profile with a signed opacity channel convert. Tests (`tests/jp2_icc.rs`):
`an_rgb_profile_passes_through_when_the_first_four_channels_differ`,
`the_afl_test_comes_before_this_decoders_refusals`,
`the_afl_test_covers_only_rgb_profiles_and_four_channels`,
`the_afl_test_applies_where_openjpeg_cannot_convert_as_sycc`.

**Stricter: inputs refused under `Srgb` that OpenJPEG decodes some other
way.** Each is `Unsupported` unless noted, and each is a row of
`srgb_reject_matrix_maps_each_input_to_its_typed_error` in
`src/jp2/srgb_tests.rs`:

- color channels that differ in depth, sign or sub-sampling, other than
  the sYCC chroma layouts below (owner decision, 2026-11-01). Under an RGB
  profile they pass through as in OpenJPEG instead (the AFL test above),
  except for 4:2:0 and 4:2:2, and except where a `cdef` leaves a differing
  color past the first four positions. There the AFL test passes and
  OpenJPEG converts whatever sits at positions 0 to 2, color or not, so
  this decoder refuses the file rather than give a wrong image. Checked: the
  fuzz seed `icc_rgb_uniform_after_afl` (five palette channels, the 4-bit
  color 3 moved to position 4 by a second color-3 entry) comes out of
  `opj_decompress` with positions 0 to 2 converted and the 4-bit channel as
  coded. For a three-component 4:2:0 or 4:2:2 file,
  whatever `colr` declares (sRGB, grayscale, another space, a profile, or
  none) and whatever its `cdef` labels, or a raw codestream,
  `opj_decompress` guesses sYCC and converts it; that guess is not made
  here.
  Checked: `jp2_sycc_420_odd.jp2` with its `EnumCS` patched to 17
  (grayscale) gives byte-identical `opj_decompress` output to the sYCC
  original, while `Srgb` here refuses it (a row of the reject matrix);
- sYCC other than full-resolution luma with chroma at 1x1, 2x1 or 2x2.
  OpenJPEG prints "CAN NOT CONVERT" and writes the samples as coded;
- signed sYCC (owner decision, 2026-11-01, on #165). OpenJPEG converts it
  without a message, as if it were unsigned, and clamps: on an 8-bit signed
  4:4:4 file, as-coded R `[-7, -62, 61, 114, ...]` comes out
  `[0, 0, 15, 0, ...]`. sYCC is defined on unsigned samples, so there is no
  right answer to match;
- three or more channels with an enumerated color space other than sRGB,
  grayscale or sYCC, or with no `colr`, and a raw codestream of three or
  more components, which declares no color space. For three components at
  the same sampling `opj_decompress -force-rgb` fails too; at 4:2:0 or
  4:2:2 it converts them as sYCC, which owner decision (2) above
  refuses. One or two channels pass through as gray (and gray plus
  opacity) whatever the color space, as OpenJPEG reads any image of two
  or fewer components: from a raw codestream, or a JP2 file with no
  `colr`, any enumerated space (sRGB included, owner decision, 2026-11-02,
  on #211, even when the two channels differ in depth or sign), or an RGB
  or YCbCr profile. Only a monochrome profile converts them, on 8-bit
  unsigned samples only (other depths are refused, below). Their
  metadata stays as declared, since nothing was converted (tests
  `a_gray_jp2_without_a_converted_color_space_passes_through`,
  `srgb_on_one_or_two_channels_passes_through`);
- a monochrome (`GRAY`) ICC profile on more than two channels (owner
  decision, 2026-11-01, on #197), for example one color channel and two
  opacity channels in `cdef`, or a palette of three columns. OpenJPEG takes
  its `numcomps > 2` branch, which passes the planes interleaved to a gray
  transform as if they were RGB, so its three-channel output is not a
  conversion. Checked on a 24x16 file with a tinted gray profile and a
  `cdef` of (color 1, opacity, opacity): `opj_decompress` writes an RGB
  PNG; the earlier output here was five channels. One gray channel (1 to 1
  or 3) and gray plus opacity (2 to 2 or 4) match OpenJPEG and are
  unchanged. Test:
  `a_gray_profile_on_more_than_two_channels_is_unsupported`;
- an ICC profile lcms would apply through a lookup table (`D2Bx` or
  `A2Bx`, which it prefers over the matrix) or the Lab connection space.
  OpenJPEG converts these; this decoder ports only the matrix/TRC pipeline
  into XYZ. A table whose header passes lcms's checks but whose contents
  lcms rejects (an `mpet` with no elements, say) is refused here too, where
  OpenJPEG writes the samples as coded;
- a YCbCr (`YCbr`) ICC profile on three or more channels that LittleCMS
  would build a transform from (orchestrator decision, 2026-11-02, on
  #199). OpenJPEG runs it as 16-bit YCbCr whatever the depth:
  `opj_decompress` 2.5.0 converts 16-bit samples and crashes on 8-bit ones.
  The refusal is a deviation at 16 bits, kept because YCbCr is not one of
  Annex I's restricted forms. On one or two channels, or where LittleCMS
  refuses the profile (intent 4, a missing tag), it passes through as in
  OpenJPEG (`refusals_are_ordered_as_in_openjpeg`);
- an RGB profile on samples other than 8- or 16-bit unsigned, or a gray
  profile on samples other than 8-bit unsigned (owner decision, 2026-11-01,
  on #165). OpenJPEG picks an lcms format from the precision alone
  (`TYPE_RGB_8` up to 8 bits, else `TYPE_RGB_16`; `TYPE_GRAY_8` for gray)
  and so scales other depths wrongly, past the samples' own range. The
  review measured a 12-bit 4095 coming out 7413. With file5's profile on a
  4-bit RGB image, measured here, gray 15 comes out 18, gray 12 comes out
  13, and (15, 0, 0) comes out with red 29, all written into 4-bit planes;
- `raw_channels` with `Srgb`: `InvalidOptions`, before anything is read.

Chroma replication grows the output, so it is checked against the decode's
sample budget before the tiles are read (`Limit`), as palette expansion is.
After a conversion `Image::jp2` reports what the samples now are, as
OpenJPEG does (owner decision, 2026-11-01, on #165): `ColorSpace::Srgb`,
or `ColorSpace::Grayscale` for a monochrome profile, with no ICC profile.
A monochrome profile whose output is not neutral (absolute colorimetric with
a tinted media white point) gives three channels, R, G and B in place of the
gray one, as `ColorSpace::Srgb`; this is checked against the sample budget
too. sYCC or an RGB profile on one or two color channels passes through, as
OpenJPEG leaves it, with the metadata as declared.
A pass-through, the default `AsCoded` decode and `read_header` report the
color space the file declares.

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
files 4, 6 and 8 are grayscale written as three identical channels. Files 2,
3, 5, 7 and 8 therefore can't be graded on the samples as coded. They are
decoded with `ColorOutput::Srgb` and graded against the references, and
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
is re-initialized for every coding pass, so a decoder that ignores `segmentation
symbols` and never reads the four decisions ending each cleanup pass still
decodes correctly: the symbols it skipped are trailing bytes in a segment nobody
reads again. Turn `restart` off and the same decoder desynchronizes immediately.
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

The vertical oracle proves the *whole* pipeline; it does not localize a fault.
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
`<name>.expected.json` (image area, tolerance, per-component samples, provenance).

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
