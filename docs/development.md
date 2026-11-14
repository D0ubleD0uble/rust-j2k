# Development environment

This page sets up the reference decoders and the extra tools, then shows how
to regenerate an oracle snapshot. It ends with
[the checks for work on a decode stage](#working-on-a-decode-stage).

You don't need any of these tools to run the tests. `cargo test` compares each
decode against the committed snapshots (`<name>.expected.json` and the
conformance references), and CI installs none of the reference decoders
(OpenJPEG, LittleCMS, eccodes). They are needed only to regenerate a snapshot.
See [correctness.md](correctness.md) for how the corpus is graded.

## Before you start

- Python 3.9 or later on `PATH`, as `python3`. Every script in `scripts/`
  runs it, including the install script. The install script does not install
  Python. We run the scripts on 3.12 and have not tested them on 3.9.
- rustup, for the nightly toolchain that cargo-fuzz needs.
- For the one-command install, Debian or Ubuntu with `apt-get`. On macOS the
  script prints the Homebrew commands instead of running them.
- For the ICC oracles (`scripts/gen-icc-oracles.py`), x86-64 Linux with glibc
  2.39. The script refuses any other platform, because LittleCMS's output
  follows the C library's math (see [srgb-output.md](srgb-output.md)). Its
  LittleCMS check also reads `ldd`, which macOS lacks.

## Install the tools

On Debian or Ubuntu, run the install script from the repository root:

```sh
scripts/install-oracle-tools.sh
```

It installs OpenJPEG and eccodes with `apt-get` (using `sudo` unless you are
root), installs cargo-deny, a nightly toolchain and cargo-fuzz, and checks
that each tool is on `PATH`. It then checks that OpenJPEG is 2.5.0 and, on
Linux, that LittleCMS is 2.14. When everything is in place, it ends with:

```text
install-oracle-tools: all oracle tools present
```

If this fails:

- `install-oracle-tools: some tools are still missing; see docs/development.md`.
  The lines above it mark each missing tool `MISSING`. Install those by hand
  from the sections below, then run the script again.
- `opj_decompress is OpenJPEG <version>, not 2.5.0: refusing to generate oracles
  with it (...)`, followed by `the installed OpenJPEG is missing or is not
  2.5.0`. Your distribution ships another version.
  [Build 2.5.0 from source](#build-openjpeg-250-from-source).
- `install-oracle-tools: warning: LittleCMS is not 2.14: ICC oracles
  (gen-icc-oracles.py) cannot be regenerated here`. This is a warning, and
  the script still succeeds. Only the ICC oracles need LittleCMS 2.14.
- `python3: command not found`, after the installs. Install Python 3 and run
  the script again.

## Check the minimum Rust version

The crate's minimum Rust version is 1.87 (`rust-version` in `Cargo.toml`),
and CI checks that the crate still builds on it. To check it locally:

```sh
rustup toolchain install 1.87
cargo +1.87 check
```

## Tool versions

OpenJPEG, and for the ICC oracles LittleCMS, are pinned because they decide
the snapshot bytes. The check is in
[`scripts/oracle_tools.py`](../scripts/oracle_tools.py), and every generator
runs it before it writes anything. eccodes is only a cross-check, and no
script checks its version. The cargo tools track their current release, since
they don't affect any oracle.

| Tool | Version | Provides | Used for |
| ---- | ------- | -------- | -------- |
| OpenJPEG | 2.5.0 | `opj_decompress`, `opj_dump`, `opj_compress` | oracle snapshots, header checks, Tier-1 vectors, synthetic fixtures |
| LittleCMS | 2.14 | linked by `opj_decompress` | ICC oracles (`scripts/gen-icc-oracles.py`) |
| eccodes | 2.34.1 | `grib_dump`, `grib_get_data`, `grib_set` | GRIB2-sourced fixtures |
| cargo-deny | current release | `cargo deny` | license and advisory gate |
| cargo-fuzz | current release, with nightly | `cargo fuzz` | fuzzing |
| Python 3 | 3.9 or later | `python3` | every script in `scripts/` |

### OpenJPEG

The reference JPEG 2000 decoder. `opj_decompress` produces the sample oracle,
`opj_dump` prints marker fields for header cross-checks, and `opj_compress`
encodes the codestreams behind the Tier-1 golden vectors and the synthetic
fixtures.

```sh
# Ubuntu 24.04 ships 2.5.0; other releases may not, and the scripts refuse them
sudo apt-get install libopenjp2-tools
```

On macOS, `brew install openjpeg` installs whatever version Homebrew ships
now. If that isn't 2.5.0, the scripts refuse it, and you need to build 2.5.0
from source.

#### Build OpenJPEG 2.5.0 from source

Build the pinned tag when your package manager has another version, or when
you want an oracle that doesn't depend on the distribution package. The build
needs Git, CMake, a C compiler and the LittleCMS headers:

```sh
sudo apt-get install git cmake build-essential liblcms2-dev
git clone --depth 1 --branch v2.5.0 https://github.com/uclouvain/openjpeg
cmake -S openjpeg -B openjpeg/build -DCMAKE_BUILD_TYPE=Release
cmake --build openjpeg/build
sudo cmake --install openjpeg/build
sudo ldconfig
```

We have not run this block as written. The install goes to `/usr/local`, and
`sudo ldconfig` refreshes the library cache so the new `libopenjp2` is found.
If `opj_decompress` then fails with `error while loading shared libraries:
libopenjp2.so.7`, the cache does not include `/usr/local/lib`. Check that
`ldconfig -p | grep libopenjp2` lists it.

Leave `-DBUILD_THIRDPARTY=ON` off. It links a bundled, static LittleCMS that
`ldd` cannot see, so the ICC oracle scripts stop with `opj_decompress is not
dynamically linked against liblcms2`. On macOS, install `cmake` and
`little-cms2` with Homebrew and build the same tag; we have not tested that
build. The ICC oracles still need Linux.

### eccodes

ECMWF's GRIB decoder, for fixtures taken from GRIB2 §5.40 (`grid_jpeg`)
messages.

```sh
# Debian/Ubuntu
sudo apt-get install libeccodes-tools   # 2.34.1 on Ubuntu 24.04

# macOS
brew install eccodes
```

eccodes and this decoder report different numbers for the same §5.40 message.
The message embeds a raw JPEG 2000 codestream. This decoder, and
`opj_decompress` on the extracted codestream, emit its *raw integer* samples.
eccodes' `grib_get_data` emits the *scaled geophysical* values instead,
`Y = (R + X · 2^E) / 10^D`, where `X` are those integers and `R`, `E`, `D`
come from the data-representation template. So:

- For the sample-level oracle the tests compare against, extract the
  codestream and run `opj_decompress`, not `grib_get_data`.
- Use eccodes as a higher-level cross-check on the scaled values. Record the
  `R`/`E`/`D` scaling (`grib_dump` shows it) with the fixture, so the mapping
  between integers and geophysical values is pinned where the oracle is made.

### cargo-deny

Runs `cargo deny check` (advisories, licenses, bans and sources), one of the
quality gates in [CONTRIBUTING.md](../CONTRIBUTING.md).

```sh
cargo install cargo-deny
```

### cargo-fuzz

libFuzzer-based fuzzing of the decoder: eight targets, from the public
`decode` entry point down to the MQ coder. It needs a nightly toolchain, an
LLVM sanitizer runtime and a C++ compiler, and runs on x86-64 and aarch64
Unix only. The targets live in the separate [`fuzz/`](../fuzz/) workspace,
and [`fuzz/README.md`](../fuzz/README.md) shows how to run them.

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
```

### Python 3

`scripts/gen-oracle.sh` uses it to turn an OpenJPEG `.pgx` decode into the
`expected.json` sample array, and the `scripts/gen-*.py` generators use it to
build the fixture corpus. The scripts use the standard library only.

## Regenerate an oracle snapshot

[`scripts/gen-oracle.sh`](../scripts/gen-oracle.sh) decodes one codestream
with `opj_decompress` and writes the sibling `<name>.expected.json` the tests
read. It records the exact command it ran in the snapshot's `oracle_command`.
It handles only a single-component codestream without sub-sampling. Every
other fixture has its own generator (see
[Fixtures with their own generator](#fixtures-with-their-own-generator)).

1. Run it from the repository root, and spell the path `tests/fixtures/...`.
   The snapshot records the input path exactly as typed, so `./tests/...`
   changes its bytes. Pass `--source` with the fixture's provenance, as
   recorded in its current snapshot:

   ```sh
   scripts/gen-oracle.sh tests/fixtures/jpeg2000_regular_latlon.j2k \
       --source "fieldglass jpeg2000_regular_latlon.grib2 (eccodes grid_jpeg repack of ECMWF regular_latlon_surface.grib2)"
   ```

   For a 9/7 codestream, add `--tolerance <FLOAT>`, the largest per-sample
   difference allowed; the default is `exact`. `--notes` adds a free-form
   note, and `-o <PATH>` writes somewhere other than the sibling file.

   It prints:

   ```text
   gen-oracle: running: opj_decompress -i tests/fixtures/jpeg2000_regular_latlon.j2k -o oracle.pgx

   gen-oracle: wrote tests/fixtures/jpeg2000_regular_latlon.expected.json
   ```

2. Check the result. With the pinned tools and an unchanged codestream,
   `git diff tests/fixtures/` shows no change. If you changed the codestream,
   run `cargo test` to see whether the decoder still matches.

If this fails:

- `gen-oracle: opj_decompress not found — run scripts/install-oracle-tools.sh
  (see docs/development.md)`. [Install the tools](#install-the-tools).
- `opj_decompress is OpenJPEG <version>, not 2.5.0: refusing to generate oracles
  with it (...)`. [Build 2.5.0 from source](#build-openjpeg-250-from-source).
- `gen-oracle: expected a single component, got 3 .pgx files (multi-component
  is out of subset)`, or `gen-oracle: component sub-sampling is '<dx>x<dy>', not
  1x1: the snapshot cannot describe it`. The fixture needs its own generator,
  described next.

### Fixtures with their own generator

Most of the corpus comes from other scripts in `scripts/`: the multi-component,
tiled, JP2 and ICC fixtures, among others. A synthetic fixture's snapshot
names its script in `provenance.source`, for example `synthetic; generated by
scripts/gen-tile-fixtures.py`. [`tests/fixtures/README.md`](../tests/fixtures/README.md)
describes the fixtures and the other oracle files with the scripts that make
them. Each script's header comment gives its usage, such as
`python3 scripts/gen-tile-fixtures.py`. Run it, then check the result as in
step 2.

Two of the GRIB2 fixtures need more than `gen-oracle.sh`. The 9/7 fixture is
re-encoded by
[`scripts/gen-irreversible-fixture.sh`](../scripts/gen-irreversible-fixture.sh),
which then calls `gen-oracle.sh` with the committed provenance. A fixture taken
from a GRIB2 message needs its §5.40 codestream extracted first, with
[`scripts/extract-grib2-codestream.py`](../scripts/extract-grib2-codestream.py).
[Regenerating the corpus](../tests/fixtures/README.md#regenerating-the-corpus)
has the exact commands for all four GRIB2 fixtures.

The ICC oracles (`scripts/gen-icc-oracles.py`) check more than OpenJPEG. If
they stop, the message says which check failed:

- `this is <machine> with <libc> <version>, not x86_64 with glibc 2.39:
  refusing to generate ICC oracles (see docs/srgb-output.md)`. Run it on
  x86-64 Linux with glibc 2.39.
- `LittleCMS is <version>, not 2.14: refusing to generate oracles with it`.
  Install LittleCMS 2.14 (`liblcms2-dev` on Ubuntu 24.04) and rebuild
  OpenJPEG against it.
- `opj_decompress is not dynamically linked against liblcms2 (built without
  LittleCMS, or with its bundled copy)`. Rebuild OpenJPEG without
  `-DBUILD_THIRDPARTY=ON`, with `liblcms2-dev` installed.

## Working on a decode stage

[correctness.md](correctness.md) explains why each of these checks matters.

### Build in the order the stages can be checked

Build a new stage in the order the per-stage golden tests can check it (see
[the Phase 1 plan](roadmap-phase1.md)), so every stage has a passing isolated
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

A stage is done when its isolated golden tests pass and it moves its phase's
oracle or conformance gate forward, with the quality gates (`cargo fmt`,
`clippy -D warnings`, `cargo test`, `cargo deny check`) green. Not before.
"It looks right" is not a gate.
