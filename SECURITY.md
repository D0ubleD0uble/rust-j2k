# Security Policy

## Reporting a vulnerability

If rust-j2k panics, hangs or allocates more than the limits allow, on any
input at the default limits, that is a bug, even when it has no security
impact. Please report it privately, not in a public issue or pull request.

Use GitHub's private vulnerability reporting: open the repository's
**Security** tab and choose **Report a vulnerability**. This opens a private
advisory that only you and we can see. Include:

- the affected version;
- the `DecodeOptions` you used, such as `strict` off or a raised limit;
- the input bytes. For a fuzzer find, send the artifact and name the target
  that found it. We replay it with `cargo +nightly fuzz run <target> <artifact>`.

We aim to acknowledge a report within a few days and to agree on a fix and
disclosure timeline with you.

Open a public issue instead for a wrong decode, a valid file refused for a
reason the README doesn't list, or an `Error::Inconsistent`. Three things are
expected behavior, not bugs: a wrong image from damaged data (JPEG 2000 has no
checksum, so some damage can't be detected), an abort when the machine runs
out of memory within the default limits, and an abort after you lift a limit
with `Unlimited` and memory runs out.

## Supported versions

Fixes land in the latest `0.x` release. A security fix may lower a default
limit in a patch release, and the CHANGELOG lists it.

| Version | Supported |
| --- | --- |
| 0.5.x | Yes |
| < 0.5 | No |

## What counts as a vulnerability

rust-j2k is a decoder: it reads binary input it did not produce, and that
input may be hostile. With the default limits, the public functions (`decode`,
`decode_with`, `read_header` and `read_header_with`) and the
`IncrementalDecoder` methods (`new`, `feed`, `refine`, `header`, `finish` and
the setters) return either a result or a typed `Error` for any input. They
never panic, read out of bounds or hang, and they allocate at most what the
limits allow. For the one-shot functions that is about 6.4 GiB at peak for an
input of up to 64 MiB, and about 102 bytes per byte of input past that, plus
up to 3 bytes for each byte of JP2 metadata boxes (step 1 of [Running a
decode service](#running-a-decode-service)). An `IncrementalDecoder` holds up
to about three times the largest one-decode peak under the options used so
far, plus up to twice the bytes fed. If the machine can't supply that, the
process aborts, as any Rust program does; that is not a bug. If a panic, an out-of-bounds read, a
hang or a larger allocation from any other input happens, report it as
above.

By default the decoded image may hold 2^29 samples (2 GiB as `i32`), or 8
samples per byte of input when that is more, so the limit grows past 64 MiB of
input. At 2^29 samples a decode peaks at about 2.7 GiB for a square tile, and at
up to about 6.4 GiB for a tile one sample wide. Past 64 MiB, a file whose
peak is its samples peaks at about 43 bytes per byte of input for a square
tile and up to about 102 for a tile one sample wide: about 4.1 GiB and 9.6
GiB for a 100 MB file, input included (scaled from measurements on smaller
images; see the [`decode`
docs](https://docs.rs/rust-j2k/latest/rust_j2k/fn.decode.html)). A file
that spends its bytes on code-block segments peaks past 43 with square
tiles too: a 96 MB file of one such tile after 39 tiles of samples peaked
at about 77 bytes per byte. About 102 is the bound for any file, plus up to 3
for each byte of JP2 metadata boxes. Padding counts
as input, even in a conformant file (`COM` segments, a JP2 `xml ` box, bytes
after `EOC`), and it compresses well: 67 MB of zeros gzips to 65 KB. Bound the
input's size (step 1 of [Running a decode service](#running-a-decode-service)).
Past 64 MiB the worst-case peak is about 102 times that size, and up to about
three times that for an `IncrementalDecoder`. A one-tile 1 x 2^30 image
followed by 128 MiB of zeros after `EOC` (134,283,241 bytes) decodes at the
defaults (`strict` on) in 23.5 s with a peak RSS of 8.30 GiB, measured. The
sample limit also sets how many code-blocks and precincts one tile may hold:
one for each 512 and each 2048 samples it allows, and at least 2^20 and 2^18.
Their bookkeeping comes to at most about a third of the output the same limit
allows past the 2^29 floor, the same share at every size (see "Code-blocks
and precincts per tile" in [`docs/resource-bounds.md`](docs/resource-bounds.md)).
For a ceiling that does not grow with the input, and the other limits to put
around a decode, see [Running a decode service](#running-a-decode-service). A
file past a limit is refused with `Error::Limit`, which is the guard working,
not a bug. These figures cover the one-shot decode functions. An
`IncrementalDecoder` keeps per-tile state between calls and can hold up to
about three times the largest one-decode peak under the options used so far (a
change of region or reduction keeps the largest), plus up to twice the bytes
fed (their `Vec` may hold twice their length in capacity) and a table of 24
bytes per tile index up to the highest seen, about 1.5 MiB for 65535 tiles
(see "Work and memory" in [`docs/incremental-decoding.md`](docs/incremental-decoding.md)).

For input from a source you trust, `DecodeOptions` can raise these limits
(`Raised`, on a `SampleLimit` or `WorkLimit`) or remove them (`Unlimited`). A
decode under a raised or removed limit may take as long, and allocate as much,
as the file asks. If an allocation fails under `SampleLimit::Unlimited`, the
process aborts, as Rust's allocation failures do.

Three things back the promise. The library has no `unsafe` code, so a
malformed input cannot cause undefined behavior: a bug can give a wrong value,
panic, hang or allocate too much, but not corrupt memory. It has no runtime
dependencies, Rust or C, so no third-party codec brings its own memory-safety
bugs. And it is fuzzed: cargo-fuzz targets feed arbitrary bytes to the public
functions and to each decoding stage, and CI runs a short fuzz session on
every pull request. [fuzz/README.md](https://github.com/D0ubleD0uble/rust-j2k/blob/main/fuzz/README.md) lists the targets.

## Running a decode service

The limits above bound each decode on its own. None of them bounds time. To
decode files from outside, put these around the decode. The method behind each
figure is in [Figures for a decode service](docs/resource-bounds.md#figures-for-a-decode-service).

1. **Cap the input.** Bound the size of the input you accept, measured in
   memory after any decompression. The decode holds it, padding counts as
   input and raises the default sample limit, and the tile-part parse keeps a
   record of under 1 KiB for each tile the input carries, at most about 52
   times the input plus 128 KiB (see "The sample limit" in
   [`docs/resource-bounds.md`](docs/resource-bounds.md)). The JP2 box walk
   keeps a 24-byte record for each box it reads or reports and for each box
   after the codestream. With the metadata built from them, a file padded
   with small boxes holds up to about 6.5 bytes for each byte of them while
   the boxes are read, and up to 3 through the rest of the decode, measured.
   A box before the codestream that is neither read nor reported holds
   nothing (see "JP2 box records" in
   [`docs/resource-bounds.md`](docs/resource-bounds.md)).
2. **Set a fixed sample ceiling.** Set `DecodeOptions::sample_limit` to
   `SampleLimit::AtMost(n)`. The decode then holds at most `n` samples, `4·n`
   bytes of output, whatever the input's length or padding. The sample
   buffers peak at no more than about 3.2 times that (a square tile
   measured 1.35, resident). Plan on 3.2 x 4n for the samples, plus one
   tile's code-block and precinct records (about 0.8 GiB at the floors),
   plus up to about 65 bytes for each byte of input for code-block
   segment records, plus the input and the tile records above. The
   records of a tile do not shrink with `n`: a tile may hold at least 2^20
   code-blocks and 2^18 precincts at any ceiling, so below about
   `n` = 2^26 they, not the samples, set the peak. The segment term is
   the most heap measured for a byte of input, on files of one-byte
   codeword segments (each 11 header and body bits). Segments of no bytes,
   at about 3 header bits each, give more segments for a byte but less
   heap. The peaks are in [Figures for a decode
   service](docs/resource-bounds.md#figures-for-a-decode-service).
   A refusal under it is an `Error::Limit` with no retry, since you set the
   ceiling. The code-blocks and precincts a tile may hold are set from `n`
   too.
   - The count is the decode's own: every component the codestream codes, in
     every tile a region reads, a palette's expanded channels and sRGB
     output.
   - Do not count from `Header::components`. It lists the output channels,
     and a palettized JP2 file can map one of many codestream components.
     Measured: a 15,069-byte JP2 file of 31 components of 4096 x 4096, whose
     `cmap` maps one of them, reads as one channel of 16.7 M samples, and the
     decode at the defaults peaks at 2.03 GiB. `AtMost(2^28)` refuses it from
     the `SIZ`, which counts 520 M samples, in under a millisecond.
     `tests/sample_limit.rs`
     (`at_most_holds_the_codestream_and_the_palette_expansion`) asserts both
     refusals, `read_header_with` and `decode_with`, on the same header. The
     method is in [Figures for a decode service](docs/resource-bounds.md#figures-for-a-decode-service).
   - `read_header_with` under the same options is a quick early check. The
     decode can still refuse a file it admits: the header read checks the
     samples as the decode does, but the tile caps only where the main header
     shows them, and a tile-part header, or a tile past its step budget, can
     take a tile over a cap (the exceptions are listed on
     `read_header_with`). Keep the limits on the decode.
3. **Run each decode under a wall-clock timeout,** in its own process, or
   in a thread you can afford to leave running. The library has no deadline
   or cancel call, and a thread cannot be stopped from outside. The work
   bounds make the time finite and let it grow with the input. Measured: a
   228,378-byte codestream built to spend 97% of the code-block work bound
   decodes at the defaults in 26 to 27 s on one core in a release build.
   That is the tier-1 time. Estimated: the bound allows max(2^26, 2^15 x
   the tile-data bytes) sample-passes at up to 8 ns each, so about 0.5 s at
   the floor and 0.25 ms a byte past it, about 120 s for 456 KB of the
   slowest data. A 456 KB file measured 52.7 s at the defaults (reported in
   #474). The sample path adds to it, and no work bound limits it: about 7
   ns for each sample the sample limit allows, so about 4 s at the default
   floor of 2^29 samples, even for a file of 102 bytes (measured 3.7 s, 2.0
   GiB peak). `AtMost(n)` bounds that term to about 7 ns x n. Padding after
   the tile data adds about 175 ns a byte, measured. Zero bytes inside a
   code-block's data count as tile data, and cost much more: a file that
   gzips to about 1 KB (196,526 bytes, 120,000 of them zeros in one
   code-block) measured 13 s at the defaults, about 108 µs a byte.
   `Raised(n)` on a work bound multiplies the time by up to `n`. Pick the
   timeout from your largest legitimate file, not from these figures. The
   method is in
   [Figures for a decode service](docs/resource-bounds.md#figures-for-a-decode-service).
4. **Size memory as concurrency times peak.** The bounds hold for each
   decode: nothing is shared between decodes, threads or decoders. So
   reserve one decode's peak (with its input) for each decode you run at
   once. Count a decode you stopped waiting for as running until it returns:
   a thread past its timeout keeps its memory. Only ending a process gives it
   back. For each live `IncrementalDecoder`, reserve up to about three times
   the largest one-decode peak under the options used so far during a
   refine, plus up to twice the bytes fed. Between calls it
   keeps less (measured: about one peak, 2.7 GiB on a 296 MB file, reported
   in #474). See "Work and memory" in
   [`docs/incremental-decoding.md`](docs/incremental-decoding.md), which also
   says when a streaming preview gets its first image.

Retry with higher limits only for input you trust, and cap the factor, as the
README's loop does.
