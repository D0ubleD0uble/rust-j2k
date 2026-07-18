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
whole-crate look happens.

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
   PR ahead of the release PR; structural work becomes issues (drafted for
   approval first, per the conventions).
4. **Documentation currency pass.** README (install version, feature table,
   status), `SECURITY.md`'s supported-versions table, `docs/`, rustdoc on the
   public surface, and CHANGELOG completeness against
   `git log <last-tag>..HEAD`.
5. **Extended gate**, beyond the per-PR gates: `cargo doc` warning-free,
   `cargo publish --dry-run` (confirm the package excludes the conformance
   corpus), and a bounded fuzz smoke over the current targets.

## Cutting a release

1. Update `CHANGELOG.md`: move items from `[Unreleased]` into a new dated version
   section and refresh the compare links at the bottom. Consolidate duplicate
   Keep-a-Changelog section headers that accumulated while features landed.
2. Bump `version` in `Cargo.toml` to match.
3. Open a PR with those two changes; merge once CI is green.
4. Tag the merged commit and push the tag:
   ```sh
   git checkout main && git pull
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```
5. The `release.yml` workflow runs the full gate, checks the tag matches the
   crate version, then pauses on the `crates-io` environment. Approve the
   deployment in the GitHub UI (Actions → the run → Review deployments).
6. On approval it publishes to crates.io and creates the GitHub Release.

## Versioning

Semantic Versioning. While the public API is a single `decode` function and the
decoder's coverage is still widening (see `docs/roadmap.md`), expect `0.x` minor
bumps to carry behavioural and possibly API changes. `1.0.0` is reserved for
when Part 1 decode coverage is general rather than the GRIB2 subset.
