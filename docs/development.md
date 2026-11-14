# Development environment

This page lists the tools that regenerate the oracle snapshots and run the
extra quality gates, with the version and an install command for each. It
ends with [the checks for a new decode stage](#working-on-a-decode-stage).

**You don't need these tools to run the test suite.** `cargo test` compares
each decode against the committed snapshots (`<name>.expected.json` and the
conformance references), so the reference decoders below are needed only to
regenerate a snapshot. CI installs none of them: the committed snapshots are
the contract, and the suite must pass with no oracle tool present. See
[`correctness.md`](correctness.md) for how the corpus is graded.

## One-command setup

On Debian/Ubuntu, [`scripts/install-oracle-tools.sh`](../scripts/install-oracle-tools.sh)
installs OpenJPEG, eccodes, cargo-deny, a nightly toolchain and cargo-fuzz,
checks that they are on `PATH`, and fails if OpenJPEG is not 2.5.0. On Linux
it also warns, without failing, when LittleCMS is not 2.14, since only the
ICC oracles need that version. On macOS it prints the Homebrew
commands to run. The cargo and rustup tools install on any platform. The
script does not install Python 3.

```sh
scripts/install-oracle-tools.sh
```

The crate's minimum Rust version is 1.87 (`rust-version` in `Cargo.toml`),
and CI checks that the crate still builds on it. To check it locally:

```sh
rustup toolchain install 1.87
cargo +1.87 check
```

## The toolchain

OpenJPEG, and for the ICC oracles LittleCMS, are pinned because they decide
the snapshot bytes; the scripts refuse any other version. eccodes is only a
cross-check, and no script checks its version. The cargo tools track their
current release, since they don't affect any oracle.

| Tool | Version | Provides | Used by |
| ---- | ------- | -------- | ------- |
| OpenJPEG | 2.5.0 | `opj_decompress`, `opj_dump`, `opj_compress` | oracle snapshots, header checks, Tier-1 vectors, synthetic fixtures |
| LittleCMS | 2.14 | linked by `opj_decompress` | ICC oracles (`scripts/gen-icc-oracles.py`) |
| eccodes | 2.34.1 | `grib_dump`, `grib_get_data` | GRIB2-sourced fixtures |
| cargo-deny | current release | `cargo deny` | license/advisory quality gate |
| cargo-fuzz | current release (+ nightly) | `cargo fuzz` | robustness fuzzing |
| Python 3 | 3.9+ | `.pgx` → `expected.json` conversion, fixture generation | `scripts/gen-oracle.sh`, `scripts/gen-*-fixtures.py` |

### OpenJPEG

The reference JPEG 2000 decoder. `opj_decompress` produces the sample oracle;
`opj_dump` prints marker fields for header cross-checks; `opj_compress`
synthesizes the codestreams behind the isolated Tier-1 golden vectors and the
`scripts/gen-*-fixtures.py` synthetic corpus.

```sh
# Ubuntu 24.04 ships 2.5.0; other releases may not (the scripts refuse them)
sudo apt-get install libopenjp2-tools

# macOS
brew install openjpeg   # may be newer than 2.5.0: build from source then
```

For an oracle that doesn't depend on the distro package, build the pinned
release tag:

```sh
sudo apt-get install liblcms2-dev   # LittleCMS 2.14 (Ubuntu 24.04), linked dynamically
git clone --depth 1 --branch v2.5.0 https://github.com/uclouvain/openjpeg
cmake -S openjpeg -B openjpeg/build -DCMAKE_BUILD_TYPE=Release
sudo cmake --build openjpeg/build --target install
```

The generators in `scripts/` check the version (`scripts/oracle_tools.py`)
and exit on anything but OpenJPEG 2.5.0, and, for the ICC oracles, LittleCMS
2.14. So install exactly 2.5.0. A source build must link the system
LittleCMS: install `liblcms2-dev`, and leave `-DBUILD_THIRDPARTY=ON` off,
because it bundles an older, static copy that the version check cannot
see.

### eccodes

ECMWF's GRIB decoder, for fixtures sourced from GRIB2 §5.40 (`grid_jpeg`)
messages.

```sh
# Debian/Ubuntu
sudo apt-get install libeccodes-tools   # 2.34.1 on Ubuntu 24.04

# macOS
brew install eccodes
```

**Sample-mapping gap.** A §5.40 message embeds a raw JPEG 2000 codestream. Our
decoder, and `opj_decompress` on the extracted codestream, emit the *raw integer*
samples. eccodes' `grib_get_data` instead emits the *scaled geophysical* values,
`Y = (R + X · 2^E) / 10^D`, where `X` are those integers and `R`, `E`, `D` come
from the data-representation template. So:

- For the sample-level oracle the harness compares against, extract the
  codestream and run `opj_decompress`, not `grib_get_data`.
- eccodes is a higher-level cross-check on the scaled values. Record the
  `R`/`E`/`D` scaling (`grib_dump` shows them) with the fixture so the
  integer ↔ geophysical mapping is pinned where the oracle is generated.

### cargo-deny

Runs `cargo deny check` (advisories, licenses, bans and sources), one of the
quality gates in [`conventions.md`](../.claude/rules/conventions.md).

```sh
cargo install cargo-deny
```

### cargo-fuzz (+ nightly)

libFuzzer-based fuzzing of the decoder: eight targets, from the public
`decode` entry point down to the MQ coder. It needs a nightly
toolchain, an LLVM sanitizer runtime, and a C++ compiler, and runs on x86-64 /
aarch64 Unix only. This page installs the tool; the targets live in the
detached [`fuzz/`](../fuzz/) workspace (see [`fuzz/README.md`](../fuzz/README.md)).

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
```

### Python 3

Used by `scripts/gen-oracle.sh` to turn an OpenJPEG `.pgx` decode into the
`expected.json` sample array, and by the `scripts/gen-*-fixtures.py` generators
that synthesize the fixture corpus. Standard library only, no third-party
packages. The scripts need Python 3.9 or later.

## Regenerating an oracle snapshot

[`scripts/gen-oracle.sh`](../scripts/gen-oracle.sh) decodes one codestream with
`opj_decompress` and writes the sibling `<name>.expected.json` the harness reads,
echoing the exact command it ran so the snapshot's provenance is captured.
Run it from the repository root and spell the path `tests/fixtures/...`: the
snapshot's `oracle_command` records the input path exactly as typed, so
`./tests/...` changes its bytes. It takes only a single-component codestream
without sub-sampling and refuses anything else. Pass `--tolerance <FLOAT>`
for a 9/7 codestream; the default is `exact`.

```sh
# reversible (5/3, lossless): bit-exact agreement
scripts/gen-oracle.sh tests/fixtures/jpeg2000_regular_latlon.j2k \
    --source "fieldglass jpeg2000_regular_latlon.grib2 (eccodes grid_jpeg repack of ECMWF regular_latlon_surface.grib2)"
```

The irreversible 9/7 fixture has its own script, which re-encodes the Eta
grid and calls `gen-oracle.sh` with the committed provenance:
[`scripts/gen-irreversible-fixture.sh`](../scripts/gen-irreversible-fixture.sh).

For a GRIB2-sourced fixture, extract the §5.40 codestream first with
[`scripts/extract-grib2-codestream.py`](../scripts/extract-grib2-codestream.py)
and pass that `.j2k`. Each fixture's exact extraction and generation
commands are recorded in
[`tests/fixtures/README.md`](../tests/fixtures/README.md); copy them from
there.

## Working on a decode stage

[correctness.md](correctness.md) explains why each of these checks matters.

### Build in the order the stages can be checked

Build a new stage in the order the per-stage golden tests can check it (see
Phase 1 in [roadmap.md](roadmap.md)), so every stage has a passing isolated
test before the next is wired on top.

### When the output differs from OpenJPEG

Assume first that the difference is a bug in this decoder. It almost always
is. Two lessons from the one exception, the inverse 9/7's `2/K`
([openjpeg-departures.md](openjpeg-departures.md#the-inverse-97s-high-pass-scale-2k)),
carry over to any future disagreement:

- A bit-exact match with OpenJPEG is evidence, not proof. It can mean the
  decoder reproduced a defect, so check what the standard says first.
- Emulate the oracle's convention completely or not at all. Changing `K`
  to match `two_invK` while leaving the gain in the step makes the divergence
  *worse*, because the factor of two is then counted twice. A half-emulation
  looks like a refutation of a correct hypothesis.

A change that departs from OpenJPEG needs the proof and the better result that
[the rules](correctness.md#when-the-oracle-and-the-standard-disagree) ask for,
and an entry in [openjpeg-departures.md](openjpeg-departures.md).

### Check that a fixture tests the feature

Before adding an entry to `IN_CLASS`, ask what about that entry would have to
change for the new code to be wrong. If nothing would, the entry is not the
oracle for that feature, and the milestone needs a fixture that can tell the
difference: a multi-layer codestream to separate LRCP from RLCP, multiple
precincts to separate RPCL from RLCP, and so on
([correctness.md](correctness.md#a-passing-entry-is-not-proof-the-feature-works)
has the cases that motivate this). When two orders give the same stream on a
fixture, as PCRL and CPRL do without a precinct partition, record the pair
next to the code, so that one feature's fixture is not taken for coverage of
both.

Mutating the new code and watching the entry fail is the cheapest way to check.
If no mutation makes it fail, the entry is not testing it. Run the mutation
against *every* fixture, not just the one you expect to break: the fixtures
that survive tell you which ones were never testing the feature.

### What "done" means

A stage is done when its isolated golden tests pass **and** it moves its
phase's oracle or conformance gate forward, with the quality gates
(`cargo fmt`, `clippy -D warnings`, `cargo test`, `cargo deny check`) green. Not
before. "It looks right" is not a gate.
