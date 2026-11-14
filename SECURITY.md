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
| 0.3.x | Yes |
| < 0.3 | No |

## What counts as a vulnerability

rust-j2k is a decoder: it reads binary input it did not produce, and that
input may be hostile. With the default limits, the public functions (`decode`,
`decode_with`, `read_header` and `read_header_with`) return either a result or
a typed `Error` for any input. They never panic, read out of bounds or hang,
and they allocate at most what the limits allow, about 3.2 GiB at peak. If the
machine can't supply that, the process aborts, as any Rust program does; that
is not a bug. If a panic, an out-of-bounds read, a hang or a larger allocation
happens, report it as above.

By default the decoded image may hold 2^28 samples (1 GiB as `i32`). A decode
peaks at an estimated 1.4 GiB for a square tile, and at up to about 3.2 GiB
for a tile one sample wide (scaled from measurements on smaller images; see the
[`decode` docs](https://docs.rs/rust-j2k/latest/rust_j2k/fn.decode.html)). A
file past a limit is refused with `Error::Limit`, which is the guard working,
not a bug.

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
every pull request. [fuzz/README.md](fuzz/README.md) lists the targets.
