# Resource bounds

This decoder refuses a file whose header asks for far more memory or work
than its size justifies. OpenJPEG allocates and works as much as the header
asks for, so a file of under a kilobyte can make it run for many seconds
(an 819-byte file takes 20.6 s), and files of 295 KB to 10 MB can make it
allocate gigabytes. This decoder caps three such costs: the samples it
allocates, the packet-header walk (tier 2) and the code-block decoding
(tier 1). A file past a cap is refused with `Error::Limit` before the work
is done. The error's `LimitDetail` says which cap it was (`kind()`, a
`LimitKind`) and gives a `WorkLimit::Raised(n)` to retry with
(`retry_with()`), and its message names both. For the samples it is the
smallest factor that admits the image. For the two work bounds it is the
larger of the smallest factor that admits what the decode has seen and twice
the factor the bound ran under, and the file may need more, so a loop of
`with_retry` calls takes about log2 of the factor the file needs. It is `None`
when the refusal shows no factor. A caller can retry from `kind()` and
`retry_with()` without reading the message, and `DecodeOptions::with_retry`
sets the option. A retry can still fail with another error once past the cap.

| Bound | Default | Option in `DecodeOptions` | Margin over encoder output | Tests |
| ----- | ------- | ------------------------- | -------------------------- | ----- |
| [Samples](#the-sample-limit) | max(2^29, 8 × the input bytes) samples at the decode's reduction | `sample_limit` | about 1.5 times (a Sentinel-2 true-color image) | `tests/sample_limit.rs`, `tests/sample_limit_alloc.rs` |
| [Tier-2 steps](#the-tier-2-work-bound) | max(2^24, 32 × the tiles' data bytes) | `packet_work_limit` | about 7.8 times (largest share 12.8%) | `tests/work_bound.rs`, `src/tier2/work_bound_tests.rs` |
| [Tier-1 sample-passes](#the-tier-1-work-bound) | max(2^26, 2^15 × the tiles' data bytes) | `code_block_work_limit` | about 2.35 times | `tests/work_limit.rs`, `src/tier1/work_bound_tests.rs` |

The encoder figures come from the committed fixtures, the Part 4 corpus, and
the Sentinel-2, GDAL and OpenJPEG test files named in each section. Each
section also lists the valid files its bound refuses. A fourth cost, resolving
tile headers, was removed by making the parse cheaper rather than by adding a
cap ([Tile header cost](#tile-header-cost)).

The per-tile code-block cap is 2^20. The struct sizes (`BlockState` 40,
`CodeBlock` 80, `SegmentState` 56, two tag-tree nodes of 8 each at 4/3 per
leaf) give about 141 bytes a block, which is only a lower bound. Measured on a
4096x4096 tile in 4x4 blocks (2^20 of them), hostile metadata takes about
129 MiB a tile with empty blocks, and about 480 MiB of heap (about 550 MiB
resident) with every block coded. The cap was 2^19 until the sample limit
became 2^29: at 2^19 a one-tile image of 23106x23106 samples or more in 32x32
blocks was refused. At 2^20 a one-tile image at least 72 samples wide and under
2^29 samples fits in 32x32 blocks (23170x23170 needs 527,259; 64x64 blocks need
132726 at the limit; 64 x 2^23 needs 770,048). A thinner image is still
refused (66 x 8,134,407 needs 1,128,027; 32 x 2^24 needs 1.54 M; 1 x 2^29
needs 16.7 M), as is one tile in 16x16 blocks or smaller near the limit.
The 2^20 code-block cap does not grow with the input the way the sample
limit does. A one-tile image past 2^30 samples in 32x32 blocks, or past
2^32 in 64x64 blocks, is still refused by the cap, however large the file.
A tiled image is held to it one tile at a time.

A few fixed guards also return `Limit`, and no option lifts them: 2^18
precincts and 2^20 code-blocks per tile, 32 POC progression volumes, 30
coded bit-planes per code-block, and an image too large for the target to
address, under any sample limit. Their `retry_with()` is `None`, and
`LimitKind::is_adjustable()` is false.

The caps count work, not time: samples, packet-walk steps and code-block
sample-passes, each defined below. The times on this page were measured on
release builds when each cap was set, before the 0.4.0 speed-ups to tier 1, the
wavelet, tier 2 and dequantization (#314, #315, #319 to #322 and #324), so the
decoder is now faster than some of them show. Where a table has "Before #N"
and "After #N" columns, they give this decoder's times on either side of that
change, measured then.

## The sample limit

The decode holds every component in its own plane, so what it allocates
grows with the sum of the component areas, not with the file. A file of a
few hundred bytes can declare a 65536x65536 image, 16 GiB of `i32`
samples. The decode counts the samples of every output channel, at the
resolution reduction asked for, from SIZ and the JP2 boxes alone, and
refuses an image of more samples than the limit with `Limit` before it
reads a tile. The limit is 2^29 samples (2 GiB as `i32`), or 8 samples per
byte of input when that is more (see
[The limit grows with the input](#the-limit-grows-with-the-input)).
The image area counted is SIZ's `(Xsiz − XOsiz)·(Ysiz − YOsiz)`, wherever
on the reference grid the origin sits. The count covers the codestream's
components, a palette's expanded
channels, and the channels sRGB output adds (sYCC chroma replicated to the
luma grid, a gray channel a monochrome profile maps to R, G and B).
`read_header_with` makes the same check, so it refuses what the decode would.

Under a window or a tile (`DecodeOptions::region`), the count is the
samples of the tiles the decode reads, in every component, at the reduction.
Those tiles cover the returned area, so the output planes fit inside the
count. Each tile read is reconstructed whole, one tile at a time, in buffers
sized by the tile, so the count bounds those too. And the packet walk builds
every component's code-block geometry in each tile read, which the count
bounds as the next sections rely on. A palette's expanded channels and the
channels sRGB output adds are counted over the returned area only. So the
sample buffers peak within the same multiple of the limit as for a whole
decode. The parse of the tile-parts is not counted: it keeps about 440 bytes
for each tile-part in the file, read or not, which is about 29 MB at 65535
tiles and at most about 21 times the file's size, and a file whose whole
image is over the limit now gets that far under a window. A
256x256 window of a 32769x16385 image in 4096x4096 tiles fits the default,
while a window of one 23171x23171 tile counts the whole tile. A
`DecodeOptions::components` subset does not lower the count, as the walk
still builds the geometry of the components it leaves out; under a window it
counts every component of the tiles read.

OpenJPEG has no such limit: it allocates what the header declares and fails
only when the allocation does.

`DecodeOptions::sample_limit` raises the limit (`WorkLimit::Raised(n)`,
n times the default for the same input) or lifts it
(`WorkLimit::Unlimited`). The refusal names the
option and the smallest `Raised` factor that admits the file. Even
`Unlimited` refuses an image whose 4-byte samples would pass `isize::MAX`
bytes, so the buffer sizes cannot overflow `usize`, on 32-bit targets
included. Below that, under `Unlimited`, an allocation the system cannot
satisfy aborts the process.

The default admits a Sentinel-2 true-color image (10980x10980 in three
components, 361.7 M samples) and RGB images up to about 179 MP, and a reduced decode of a larger image
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
| `T31UDQ_20220811T105631_TCI.jp2`, 10980x10980x3, 125 MB, 256x256 tiles | full, bit-exact | 26.8 s | 1.47 GiB | 22.9 s | 1.38 GiB |
| a lossless 23170x23170 one-tile 5/3 image of 5 levels, 286 MiB (536.8 M samples) | full | 83.5 s | 2.90 GiB | 53.0 s | 2.34 GiB |

The TCI is the one the default refused at 2^28 (#436). It is the 10 m true-color
image of Sentinel-2 tile 31UDQ, sensed 2022-08-11 (public bucket
`gcp-public-data-sentinel-2`); all three planes match `opj_decompress`
sample for sample. The TCI times are wall time from `/usr/bin/time` of one
run each, on a machine at load 14 to 20, so they are loose; OpenJPEG ran
under `MemoryMax=16G`. The last row is the largest square one-tile image
the 2^29 floor admits, made by `opj_compress -n 5` from a pseudo-random
gradient; it is where the 1.35 times below stops being
an extrapolation.

The peak is about 1.0 times the 4-byte output for the Sentinel-2 band (460
MiB of samples), and about 1.1 times for the TCI (1.35 GiB of samples, 119 MiB
of input in memory). It is about 1.35 times for one square tile of one
component (see the table in `src/samples.rs`), so at the 2^29 floor a square
tile peaks at about 2.7 GiB, plus the input: 2.9 GiB measured in the last row
above. A tall, narrow tile peaks
higher: about 1.5 to 1.7 times when it is 8 to 1024 samples wide, and about
2.6 times when it is one sample wide (at 2^24 samples, 3 levels). The
multiple grows with the wavelet levels, as the code-blocks' fixed overhead
is a larger share of it: 2.7 at 5 levels and 3.2 at 8.
While a palette expands the peak is about twice the output. Tests:
`tests/sample_limit.rs` and `tests/sample_limit_alloc.rs`.

### The limit grows with the input

The default limit for an input of `B` bytes is

    max(2^29, 8 × B) samples

so it is 2^29 up to 64 MiB of input, and grows by 8 samples per byte past
that, just as the two work bounds grow with the tiles' data. `B` is every byte
the decode is given: a JP2 file's boxes count, and so do bytes after the
codestream. `read_header_with` counts the same bytes, so it still refuses
what the decode would. Each refine of an `IncrementalDecoder` is held to the
limit of the bytes fed so far, as a one-shot decode of them would be, so a
refusal there is a wait, not an error (see
[incremental-decoding.md](incremental-decoding.md)).

A small file is held to the floor, 2^29 samples: a file of 100 bytes cannot
ask for more. A large file may ask for at most 32 bytes of `i32` output per
byte of input. The large valid files measured hold far fewer samples per
byte: the Sentinel-2 TCI about 2.9, the lossless 23170x23170 one-tile file
about 1.8, and the one below about 2.3, so 8 is about 2.7 times the most
seen. A file over the floor that holds more than 8 samples per byte, such as
a large image of flat color, still needs a raised limit.

`WorkLimit::Raised(n)` is n times the default for the same input: n times
2^29 up to 64 MiB, and 8·n samples per byte past it. A factor never allows
less than `Default`, and a larger factor never allows less than a smaller
one. The factor a refusal names is still the smallest that admits the
count, as it divides by the default of the same input. No limit, not even
`WorkLimit::Unlimited` and whatever the input size, goes past the samples
the target can address. On a 32-bit target that is 2^29 − 1, one below the
floor.

The table gives the worst-case peak for a file of each size, from the multiples
above: about 1.35 times the output for a square tile, and up to about 3.2 times
for a tile one sample wide, plus the input. The 100 MB figures quoted in the
other docs come from this table and include the input.

| Input | Default limit | Output (`i32`) | Peak, square tile | Peak, one sample wide |
| ----- | ------------- | -------------- | ----------------- | --------------------- |
| 100 bytes | 2^29 | 2 GiB | about 2.7 GiB | about 6.4 GiB |
| 1 MB | 2^29 | 2 GiB | about 2.7 GiB | about 6.4 GiB |
| 100 MB | 8 × 10^8 | 3.0 GiB | about 4.1 GiB | about 9.6 GiB |
| 1 GB | 8 × 10^9 | 30 GiB | about 41 GiB | about 96 GiB |

Past 64 MiB of input that is about 43 bytes of peak memory per byte of input
for a square tile, and up to about 102 for a tile one sample wide. The
figures past the floor are scaled from the measured multiples, not measured.
A tiled image peaks lower, as each tile is reconstructed in turn, and a
one-tile image in 64x64 code-blocks is also held to the 2^20 code-blocks a
tile allows, about 2^32 samples. An `IncrementalDecoder` can hold up to about
three times one decode's peak, plus the bytes fed.

Measured on a release build, a lossless 23171x23171 one-tile 5/3 image of 5
levels (536.9 M samples, just over 2^29; 234 MB, from `opj_compress -n 5`
on a gradient with noise), against `opj_decompress` 2.5.0:

| Decoder | Result | Time | Peak RSS |
| ------- | ------ | ---- | -------- |
| this one, before #437 (2^29 for any input) | `Limit`, `Raised(2)` | 0.1 s | 225 MiB (the input) |
| this one, after #437 | decodes, bit-exact | 37.5 s | 2.84 GiB |
| `opj_decompress` | decodes | 30.9 s | 2.28 GiB |

Padding raises the limit too, even in a conformant file decoded with `strict`
on: `COM` segments, a JP2 `xml ` box and bytes after `EOC` all count. The bytes
counted are the bytes in memory, not on the wire, and padding compresses well:
67 MB of zeros gzips to 65 KB. So a service should bound the input size it
accepts, measured after any decompression. Past 64 MiB the worst-case peak is
about 102 times that size, and up to about three times that for an
`IncrementalDecoder`. A one-tile 1 x 2^30 image needs 128 MiB of input to fit
the default. Measured: with 128 MiB of zeros after `EOC` (134,283,241 bytes) it
decodes at the defaults (`strict` on) in 23.5 s with a peak RSS of 8.30 GiB, and
at 134,065,513 bytes it is refused, naming `Raised(2)`. Time grows with the
padding too. The work bounds count only tile data, but the inverse DWT and the
output scale with the sample limit, which padding raises: about 175 ns per byte
of padding, measured.

No `WorkLimit` sets a limit below `Default`. For a ceiling that does not grow
with the input, call `read_header_with` with no region and no component subset,
and refuse the file when the summed area of its components is too large. Under a
region or a subset the header describes only what is returned, while the limit
counts every tile read, whole and in every component (a 16x16 window of one
23171x23171 tile counts 536.9 M samples). So compute that count from
`image_area` and `tiles` instead: the whole tiles the region touches, in every
component.

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

| Shape | Size | Before #235 | After #235 | OpenJPEG 2.5.0 |
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
about what 10 MB of real data takes to decode. A step costs 1 to 55 ns: a
positional walk's points over 16384 components and 200 tiles, each with a
packet to read, take 24 to 28 ns a step over 32 POC volumes and 33 to 55 ns
with none. The conformance codestreams over 100 KB decode at 0.19 (`p1_05`) to
0.73 (`p1_04`) µs a byte, the best of five release-build decodes of each. The
bound comes to about 0.02 to 1.8 µs a byte, so at most a refusal takes about
2.5 times the slowest conformance decode per byte. Over all 23 the
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
component there. Since #272 they list no subbands at all: Tier-1 and the
inverse DWT pass them through from the tile-component's rectangle, with no
buffer. Measured on release builds, decode only (`deep` in
`scripts/gen-work-bound-fixtures.py`, a row of one-sample tiles at y = 1, 32
levels, one layer, every packet 0x00; OpenJPEG's time includes writing its
output):

| Shape | Size | Before #271 | After #271 | After #272 | OpenJPEG 2.5.0 |
| ----- | ---- | ----------- | ---------- | ---------- | -------------- |
| 1 tile, 16384 components | 66 KB | 0.5 s, 383 MB | 0.19 s | 0.11 s | 0.45 s, 192 MB |
| 64 tiles, 1024 components | 70 KB | 1.7 s | 0.51 s | 0.30 s | 0.29 s |
| 64 tiles, 16384 components | 1.1 MB | `Limit`, 21 s | 11.3 s, 226 MB | 6.9 s | 5.6 s, 1.3 GB |

The "After #271" and "After #272" columns were measured together; the
"Before #271" column is from an earlier session on other load.

The last is about 6 µs a byte, 1.2 times OpenJPEG, and the slowest shape
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
whatever the reduction. Each code-block of the kept resolutions holds at
least one sample, so there are no more of them than the reduced image's
samples (under a window or a tile, the reduced samples of the tiles read),
which the sample limit holds. With 4x4 code-blocks and the default precincts
that is samples / 16, 2^25 at the 2^29 floor, about 1 to 2.3 seconds to
build at 30 to 70 ns each (arithmetic from the per-block cost, not measured
at that count). Past 64 MiB of input the limit is 8 samples per byte, so
there is at most one code-block per two bytes of input, or 15 to 35 ns of
build time per byte. Small
precincts can force code-blocks down to 1x1, and then it is the per-tile cap
of 2^20 code-blocks that bounds each tile. A decode at full resolution drops no
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
pass over precincts, so the decode instead keeps a count per precinct of
the layers emitted. Each volume emits a precinct's layers from 0 up, so a
packet was emitted before exactly when its layer is below that count. The
counts take 4 bytes per precinct of the tile, at most 1 MB under the
geometry's cap of 2^18 precincts, whatever the packet count. The bitmap
stays capped at the default bound's size and the counts do not grow as the
walk goes, so raising the bound raises the time a file may take, not its
memory. That holds with `strict` off too, where the walk counts every
packet past the end of the data, a step each, without reading it: those
packets take no memory of their own.

Each skip by the counts is charged 12 steps. The counts replaced a set of
the packets read (#438), where a skip was a lookup that missed the cache,
80 to 180 ns, so 12 steps priced a step at 15 ns, toward the cheap end of
what a step takes. The charge is kept, so the same files are refused, and
it errs higher still. The set held an entry for each packet past the end
too, so with `strict` off its memory grew with the bound. Measured on
release builds, each with the same result before and after:

| File | Options | With the set | With the counts |
| ---- | ------- | ------------ | --------------- |
| A 200-byte fuzz input | `strict` off, `Raised(8)` | `Limit`, 26.7 s, 3.6 GB | `Limit`, 2.8 s, 33 MB |
| `pocspent_2tiles_60000_559.j2k` | `strict` off, `Raised(3)` | `Limit`, 6.4 s, 887 MB | `Limit`, 0.74 s, 2.9 MB |
| The same | strict, `Raised(3)` | `Codestream`, 0.00 s, 2.9 MB | `Codestream`, 0.01 s, 2.8 MB |
| 2^14 precincts in RPCL, 10^7 packets in the data | `strict` off | `Limit`, 78 s, 7.1 GB | `Limit`, 5.4 s, 16 MB |
| The same | strict | `Codestream`, 3.4 s, 237 MB | `Codestream`, 0.55 s, 16 MB |
| `setskip(65535, 31)` | strict | `Limit`, 20.8 s, 240 MB | `Limit`, 0.72 s, 19 MB |

With `strict` off a decode now holds what a strict one does
(`packets_past_the_end_take_no_memory_under_a_raised_bound` in
`tests/strict_off_poc_alloc.rs`). The timings ran beside other work, so
the "With the set" column is slower than the table below for the same
shapes; that table's memory for the shapes past the bitmap is from before
the counts.

`read_header`'s walk keeps no counts: past its bitmap, only where the decode
would keep them, it gives up. For a `Psot = 0` codestream in a JP2 file the codestream
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

| Shape | Size | `decode` before #261 | `decode` after #261 | `read_header` before #261 | `read_header` after #261 | OpenJPEG 2.5.0 |
| ----- | ---- | -------------------- | ------------------- | ------------------------- | ------------------------ | -------------- |
| 16 precincts, 65535 layers, 31 identical volumes | 1 MB | 5.7 s, 105 MB | 0.15 s, 3.5 MB | 5.3 s, 105 MB | 0.15 s, 3.5 MB | 0.10 s, 9 MB |
| 160 components, 65535 layers, 2 volumes | 10.5 MB | 4.5 s, 824 MB | 0.32 s, 14 MB | 4.2 s, 824 MB | 0.33 s, 14 MB | 0.57 s, 37 MB |
| 160 precincts, 65535 layers, 32 volumes | 10.5 MB | 65 s, 824 MB | 1.5 s, 14 MB | 71 s, 824 MB | 1.5 s, 14 MB | refused, 32 POCs |
| 2^14 precincts, 65535 layers, 2 volumes, 10^7 packets in the data | 10 MB | `Codestream`, 1.7 s, 828 MB | `Limit`, 0.003 s, 16 MB | 1.8 s, 828 MB | 0.003 s, 16 MB | 53 s, 2.1 GB |
| The same in RPCL | 10 MB | `Codestream`, 1.9 s, 828 MB | `Codestream`, 1.2 s, 238 MB (before #438) | 2.0 s, 828 MB | not walked, 0.004 s, 16 MB | 153 s, 2.1 GB |
| `setskip(65535, 31)`: RPCL, 6.5e8 packets reached, 1.4e7 read | 14 MB | decodes, 46.7 s, 240 MB | `Limit`, 5.5 s, 240 MB (before #438) | 44 s | not walked, 0.003 s, 19 MB | |

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

The charge on skips past a POC sequence's bitmap refuses another class. Take
one 38608x1 tile in RPCL, no wavelet levels, 1x1 precincts, one
component at `XRsiz` 254 and 64 at 255. The first sets the sweep's step to
254, which meets only the first of each other component's 152 precincts,
so the volumes reach 9880 packets a layer but read 216. That is more than
the bitmap may hold, so the packets read are held as counts, and every volume
after the first skips each of them at 12 steps. With 2048 layers (442 KB)
one volume decodes and 31 are `Limit` after 0.05 s; OpenJPEG decodes both
in 0.14 s or less. `setskip` in `scripts/gen-work-bound-fixtures.py`
writes the shape, and `skips_from_the_poc_counts_are_bounded` in
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
walk, and a geometry bound at the decode's 2^20 code-blocks would let
`read_header` hold about 150 MB for a 66 KB file.

The walk to the box end also changes what a decode returns, not only how long
it takes. When it gives up at the default bound, the tile-part runs to the end
of the file, and the boxes after `jp2c` count as bytes left after the last
packet. A raised `packet_work_limit` lets the walk finish and end at the box.
So raising the limit can change the outcome of a decode without any
`Error::Limit` having asked for it. At the default,
`tests/fixtures/psot0/jp2_t2_sweep_cprl_poc.jp2` and
`tests/fixtures/work_bound/jp2_t2skip_512_20000.jp2` give
`Codestream: tile 0 has 6 byte(s) left after the last packet`, and with `strict` off, a report with
a `LeftoverBytes` entry. Under `WorkLimit::Raised(2)` both decode completely,
with an empty report. OpenJPEG reads every such tile-part to the end of the
file and ignores the bytes left after the last packet, whatever its walk costs
([`Psot = 0` at the end of
`jp2c`](openjpeg-departures.md#psot--0-at-the-end-of-jp2c)), and decodes the
first file. It fails on the second for another reason ("segment too long" in a
code-block).

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

| Shape | Size | Sample-passes | Before #278 | After #278 | OpenJPEG 2.5.0 |
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

### HT code-blocks

The bound counts HT code-blocks too: `tier1::work` asks the block's coder for
its passes (`ht::passes_run`), so an HT block costs the passes its decode
runs over its samples. That is at most the three of one HT set (cleanup,
SigProp and MagRef), however many passes its packets name, and one pass when
its refinement segment is empty or has no bit-plane left to refine. The unit
test `the_work_counts_the_passes_an_ht_block_runs` pins each case. In a
MIXED tile-component each block costs what its own coder runs, so a Part 1
block there costs its Part 1 passes
(`the_work_counts_each_blocks_own_coder_in_a_mixed_tile_component`).

Because an HT block runs at most 3 passes, the bound cannot refuse an HT file at
the default. A block's work is at most 3 × 4096 = 12288 sample-passes. A block
of no bytes in its first contribution holds only placeholder passes and does no
work, so a counted pass needs a cleanup segment of at least 2 bytes. With the
block's packet header (7 bits for one pass, 11 for two, 14 for three) that is
about 1400 sample-passes a byte for one pass, 1900 for two and 2600 for three.
So the most an HT file asks for a byte is about 2600, against the bound's 32768,
12.6 times more. Only the sample limit bounds an HT decode, and the work under
it is at most 3 sample-passes per sample: an image of 2^29 samples asks for
1.6e9 sample-passes. The worst case is a one-column image, which makes 1x1024
blocks, at the slowest rates below (10 to 13.7 ns): 16 to 22 s (extrapolated,
not measured). Past 64 MiB of input the sample limit grows by 8 samples per
byte, which is at most 24 sample-passes per byte, or 0.25 to 0.35 µs per byte at
those rates. The bound still counts HT blocks, so a change to what a block may
cost is checked by the same arithmetic as Part 1, and `WorkLimit::Raised` and
`Unlimited` mean the same for both.

The rate of an HT sample-pass was measured against the one the bound uses
for Part 1 (3.3 ns where a pass finds little to code, 7 to 8 ns on the
slowest data). `htbomb` in `tests/work_limit.rs` builds the HT shape of the
`t1bomb` above: no wavelet level, every 64x64 code-block included with all
three passes, the 2-byte cleanup segment `0xF2 0x00` (the shortest that
decodes) and a 1-byte refinement segment. Release build, a shared and loaded
machine:

| Shape | Size | Sample-passes | Share of the bound | This decoder | OpenJPEG 2.5.0 |
| ----- | ---- | ------------- | ------------------ | ------------ | -------------- |
| `htbomb`, 1024x1024 (committed) | 1.3 KB | 3.1e6 | 4.7% (the floor) | 21 ms | 4 ms |
| `htbomb`, 8192x8192 | 79 KB | 2.0e8 | 7.7% | 1.40 s | 0.29 s |

That is 6.8 to 7.1 ns a sample-pass, as slow as Part 1's slowest data. Random
HT segments that decode (20000 per shape, 3 passes, minimum of 5 runs of each
block) cost a mean of 6.4 ns a sample-pass over 64x64 blocks and 5.8 to 6.0 ns
over 1024x4 and 4x1024, and at worst 10.3, 10.0 and 9.4. A 1x1024 block costs
a mean of 7.7 and at worst 13.7, its per-block set-up being a larger share.
Per byte, the worst HT file spends 2600 × 13.7 ns, about 0.036 ms a byte,
against the 0.25 ms a byte the bound allows for Part 1. This decoder takes
about 5 times as long as OpenJPEG on these files.

`the_ht_bomb_spends_a_small_share_of_the_tier1_bound` (in
`src/tier1/work_bound_tests.rs`) asserts the committed file's work and share,
`an_ht_bomb_decodes_at_the_default_bound` (in `tests/work_limit.rs`) that it
decodes at the default as OpenJPEG decodes it, and
`the_large_ht_bomb_decodes_at_the_default_bound` (ignored) the 8192x8192 row.
No file is refused by the bound for its HT blocks, so there is no public-API
test of such a refusal; `every_committed_file_decodes_far_below_the_tier1_work_bound`
measures every HT corpus file against it with the others.

## Incremental decoding

`IncrementalDecoder` holds a refine to the packet bound of a decode of its
bytes on its own, not cut by what earlier refines spent. The code-block bound
is shared: a refine gets what earlier refines left of it, and a refine that
runs out waits for more bytes. Walks that read a tile from the start again
(tiles not kept, and walks started again or built once there is room) are
held, over all the refines, to four times that packet bound, five at most
counting the walk that crosses it, and the bound follows `packet_work_limit`,
with none under `WorkLimit::Unlimited`. `finish` is one more decode under its
own bounds. See
[incremental-decoding.md](incremental-decoding.md#work-and-memory).

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

| Shape | Size | Before #256 | After #256 | OpenJPEG 2.5.0 |
| ----- | ---- | ----------- | ---------- | -------------- |
| 4096 tiles of 16384 components | 110 KB | 2.2 s | 3 ms | fails to read the header, 10 ms |
| 128x128 tiles of 16384, heavy header | 295 KB | 63.9 s | 9 ms | 22 s, 55 GB |
| 65025 tiles of 16384 | 1.0 MB | 45.8 s | 17 ms | 25 s, 55 GB |
| The same, a COD and a QCD in every tile | 2.3 MB | 78.8 s | 26 ms | |
| 65025 tiles, 1 MB of TLM entries, decodes | 2.0 MB | 13.2 s | 56 ms | 0.34 s |
| 65025 tiles, 4 MB of TLM entries, decodes | 5.2 MB | 85.5 s | 63 ms | 0.35 s |

The first four are refused at tile 0, where every component but the first holds
no sample. The OpenJPEG figures for the 16384-component shapes are from #256.
