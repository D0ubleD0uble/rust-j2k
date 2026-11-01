# Orchestration profile

Read by the `orchestrate` skill. Run state lives in the handoff file, not here.
No personal data in this file (it is committed).

```yaml
repo: D0ubleD0uble/rust-j2k
trunk: main
handoff: ~/.claude/orchestrate-state/D0ubleD0uble-rust-j2k/handoff.md
handoff_archive: ~/.claude/orchestrate-state/D0ubleD0uble-rust-j2k/handoff-archive.md

slots: { default: 2, max: 3 }
models: { default: sonnet, mechanical: haiku, complex: opus }
  # complex: new parsers of untrusted input (#160), colour math (#165),
  # anything touching tier1/tier2/dwt numerics, and every review.
focus_themes: ["phase3: milestone 'Phase 3 — JP2 file format decode'"]
standing_orders:
  - "Keep one worker on the Phase 3 chain (#159 -> #160 -> #161..#166 -> #167) while it has an eligible issue."

labels: { decision: needs-decision, judgement: needs-judgement }
hotspots:
  - "src/lib.rs, src/error.rs, src/image.rs (public API: Image, DecodeOptions, Error)"
  - "src/jp2/mod.rs until #160 has merged; afterwards per-box files may run in parallel"
  - "tests/support/mod.rs (Expected schema) and tests/fixtures/conformance/regenerate.py"
  - "fuzz/Cargo.toml and fuzz/fuzz_targets/ (one worker adding targets at a time)"
owner_only_edits:
  - ".github/** (CI workflows, dependabot, templates)"
  - ".pre-commit-config.yaml, deny.toml, codecov.yml"
  - "Cargo.toml [dependencies]/[dev-dependencies]/rust-version, fuzz/Cargo.toml dependencies"
  - "loosening any conformance bound, tolerance, manifest grade, or removing an IN_CLASS entry"
  - "any output that deviates from OpenJPEG (needs ISO-text proof + demonstrated improvement, then owner approval)"
  - "public API beyond what the issue specifies"
  - "releases, tags, crates.io publish, CHANGELOG version headers"
busy_rules: []

worker_brief: .claude/orchestrate-worker.md
launch_prompt: "Repo rust-j2k, a pure-Rust JPEG 2000 decoder; correctness is the OpenJPEG oracle."
launch_rules:
  - "df -h / must show >= 25 GB free before launching (each worktree target/ is 1-3 GB)."

gate:
  run: "rustup update stable && cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test && cargo deny check"
  notes: "CI (required checks: rustfmt, clippy, test, msrv, cargo-deny, fuzz smoke) is the merge gate; the local run is the worker's pre-PR gate. If fuzz/ changes, also: cargo +nightly fuzz build. Never --no-verify."
merge: "gh pr merge <pr> --squash --delete-branch (only after all required checks pass on the reviewed head SHA)"
risky: ["src/jp2/** parsers", "fuzz targets", "colour conversion", "public API", "tier1/tier2/dwt numerics"]
review: { policy: every_pr, model: opus, extra: "Invoke the diverge skill on 'what would a normal review of this PR miss', then verify each candidate against files or by running it." }
standing_approvals:
  - "2026-10-31: file standalone issues found by PR reviews and the periodic critique directly (issue-writing rules apply); list them at the next pause. Findings in a queued issue's files go as a comment on that issue."
owner_only_how: "The orchestrator states an approval in the launch prompt, naming the exact files. Nothing else counts (agents share the owner's GitHub account)."
guard_grep: ["#[ignore]", "IN_CLASS", "bit_exact", "max_pae", "max_mse", "MAX_", "Unsupported", "unsafe", "allow(clippy"]
cadence: { review_every: 3, review_how: "Opus read-only critique in a fresh scratchpad clone over the merged range, using the diverge skill; findings routed as issues or comments." }

heartbeat: { cron: "7,22,37,52 * * * *" }
health: ["df -h /", "free -g", "uptime"]

decision_skill: next-decision
issue_writing: [".github/ISSUE_TEMPLATE/feature_request.md", ".github/ISSUE_TEMPLATE/bug_report.md"]
memory:
  - project_rust_j2k.md
  - project_phase3_decisions.md
  - project_codestream_error_mapping.md
  - feedback_openjpeg_is_the_default_reference.md
  - feedback_issue_writing.md
  - feedback_docs_voice.md
  - reference_openjpeg_oracle.md
constraints:
  - "Pure Rust only; no C/native dependency ever; no unsafe."
  - "Never commit personal data (emails, names, machine paths)."
  - "Never edit the owner's main checkout; workers use harness-owned worktrees (Agent isolation: worktree)."
  - "Phase 3 owner decisions in docs/roadmap-phase3.md are settled; do not re-open them."

local:
  protected_checkouts: ["the repo's main checkout"]
```
