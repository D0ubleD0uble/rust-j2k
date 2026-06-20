# Project conventions

Committed so they reach every session: local, Claude Code on the web, and any
automated run. Keep this file free of personal data: no names, email addresses,
or machine paths.

## Branching & pull requests
- Trunk-based. Branch off the default branch (`main`) and open PRs against it
  (`gh pr create --base main`).
- Put `Closes #N` in the PR body so merging auto-closes the issue. Don't close
  issues by hand.
- Don't run `gh issue create` without explicit per-issue approval. Draft the
  title and body inline for review first.

## Commits
- Conventional Commits: `feat:`, `fix:`, `docs:`, `chore:`, `ci:`, `test:`, etc.
- Never write personal data (emails, names, private paths) into tracked files;
  use the repo's configured noreply git identity.

## Quality gates (must pass before opening a PR)
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo deny check`

While the decoder is a skeleton, `src/lib.rs` carries a crate-level
`allow(dead_code, unused_variables)`; remove it as the stages land so clippy's
`-D warnings` does real work.

## Correctness is defined by the oracle, not by self-consistency
- This crate decodes a binary format. Every stage is cross-checked against a
  trusted reference decoder (OpenJPEG, or eccodes for GRIB2-sourced files), not
  against our own output. Lossless fixtures must match bit-exactly; lossy
  fixtures within a stated tolerance.
- Keep the test suite dependency-free at runtime: commit fixtures and their
  oracle snapshots, and record each fixture's provenance so the corpus is
  reproducible. The reference decoder is only needed to (re)generate oracles.

## Stay pure-Rust
- The reason this crate exists is to avoid a C JPEG 2000 dependency. Do not add
  a native/C-binding dependency. Decode-only, no `unsafe` unless a measured hot
  path needs it and it is justified and tested.

## Parallel subagents
- When delegating to parallel subagents, use harness-owned worktrees (the Agent
  `isolation: "worktree"` option). Pre-staged `git worktree add` paths are
  rejected by the subagent sandbox.
