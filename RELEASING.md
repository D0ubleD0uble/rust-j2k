# Releasing

The crate is published to [crates.io](https://crates.io/crates/rust-j2k) by the
`release.yml` workflow when a `v*` tag is pushed. Publishing uses crates.io
**trusted publishing** (OIDC), so there is no long-lived API token in the repo,
and the `publish` job waits for a reviewer to approve the `crates-io`
environment.

## One-time bootstrap (first release only)

crates.io has no "pending publisher" concept, so the trusted publisher cannot be
registered until the crate already exists on crates.io. The first version must be
published manually:

1. Create a scoped API token at <https://crates.io/settings/tokens> with the
   `publish-new` and `publish-update` scopes for `rust-j2k`.
2. From a clean checkout of the tagged commit:
   ```sh
   cargo publish --token "$CRATES_IO_TOKEN"
   ```
3. On crates.io, open the crate's **Settings → Trusted Publishing** and add a
   GitHub publisher:
   - Repository owner: `D0ubleD0uble`
   - Repository name: `rust-j2k`
   - Workflow filename: `release.yml`
   - Environment: `crates-io`
4. Revoke the token from step 1 — it is no longer needed.

After this, every later release goes through the workflow with no token.

## Pre-release review

Every release starts with a full-project review, before any version bump. The
point is production completeness: the tag should never be the first time a
whole-crate look happens. [docs/release-validation.md](docs/release-validation.md)
describes every check a release runs, including the manual rounds,
differential checks and mutation testing that are not listed here.

1. **Pre-flight.** `rustup update stable` (CI uses the newest stable), clean
   tree on an up-to-date `main`, latest `main` CI run green.
2. **Three whole-crate reviews**, each by a fresh reviewer (delegated agents
   work well; keep the scopes disjoint):
   - *Architecture*: module boundaries, dependency direction, public API shape
     against the next roadmap phases, error taxonomy, test-harness structure.
   - *Code quality*: panic paths reachable from `decode`, copy-paste
     divergence, doc-comment accuracy against the code, lint and test hygiene.
   - *Security*: every input-driven allocation against its guard (guard before
     allocation), attacker-steered loop bounds and quadratic blowups, the CI
     and release workflows, fuzz coverage against the current feature surface.
3. **Fix what emerges.** Triage by severity: fix-now findings land in a review
   PR ahead of the release PR; structural work becomes issues.
4. **Documentation currency pass.** README (the "What it decodes" and "What
   it doesn't do" lists, "How it's checked"), `SECURITY.md`'s
   supported-versions table, `docs/`, rustdoc on the public surface, and
   CHANGELOG completeness against `git log <last-tag>..HEAD`. Check that the
   `description` in `Cargo.toml` matches the README's opening.
5. **Extended gate**, beyond the per-PR gates: `cargo doc` warning-free,
   `cargo publish --dry-run` (confirm the package excludes the conformance
   corpus), and a bounded fuzz smoke over the current targets.

## Cutting a release

1. Update `CHANGELOG.md`: move items from `[Unreleased]` into a new dated version
   section and refresh the compare links at the bottom. Consolidate duplicate
   Keep-a-Changelog section headers that accumulated while features landed.
2. Bump `version` in `Cargo.toml` to match, and the `rust-j2k` entry in both
   lockfiles. Cargo updates `Cargo.lock` on the next build, but not
   `fuzz/Cargo.lock`, which belongs to the detached fuzz workspace, so edit
   its `version` line by hand.
3. Update the supported-versions table in `SECURITY.md` to the new minor
   version.
4. Point the published docs at the release, so docs.rs, crates.io and the
   `docs/` folder packaged in the crate show the docs for that version
   rather than `main`:
   - In `src/`, `docs/` and `README.md`, change every `rust-j2k/blob/main/`
     and `rust-j2k/tree/main/` link, and every such link pinned at an older
     tag, to the new tag
     (`sed -i -E 's#rust-j2k/(blob|tree)/(main|v[0-9]+\.[0-9]+\.[0-9]+)/#rust-j2k/\1/vX.Y.Z/#g' $(grep -rlE 'rust-j2k/(blob|tree)/' src docs README.md)`;
     this is GNU sed, and BSD or macOS sed needs `sed -i '' -E` instead).
   - In `README.md`, make each relative link (`docs/...`, `CONTRIBUTING.md`,
     `SECURITY.md`, the license files) absolute at the tag:
     `https://github.com/D0ubleD0uble/rust-j2k/blob/vX.Y.Z/<path>`.

   The tag does not exist yet when the PR is written, but it does by the time
   docs.rs builds the published crate. Afterwards, both
   `grep -rnoE 'rust-j2k/(blob|tree)/[^/]+/' src docs README.md | grep -vE '(blob|tree)/vX.Y.Z/'`
   (with the new version in place of `vX.Y.Z`; `-o` prints each link on its
   own, so a stale link on the same line as a new one still shows) and
   `grep -nE '\]\((docs|[A-Z][A-Za-z-]*\.md|LICENSE)' README.md` should print
   nothing. Between releases, new links use `main` or relative paths; step 9
   puts the pinned ones back.
5. Open a PR with those changes; merge once CI is green.
6. Tag the merged commit and push the tag:
   ```sh
   git checkout main && git pull
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```
7. The `release.yml` workflow runs the full gate, checks the tag matches the
   crate version, then pauses on the `crates-io` environment. Approve the
   deployment in the GitHub UI (Actions → the run → Review deployments).
8. On approval it publishes to crates.io and creates the GitHub Release.
9. Once the release is published, open a PR that moves the pinned links in
   `src/`, `docs/` and `README.md` back to `main`, so `main` does not keep
   pointing at the old tag:
   `sed -i -E 's#rust-j2k/(blob|tree)/v[0-9]+\.[0-9]+\.[0-9]+/#rust-j2k/\1/main/#g' $(grep -rlE 'rust-j2k/(blob|tree)/v' src docs README.md)`.
   `grep -rnE 'rust-j2k/(blob|tree)/v[0-9]' src docs README.md` should then
   print nothing. The tagged commit keeps its pins, so the published crate
   still shows the release's docs. The `README.md` links that step 4 made
   absolute can stay absolute, now at `main`.

## Versioning

Semantic Versioning. While the decoder's coverage is still widening (see
`docs/roadmap.md`), expect `0.x` minor bumps to carry behavioral and possibly
API changes. What the public API covers, and what can change, is
in [`docs/stability.md`](docs/stability.md). `1.0.0` is
reserved for when Part 1 decode coverage is general rather than the GRIB2
subset.
