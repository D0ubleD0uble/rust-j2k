# Resource bounds

This decoder refuses a file whose header asks for far more memory or work
than its size justifies. OpenJPEG allocates and works as much as the header
asks for, so a file of under a kilobyte can make it run for many seconds
(an 819-byte file takes 20.6 s), and files of 295 KB to 10 MB can make it
allocate gigabytes. This decoder caps three such costs: the samples it
allocates, the packet-header walk (tier 2) and the code-block decoding
(tier 1). A file past a cap is refused with `Error::Limit` before the work
is done. The error's `LimitDetail` says which cap it was (`kind()`, a
`LimitKind`) and gives a `Raise::Raised(n)` to retry with
(`retry_with()`), and its message names both. A `Raise` converts into the
option's type: a `SampleLimit` for the sample limit and the tile caps it
sets, a `WorkLimit` for a work bound. A code-block work refusal, and
a refusal of a tile over the cap on code-blocks or precincts, also gives the
tile refused and the amount it needs (`refused_at()`: sample-passes,
code-blocks or precincts by `kind()`). Under a sample
limit of `SampleLimit::AtMost` it gives no factor to retry with (see
[A fixed ceiling](#a-fixed-ceiling)).
For the samples it is the smallest factor that admits the image. For the two
work bounds, and for the code-blocks and precincts a tile holds, it is the
larger of the smallest factor that admits what the decode has seen and twice
the factor the bound ran under, and the file may need more, so a loop of
`raised_for` calls takes about log2 of the factor the file needs. It is
`None` when the refusal shows no factor. A caller can retry from `kind()`
and `retry_with()` without reading the message, and `DecodeOptions::raised_for`
sets the option. A retry can still fail with another error once past the cap.

| Bound | Default | Option in `DecodeOptions` | Margin over encoder output | Tests |
| ----- | ------- | ------------------------- | -------------------------- | ----- |
| [Samples](#the-sample-limit) | max(2^29, 8 × the input bytes) samples at the decode's reduction | `sample_limit` | about 1.5 times (a Sentinel-2 true-color image) | `tests/sample_limit.rs`, `tests/sample_limit_alloc.rs` |
| [Code-blocks and precincts per tile](#code-blocks-and-precincts-per-tile) | max(2^20, samples allowed / 512) code-blocks and max(2^18, samples allowed / 2048) precincts | `sample_limit` | about 2 times at the floor (a 23170x23170 one-tile image in 32x32 code-blocks) | `tests/tile_geometry_cap.rs`, `tests/tile_geometry_alloc.rs` |
| [Tier-2 steps](#the-tier-2-work-bound) | max(2^24, 32 × the tiles' data bytes) | `packet_work_limit` | about 7.8 times (largest share 12.8%) | `tests/work_bound.rs`, `src/tier2/work_bound_tests.rs` |
| [Tier-1 sample-passes](#the-tier-1-work-bound) | max(2^26, 2^15 × the tiles' data bytes) | `code_block_work_limit` | about 2.35 times | `tests/work_limit.rs`, `src/tier1/work_bound_tests.rs` |

The encoder figures come from the committed fixtures, the Part 4 corpus, and
the Sentinel-2, GDAL and OpenJPEG test files named in each section. Each
section also lists the valid files its bound refuses. A fourth cost, resolving
tile headers, was removed by making the parse cheaper rather than by adding a
cap ([Tile header cost](#tile-header-cost)).

A tile may hold one code-block for each 512 samples the sample limit allows,
and one precinct for each 2048, and at least 2^20 code-blocks and 2^18
precincts. At the 2^29 floor of the default limit the two rules meet, so a
file of up to 64 MiB has the caps of 2^20 and 2^18; past that they grow with
the input as the sample limit does, and `SampleLimit::Raised(n)` and
`SampleLimit::AtMost(n)` on `sample_limit` set them too. A tile past a cap is
refused with `Limit` of kind `TileCodeBlocks` or `TilePrecincts`, which
`raised_for` raises through `sample_limit`
([Code-blocks and precincts per tile](#code-blocks-and-precincts-per-tile)).
Where the sample limit that would admit the tile is past what the target
can address, the kind is `AddressSpace` instead. On a 32-bit target, where
that is 2^29 - 1 samples, every tile past a cap is refused so.

A few fixed guards also return `Limit`, and no option lifts them: 32 POC
progression volumes, 30 coded bit-planes per code-block, and an image too
large for the target to address, under any sample limit (so is a tile whose
code-blocks or precincts need a sample limit past what it can address). Their
`retry_with()` is `None`, and `LimitKind::is_adjustable()` is false.

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
`read_header_with` makes the same sample check, so it refuses the sample
count as the decode would. It checks the tile caps below only where the main
header shows them (see [Code-blocks and precincts per
tile](#code-blocks-and-precincts-per-tile)).

Under a window or a tile (`DecodeOptions::region`), the count is the
samples of the tiles the decode reads, in every component, at the reduction.
Those tiles cover the returned area, so the output planes fit inside the
count. Each tile read is reconstructed whole, one tile at a time, in buffers
sized by the tile, so the count bounds those too. And the packet walk builds
every component's code-block geometry in each tile read, which the count
bounds as the next sections rely on. A palette's expanded channels and the
channels sRGB output adds are counted over the returned area only. So the
sample buffers peak within the same multiple of the limit as for a whole
decode. The parse of the tile-parts is not counted. It keeps about 720
bytes for each tile that has a tile-part, read or not, plus about 20 bytes
for each further tile-part of a tile, and 2 bytes for each tile the SIZ grid
declares. A tile-part takes at least 14 bytes of input, so this is at most
about 52 times the input plus 128 KiB. A file of 65535 tiles, each in one
15-byte tile-part, peaks at 47 MB, and one that declares 65535 tiles and
carries one tile-part peaks at 132 KB, strict or not, however long that
tile-part is (`tests/tile_record_alloc.rs`). A file whose whole image is
over the limit now gets that far under a window. A 256x256 window of a
32769x16385 image in 4096x4096 tiles fits the default,
while a window of one 23171x23171 tile counts the whole tile. A
`DecodeOptions::components` subset does not lower the count, as the walk
still builds the geometry of the components it leaves out; under a window it
counts every component of the tiles read.

OpenJPEG has no such limit: it allocates what the header declares and fails
only when the allocation does.

`DecodeOptions::sample_limit` raises the limit (`SampleLimit::Raised(n)`,
n times the default for the same input), lifts it
(`SampleLimit::Unlimited`), or sets a fixed ceiling (`SampleLimit::AtMost(n)`,
see [A fixed ceiling](#a-fixed-ceiling)). Under the first two the refusal
names the option and the smallest `Raised` factor that admits the file. Even
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

`SampleLimit::Raised(n)` is n times the default for the same input: n times
2^29 up to 64 MiB, and 8·n samples per byte past it. A factor never allows
less than `Default`, and a larger factor never allows less than a smaller
one. The factor a refusal names is still the smallest that admits the
count, as it divides by the default of the same input. No limit, not even
`SampleLimit::Unlimited` and whatever the input size, goes past the samples
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
They are for a file whose peak is its samples. A file that spends its bytes
on codeword segments peaks past 43 with square tiles, but stays under about
102: see [Segment records](#segment-records), where the records take up to
about 65 bytes of heap per byte of input and a 96 MB file peaks at about 77
bytes per byte resident.
A tiled image peaks lower, as each tile is reconstructed in turn, and a
one-tile image is also held to the code-blocks and precincts a tile allows
([Code-blocks and precincts per tile](#code-blocks-and-precincts-per-tile)),
which grow with the limit. An `IncrementalDecoder` can hold up to about
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

### A fixed ceiling

`SampleLimit::AtMost(n)` sets the sample limit to `n` samples, whatever the
input's length, so padding cannot raise it. It is not a factor of the default:
`n` is the limit for a file of 100 bytes and for one of 1 GB alike, and only
the samples the target can address cap it. It holds every count the default
does: every component of the tiles read (a 16x16 window of one 23171x23171 tile
counts 536.9 M samples), a palette's expanded channels, and the channels sRGB
output adds. `read_header_with` with the same options refuses the sample count
as the decode would, before any tile is read, and the tile caps where the main
header shows them.

The output is then at most `4·n` bytes, and the sample buffers peak at no more
than about 3.2 times that, from the measurements above (a square tile measured
1.35, resident; the allocated bytes measure 2.0 for a square tile and up to
about 2.3 for a tile one or two samples wide). Other memory is on top: the
input itself, and the parse of the tile-parts, which keeps a record for each
tile the input carries, at most about 52 times the input plus 128 KiB (see
[The sample limit](#the-sample-limit)). `AtMost(2^26)` is 256 MiB of output
and sample buffers of at most about 820 MiB, plus the input and its tile
records. So bound the input's size under `AtMost` too, as the decode holds
the input. The code-blocks and precincts a tile may hold are set from `n` as
well, at least 2^20 and 2^18 (see [Code-blocks and precincts per
tile](#code-blocks-and-precincts-per-tile)). Past 2^29 their bookkeeping adds
at most about a third of the `4·n` bytes; below it, the floors are what a
tile may hold, and they do not shrink with `n`. One tile's records then add
up to about 0.8 GiB whatever `n` is (an estimate: 2^20 code-blocks x 560
bytes + 2^18 precincts x 750 bytes, from the peaks in [Figures for a decode
service](#figures-for-a-decode-service)). So below about 2^26 the records, not
the samples, set the peak. The figure to plan on is 3.2 x 4n for the samples,
plus about 0.8 GiB for one tile's records, plus up to about 65 bytes for each
byte of input for [segment records](#segment-records), plus the input. The
measured peaks against `n` are in that section. An `IncrementalDecoder` can
hold up to about three times one decode's peak, plus the bytes fed. What to
set around a decode besides the ceiling (the input size, a timeout, memory for each concurrent decode) is in
[Running a decode service](../SECURITY.md#running-a-decode-service).

A refusal under `AtMost(n)` has kind `Samples` (or `TileCodeBlocks` or
`TilePrecincts`, for a tile's geometry) and names the ceiling and the count.
It gives no retry (`retry_with` is `None`, so `raised_for` is `None`): the
caller set the ceiling. In an `IncrementalDecoder` it is an error, not a
wait, as more bytes do not raise it, unless the refusal comes from a later
tile while a refine counts the code-block work: that ends the count as
`Waiting::BoundSpent` (see [Incremental
decoding](incremental-decoding.md)). `AtMost(0)`, and `AtMost` on
`packet_work_limit` or `code_block_work_limit`, is `InvalidOptions`.

### Figures for a decode service

The [operator recipe](../SECURITY.md#running-a-decode-service) quotes these.
Each was measured on a release build, by timing `decode_with` or as
`/usr/bin/time` reports the peak resident set, unless it says estimated.

**The palettized file.** `tests/sample_limit.rs` builds it:
`many_tiles_of_empty_packets(8, 512, 31)` (31 components of 4096 x 4096 in 64
tiles of 512, six levels, an empty packet each) in a JP2 file whose `pclr` and
`cmap` map component 0 to one channel (`palette_jp2`). It is 15,069 bytes. The
header lists one channel of 2^24 samples. The decode at the defaults holds
31 x 2^24 samples and took 6.5 s with a peak of 2.03 GiB (2,130,788 KiB) in a
standalone program, and 2.07 GiB under the test harness. Reproduce it with
`/usr/bin/time -v cargo test --release --test sample_limit -- --ignored
the_palettized_file_decodes_every_component` and read "Maximum resident set
size". `AtMost(2^28)` refuses it in well under a millisecond: "a decode of 31
components needs 520093696 samples". The test
`at_most_holds_the_codestream_and_the_palette_expansion` asserts the refusal on
the same header, cut before its tile data.

**The peak under `AtMost(n)`.** Each file was decoded once with
`DecodeOptions::default().with_sample_limit(SampleLimit::AtMost(n))` in a
release build, `strict` on, and the peak is `/usr/bin/time -v`'s "Maximum
resident set size". The `t1bomb` files come from
`scripts/gen-tier1-work-bound-fixtures.py` (`t1bomb(size, cb, passes)` with
the arguments in the table, `cb` the log2 of the code-block side), decoded
with `decode_the_file_in_peak_file` as in [Segment
records](#segment-records). The 2^20
row is `at_most_2_to_the_20_peaks_on_the_tile_caps` in
`tests/sample_limit.rs`, which asserts the file and decodes it: reproduce it
with `/usr/bin/time -v cargo test --release --test sample_limit -- --ignored
at_most_2_to_the_20_peaks_on_the_tile_caps` (238,784 KiB with the harness).
The 2^18 row is a 512 x 512 tile in 1 x 1 precincts of empty packets, from the
same builder.

| `AtMost(n)` | 3.2 x 4n | File | Measured peak |
| ----------- | -------- | ---- | ------------- |
| 2^18 | 3.2 MiB | 512 x 512, 1 x 1 precincts, 262,226 bytes | 81 MiB |
| 2^20 | 13 MiB | 1024 x 1022, 2 x 2 precincts, 261,719 bytes | 233 MiB |
| 2^22 | 51 MiB | `t1bomb(2048, 2, 1)`, 513,720 bytes | 105 MiB |
| 2^24 | 205 MiB | `t1bomb(4096, 2, 1)`, 2,054,638 bytes | 429 MiB |
| 2^26 | 820 MiB | `t1bomb(8192, 3, 1)`, 2,054,638 bytes | 621 MiB |

Every file decoded. Past about 2^26 the sample buffers dominate. The 0.8 GiB
planning term is an estimate. The 2^24 row gives the cost of a code-block: its
sample buffers of a square tile are about 1.35 x 4n = 86 MiB, which leaves
(429 - 86) MiB over about 2^20 code-blocks, about 340 bytes each of
resident memory. On the heap alone the same file is 241 bytes a code-block
over the output (see [Code-blocks and precincts per
tile](#code-blocks-and-precincts-per-tile)). The 0.8 GiB estimate uses 560,
which is above both, so it is on the high side. The 2^20 row holds 261,632 precincts and about 800,000 code-blocks, nearly both caps, and
peaks at 233 MiB, so the two caps together have not been shown to reach the
estimate; the precinct figure of 750 bytes is from #583's probes and is not
isolated here.

### Segment records

The caps count code-blocks, not codeword segments. With the code-block style
`termall` (0x04) each coding pass is its own segment, and a file can code a
segment of no bytes in about 3 header bits, since `Lblock` is at least 3; a
segment of one byte takes 11 bits with its body. The walk keeps a 4-byte
pass count for each segment and, for a segment with bytes, a 16-byte chunk,
and the packet being read a 24-byte note of it. Once the walk is done, the
packet's notes are freed and tier-1's records are built beside the walk's:
an 8-byte head for each segment and a 16-byte slice for each chunk. With the
room a list keeps as it grows, a segment of no bytes costs about 15 bytes of
heap and one of a byte about 85. The lists keep the most room when a block
holds just past a power of two of segments, 33 or 65. Neither the caps nor
`AtMost(n)` hold the records down; the input's length does.

A one-shot decode frees a tile's records before its inverse wavelet, so they
do not add to that tile's peak. They do add to the samples the earlier tiles
have put in the image, which is held until the decode returns.

Each file below is from `scripts/gen-tier1-work-bound-fixtures.py --memory
DIR`, decoded once in a release build, and the peak is "Maximum resident set
size" from `/usr/bin/time -v`. `termall_SIZE_2_P.j2k` is one tile in 4 x 4
code-blocks of `P` one-pass segments of no bytes, and `termall_SIZE_2_P_1.j2k`
the same with segments of one byte. `filled_8xR_4096_P_1.j2k` is 8 x `R`
tiles of 4096 x 4096, each an empty packet but the last, which is
`termall_4096_2_P_1`. Before is the decoder that kept about 130 bytes a
segment (#585); the runs past 20 GiB were stopped at a 20 GiB memory cap.

| File | Size | Limit | Peak | Per byte | Before |
| ---- | ---- | ----- | ---- | -------- | ------ |
| `termall_1024_2_88` | 2.3 MB | `AtMost(2^20)` | 98 MiB | 44 | 720 MiB |
| `termall_2048_2_88` | 9.3 MB | `AtMost(2^22)` | 381 MiB | 43 | 2.80 GiB |
| `termall_4096_2_88` | 37 MB | `AtMost(2^24)` | 1.50 GiB | 43 | 11.2 GiB |
| `termall_4096_2_88` | 37 MB | the defaults | 1.50 GiB | 43 | 11.2 GiB |
| `termall_2048_2_88_1` | 32 MB | the defaults | 1.25 GiB | 41 | 5.93 GiB |
| `termall_2048_2_65_1` | 24 MB | the defaults | 1.10 GiB | 49 | 5.11 GiB |
| `termall_2048_2_33_1` | 12 MB | the defaults | 604 MiB | 51 | 2.60 GiB |
| `termall_4096_2_65_1` | 96 MB | `AtMost(2^24)` | 4.41 GiB | 49 | over 20 GiB |
| `filled_8x4_4096_33_1` | 49 MB | the defaults | 4.39 GiB | 96 | 12.3 GiB |
| `filled_8x5_4096_65_1` | 96 MB | the defaults | 6.91 GiB | 77 | over 20 GiB |

The per-byte figures are bytes of peak for each byte of input, input
included. One tile of segments peaks at up to about 51 bytes a byte, and the
heap allocated at up to about 63, as some of the room the lists keep is never
written: the segment term of the `AtMost(n)` plan is about 65. The last two
rows add the 2^29 and 40 x 2^24 samples of the earlier tiles: 4.39 GiB is
under the 6.4 GiB of a file of up to 64 MiB, and 77 bytes a byte under the
102 past it. `termall_4096_2_65_1` under `AtMost(2^24)` peaks at 4.41 GiB
against the 6.9 GiB the plan gives (3.2 x 4n + 0.8 GiB + 65 bytes a byte of
input + the input).

Reproduce a row with `PEAK_FILE=DIR/termall_1024_2_88.j2k PEAK_LOG2_N=20
/usr/bin/time -v cargo test --release --test sample_limit -- --ignored
decode_the_file_in_peak_file` (100,044 KiB with the harness), and leave
`PEAK_LOG2_N` unset for the defaults. `cargo test --release --test
segment_records_alloc -- --ignored` decodes the same shapes, built in the
test, and checks their heap: a single tile under 72 bytes a byte, the
`AtMost(2^24)` file under its plan, and the filled files under 6.4 GiB and
102 bytes a byte.

**The sample path.** A 102-byte file of 23170 x 23170 samples (536,848,900,
under 2^29), one tile of five levels and an empty packet for each
resolution, has no code-block data. At the defaults it decodes in about 3.7 s
with a peak of 2.0 GiB (2,112,568 KiB). That is about 7 ns a sample, in the
inverse transform and the output, and no work bound limits it. The sample
limit does, so the time is about 7 ns x the samples it allows: about 4 s at
2^29, and 7 ns x n under `AtMost(n)`. It adds to the tier-1 time. Reproduce
it with `/usr/bin/time -v cargo test --release --test sample_limit --
--ignored the_header_only_file_spends_its_time_in_the_sample_path`.

**Zero bytes inside a code-block.** Zero bytes in a code-block's data count as
tile data, so they raise the work bound, and the decode still runs its passes
over them. `t1bomb(8192, 6, 88)` with a 120,000-byte last code-block of zeros
is 196,526 bytes, gzips to about 930 bytes with `gzip -9`, and decodes at the defaults in 13.0 s
(peak 269 MiB), about 108 µs a byte. The same file without the zeros (76,525
bytes) is refused in 10 ms by the code-block work bound. The file was made by
`scripts/gen-tier1-work-bound-fixtures.py --memory DIR`
(`zeros_8192_6_88.j2k`, and `zeros_none_8192_6_88.j2k` without the zeros). A
length of 120,000 needs its Lblock raised: Lblock starts at 3 and the length
field is Lblock + floor(log2(88)) bits, so 8 increments (eight 1 bits and a
0) put the field at 17 bits, enough for 120,000. Neither file is committed.
Reproduce the time with `PEAK_FILE=DIR/zeros_8192_6_88.j2k /usr/bin/time -v cargo test --release
--test sample_limit -- --ignored decode_the_file_in_peak_file` (13.1 s, 269
MiB).

**The retry loop.** The README's loop makes at most five decodes (the first
and four retries). Each of the three bounds can be raised to 4 times its
default, so across the kinds the tier-1 work reaches about 1 + 4 + 4 + 4 + 4
= 17 times one default decode's (a first refusal can already name
`Raised(4)`). That sum is an estimate from the caps in the loop, not a
measurement. The memory peak is one decode at the largest factor,
not five, plus the tile records above.

**The time.** The file is `t1bomb(16384, 6, 27)` from
`scripts/gen-tier1-work-bound-fixtures.py`: one 16384 x 16384 tile in 64x64
code-blocks of 27 passes and one byte each. It is 228,378 bytes, and its
7.25e9 sample-passes are 97% of the code-block bound
(max(2^26, 2^15 x 228,378) = 7.48e9). At the defaults it decodes in 26 to 27 s
on one core, with a peak of 1.04 GiB. A variant of 36 passes (239,958 bytes) is
refused in 34 ms. The estimate for the slowest data is the bound times 8 ns a
sample-pass (see [The tier-1 work bound](#the-tier-1-work-bound)): 0.5 s at the
floor, and 2^15 x 8 ns, about 0.25 ms, for each byte of tile data past it. A
456 KB file taking 52.7 s at the defaults was reported in #474.

### Code-blocks and precincts per tile

The sample limit bounds the decoded buffers, but each code-block and each
precinct of a tile also costs bookkeeping, and legal 4x4 code-blocks make
their count up to the samples / 16, and small precincts one a sample. So a
tile may hold at most max(2^20, the samples the sample limit allows / 512)
code-blocks and max(2^18, those samples / 2048) precincts, counted across its
components and resolutions at full resolution, whatever the reduction or the
region. The decode counts each tile under its own header (a tile-part
`COD` or `COC` resolved over the main header's) before it builds the tile's
geometry. `read_header_with` counts each tile the decode reads under the
main header, reading nothing past the first `SOT`, and refuses a tile over
a cap there when the decode refuses it too, with the decode's error. A tile
whose size shows it within both caps is not counted. Only for a tile over a
cap under the main header does it read further. It follows the tile-parts
by their `Psot`, reading only their `SOT`s, as far as the first tile-part of
the tile it counts, and notes where it passed the first tile-part of each
later tile it reads, so the tile-parts are walked once in all and no further
than the last tile it counts (to the end where that tile has no
tile-part). It reads each such tile's first tile-part header at that
offset, within its `Psot` (no tile data, no `PPM`, nothing joined), in index
order, so one tile's coding parameters are held at a time. The decode counts
each tile under its own header in index order, so a tile-part `COD` or `COC`
can take a lower tile over a cap, which the decode then refuses first. So
before such a tile is counted, each lower tile read since the last one
counted whose size does not rule that out (at 33 resolutions a component,
the most a header gives) is counted under its own header too. Under a
reduction every lower tile read is, whatever its size, as a tile-part `COD`
or `COC` can give fewer levels than the reduction takes, which the decode
checks before the caps. A lower tile whose first tile-part has no `COD` or
`COC` is coded as the main header has it, which passed both checks, so it
is skipped without reading its header or charging the budget; finding that
steps over the header's marker segments by their lengths. The first tile
refused is then the lowest, as in the decode. With `strict` on the tiles
read up to the one over a cap are looked up before the lower tiles are
counted, and where one has no tile-part the read gives the decode's error
for the tile-parts, as the decode refuses such a file before it counts any
tile. A tile-part `COD` that takes a tile over
a cap with no tile over a cap under the main header after it is left to
the decode, so a file with no tile over a cap under the main header is
still read no further than the first `SOT`. A
tile-part `COD` or `COC` that brings the tile within the caps admits it,
and so does `strict` off when the tile has no tile-part, as the decode then
leaves it out. The decode parses every tile-part before it counts a tile,
whatever the selection, so it refuses a file where a tile-part is at
fault, or with `strict` on where a tile has no tile-part, with its error
for the tile-parts: the first fault its parse finds, or the missing tile.
So where the count refuses a tile, stops at a budget (below), or with
`strict` on finds a tile with no tile-part, the header read looks for
that error too, with a walk of its own (`codestream::tile_parts_fault`).
It follows the parse step for step, reading every `SOT` and every
tile-part header that holds more than `SOD`, but keeps 6 bytes a tile in
place of the decode's tile records (up to about 52 times the input plus
128 KiB, above), and, once any tile's first tile-part has its own coding,
4 more for every tile (8 on an input of 4 GiB or more). Where a later
tile-part restates a coding value of the tile's first tile-part, it reads
that header again, at most twice a tile. The first time, it notes where
the restating header lies, 4 more bytes for every tile (8 on an input of
4 GiB or more), and a later header of the same bytes is not checked
again. The second time, at a restating header of other bytes, the coding
it reads is kept for the tile's later restatements: about 370 bytes of
heap for each such tile, about 6 times the bytes of the smallest such
tile, and more for each `COC` or `QCC`, up to about 13 times the bytes of
those tiles' tile-parts. So the walk's work stays
within a small multiple of the input: on a 12.5 MB file of 2048 tiles
whose first tile-part headers of 143 `QCC`s each 254 later tile-parts
restate, the refusal takes about 135 ms in release, as long as the
decode takes to refuse it.
It checks each tile's own header as it reads the tile's first tile-part,
from the header in hand, as the parse does. It reads the main header in
place and keeps no tile record, so it copies no `PPM`
data. The decode's own parse holds the `PPM` chunks in its tile records, as
the heap bound above counts. Where the read
refuses no tile, a tile-part header the decode finds at fault is left to
the decode. A differential test over every committed codestream, cut and
with its `SOT`s changed, and the `header` fuzz target keep that walk's
error the same as the parse's. The count has a budget of 2^22 steps under the
main header, a step for each resolution of each tile-component, and as many
under the tiles' own headers, where reading a header that copies the main
header's coding parameters costs 11 steps a component. It leaves any tile
past a budget to the decode, so only a header of many large tiles of many
components and levels gets that far, and past it `read_header_with` can
return a header for a file the decode refuses. The walk takes no step of a
budget, as it reads one `SOT` a tile-part. So the time is one walk of the
tile-parts at most plus the budgets, and the heap one tile's header, a byte
a tile for the tiles the selection reads, and 8 bytes a tile for the
offsets, except where the count refuses a tile, stops at a budget, or with
`strict` on finds a tile with no tile-part: that costs the walk for the
decode's error too: one more pass over the tile-parts, each restated
tile's first header read at most twice more, and up to 14 bytes a tile
(22 on an input of 4 GiB or more) plus the coding kept for the tiles
restated with headers that differ. Where each of
254 later tile-parts
restates a `COD` over a first tile-part header of 2^18 empty `COM`s, a
refusal takes 3.4 ms on a 1 MB file of one such tile and 10 ms on 4 MB of
four. Measured in release on
65535 tiles of 1024 x 1024, each over the
precinct cap under the main header and brought within by its first
tile-part's `COD`, with 255 tile-parts a tile (16.7 million tile-parts,
235 MB), reading a window of 1530 tiles: 45 ms where each tile's
tile-parts come together and the window is the last rows, so the walk runs
to the end; 1.8 ms where the first tile-parts all come first; and under a
millisecond with nothing over a cap. With 16 tile-parts a tile (15.7 MB)
the same reads, under `Raised(3)` (the default refuses 1.6 G samples there,
over 2^29), take 4.4 ms and 1.9 ms. With the last tile left out and
`Region::Tile` of it, the refusal takes 240 ms and 704 KiB of heap at 255
tile-parts a tile, with `strict` on or off, where the decode's own parse
takes 1.7 s and 290 MiB (`tests/tile_parts_fault_cost.rs`). With one
tile-part a tile and the last tile refused (0.9 MB), the heap is 641 KiB.
Measured in
`tests/tile_geometry_header_alloc.rs` and the tests of
`tests/tile_geometry_cap.rs`: 300 bytes of heap within the caps and 968 B
for a tile rescued; 704 KiB, one header, for 4096 components whether 4 or
256 tiles are rescued; 575 KiB for a column of 257 of 65535 tiles rescued,
almost all of it the offsets of tiles 0 to 65280; 63 ms for 1000 tiles of
4096 components each with a tile-part `COD`; and under a millisecond for
1024 tiles whose `Psot` ends inside a `COM` that holds the next tile-part,
followed by 4 MiB of `COM`.
`tests/header_tile_parts_fault.rs` shows a tile refused behind a fault in
the tile-parts, with `strict` on and off. The decode can also refuse
the file before it gets to the tile refused, for what only decoding finds,
and so name another tile, kind or factor, or give another error: a fault or
a work bound in the packets or code-blocks of a lower tile (with `strict`
on, data that ends early or runs on past the last packet), or, with
`strict` off, the charge of a step
for each tile-component of the tiles read that have no tile-part, past the
packet work bound. The header read decodes no packet, so it does not
follow the decode there; `tests/tile_geometry_cap.rs` shows a case of the
first and of the last.
The main header is parsed into memory, and its `PPM` data is held as one
`Vec` for each `Nppm` record, 24 bytes of header each however short the
record. A main header of 16.8 MB of `PPM` markers holding 4.19 million
empty records is 100 MB parsed, and `read_header_with` peaks at 168 MB on
it, about 10 times the input. The decode holds the same parse.
`IncrementalDecoder::header` keeps the walk and the counts of the tiles'
own headers between calls, and goes on from where the last call stopped.
Calls after each feed walk the tile-parts about once in all, and each call
is charged the budgets as a read from the start is: after each 256 KiB fed
of the 235 MB file above, 66 ms in all. Each call also parses the main
header again, so the cost of a call is the main header plus the tile-parts
fed since, up to the budgets; a main header of megabytes of `PPM` data is
parsed on every call. The header of a tile-part not yet
fed in full, or of a last tile-part whose `Psot` is 0, is read again on
each call. What it keeps, about 140 bytes for each tile counted under its
own header and a byte for each other tile, is held until the options
change. It keeps no copy of the main header, only a key of it (two
integers). A file whose tiles' sizes show them within the caps keeps nothing
(`tests/incremental_header_cache_alloc.rs`: 0 bytes held after a call on a
16.8 MB main header of `PPM` data, where a copy of the parsed header held
100 MB); any other keeps a few hundred bytes, plus the per-tile counts. The walk for the
decode's error for the tile-parts is kept too, with its heap above, where
it stood after the last tile-part it read whose `Psot` is not 0 and whose
end is followed by an `SOT`: every step before there read only bytes a
longer input holds the same. Each call that refuses a tile, stops at a
budget or meets a tile whose own header is at fault goes on from there,
reading the tile-parts fed since and the one it was kept before. It checks
each tile's own header as it reads the tile's first tile-part, and the
tile's `POC` volumes as each later one adds to them, so a walk that stops
on a fault is kept at the start of that tile-part and reads it again on
the next call. The walk depends on no option but
`strict`, which the header read always has off, so it is kept over a
change of options. With tile 0 refused and `Region::Tile(0)`, calls after
each 1 MiB fed of the 235 MB file took 21.5 s in all with a walk from the
start on each, and take 170 ms. With tile 1's own header at fault as
well they took 18 s and take 1.3 ms: the walk stops at that header and
is kept there, so a call reads only the tile-part at fault. On a file of
65535 tiles of 4096 components and 255 tile-parts each (233 MB), whose
count runs out of its budget and whose header reads, they took 6.9 s and
take 410 ms, most of it the budget each call spends. These are in `tests/incremental_header_cost.rs`, which bounds each by 8 times
one read of the whole file. With `strict` off, as
`IncrementalDecoder::header` reads, the `TNsot` look-ahead is not taken:
it would read past the tile-parts walked, and it changes no step and no
error of that walk, as "tile N declares A tile-parts in one `SOT` and B
in another" gives the counts the two `SOT`s state.
In an `IncrementalDecoder` a refusal under the default or a `Raised` limit
is a wait (`Waiting::SampleLimit`), as the caps grow with the bytes fed.

A refusal names the `SampleLimit::Raised` factor of `sample_limit` that admits
both the tile's precincts and its code-blocks, or twice the factor it ran
under when that is more, as the work bounds do, since the tiles after it are
not counted. Past `u32::MAX` times the default it names `Unlimited`. Under
`AtMost(n)` the caps scale from `n`, and a refusal names no retry.

Measured on the heap over the output, empty code-blocks take 88 bytes each,
whatever the shape: 2^21 4x4 code-blocks with no levels under
`AtMost(2^30)` (`tests/tile_geometry_alloc.rs`), and the same for 4
components and 2 levels or 3 components and 5 levels. A precinct, its
code-blocks included, takes about 250 to 720 bytes: 252 for 2^21 1x1
precincts of one code-block each under `AtMost(2^32)`, 720 for one
component of 8 levels in 4x4 precincts. With every block coded, a
4096x4096 tile of 2^20 4x4 code-blocks (`t1bomb(4096, 2, 1)`) peaks at
about 305 MiB of heap, 241 MiB of it over the 64 MiB of output, about 241
bytes a code-block. The struct sizes (`BlockState` 24, `CodeBlock` 88, two
tag-tree nodes of 8 each at 4/3 per leaf) give about 133 bytes a block,
and a coded block adds its boxed `SegmentStates` of 80 bytes and its
lists. So at the caps a tile's code-blocks hold up to about 0.47 bytes per
sample the limit allows (241 bytes over 512 samples a block) and its
precincts up to about 0.35 (720 bytes over 2048 samples a precinct), so a
tile at both caps up to about 0.82 bytes per sample: about a fifth of the 4
bytes per sample of output the same limit allows. Past the 2^29 floor that
share holds at every size, as both caps and the output grow with the
limit; below it, the floors are what a tile may hold.

At the floor, a one-tile image at least 72 samples wide and under 2^29
samples fits in 32x32 blocks (23170x23170 needs 527,259; 64x64 blocks need
132726 at the limit; 64 x 2^23 needs 770,048). A thinner image is refused
(66 x 8,134,407 needs 1,128,027; 32 x 2^24 needs 1.54 M; 1 x 2^29 needs
16.7 M), as is one tile in 16x16 blocks or smaller near the limit, unless the
limit is raised. A tiled image is held to the caps one tile at a time.

Before the caps grew with the limit, a 40000x40000 one-tile image of one
component in 32x32 code-blocks with 5 levels (2,083,325 code-blocks), padded
to 210 MB so that the default sample limit admits its 1.6 x 10^9 samples, was
refused at every option with `retry_with()` of `None`, while `read_header`
returned `Ok`. It now decodes at the defaults (its cap is 3,281,250
code-blocks). Measured on a release build, with empty code-blocks
(`forty_thousand_square_in_32x32_blocks_decodes_at_the_defaults` in
`tests/tile_geometry_cap.rs`, ignored), against `opj_decompress` 2.5.0, both
bit-exact against each other:

| Decoder | Reduction | Time | Peak RSS |
| ------- | --------- | ---- | -------- |
| rust-j2k | 3 | 0.43 s | 419 MiB |
| `opj_decompress -r 3` | 3 | 0.53 s | 699 MiB |
| rust-j2k | 0 | 17.5 s | 7.65 GiB |
| `opj_decompress` | 0 | 19.2 s | 6.55 GiB |

The times include reading the 210 MB file and, for `opj_decompress`, writing
its output.

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
tiles' data bytes) steps. A step is a packet read, a span stepped over, or a
sweep position, once for each (component, resolution) checked there. Only
the pairs that hold a precinct are checked, and RPCL sweeps no resolution
where none does. Code-block entries that read a bit are not counted, since
the header's bytes already pay for them. A tile-component that holds no
sample costs 32 steps for each of its resolutions
([Tile-components with no sample](#tile-components-with-no-sample)). Past
the bound the decode is `Limit`. It is one bound for the whole decode, not
one per tile. `DecodeOptions::packet_work_limit`
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

At full resolution, setting up a tile's geometry is not counted, but for a
tile-component that holds no sample
([Tile-components with no sample](#tile-components-with-no-sample)). A tile of
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

### Tile-components with no sample

A tile narrower or shorter than a component's sub-sampling holds no sample
of it (ISO B.3, Eq. B-12). Such a tile-component has no precinct and no
packet, and decodes as the standard defines it (#595). OpenJPEG 2.5.0
agrees where it decodes such a file, and fails some
([openjpeg-departures.md](openjpeg-departures.md#a-tile-with-no-sample-of-a-component)).
Its setup and decode still cost about 0.3 µs, plus 0.1 µs for each
resolution, and no byte pays for them. So each resolution of a tile-component with no sample costs
32 steps, as the end of a tile's data does with `strict` off: the default
bound's 2^24 steps hold about a quarter of a second of it. A tile-component
that holds a sample has a packet, which pays for it, so a file without an
empty one spends no more than it did.

Measured on release builds. Each shape is one-sample tiles from the origin,
component 0 full size and the others sub-sampled 255 both ways, so each of
them holds one sample, in one tile, and every other tile holds none. The
first three are `one_tile_holds_the_rest` in `tests/empty_tile_component.rs`,
with the sample in tile 0; the last is `tilecomp_255_16384` from
`scripts/gen-tile-header-fixtures.py --big`, with the sample in the last
tile. "Not charged" is the decode with the fix for #595 but without the
charge.

| Shape | Size | Not charged | Charged | OpenJPEG 2.5.0 |
| ----- | ---- | ----------- | ------- | -------------- |
| 64x64 tiles, 4096 components | 78 KB | 10.2 s | `Limit`, 0.2 s | 11.2 s, 17.8 GB |
| 64x64 tiles, 16384 components | 127 KB | 30.3 s | `Limit`, 0.24 s | |
| 32x32 tiles, 4096 components, 32 levels | 163 KB | 15.7 s | `Limit`, 0.24 s | |
| 255x255 tiles, 16384 components | 1.0 MB | over 120 s | `Limit`, 0.3 to 0.5 s | 25 s, 55 GB |

The OpenJPEG figure for the last shape is from #256.
`the_setup_of_an_empty_tile_component_spends_the_work_bound` checks the
charge against the default bound, with and without wavelet levels.

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
build time per byte. Small precincts can force code-blocks down to 1x1,
and then it is the per-tile caps on code-blocks and precincts that bound
each tile. A decode at full resolution drops no resolution, so it spends
exactly what it did before; a file built to spend
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
counts take 4 bytes per precinct of the tile, whatever the packet count:
at most 1 MB under the 2^18 precincts a tile may hold at the floor of the
sample limit, and 2 bytes for each 1024 samples the limit allows past it.
The bitmap stays capped at the default bound's size and the counts do not
grow as the walk goes, so raising the bound raises the time a file may take,
not its memory. That holds with `strict` off too, where the walk counts every
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
walk, and a geometry bound at the decode's 2^20 code-blocks or more would let
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
the decode then spends at least that much on its components. A
tile-component that holds a sample has a packet, a byte of the file, and
one that holds none costs steps of the packet work bound
([Tile-components with no sample](#tile-components-with-no-sample)), so
tiles × components is bounded for a file that decodes. TLM entries are
checked and dropped, since nothing reads them. Measured on release builds
(`scripts/gen-tile-header-fixtures.py --big`):

| Shape | Size | Before #256 | After #256 | OpenJPEG 2.5.0 |
| ----- | ---- | ----------- | ---------- | -------------- |
| 4096 tiles of 16384 components | 110 KB | 2.2 s | 3 ms | fails to read the header, 10 ms |
| 128x128 tiles of 16384, heavy header | 295 KB | 63.9 s | 9 ms | 22 s, 55 GB |
| 65025 tiles of 16384 | 1.0 MB | 45.8 s | 17 ms | 25 s, 55 GB |
| The same, a COD and a QCD in every tile | 2.3 MB | 78.8 s | 26 ms | |
| 65025 tiles, 1 MB of TLM entries, decodes | 2.0 MB | 13.2 s | 56 ms | 0.34 s |
| 65025 tiles, 4 MB of TLM entries, decodes | 5.2 MB | 85.5 s | 63 ms | 0.35 s |

The first two hold no sample of any component but the first, so they are
refused with `SIZ` (`Error::Marker`) since #595; the 3 and 9 ms figures are
from before, when they were refused at tile 0. In the next two each of
those components holds one sample, in the last tile, and they are refused
by the packet work bound in 0.3 to 0.5 s
([Tile-components with no sample](#tile-components-with-no-sample)); the
17 and 26 ms figures are from before #595, when they too were refused at
tile 0. The OpenJPEG figures for the 16384-component shapes are from #256.
