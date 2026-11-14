# Development environment

This page lists the tools that regenerate the oracle snapshots and run the
extra quality gates, with the version and an install command for each.

**You don't need these tools to run the test suite.** `cargo test` compares
each decode against the committed snapshots (`<name>.expected.json` and the
conformance references), so the reference decoders below are needed only to
regenerate a snapshot. CI installs none of them: the committed snapshots are
the contract, and the suite must pass with no oracle tool present. See
[`correctness.md`](correctness.md) for how the corpus is graded.

## One-command setup

On Debian/Ubuntu, [`scripts/install-oracle-tools.sh`](../scripts/install-oracle-tools.sh)
installs OpenJPEG, eccodes, cargo-deny, a nightly toolchain and cargo-fuzz,
checks that they are on `PATH`, and fails if OpenJPEG is not 2.5.0 (on Linux
it also checks that LittleCMS is 2.14). On macOS it prints the Homebrew
commands to run. The cargo and rustup tools install on any platform. The
script does not install Python 3.

```sh
scripts/install-oracle-tools.sh
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
