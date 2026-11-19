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
with `WorkLimit::Unlimited` and memory runs out.

## Supported versions

Fixes land in the latest `0.x` release.

| Version | Supported |
| --- | --- |
| 0.5.x | Yes |
| < 0.5 | No |

## What counts as a vulnerability

rust-j2k is a decoder: it reads binary input it did not produce, and that
input may be hostile. With the default limits, the public functions (`decode`,
`decode_with`, `read_header` and `read_header_with`) return either a result or
a typed `Error` for any input. They never panic, read out of bounds or hang,
and they allocate at most what the limits allow: about 6.4 GiB at peak
for an input of up to 64 MiB, and about 102 bytes per byte of input past
that. If the
machine can't supply that, the process aborts, as any Rust program does; that
is not a bug. If a panic, an out-of-bounds read, a hang or a larger allocation
happens, report it as above.

By default the decoded image may hold 2^29 samples (2 GiB as `i32`), or 8
samples per byte of input when that is more, so the limit grows past 64 MiB of
input. At 2^29 samples a decode peaks at about 2.7 GiB for a square tile, and at
up to about 6.4 GiB for a tile one sample wide. Past 64 MiB the peak grows to
about 43 bytes per byte of input for a square tile and up to about 102 for a
tile one sample wide: about 4.1 GiB and 9.6 GiB for a 100 MB file, input
included (scaled from measurements on smaller images; see the [`decode`
docs](https://docs.rs/rust-j2k/latest/rust_j2k/fn.decode.html)). Padding counts
as input, even in a conformant file (`COM` segments, a JP2 `xml ` box, bytes
after `EOC`), and it compresses well: 67 MB of zeros gzips to 65 KB. So bound
the size of the input you accept, measured in memory after any decompression.
Past 64 MiB the worst-case peak is about 102 times that size, and up to about
three times that for an `IncrementalDecoder`. A one-tile 1 x 2^30 image followed
by 128 MiB of zeros after `EOC` (134,283,241 bytes) decodes at the defaults
(`strict` on) in 23.5 s with a peak RSS of 8.30 GiB, measured. For a ceiling
that does not grow with the input, set `DecodeOptions::sample_limit` to
`WorkLimit::AtMost(n)`: the decode then holds at most `n` samples, `4·n` bytes
of output, whatever the input's length or padding. The sample buffers peak at
no more than about 3.2 times that (a square tile measured 1.35, resident).
Memory that grows with the input is on top: the input itself, and the parse of
the tile-parts, which keeps a record of under 1 KiB for each tile in the file,
up to about 62 times a file of empty tiles (see "The sample limit" in
[`docs/resource-bounds.md`](docs/resource-bounds.md)). So bound the input's
size under `AtMost` too. It counts what the default limit counts: every tile a
region reads, in every component, a palette's expanded channels and sRGB
output. `read_header_with` with the same options refuses the file before any
tile is read. A file past a limit is refused with `Error::Limit`, which is the
guard working, not a bug. These figures cover the one-shot decode functions.
An `IncrementalDecoder` keeps per-tile state between calls and can hold up to
about three times one decode's peak, plus the bytes fed and a table of 24 bytes
per tile index up to the highest seen, about 1.5 MiB for 65535 tiles (see "Work
and memory" in [`docs/incremental-decoding.md`](docs/incremental-decoding.md)).

For input from a source you trust, `DecodeOptions` can raise these limits
(`WorkLimit::Raised`) or remove them (`WorkLimit::Unlimited`). A decode under
a raised or removed limit may take as long, and allocate as much, as the file
asks. If an allocation fails under `WorkLimit::Unlimited`, the process aborts,
as Rust's allocation failures do.

Three things back the promise. The library has no `unsafe` code, so a
malformed input cannot cause undefined behavior: a bug can give a wrong value,
panic, hang or allocate too much, but not corrupt memory. It has no runtime
dependencies, Rust or C, so no third-party codec brings its own memory-safety
bugs. And it is fuzzed: cargo-fuzz targets feed arbitrary bytes to the public
functions and to each decoding stage, and CI runs a short fuzz session on
every pull request. [fuzz/README.md](https://github.com/D0ubleD0uble/rust-j2k/blob/main/fuzz/README.md) lists the targets.
