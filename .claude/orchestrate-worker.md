# Worker brief

You are one of several workers, each in its own git worktree, run by an
orchestrator. Read this and `.claude/rules/conventions.md` before you start.

## Your issue
- The orchestrator assigned you one issue. Don't pick another.
- Read the issue and all its comments, and the docs it names
  (`docs/roadmap-phase3.md` for Phase 3 work, `docs/correctness.md` always).
- If a comment defers the issue or leaves a decision open, stop and report.

## Where you work
- Branch from the latest `origin/main`. Rebase on `main` before you gate.
- Stay in your worktree. Never touch the main checkout or another worker's
  worktree. Other workers' areas are named in your launch prompt: stay out.
- Scratch output goes in `scratch/` inside your worktree (git-ignored);
  delete it before you hand back.

## Correctness
- OpenJPEG (`opj_decompress`, `opj_dump`) is the oracle. Lossless fixtures
  match bit-exactly; lossy ones within the stated bound. Deviating from
  OpenJPEG needs ISO-text proof and a measured improvement, and the owner's
  approval: stop and report instead of deviating.
- Tests go through the real public path (`decode`, `read_header`) and fail
  first against `main`. When new types can't compile on `main`, mutate each
  new check instead and show a test catches it.
- Record provenance for every fixture (source, commit, encoder, command).
- Untrusted input: no panic, no allocation sized from an unchecked field, a
  typed error for every reject (the variants are documented in
  `src/error.rs`: Codestream = structural, Unsupported = valid but outside the
  subset, Marker = illegal field, FileFormat = box layer).

## Owner-only edits
Without an approval naming the exact files in your launch prompt, do not
edit: `.github/**`, `.pre-commit-config.yaml`, `deny.toml`, `codecov.yml`,
dependency or MSRV lines in `Cargo.toml`/`fuzz/Cargo.toml`, any conformance
bound or `IN_CLASS` entry (loosening), or public API beyond the issue.

## Gate
Once, on the finished tree:

```sh
rustup update stable
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo deny check
```

If you changed `fuzz/`, also `cargo +nightly fuzz build`. Never skip a step,
never loosen a check, never `--no-verify`.

## Commits and the PR
- Conventional Commits, the repo's noreply git identity, and the attribution
  lines from your session.
- Open the PR against `main` with `Closes #N` (or `Part of #N` if your launch
  prompt says build-then-judge). **Do not merge.**
- Fix small related problems in the same PR. File nothing yourself; put
  standalone follow-ups in your report as drafts.
- Docs voice: plain and short; no roadmap phase names on API-facing surfaces
  (rustdoc, error strings, README, CHANGELOG).
- Never kill processes by broad pattern.

## Report (short)
1. PR URL and head SHA.
2. Gate result (each command) on that SHA.
3. Acceptance criteria -> the test that proves each.
4. Self-review of your own diff: what you checked, what you found and fixed,
   what remains, and a risk rating (mechanical / normal / risky).
5. Follow-up drafts; issues you commented on.
6. Anything the owner must decide or judge.
