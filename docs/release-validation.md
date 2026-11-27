# Release validation

This page describes how a release candidate is checked before it is tagged:
what each check catches, how to run it, what it costs and when it runs.
[RELEASING.md](../RELEASING.md) holds the release steps. This page explains
the checks behind them, including the ones RELEASING.md does not list.

The checks fall into nine layers. Each layer catches a kind of problem the
others miss, so a release runs all of them:

| # | Layer | Catches | Runs |
| - | ----- | ------- | ---- |
| 1 | [Per-PR gates and review](#1-per-pr-gates-and-review) | regressions in a single change | every PR |
| 2 | [The oracle corpus](#2-the-oracle-corpus) | wrong images on known files | every PR, in `cargo test` |
| 3 | [Fuzzing](#3-fuzzing) | panics, infinite loops and unbounded allocation on hostile input | every PR (smoke), and before a release |
| 4 | [Whole-crate reviews](#4-whole-crate-reviews) | problems that span files and PRs | once per release |
| 5 | [Readiness check](#5-readiness-check) | a release that would fail to build or publish, or a breaking change the CHANGELOG does not list | once per release |
| 6 | [Manual rounds](#6-manual-rounds) | problems only a real user, a real file or a person's eye finds | once per release, on the final candidate |
| 7 | [Package contents](#7-package-contents) | files in the `.crate` that should not ship | once per release; the conformance-corpus rule also runs on every PR (`package-hygiene`) |
| 8 | [Differential checks](#8-differential-checks) | undocumented changes since the last release, inputs that exceed the resource bounds, a different answer on malformed input, on a 32-bit target, or from a second HTJ2K decoder | once per release, on the final candidate |
| 9 | [Mutation testing](#9-mutation-testing) | tests that do not fail when new code is broken | once per release, on the code added since the last one |

Layers 1 to 3 run on every PR (layer 3 as a short smoke). For a release, a
coordinator (an agent or the maintainer) starts the agents that run layers 4
to 9 and triages what they find. The order is: the whole-crate reviews (4),
then the readiness check (5, which includes the pre-release fuzz run and the
package contents review), then the manual rounds (6), the differential
checks (8) and the mutation run (9) on the final candidate. The final
candidate is `main` once every
fix from layers 4 and 5 has merged, before the release PR. [Routing
findings](#routing-findings) says where each finding goes.

## Rules every layer follows

- **The oracle decides correctness.** Output is compared with OpenJPEG, or
  with eccodes for GRIB2 grids, never with this crate's own earlier output.
  Lossless output must match bit for bit, and lossy output must stay within
  its stated tolerance. [correctness.md](correctness.md) has the rules and
  the ISO/IEC 15444-4 grading.
- **Which answer wins.** The standard comes first. Next comes what a user
  hoping for automatic error correction would expect, usually as OpenJPEG
  does it. Last, an error rather than a wrong image
  ([correctness.md](correctness.md#when-the-oracle-and-the-standard-disagree)).
  The maintainer decides each departure from OpenJPEG, and
  [openjpeg-departures.md](openjpeg-departures.md) lists them. A check that
  finds a difference first looks it up there.
- **The shared-CPU budget.** Local runs often share a machine with other
  work, so keep them light. Run heavy steps under `nice -n 19 ionice -c3`
  with `CARGO_BUILD_JOBS=2`, wait while the 1-minute load is above 8, fuzz
  at most 60 s per target, run one fuzz session at a time, and run
  `opj_decompress` under a memory cap, for example `systemd-run --user
  --scope -p MemoryMax=4G opj_decompress ...`.

## 1. Per-PR gates and review

**Catches:** a change that breaks formatting, lints, tests, the license and
advisory policy, or the minimum Rust version, and the bugs, untested code and
costly API choices a second reader finds.

**Run:** before opening a PR:

```sh
rustup update stable
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo clippy --lib -- -D warnings
cargo test
cargo deny check
```

CI (`.github/workflows/ci.yml`) runs these, plus `cargo test --doc`, a `cargo
check` on the minimum Rust version, the package hygiene check and packaged tests ([layer
7](#7-package-contents)), the fuzz smoke ([layer 3](#3-fuzzing)), `cargo test`
on macOS aarch64, and on `wasm32-wasip1` the library tests (`--lib`) and the
`jp2_icc`, `sample_limit` and `limit_32bit` test binaries.
Coverage, CodeQL and Semgrep run in their own workflows.

Each PR also gets an independent review by a reviewer who did not write it.
The reviewer breaks the new code by hand to confirm a test catches it, and
reviews again after each fix until the verdict is "merge as is". A fix that
would be cheap now and expensive after the release, such as a public API name
or an error shape, is folded in before merge.

**Cost:** a few minutes of CI per push, and one review per PR.

**When:** every PR.

## 2. The oracle corpus

**Catches:** a decode that differs from the oracle on any committed file under
`tests/fixtures/`: the crate's own fixtures, the ISO/IEC 15444-4 conformance
codestreams and JP2 files, the real JP2 files from GDAL, and the HTJ2K corpus.

**Run:** `cargo test`. The oracle snapshots are committed, so no reference
decoder is needed.
[development.md](development.md#regenerate-an-oracle-snapshot) explains how to
regenerate a snapshot, and [correctness.md](correctness.md) explains what
makes a fixture prove anything.

**Cost:** part of `cargo test`.

**When:** every PR.

## 3. Fuzzing

**Catches:** panics, infinite loops and unbounded allocation on malformed
input. On such input the decoder must return an `Error`. Some targets also
check that two ways of decoding the same input agree. The `decode_partial`
target compares a decode with `strict` off to a strict decode. The
`incremental_decode` target compares each `refine` to `decode_with` with
`strict` off on the bytes fed so far. The `header` target compares
`read_header_with` to `decode_with`.

**Run:** [fuzz/README.md](../fuzz/README.md) lists the thirteen targets, how to
seed each one and the commands. A local run looks like this:

```sh
nice -n 19 ionice -c3 cargo +nightly fuzz run decode -- \
  -dict=fuzz/j2k.dict -max_total_time=60 -rss_limit_mb=4096
```

The CI smoke (the `fuzz-smoke` job in `.github/workflows/ci.yml`) builds every
target, then runs all ten for 15 to 60 s each, with a per-input `-timeout=30`
and `-rss_limit_mb=4096`. An input that runs longer than 30 s fails the job.
The [corpus replay](#corpus-replay-against-the-resource-bounds) checks slow
inputs against a tighter limit.

Before a release, every target runs locally for 60 s, one after another,
seeded as fuzz/README.md describes. This is the pre-release fuzz run, which
fuzz/README.md calls the pre-release smoke. The CI smoke does not replace it. Record the result in the "Recorded runs" table in
[fuzz/README.md](../fuzz/README.md#recorded-runs).

**Cost:** about 6 minutes of fuzzing per push in CI, plus the build. The
pre-release run takes about 10 minutes of one core, plus seeding.

**When:** the smoke on every PR, and the pre-release run once per release,
during the [readiness check](#5-readiness-check).

## 4. Whole-crate reviews

**Catches:** what a per-PR review cannot see: module boundaries, the shape of
the public API before it is published, panic paths and copy-paste divergence
across files, allocation guards, and fuzz coverage of new features.

**Run:** three fresh reviewers, each with one scope (architecture, code
quality, security), as RELEASING.md describes under "Pre-release review". The
coordinator routes each finding as described in [Routing
findings](#routing-findings).

**Cost:** three agent reviews of the whole crate, plus the fixes.

**When:** once per release, after the last feature PR merges and before the
version bump.

## 5. Readiness check

**Catches:** a release that would fail to build, document or publish, or that
would ship a breaking change the CHANGELOG does not mention.

**Run:** on an up-to-date `main` with green CI, run these checks. Most come
from RELEASING.md's "Pre-release review", which does not list the minimum Rust
version check, the semver check or the dry run of the release cut.

- Rerun the pre-flight (step 1 of RELEASING.md's "Pre-release review") once
  every finding from the [reviews](#4-whole-crate-reviews) routed to [fix
  before the tag](#routing-findings) is closed.
- Run the [per-PR gate](#1-per-pr-gates-and-review), the minimum Rust
  version check and the extended gate (step 5 of the same section):

  ```sh
  cargo +<rust-version> check      # rust-version from Cargo.toml
  RUSTDOCFLAGS='-D warnings' cargo doc --no-deps
  cargo publish --dry-run          # prints the packaged size
  ```

- Do the [pre-release fuzz run](#3-fuzzing) over every target.
- Review the [package contents](#7-package-contents).
- Run `cargo semver-checks check-release --baseline-version <last release>`
  before the version bump, so it reports every break, and check that the
  CHANGELOG lists each one.
- Do the documentation currency pass (step 4 of the same section), which
  checks each doc against the code being released.
- Dry-run "Cutting a release" on a scratch branch: the CHANGELOG section,
  the version bump, the `SECURITY.md` table, and the link pinning with its
  two `grep` checks. Then run the tag check from `release.yml`. It must
  print the version the tag will carry:

  ```sh
  cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'
  ```

Nothing from the scratch branch is pushed.

**Cost:** one agent session, most of it spent waiting on builds and the fuzz
run.

**When:** once per release, after the review fixes merge.

## 6. Manual rounds

Four rounds run once per release, on the final candidate, after every other
fix has merged, so they test what will ship. They are not scripted and do not
run in CI. An agent works each round by hand, and the visual check also needs
the maintainer. Each round is a new agent session with no earlier context. No
round commits anything, and scratch files stay outside the repository.

### Downstream consumer

**Catches:** API breaks the migration notes miss, and regressions in real
GRIB2 decoding.

**Run:** an agent clones a crate that uses this one to read GRIB2, such as
fieldglass (a sibling GRIB2 project), into a scratch directory, points it at
the candidate with `[patch.crates-io]`, and ports it using only the CHANGELOG
migration notes. It runs the consumer's tests, checks the decoded fields bit
for bit against eccodes, compares decode time with the last release, and
reports each step the migration notes left out or got wrong. The clone is
never pushed.

**Cost:** one agent session.

### Outside corpus

**Catches:** files no fixture covers, such as other encoders, sizes and
options.

**Run:** an agent gathers 100 or more public and self-encoded files that are
not in the fixtures, weighted toward the release's new features (for example,
HTJ2K in 0.5.0). For each file it compares this decoder with `opj_decompress`
on the full image, at each reduction (`-r`), for a window (`-d`, decoded here
with `Region::Window`) and for a tile (`-t`, `Region::Tile`). It also
compares:

- a truncated file with `opj_decompress -allow-partial`, decoded with
  `strict` off;
- a layer cap with `opj_decompress -l`, decoded here with
  `max_quality_layers`;
- an `IncrementalDecoder` fed random chunks: each `refine` result must equal
  a strict-off decode of the same prefix, and once the whole file is fed,
  `finish` must equal `decode` of the file.

Each file this decoder refuses but `opj_decompress` decodes is looked up in
[openjpeg-departures.md](openjpeg-departures.md). The agent also compares time
and peak memory with `opj_decompress`, and keeps a manifest of the files and
their sources in its scratch directory.

**Cost:** one agent session and some download time.

### Visual check

**Catches:** gross errors that numbers can hide, such as a swapped channel, a
wrong color space or a misplaced tile.

**Run:** an agent builds a local browser viewer that shows this decoder's
output beside OpenJPEG's (with LittleCMS for ICC profiles), an amplified
difference image, and the maximum and mean absolute difference. A click
toggles between the two images. It covers ICC profiles, palettes, alpha
channels, bit depths, reductions, `Region`, partial decode and incremental
refines. The maintainer steps through it. A coordinating agent, if there is
one, records what the maintainer finds and routes it.

**Cost:** one agent session, and the maintainer's time at the screen.

### Docs-only consumer

**Catches:** gaps and false claims in the published docs.

**Run:** an agent that sees only the `cargo package` tarball and the rendered
rustdoc writes a small program covering each main task (decode, options,
header, region, retrying with `DecodeOptions::with_retry` after an
`Error::Limit`, partial and incremental decode) and reports where the docs
fell short. In 0.5.0 it found a false scope claim, a missing tile grid on
`Header`, and doctests that read internal fixtures (#428).

**Cost:** one agent session.

## 7. Package contents

**Catches:** files in the published crate that are only for working on the
repository, or whose license does not allow the crate to ship them.

**Run:**

```sh
cargo package --list --allow-dirty
```

Read the whole list. It should hold only:

- the library, `Cargo.toml` and the files Cargo adds itself
  (`Cargo.toml.orig`, `Cargo.lock`, `.cargo_vcs_info.json`);
- the README, the CHANGELOG, `SECURITY.md` and the license files;
- the `docs/` pages written for users of the crate, not the roadmap,
  development or release pages;
- the examples, the tests, and the fixtures not excluded below;
- `benches/`, if `cargo package` needs it to verify the package.

The `exclude` key in `Cargo.toml` must keep out four fixture sets (the
conformance corpus, the GDAL JP2 files, the OpenJPH files and the benchmark
inputs) and everything used only to work on the repository, such as
`.claude/`, `.github/` and `scripts/`. For example, the 0.5.0 check found
`.claude/`, `.github/` and `scripts/` in the package (#429). The
`package-hygiene` CI job checks that the license-restricted conformance
corpus (`tests/fixtures/conformance/`) never ships, and runs the crate's own
tests from the unpacked package.

That job runs them on every PR: it packages the crate, unpacks the `.crate`
into a fresh directory with its own `CARGO_TARGET_DIR`, runs `cargo test` there
and prints the skip-line count. For a release, run them by hand too, to read
the skip count against the last release:

```sh
cargo package --allow-dirty
cd target/package/rust-j2k-X.Y.Z && cargo test
```

It must pass with no failures. A test that reads an excluded fixture skips and
prints a line with `left out of the published package`. Count them with
`... 2>&1 | grep -o 'left out of the published package' | wc -l` and note the
number; `grep -c '^skipped'` undercounts, because the harness's own output can
share a line with a skip message. A test skips only where
`tests/fixtures/REPO_ONLY`, an empty file the package leaves out, is missing
and there is no `.git` next to `Cargo.toml`. In the repository the test fails
instead if a fixture is missing, and a missing marker in a git checkout panics,
so a moved marker turns every CI job red. A packaged crate, `cargo vendor`
output and a distro build have no `.git`, so they skip, whatever `CI` is set
to. A test that reads only excluded fixtures skips whole. A test that
mixes packaged and excluded fixtures skips only the excluded entries, each
with its own line, and runs the rest. The shared helper is
`tests/support/skip.rs`. Cargo can reuse a stale build of the crate from an
earlier unpack at the same path, which gives failures that are not real, so
run `cargo clean -p rust-j2k` in the unpacked directory before `cargo test`.

**Cost:** a few minutes, plus a second build of the crate and its tests.

**When:** once per release, during the readiness check. The `package-hygiene`
job checks the conformance rule and runs the packaged tests on every PR.

## 8. Differential checks

Five checks run once per release, on the final candidate. Each compares it
with a fixed reference: the last release, the documented resource budgets,
OpenJPEG on malformed input, the same tests on a 32-bit target, or a second
HTJ2K decoder.

### Against the last release

**Catches:** a change in output or in an error message that the CHANGELOG does
not mention.

**Run:** build a small program twice, once against the last release from
crates.io and once against the candidate. Each build decodes every committed
fixture and every input in the local `decode`, `decode_partial` and `header`
fuzz corpora (the ones whose inputs are whole files) at reductions 0, 1 and 2,
and prints a hash of each image or the error's `Display` text. The fuzz
corpora differ by machine, so record their size with the result. Every
difference is checked against the CHANGELOG. A difference the CHANGELOG does
not explain is a finding: either a regression, or a change that needs a
CHANGELOG entry.

**Cost:** one agent session; the decodes take minutes.

### Corpus replay against the resource bounds

**Catches:** inputs that run longer or use more memory than
[resource-bounds.md](resource-bounds.md) says they can.

**Run:** replay each target's corpus on a release build without a sanitizer,
then fuzz each target for 60 s under the same limits. This is separate from
the pre-release fuzz run in layer 3, which uses the default sanitizer build.
The replay for `decode` is:

```sh
nice -n 19 ionice -c3 cargo +nightly fuzz run -O -s none decode fuzz/corpus/decode -- \
  -runs=0 -timeout=3 -rss_limit_mb=1536 -report_slow_units=1
```

Repeat for each target, replacing `decode` and `fuzz/corpus/decode`. For the
60 s run, replace `-runs=0` with `-max_total_time=60`. The 3 s timeout and the
1536 MB cap are tripwires, set low to flag inputs worth a look. They are not
the budgets: those are the default bounds in resource-bounds.md.

For each slow unit, timeout or out-of-memory report, check whether the input's
work fits the default bounds in [resource-bounds.md](resource-bounds.md) (for
example, at most max(2^26, 2^15 × the tiles' data bytes) [tier-1
sample-passes](resource-bounds.md#the-tier-1-work-bound)). If it fits, the
report is expected. If it does not, a bound is wrong, and that is a finding.

**Cost:** a few minutes per target.

### Malformed input against OpenJPEG

**Catches:** a file this decoder decodes or refuses differently from
`opj_decompress` that [openjpeg-departures.md](openjpeg-departures.md) does not
explain, and a panic on hostile input.

**Run:** feed every input in the local `decode` fuzz corpus (and the other
corpora that hold whole files) to this decoder and to `opj_decompress`, the
latter under a memory cap. Compare the image, or the fact of a refusal. Look
each difference up in the departures page. One the page does not list is a
finding.

**Found in 0.5.0:** no difference that was this decoder's fault, and no
panic. Some inputs differed because OpenJPEG refuses a tile height of
`0xFFFFFFFF` on a small image through an integer overflow of its own, which is
now a [departures
entry](openjpeg-departures.md#a-tile-size-that-overflows-openjpegs-tile-count).
The check found no bug here, but it found a departure the page lacked and it
costs one session, so it stays.

**Cost:** one agent session.

### The 32-bit target

**Catches:** code that works only where `usize` is 64 bits: a size computed in
`usize`, an index cast, or a limit that differs by word size.

**Run:** the whole test suite under `wasm32-wasip1`, not only the library
tests and the three test binaries (`jp2_icc`, `sample_limit`, `limit_32bit`)
CI runs there, then the corpus hash from [Against the last
release](#against-the-last-release) on both targets:

```sh
CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime run --dir=$PWD::$PWD" \
  cargo test --target wasm32-wasip1 --tests --no-fail-fast
```

Every test binary passes: in this run 67 binaries, 1580 tests passed, 7
ignored, none failed. The tests take about six minutes, plus the build; the
longest binaries are the library's unit tests and `incremental`, each about
100 s. The run does not execute the `#[ignore]`d tests, as on every target
(the attributes read `"slow: about ..."`, and the one in
`tests/tile_geometry_cap.rs` reads `"decodes 1.6 x 10^9 samples..."`). Run the
three slow tests of `work_limit`
(`the_large_ht_bomb_decodes_at_the_default_bound`,
`the_issue_bomb_decodes_with_the_limit_lifted` and
`a_ladder_over_many_tiles_that_each_reach_the_bound_takes_log_steps`) as well,
in a release build:

```sh
CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime run --dir=$PWD::$PWD" \
  cargo test --release --target wasm32-wasip1 --test work_limit -- --include-ignored
```

It passes 42 tests in about 66 s. The ignored test in
`tests/tile_geometry_cap.rs` (a 40000-square tile) is gated to 64-bit, because
the caps never rise above their floor on 32-bit and the tile is
`AddressSpace` there. The ignored test in `src/image.rs` (about 30
CPU-minutes) is not part of this run, and the one in `src/jp2/math_tests.rs`
is not built for wasm32 (it needs the host's glibc). A wasm test binary stops
at its first failure, because a panic aborts there, so a failing run names
one test a binary. To list them all, run each test in its own process
(`<binary>.wasm --list`, then `--exact <name>`).

Some tests write under the target directory. If `CARGO_TARGET_DIR` (or a
`build.build-dir`) is outside the repository, the runner must preopen it too,
with `--dir=<target>::<target>`.

Tests that differ on a 32-bit target say so where they are. Most of the
differences are in the tile caps: the sample limit tops out at 2^29 - 1
samples there, so a tile over a cap's floor is currently `AddressSpace` with
nothing to retry with, where a 64-bit target gives
`TileCodeBlocks` or `TilePrecincts` with a `Raised` retry. Both give the
same `refused_at`: the tile, the amount and its unit. These tests keep an
expectation for each target, in `tests/tile_geometry_cap.rs`,
`tests/work_limit.rs`, `tests/incremental.rs` and
`tests/fuzz_seed_outcomes.rs`. What is gated to 64-bit has no 32-bit meaning
(a ceiling past 2^29 samples, a retry that scales the cap). What is ignored
on WASI needs what it lacks: a process to run `python3` in, or an unwinding
panic. The `should_panic` test `one_entry_over_the_bound_panics` in
`tests/fuzz_partial_oracle.rs` is the exception: it is listed and reported as
`ignored`, because under panic=abort libtest does not run a test that must
panic, so it does not run there.

Print a hash of each corpus input's output (or its error text) from a small
program built for each target, and compare the two lists.

**Found in 0.5.0:** nothing. The tests pass, and the 32-bit and 64-bit
outputs were identical on every corpus input. The run guards a blind spot that
no other layer covers, and it costs one build, so it stays.

**Cost:** one agent session; the wasm run is slower than a native one.

### A second HTJ2K oracle

**Catches:** a wrong HTJ2K image that no oracle flags. OpenJPEG refuses 31 of
the HT conformance files, so those are graded only against the ISO/IEC 15444-4
reference images.

**Run:** decode each HT conformance file with Grok (`grk_decompress`) and
compare it with this decoder's output: bit for bit on a reversible file,
within the bound on an irreversible one. Grok is not a dependency and is not
needed by `cargo test`; this check runs it by hand.

**Found in 0.5.0:** every HT file decodes in both. The reversible ones match
bit for bit. Some irreversible files differ, and the cause is a bug in how
Grok applies a region of interest, not this decoder. The check gives the 31
files OpenJPEG refuses a second opinion for the price of one tool, so it
stays, and it runs when a release changes the HT code.

**Cost:** one agent session, plus installing Grok.

## 9. Mutation testing

**Catches:** a branch of new code that no test pins. A mutant that survives is
a change to the code that the tests do not notice.

**Run:** `cargo-mutants` on the code added since the last release:

```sh
git diff <last release tag>..HEAD > scratch/release.diff
nice -n 19 ionice -c3 cargo mutants --in-diff scratch/release.diff -j 2
```

Read each missed mutant. A test for it, or a note on why it is equivalent, is a
fix before the tag.

**Found in 0.5.0:** most mutants on the new surface were caught. One missed
mutant was a real gap: no test covered the `Samples` arm of
`DecodeOptions::with_retry` (fixed in #440). The rest were harmless. The run
found a gap the reviews missed, so it stays.

**Cost:** one agent session; a run over a release's changes is long, so use `-j
2` and the shared-CPU budget.

## Routing findings

Any layer can find a problem. Each finding goes one of three ways:

- **Fix before the tag.** A bug, a false doc claim, or anything that would
  be expensive to change after publishing. It is filed in the release
  milestone and fixed. Each layer that could catch a problem in the fix runs
  again.
- **File for later.** Structural work, or a gap that needs no change in the
  release. It becomes an issue outside the milestone.
- **Ask the maintainer.** A departure from OpenJPEG, a change to a gate or
  limit, a public API choice, or an edit to CI, `deny.toml` or the
  dependencies.

The maintainer tags and publishes, and does so only after every layer has
passed on the final candidate.

## Known blind spots

- The fuzz targets check for crashes and agreement between entry points, not
  for correct output.
- In the local run the `incremental_decode` fuzz target is capped at
  `-max_len=4096`, so it never sees large files.
- Error messages are compared only for the errors the tests, the fixtures
  and the fuzz corpora produce.
- The downstream consumer uses only the part of JPEG 2000 Part 1 (ISO/IEC
  15444-1) that GRIB2 needs.
- The visual check catches only gross errors.
- The corpus replay runs without a sanitizer, so it bounds peak memory
  (`-rss_limit_mb`), not single allocations.

## Not done, and why

- A docs.rs-style build: `Cargo.toml` passes `--cfg docsrs`, but no code
  uses it and the crate has no features, so `cargo doc` already shows what
  docs.rs builds.
- Windows CI and Miri: the library is pure Rust with no `unsafe` and no OS
  calls.
- `-Z minimal-versions`: the crate has no runtime dependencies.
- Long fuzz runs: they do not fit the shared-CPU budget.
- A fourth whole-crate review, or another performance run before the tag:
  little return over the three reviews and the benchmarks in
  [performance.md](performance.md).
- A release candidate on crates.io: the downstream round finds the same
  problems sooner.

## Open ideas

Checks that are not part of a release today. Each is a candidate to try. One
that finds real problems joins the layers above, as the malformed-input
differential, the 32-bit run, the second HTJ2K oracle and the mutation run did.
- Property tests: a `Region` decode equals the crop of the whole image, and
  retrying with `DecodeOptions::with_retry` after each `Error::Limit` never
  loops and ends at the same image as a decode with the limit already
  raised.
