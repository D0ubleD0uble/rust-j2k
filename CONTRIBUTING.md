# Contributing

Thanks for your interest in rust-j2k. This file is the short version; the
authoritative project conventions live in
[.claude/rules/conventions.md](.claude/rules/conventions.md), and the design
docs are in [docs/](docs/).

## Ground rules

- **Pure Rust, no C.** The reason this crate exists is to avoid a C JPEG 2000
  dependency. Never add a native or C-binding dependency. No `unsafe` unless a
  measured hot path needs it and it is justified and tested — the decode path is
  currently `unsafe`-free.
- **Decode-first.** The near-term phases are decode-only; an encoder is a later
  roadmap phase. See [docs/roadmap.md](docs/roadmap.md) and
  [docs/scope.md](docs/scope.md) for what belongs in which phase.
- **Correctness is defined by the oracle, not by self-consistency.** Every stage
  is cross-checked against a trusted reference decoder (OpenJPEG, or eccodes for
  GRIB2-sourced files). Lossless fixtures must match bit-exactly; lossy fixtures
  within a stated tolerance. The full strategy is in
  [docs/correctness.md](docs/correctness.md).

## Getting set up

`cargo test` is self-contained and needs no external tools — it grades decodes
against committed `expected.json` snapshots. External reference tools are only
needed to *regenerate* those snapshots; see
[docs/development.md](docs/development.md).

Optional but recommended: install the [pre-commit](https://pre-commit.com)
hooks so the gates run automatically — fmt/clippy on commit, test/deny and
security scans on push:

```sh
pip install pre-commit
pre-commit install --hook-type pre-commit --hook-type pre-push
```

## Quality gates

All of these must pass before a PR is ready (CI enforces the same set):

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo check                                  # on the MSRV, 1.87
cargo deny check
```

Robustness is fuzzed with `cargo-fuzz`; CI runs a short smoke over `decode`. To
run it locally: `cargo +nightly fuzz run decode`.

## Branching, commits, and PRs

- Trunk-based: branch off `main` and open PRs against it
  (`gh pr create --base main`).
- [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`,
  `docs:`, `chore:`, `ci:`, `test:`, etc.
- Put `Closes #N` in the PR body so merging auto-closes the issue; don't close
  issues by hand.
- Never write personal data (emails, names, private paths) into tracked files;
  use the repo's configured noreply git identity.

## Releases

Maintainers: the release process (tagging, the gated crates.io publish) is in
[RELEASING.md](RELEASING.md).

## Code of Conduct

By participating, you agree to abide by our
[Code of Conduct](CODE_OF_CONDUCT.md).
