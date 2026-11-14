# Contributing

Thanks for your interest in rust-j2k. To report a crash or a hang, see
[SECURITY.md](SECURITY.md). For other bugs and ideas, open an issue. The design
docs are in [docs/](docs/).

## Ground rules

- The crate stays pure Rust. It exists so that decoding JPEG 2000 needs no C
  library, so don't add a native or C-binding dependency. The library has no
  `unsafe` code. Add some only for a measured hot path, with a written
  justification and tests.
- The crate only decodes. An encoder is planned
  ([docs/roadmap.md](docs/roadmap.md)), and [docs/scope.md](docs/scope.md)
  shows what is supported.
- Tests compare our output with a trusted reference decoder, never with our
  own earlier output. The reference is OpenJPEG, or eccodes for a GRIB2 file,
  plus the reference images of the JPEG 2000 conformance suite. Lossless
  fixtures must match exactly, and lossy ones within a stated tolerance. The
  full approach is in [docs/correctness.md](docs/correctness.md).

## Getting set up

`cargo test` needs no external tools. It compares each decode with committed
snapshots of the reference decoder's output.

To run every check below you also need cargo-deny, Rust 1.87 (for the
minimum-version check) and, for fuzzing, nightly with cargo-fuzz. On Debian or
Ubuntu, `scripts/install-oracle-tools.sh` installs cargo-deny, nightly and
cargo-fuzz, along with OpenJPEG and eccodes for regenerating snapshots; add
Rust 1.87 with `rustup toolchain install 1.87`.
[docs/development.md](docs/development.md) has the commands for other systems.

We recommend the [pre-commit](https://pre-commit.com) hooks. On commit they
run `cargo fmt --check`, clippy, file checks (YAML, JSON, TOML, merge
conflicts, trailing whitespace, end of file and large files), shellcheck,
actionlint and a secret scan (gitleaks). On push they run `cargo test`,
`cargo deny check` and semgrep. If a hook fixes a file, the commit stops; stage
the change and commit again.

```sh
pip install pre-commit
pre-commit install --hook-type pre-commit --hook-type pre-push
```

## Checks every pull request must pass

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo +1.87 check                            # the minimum Rust version
cargo deny check
```

CI runs these too, plus a short fuzz run, the full test suite on macOS (Apple
Silicon), and the math and ICC tests on wasm32.

The decoder is fuzzed with cargo-fuzz. To run the main target for a minute
with the same memory limit as CI:

```sh
cargo +nightly fuzz run decode -- -dict=fuzz/j2k.dict -max_total_time=60 -rss_limit_mb=4096
```

[fuzz/README.md](fuzz/README.md) covers every target.

## Tests for a decoding change

A change to decoding needs a test that compares against a committed fixture
and its reference snapshot. [docs/development.md](docs/development.md#regenerating-an-oracle-snapshot)
shows how to make a snapshot. Record where the fixture came from, and the
commands that made it, in [tests/fixtures/README.md](tests/fixtures/README.md).

## Benchmarks

If your change affects decode speed, compare the medians of
`cargo bench --bench decode` before and after, on the same machine. Use an idle
machine if you can. On a shared one, run under `nice` and compare the minimum,
which other jobs disturb least. The bench uses only `std` and committed
fixtures, and [benches/decode.rs](benches/decode.rs) explains its options.

## Opening a pull request

- Fork the repo, create a branch from `main`, and open the PR against `main`
  (`gh pr create --base main`).
- Use [Conventional Commits](https://www.conventionalcommits.org/): `feat:`,
  `fix:`, `docs:`, `chore:`, `ci:`, `test:` and so on.
- Put `Closes #N` in the PR body so that merging closes the issue.
- Don't write personal data (emails, names, private paths) into tracked files.
  Commit with an identity you are happy to publish, such as your GitHub noreply
  address.

The release process, from tagging to the crates.io publish, is in
[RELEASING.md](RELEASING.md).

## Code of Conduct

By participating, you agree to abide by our
[Code of Conduct](CODE_OF_CONDUCT.md).
