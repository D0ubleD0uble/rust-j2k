# Security Policy

## Supported versions

This crate is pre-1.0 and developed on a single release line. Security fixes land
on the latest published `0.x` release.

| Version | Supported |
| --- | --- |
| 0.3.x | ✅ |
| < 0.3 | ❌ |

## Reporting a vulnerability

Please report security issues privately, not in public issues or pull requests.

Use GitHub's private vulnerability reporting: open the repository's **Security**
tab and choose **Report a vulnerability** (this opens a private advisory visible
only to the maintainers). Include the affected version, a description, and a
reproducer if you have one. For a decode crash, send the input bytes or the
fuzzer artifact.

We aim to acknowledge a report within a few days and to coordinate a fix and
disclosure timeline with you.

## Threat model

rust-j2k is a decoder: it parses binary input it did not produce and that may be
hostile. The robustness contract for the public entry points (`decode`,
`decode_with`, `read_header` and `read_header_with`) is that for **any** input
they return either a result or a typed `Error`, never a panic, an abort, an
out-of-bounds access, unbounded allocation, or a hang.

The promises of no hang and no unbounded allocation hold at the default limits.
By default the decoded image may hold 2^28 samples (1 GiB as `i32`), and a
decode peaks at about 1.4 GiB for a square tile and at a few GiB for a tall,
narrow one (the `decode` docs give the details). A file past a limit is refused with
`Error::Limit`, which is the guard working, not a bug.

`DecodeOptions` can raise (`WorkLimit::Raised`) or lift (`WorkLimit::Unlimited`)
the bounds on packet-header work, code-block work and image size, for input
from a trusted source. A decode under a raised or lifted limit may take as long,
and allocate as much, as the file asks. If an allocation fails under
`WorkLimit::Unlimited`, the process aborts, as Rust's allocation failures do.

Three things back the contract. The library has no `unsafe` code, so a
malformed input cannot cause undefined behavior: a bug can give a wrong value,
panic, hang or allocate too much, but not corrupt memory. It has no runtime
dependencies, Rust or C, so no third-party codec brings its own memory-safety
bugs. And it is fuzzed: `cargo-fuzz` targets drive `decode`
(with `strict` on and off) and `read_header` with arbitrary bytes, and the
stages on their own (the JP2 box reader, the ICC profile reader and sRGB
conversion, tier-1, the MQ decoder, and tier-2 packet parsing with everything
after it). A panic fails a fuzz run, and so does an allocation past libFuzzer's
`-rss_limit_mb` or an input that runs past its `-timeout`. On every pull
request and every push to `main`, CI runs seven of the eight targets in
`fuzz/`, each for 15 to 60 seconds with a 4096 MB memory limit; the `strict`-off
target, `decode_partial`, is built but not run.

A panic on any input, or a hang or excessive allocation at the default limits,
is a bug. If you find one,
report it as above. Even without a security impact, it is a robustness defect
we want to fix.
