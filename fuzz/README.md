# Fuzzing the decoder

`decode` parses bytes it did not produce, so malformed input is a first-class
case (see [`docs/correctness.md`](../docs/correctness.md) → Robustness). This
crate stands up a [`cargo fuzz`](https://github.com/rust-fuzz/cargo-fuzz)
(libFuzzer) target over the public [`rust_j2k::decode`] entry point. The bar:

- no panics, aborts, or out-of-bounds access;
- no unbounded allocation (a malformed SIZ cannot steer the buffers into an
  overflowing or out-of-memory allocation — `Xsiz * Ysiz` is bounded at parse
  time);
- no infinite loops;
- every rejected input returns a typed [`rust_j2k::Error`], never a crash.

## Layout

This is a **detached workspace** (note the empty `[workspace]` table in
`Cargo.toml`), so `libfuzzer-sys` never enters the library's dependency graph:
`cargo build`, `cargo test`, `cargo clippy --all-targets`, and `cargo deny
check` at the repo root do not see it, and the no-C-dependency / clean
cross-compile guarantee for `rust-j2k` is preserved. `libfuzzer-sys` links
LLVM's libFuzzer at build time through the nightly compiler's sanitizer
support; it is a build/test tool here, not a runtime dependency of the codec.

```
fuzz/
  Cargo.toml            # detached crate, depends on libfuzzer-sys + rust-j2k
  fuzz_targets/decode.rs # the target: drives rust_j2k::decode(&[u8])
```

## Running

Requires the nightly toolchain and `cargo fuzz`:

```sh
rustup toolchain install nightly
cargo +nightly install cargo-fuzz --locked
```

Run the target. Seeding from the committed conformance fixtures gives libFuzzer
real codestreams to mutate, which reaches far more of the pipeline than random
bytes:

```sh
# Seed the corpus once from the conformance fixtures.
mkdir -p fuzz/corpus/decode && cp tests/fixtures/*.j2k fuzz/corpus/decode/

# Time-boxed run (CI-friendly: no panics, no OOM, no hangs).
cargo +nightly fuzz run decode -- -max_total_time=300 -rss_limit_mb=2048
```

A crash is written to `fuzz/artifacts/decode/`. Reproduce and minimize it with:

```sh
cargo +nightly fuzz run decode fuzz/artifacts/decode/crash-<hash>
cargo +nightly fuzz tmin decode fuzz/artifacts/decode/crash-<hash>
```

`corpus/`, `artifacts/`, `coverage/`, and `target/` are git-ignored; only the
target and its manifest are tracked, so the corpus stays reproducible from the
committed fixtures.
