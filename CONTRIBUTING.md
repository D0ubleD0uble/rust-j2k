# Contributing

Thanks for your interest in rust-j2k. This file is the short version. The full
project conventions are in
[.claude/rules/conventions.md](.claude/rules/conventions.md), and the design
docs are in [docs/](docs/).

## Ground rules

- **Pure Rust, no C.** The reason this crate exists is to avoid a C JPEG 2000
  dependency. Never add a native or C-binding dependency. No `unsafe` unless a
  measured hot path needs it and it is justified and tested. The library has no
  `unsafe` code.
- **Decode-first.** The near-term phases are decode-only; an encoder is a later
  roadmap phase. See [docs/roadmap.md](docs/roadmap.md) and
  [docs/scope.md](docs/scope.md) for what belongs in which phase.
- **Correctness is defined by the oracle, not by self-consistency.** Every stage
  is cross-checked against a trusted reference decoder, OpenJPEG. Lossless
  fixtures must match bit-exactly; lossy fixtures within a stated tolerance. A
  new fixture is committed with its oracle snapshot and a record of where it
  came from; [docs/development.md](docs/development.md) shows how to generate
  the snapshot. The full strategy is in
  [docs/correctness.md](docs/correctness.md).

## Getting set up

`cargo test` needs no external tools: it grades decodes against committed
`expected.json` snapshots. External reference tools are only
needed to *regenerate* those snapshots; see
[docs/development.md](docs/development.md).

The gates below also need `cargo install cargo-deny` and
`rustup toolchain install 1.87`; fuzzing needs nightly and cargo-fuzz.
[docs/development.md](docs/development.md) has install commands for each.

We recommend the [pre-commit](https://pre-commit.com) hooks. On commit they run
fmt, clippy, file-hygiene fixers and a secret scan (gitleaks); on push, test,
deny and semgrep. A fixer that changes a file stops the commit; stage the
change and commit again.

```sh
pip install pre-commit
pre-commit install --hook-type pre-commit --hook-type pre-push
```

## Quality gates

All of these must pass before a PR is ready. CI runs them too (tests as
`cargo test --all-targets` and `cargo test --doc`), plus a fuzz smoke and test
runs on macOS and wasm32:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo +1.87 check                            # the MSRV
cargo deny check
```

Robustness is fuzzed with `cargo-fuzz`. CI runs a short smoke of each target
in `fuzz/fuzz_targets/` except `decode_partial`. To run one locally for a
minute: `cargo +nightly fuzz run decode -- -dict=fuzz/j2k.dict
-max_total_time=60`. [fuzz/README.md](fuzz/README.md) covers every target.

## Benchmarks

If your change affects decode speed, measure it with
`cargo bench --bench decode`. It times `decode_with` on committed fixtures and
uses only `std`: no extra dependency and no network. Each file gets one warm-up
decode and then 10 timed ones. The output has one line per file, in a fixed
order, with the median first and then the min, max and file size. The first
lines look like this:

```text
rust-j2k decode bench: 1 warm-up, then 10 timed iterations; times in ms
file                            median       min       max       KiB
p0_07.j2k                      1710.18   1267.35   3238.59      7701
p0_08.j2k                       590.67    578.98    667.16      3732
```

The set covers 5/3 and 9/7, MCT, a JP2, a GRIB2-shaped codestream and a small
real GRIB2 one. A full run takes under a minute. If you run the bench from the
published crate rather than a clone, the excluded conformance and `bench`
fixtures are skipped with a note.

- `RUST_J2K_BENCH_ITERS=N` sets the timed iterations (default 10).
- `RUST_J2K_BENCH_DIR=<dir>` also times every `.j2k`, `.j2c`, `.jp2` and
  `.jpx` file in `<dir>`, in name order, after the built-in set.
- Arguments that do not start with `-` keep only files whose name contains
  one of them: `cargo bench --bench decode -- p0_08`.

For numbers you can compare between runs, use an idle machine and compare
medians from the same machine. On a shared machine, run under `nice` and
compare the min, which other jobs disturb least. `cargo test` does not build or run the
bench.

## Opening a pull request

- Fork the repo, create a branch from `main`, and open the PR against `main`
  (`gh pr create --base main`).
- [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`,
  `docs:`, `chore:`, `ci:`, `test:`, etc.
- Put `Closes #N` in the PR body so merging auto-closes the issue; don't close
  issues by hand.
- Never write personal data (emails, names, private paths) into tracked files.
  Commit with an identity you are happy to publish, such as your GitHub
  noreply address.

## Releases

Maintainers: the release process (tagging, the gated crates.io publish) is in
[RELEASING.md](RELEASING.md).

## Code of Conduct

By participating, you agree to abide by our
[Code of Conduct](CODE_OF_CONDUCT.md).
