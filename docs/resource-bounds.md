# Resource bounds

This decoder refuses a file whose header asks for far more memory or work
than its size justifies. OpenJPEG allocates and works as much as the header
asks for, so a file of under a kilobyte can make it run for many seconds
(an 819-byte file takes 20.6 s), and files of 295 KB to 10 MB can make it
allocate gigabytes. This decoder caps three such costs: the samples it
allocates, the packet-header walk (tier 2) and the code-block decoding
(tier 1). A file past a cap is refused with `Error::Limit` before the work
is done. The `Limit` message names the option that raises the cap and the
smallest `WorkLimit::Raised(n)` that admits the file.

| Bound | Default | Option in `DecodeOptions` | Margin over encoder output | Tests |
| ----- | ------- | ------------------------- | -------------------------- | ----- |
| [Samples](#the-sample-limit) | 2^28 samples at the decode's reduction | `sample_limit` | about 2.2 times (a Sentinel-2 10 m band) | `tests/sample_limit.rs`, `tests/sample_limit_alloc.rs` |
| [Tier-2 steps](#the-tier-2-work-bound) | max(2^24, 32 × the tiles' data bytes) | `packet_work_limit` | about 7.8 times (largest share 12.8%) | `tests/work_bound.rs`, `src/tier2/work_bound_tests.rs` |
| [Tier-1 sample-passes](#the-tier-1-work-bound) | max(2^26, 2^15 × the tiles' data bytes) | `code_block_work_limit` | about 2.35 times | `tests/work_limit.rs`, `src/tier1/work_bound_tests.rs` |

The encoder figures come from the committed fixtures, the Part 4 corpus, and
the Sentinel-2, GDAL and OpenJPEG test files named in each section. Each
section also lists the valid files its bound refuses. A fourth cost, resolving
tile headers, was removed by making the parse cheaper rather than by adding a
cap ([Tile header cost](#tile-header-cost)).

A few fixed guards also return `Limit`, and no option lifts them: 2^18
precincts and 2^19 code-blocks per tile, 32 POC progression volumes, and 30
coded bit-planes per code-block.

The caps count work, not time: samples, packet-walk steps and code-block
sample-passes, each defined below. The times on this page were measured on
release builds when each cap was set, before the 0.4.0 speed-ups to tier 1, the
wavelet, tier 2 and dequantization (#314, #315, #319 to #322 and #324), so the
decoder is now faster than some of them show. Where a table has a "Before"
column, it gives this decoder's time before the change named in its header.

## The sample limit

The decode holds every component in its own plane, so what it allocates
grows with the sum of the component areas, not with the file. A file of a
few hundred bytes can declare a 65536x65536 image, 16 GiB of `i32`
samples. The decode counts the samples of every output channel, at the
resolution reduction asked for, from SIZ and the JP2 boxes alone, and
refuses an image of more than 2^28 samples (1 GiB as `i32`) with `Limit`
before it reads a tile. The image area counted is SIZ's
`(Xsiz − XOsiz)·(Ysiz − YOsiz)`, wherever on the reference grid the origin
sits. The count covers the codestream's components, a palette's expanded
channels, and the channels sRGB output adds (sYCC chroma replicated to the
luma grid, a gray channel a monochrome profile maps to R, G and B).
`read_header_with` makes the same check, so it refuses what the decode would.
OpenJPEG has no such limit: it allocates what the header declares and fails
only when the allocation does.

`DecodeOptions::sample_limit` raises the limit (`WorkLimit::Raised(n)`,
n times 2^28) or lifts it (`WorkLimit::Unlimited`). The refusal names the
option and the smallest `Raised` factor that admits the file. Even
`Unlimited` refuses an image whose 4-byte samples would pass `isize::MAX`
bytes, so the buffer sizes cannot overflow `usize`, on 32-bit targets
included. Below that, under `Unlimited`, an allocation the system cannot
satisfy aborts the process.

The default admits a Sentinel-2 10 m band (10980x10980, 120.6 M samples)
and RGB images up to about 89 MP, and a reduced decode of a larger image
when the reduced image fits: GDAL's `single_block_32769_16385.jp2` (537 M
samples) decodes at reduction 3 (4097x2049), bit-exact against
`opj_decompress -r 3` (#281, 2026-11-12). Measured on a release build against
`opj_decompress` 2.5.0 (wall time on a shared, loaded machine; peak resident
memory from `/usr/bin/time -v`):

| File | Decode | Time | Peak RSS | OpenJPEG time | OpenJPEG peak RSS |
| ---- | ------ | ---- | -------- | ------------- | ----------------- |
| `T31UDQ_20240105T105339_AOT_10m.jp2`, 10980x10980, 690 KB | full, bit-exact | 4.2 to 4.6 s | 470 MiB | 3.6 to 4.4 s | 471 MiB |
| the same | reduction 2 (2745x2745) | 0.78 s | 32 MiB | | 36 MiB |
| `single_block_32769_16385.jp2` | reduction 3, bit-exact | 1.05 s | 86 MiB | 0.25 s | 92 MiB |

The peak is about 1.0 times the 4-byte output for the Sentinel-2 band (460
MiB of samples) and about 1.35 times for one square tile of one component
(see the table in `src/samples.rs`), so at the default a square tile would
peak at about 1.4 GiB (extrapolated, not measured). A tall, narrow tile peaks
higher: about 1.5 to 1.7 times when it is 8 to 1024 samples wide, and about
2.6 times when it is one sample wide (at 2^24 samples, 3 levels). The
multiple grows with the wavelet levels, as the code-blocks' fixed overhead
is a larger share of it: 2.7 at 5 levels and 3.2 at 8.
While a palette expands the peak is about twice the output. Tests:
`tests/sample_limit.rs` and `tests/sample_limit_alloc.rs`.

## The tier-2 work bound

The decode bounds the steps of its packet-header walk (#235, 2026-11-11). This
section says what a step is, why the bound is needed, and what it refuses.

### Stepping over code-blocks

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
on a nearly flat 2048x2048 image (`tests/fixtures/work_bound/opj_*`) used to
visit 262144 code-blocks per layer; it now decodes in 0.15 s.

### The bound

The walk needs a bound because a one-byte header can still step over each
row of its precinct, and a positional order can sweep positions that hold
no packet. Walking every tile's packets may take at most max(2^24, 32 × the
tiles' data bytes) steps. A step is a
packet read, a span stepped over, or a sweep position, once for each
(component, resolution) checked there. Only the pairs that hold a precinct
are checked, and RPCL sweeps no resolution where none does. Code-block
entries that read a bit are not counted, since the header's bytes already
pay for them. Past the bound the decode is `Limit`. It is one bound for the
whole decode, not one per tile. `DecodeOptions::packet_work_limit`
raises it (`WorkLimit::Raised(n)`, n times the floor and the rate) or lifts
it (`WorkLimit::Unlimited`), for the walk `read_header_with` makes too
([The `read_header` walk](#the-read_header-walk)).

Measured on release builds (`scripts/gen-work-bound-fixtures.py`):

| Shape | Size | Before #235 | Since #235 | OpenJPEG 2.5.0 |
| ----- | ---- | ----------- | ---------- | -------------- |
| LRCP, 2^17 4x4 code-blocks, 1000 one-byte packets | 1.1 KB | 4.6 s | decodes, 14 ms | 6.8 s |
| LRCP, 2^14 code-blocks, 65535 layers | 66 KB | 28.7 s | decodes, 8 ms | 0.3 s |
| LRCP, 2^19 code-blocks, 65535 layers | 66 KB | | decodes, 0.09 s | 26.5 s |
| As above, first packet resolves the root (`t2skip`), 2^19 blocks | 66 KB | | `Limit`, 0.5 s | 37.7 s |
| `t2skip`, 512 x 256 code-blocks, 40000 layers | 40 KB | | `Limit`, 0.4 s | 6.1 s |
| PCRL sweep, 1024 components, step 2 on a 2048x2048 tile | 9.3 KB | 28.9 s | `Limit`, 0.3 s | 37.4 s |
| CPRL, one 2^24-row column, 31 POC volumes | 819 B | 6.5 s | `Limit`, 0.1 s | 20.6 s |

The bound scales with size, so a large file of the same shape still takes as
long as its bytes allow: `t2skip` in 153 tiles, 10 MB, is refused after 3.1 s,
about what 10 MB of real data takes to decode. A step costs 2 to 15 ns. The
conformance codestreams over 100 KB decode at 0.19 (`p1_05`) to 0.73 (`p1_04`)
µs a byte, the best of five release-build decodes of each. Over all 23 the
range is wider, 0.06 to 0.75 µs a byte. The cheapest refusals are in the layer
orders over resolutions with no precinct, at about 2 ns a step: `layer_edge`
(655 KB, 33 steps a byte) is refused in 0.015 to 0.04 s, 0.02 to 0.06 µs a
byte, and OpenJPEG decodes it in 0.08 s.

### Encoder output against the bound

Encoder output stays far below the bound. Every conformance file spends under
0.5% of it (`p1_05` the largest share, at 0.247 steps per byte; `p0_13` spends
0.76 per byte but is 2.5 KB, under the floor). `opj_compress` output takes 0.04
to 88 steps per byte; the high end is files of a few hundred bytes with 4x4
code-blocks and 64 or 100 layers, which stay under the floor (the committed ones
use under 0.1% of it). The largest share is 12.8%: a sparse 2048x2048 image in
4x4 code-blocks with 64 layers, 469 KB at 4.6 steps per byte.
`every_committed_file_walks_far_below_the_work_bound` (in
`src/tier2/work_bound_tests.rs`) prints the table for the committed files and
asserts each is under 1/32 of it.

### Tile geometry

At full resolution, setting up a tile's geometry is not counted. A tile of
one sample at an odd origin has 32 resolutions per component that hold no
precinct, and each component of each tile costs about a byte. Since #271
those resolutions get no band or code-block geometry, and the walk checks no
component there. Measured on release builds (`deep` in
`scripts/gen-work-bound-fixtures.py`, a row of one-sample tiles at y = 1, 32
levels, one layer, every packet 0x00):

| Shape | Size | Before #271 | Since #271 | OpenJPEG 2.5.0 |
| ----- | ---- | ----------- | ---------- | -------------- |
| 1 tile, 16384 components | 66 KB | 0.5 s, 383 MB | 0.18 s | 0.4 s, 192 MB |
| 64 tiles, 1024 components | 70 KB | 1.7 s | 0.41 s | 0.24 s |
| 64 tiles, 16384 components | 1.1 MB | `Limit`, 21 s | 10.5 s, 226 MB | 5.5 s, 1.3 GB |

The last is about 10 µs a byte, twice OpenJPEG, and the slowest shape
known per byte. The same change lets the layer orders decode 8 such
components in one tile, and the positional orders a 16-component column
of 2^17 rows, which were `Limit` and which OpenJPEG decodes
(`tests/work_bound.rs`).

### Geometry at a reduction

At a resolution reduction the geometry is counted too: one step for each
code-block of a resolution the reduction drops (#281). Every tile builds its
geometry at full resolution whatever the reduction, and the per-tile caps on
precincts and code-blocks do not bound a file of many tiles. Once the sample
limit was counted at the reduction, a small file could build a great deal of
geometry it never decodes: 2800x2800 tiles in 4x4 code-blocks, six levels, one
empty byte per packet, decoded at reduction 6, took 10.3 s for 256 tiles
(5.5 KB), 147 s for 4096 tiles (87 KB) and about 40 minutes for 65025 tiles
(1.4 MB). Each is now refused in about 1.1 to 1.7 s.

Nothing else of the geometry needs a charge. Each precinct is already paid
for: the walk reads each precinct's packet in each layer, a step each,
whatever the reduction. The code-blocks of the kept resolutions are no more
than the reduced image's samples / 16, which the sample limit holds to
2^24, about a second to build. A decode at full resolution drops no
resolution, so it spends exactly what it did before; a file built to spend
exactly the bound (`the_layer_orders_charge_each_component_they_check` in
`tests/work_bound.rs`) still decodes.

A code-block costs about 30 to 70 ns to build, a few steps' worth, but one
step each keeps real files far inside the bound. At reductions 1 to 3 the
committed files spend at most 1.6% of it (a sparse 2048x2048 image in 4x4
code-blocks at reduction 3); the Sentinel-2 10 m band about 0.15%; a
2^28-sample single tile in 64x64 code-blocks about 0.4%; and of the
Sentinel-2, GDAL and OpenJPEG test files measured, none more than 1.9%
(`s2_MSK_QUALIT_B02.jp2` at reduction 3).

### Packets a POC sequence reaches twice

A POC marker's volumes may reach the same packet more than once; the
first reads it and the rest skip it (OpenJPEG's `include` array). The
decode keeps one bit per packet the volumes reach, and skips, by that
bitmap, a packet already read in about 4 ns, without a step: there are at
most 31 skips per packet read, and charging them would refuse the 1 MB
file in the first row of the table below. The bitmap is used when it has no
more bits than the steps left, since a walk that reads every packet takes a
step for each; that keeps it under 2 MB below the floor and 4 bytes per byte
of data above it. `read_header` holds the same bitmap, sized from the decode's
bound for the same tile data, so it holds a bitmap wherever the decode does. A
walk in layer orders that reaches more packets than its steps left cannot
finish and is `Limit` before it starts.

With more packets than the bitmap holds, a walk with a positional order can
pass over precincts, so the decode keeps the packets read in a set, at most
one per byte of packet headers and about 24 bytes each. The bitmap stays
capped at the default bound's size, so raising the bound raises the time a
file may take, not its memory: past that size the packets read go in the
set. A skip from the set is a lookup that misses the cache, 80 to 180 ns
against at most 15 ns for a step, so each one is charged 12 steps.

`read_header`'s walk keeps no set: past its bitmap, only where the decode would
keep one, it gives up. For a `Psot = 0` codestream in a JP2 file the codestream
then runs to the end of the file, like the other tiles too costly to walk
(`a_psot_zero_tile_too_costly_to_walk_reads_to_the_end` in
`tests/psot_zero.rs`): boxes after `jp2c` are not reported, and the decode
refuses their bytes as left after the last packet. `setskip(1024, 31)` (the
shape in the last row of the table below, with 1024 layers and 31 identical
volumes), 1.0e7 packets reached, ends at its box and decodes, as on OpenJPEG;
`setskip(2048, 2)`, 2.0e7, runs to the end
(`a_psot_zero_poc_sequence_ends_at_its_box_within_the_decodes_bitmap` in
`tests/work_bound.rs`). Measured on release builds (`pocdup` in
`scripts/gen-work-bound-fixtures.py`; times and peak memory; `read_header` is on
the `Psot = 0` JP2 form):

| Shape | Size | `decode` before #261 | `decode` since #261 | `read_header` before #261 | `read_header` since #261 | OpenJPEG 2.5.0 |
| ----- | ---- | -------------------- | ------------------- | ------------------------- | ------------------------ | -------------- |
| 16 precincts, 65535 layers, 31 identical volumes | 1 MB | 5.7 s, 105 MB | 0.15 s, 3.5 MB | 5.3 s, 105 MB | 0.15 s, 3.5 MB | 0.10 s, 9 MB |
| 160 components, 65535 layers, 2 volumes | 10.5 MB | 4.5 s, 824 MB | 0.32 s, 14 MB | 4.2 s, 824 MB | 0.33 s, 14 MB | 0.57 s, 37 MB |
| 160 precincts, 65535 layers, 32 volumes | 10.5 MB | 65 s, 824 MB | 1.5 s, 14 MB | 71 s, 824 MB | 1.5 s, 14 MB | refused, 32 POCs |
| 2^14 precincts, 65535 layers, 2 volumes, 10^7 packets in the data | 10 MB | `Codestream`, 1.7 s, 828 MB | `Limit`, 0.003 s, 16 MB | 1.8 s, 828 MB | 0.003 s, 16 MB | 53 s, 2.1 GB |
| The same in RPCL | 10 MB | `Codestream`, 1.9 s, 828 MB | `Codestream`, 1.2 s, 238 MB | 2.0 s, 828 MB | not walked, 0.004 s, 16 MB | 153 s, 2.1 GB |
| `setskip(65535, 31)`: RPCL, 6.5e8 packets reached, 1.4e7 read | 14 MB | decodes, 46.7 s, 240 MB | `Limit`, 5.5 s, 240 MB | 44 s | not walked, 0.003 s, 19 MB | |

### Valid files the bound refuses

The bound does refuse some valid files. Take one tile, no wavelet levels,
4x4 code-blocks in one precinct, LRCP, and many layers that each include
one new code-block at a scattered place (one pass, one byte), with the
blocks included earlier reading a "no new passes" bit. The new blocks keep
the inclusion tree from settling high up, so each layer steps over many
spans narrower than the precinct, one step per row each: about W·√n steps
a layer for a precinct W blocks wide after n layers. The steps per byte
grow with the layer count, so no constant per byte admits the whole class.
At 2048x2048 or more it passes the bound from about 600 to 800 layers:

| Image | Layers | Size | Refused after | OpenJPEG |
| ----- | ------ | ---- | ------------- | -------- |
| 2048x2048 | 800 | 527 KB | 1.1 s | 6.9 s |
| 2896x2896 | 600 | 343 KB | 1.2 s | 10.5 s |
| 2896x2896 | 800 | 585 KB | 1.5 s | 15.4 s |

OpenJPEG decodes all three. No encoder in the corpus writes this shape; the
files were built by hand for #235.

The charge on skips from a POC sequence's set refuses another class. Take
one 38608x1 tile in RPCL, no wavelet levels, 1x1 precincts, one
component at `XRsiz` 254 and 64 at 255. The first sets the sweep's step to
254, which meets only the first of each other component's 152 precincts,
so the volumes reach 9880 packets a layer but read 216. That is more than
the bitmap may hold, so the packets read go in a set, and every volume
after the first skips each of them at 12 steps. With 2048 layers (442 KB)
one volume decodes and 31 are `Limit` after 0.05 s; OpenJPEG decodes both
in 0.14 s or less. `setskip` in `scripts/gen-work-bound-fixtures.py`
writes the shape, and `skips_from_the_poc_set_are_bounded` in
`tests/work_bound.rs` builds it. It was found in #261.

### The `read_header` walk

The bound covers the walk `read_header` makes for a `Psot = 0` tile-part
([`Psot = 0` at the end of
`jp2c`](openjpeg-departures.md#psot--0-at-the-end-of-jp2c)). That walk counts
the same steps under its own bound, max(2^22, 8 × the tile's data bytes), no
larger at any size, so for a single-tile codestream a tile that walk finishes
is one the decode's walk finishes too. A JP2 decode makes that walk first, and
then walks the tile again under a fresh bound, so it can spend about 1.25 times
the decode's bound (`jp2_t2_sweep_cprl_tall` in
`tests/fixtures/psot0/README.md`). The decode's bound does not replace that
walk's geometry bound of 2^14 code-blocks: `read_header` allocates for its
walk, and a geometry bound at the decode's 2^19 code-blocks would let
`read_header` hold about 75 MB for a 66 KB file.

## The tier-1 work bound

Each coding pass of a code-block scans all of the block's samples, however few
bytes the pass carries, and a packet header can ask for a block's 88 passes
(3 × 30 − 2, for 30 bit-planes) in about 4 bytes. The decode counts the work of
each tile as the passes the block decoder runs times each block's samples,
before it decodes the tile's code-blocks. Each block counts the passes its
packets give it, at most `3 × top − 2` for its `top` coded bit-planes, and the
blocks a resolution reduction drops are not counted. The decode may spend
max(2^26, 2^15 × the tiles' data bytes) such sample-passes, one bound for every
tile; past it the decode is `Limit` (#250, 2026-11-12).
`DecodeOptions::code_block_work_limit` raises or lifts it.

A sample-pass costs about 3.3 ns where the pass finds little to code, but
7 to 8 ns on the data slowest to decode (every byte 0x4e), so the floor is
about 0.5 s and the rate about 0.25 ms a byte. A crafted 1024x1024 file of 64
passes per block that spends the floor decodes in 0.49 s, and a 4096x4096 file
of 27 passes per block that fits just under the bound (14 KB) in 3.9 s,
where `opj_decompress` takes 0.83 s. Measured on
release builds (`scripts/gen-tier1-work-bound-fixtures.py`; `t1bomb` is one
tile, no wavelet levels, every 64x64 code-block included with 88 passes
and one byte):

| Shape | Size | Sample-passes | Before #278 | Since #278 | OpenJPEG 2.5.0 |
| ----- | ---- | ------------- | ----------- | ---------- | -------------- |
| `t1bomb`, 8192x8192 | 76 KB | 5.9e9 | decodes, 19.3 s | `Limit`, 6 ms | decodes, 9.0 s |
| `t1bomb`, 1024x1024 (committed) | 1.3 KB | 9.2e7 | decodes, 0.29 s | `Limit`, under 1 ms | decodes, 0.14 s |

With the bound lifted, the 8192x8192 file decodes in the same 19 s
(`the_issue_bomb_decodes_with_the_limit_lifted` in `tests/work_limit.rs`,
ignored by default).

The bound is about 2.35 times the most encoder output spends. Over the
committed files, the conformance corpus and the real-world files, most spend
under 120 sample-passes per byte;
`tests/fixtures/budget/flat_1024_lossless.j2k` spends 7100 in 202 bytes, under
the floor. Encoder output that spends the most per byte has many code-blocks
that each run many passes for a few bytes. `opj_compress` 2.5.0 writes the most
with no wavelet level (`-n 1`) on a flat image, where every block does: a flat
16-bit 8192x8192 image (221 KB) spends 13933 per byte, 42.5% of the bound, and
RLCP, `-M 8` or 2048x2048 tiles change little; flat 8-bit spends 7230, 22%.
With one level (`-n 2`), a flat 16-bit image with 256 specks spends 12396, 38%.
At the defaults, a flat 16-bit image with 4 to 64 sparse specks spends about
5600, 17% (8192x8192 with 64 specks is 20 KB). Smooth fields, noise and
photographs (lossless and 9/7 at ratios from 20 to 10000, 16x16 to 64x64
code-blocks) spend far less. The committed `opj_flat2048_16bit_n1`
(2048x2048, 16-bit, `-n 1`, 14 KB) spends 13933 per byte, 42.5%, and
`opj_specks2048_16bit` (2048x2048, 16-bit, 128 specks, 28 KB) 3600, 11%.
`every_committed_file_decodes_far_below_the_tier1_work_bound` (in
`src/tier1/work_bound_tests.rs`) prints the table for the committed files and
asserts each is under half the bound, which the worst encoder output passes
with room.

A crafted file can ask for about 100000 sample-passes a byte (88 passes over
4096 samples for about 4 bytes of packet header and data); the bound allows
32768, about a third of that, and refuses the file before doing the work. So
the gap between what encoders write and what a file can ask for is only about 7
times. A file at the bound still takes about 0.25 ms a byte, against the 0.2 to
0.7 µs a byte real data takes; a valid file a little past what was measured,
such as a flat image coded with no wavelet level at a little more than twice
the passes per byte, would be refused, and `code_block_work_limit` is the way
to decode it. The rate stays 2^15 rather than 2^14, which would refuse the
`-n 1` files, and it is scaled by the tile bytes, packet headers included, as
the tier-2 bound is (#278, 2026-11-12).

## Bounds removed by a cheaper parse

### Tile header cost

A tile's header is the main header with the tile's own COD, COC, QCD, QCC,
RGN and POC markers laid over it. Resolving one is sized by the component
count, up to 16384, and a codestream can hold 65535 tiles. Resolving every
tile's header up front cost tiles × components, before any check ran (#256).

The parse checks each tile's markers against the main header without
resolving a header, in time proportional to the tile's own markers. A
component the tile names in a COC or QCC is checked on its own. A tile COD
or QCD lands on every other component, so it is checked once, against the
most demanding of them: the fewest QCD steps under a tile COD, the most
levels under a tile QCD, and a count of each wavelet. Only a tile found at
fault is resolved in full, so the error reads as it always did.
`the_quick_tile_check_agrees_with_the_full_resolution` (in
`src/codestream/tests.rs`) checks that the two
agree on 20000 random main and tile headers, with up to 32 levels, 97 step
entries and 300 components, and
`the_quick_tile_check_agrees_with_the_full_resolution_from_the_bytes` on
5000 written as codestreams of two tile-parts, a POC in either.

At decode, a tile with no markers runs under the main header, borrowed. A
tile with markers is resolved in full, sized by the component count, and
the decode then spends at least that much on its components: every
tile-component must hold a sample, so tiles × components is at most the
sample limit for a file that decodes. TLM entries are checked and dropped,
since nothing reads them. Measured on release builds
(`scripts/gen-tile-header-fixtures.py --big`):

| Shape | Size | Before #256 | Since #256 | OpenJPEG 2.5.0 |
| ----- | ---- | ----------- | ---------- | -------------- |
| 4096 tiles of 16384 components | 110 KB | 2.2 s | 3 ms | fails to read the header, 10 ms |
| 128x128 tiles of 16384, heavy header | 295 KB | 63.9 s | 9 ms | 22 s, 55 GB |
| 65025 tiles of 16384 | 1.0 MB | 45.8 s | 17 ms | 25 s, 55 GB |
| The same, a COD and a QCD in every tile | 2.3 MB | 78.8 s | 26 ms | |
| 65025 tiles, 1 MB of TLM entries, decodes | 2.0 MB | 13.2 s | 56 ms | 0.34 s |
| 65025 tiles, 4 MB of TLM entries, decodes | 5.2 MB | 85.5 s | 63 ms | 0.35 s |

The first four are refused at tile 0, where every component but the first holds
no sample. The OpenJPEG figures for the 16384-component shapes are from #256.
