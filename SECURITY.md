# Security Policy

## Supported versions

This crate is pre-1.0 and developed on a single release line. Security fixes land
on the latest published `0.x` release.

| Version | Supported |
| --- | --- |
| 0.1.x | ✅ |
| < 0.1 | ❌ |

## Reporting a vulnerability

Please report security issues privately, not in public issues or pull requests.

Use GitHub's private vulnerability reporting: open the repository's **Security**
tab and choose **Report a vulnerability** (this opens a private advisory visible
only to the maintainers). Include the affected version, a description, and a
reproducer if you have one — for a decode crash, the input bytes or a fuzzer
artifact are ideal.

We aim to acknowledge a report within a few days and to coordinate a fix and
disclosure timeline with you.

## Threat model

rust-j2k is a decoder: it parses binary input it did not produce and that may be
hostile. The robustness contract for the public `decode` entry point is that for
**any** input it returns either a decoded `Image` or a typed `Error` — never a
panic, an abort, an out-of-bounds access, unbounded allocation, or a hang.

A few things that back this up:

- **No `unsafe`.** The decode path contains no `unsafe` code, so a malformed
  input cannot cause undefined behaviour or memory unsafety; the worst a bug can
  do is an incorrect value or a panic.
- **No C dependency.** The crate is pure Rust with no native/C codec, removing
  that class of memory-safety risk entirely.
- **Fuzzed.** A `cargo-fuzz` target drives `decode` with arbitrary bytes; a
  panic, an allocation past the memory limit, or a hang fails the run. CI runs a
  bounded smoke on every change. See `fuzz/`.

A panic, hang, or excessive allocation on any input is treated as a bug. If you
find one, please report it as above — even without a security impact, it is a
robustness defect we want to fix.
