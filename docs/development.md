# Development environment

This page lists the developer tools the repo uses to regenerate conformance
oracles and run the extra quality gates, with a pinned version and an install
command for each.

**These tools are not needed to run the test suite.** `cargo test` compares a
decode against the committed `<name>.expected.json` snapshots, which are checked
into the repo. The reference decoders below are only needed to *(re)generate* a
snapshot. **CI does not install any of them** — the committed snapshots are the
contract, and the suite must stay green without a single oracle tool present.
See [`correctness.md`](correctness.md) for how the corpus is graded.

## One-command setup

On Debian/Ubuntu, [`scripts/install-oracle-tools.sh`](../scripts/install-oracle-tools.sh)
installs everything below and verifies it is on `PATH`. On macOS it prints the
Homebrew commands to run. The cargo/rustup tools install on any platform.

```sh
scripts/install-oracle-tools.sh
```

## The toolchain

The oracle decoders are version-pinned because they decide the snapshot bytes;
the cargo gates track their current release (they do not affect oracle output).

| Tool | Version | Provides | Used by |
| ---- | ------- | -------- | ------- |
| OpenJPEG | 2.5.0 | `opj_decompress`, `opj_dump`, `opj_compress` | oracle snapshots, header checks, Tier-1 vectors, synthetic fixtures |
| eccodes | 2.34.1 | `grib_dump`, `grib_get_data` | GRIB2-sourced fixtures |
| cargo-deny | current release | `cargo deny` | license/advisory quality gate |
| cargo-fuzz | current release (+ nightly) | `cargo fuzz` | robustness fuzzing |
| Python 3 | 3.8+ | `.pgx` → `expected.json` conversion, fixture generation | `scripts/gen-oracle.sh`, `scripts/gen-*-fixtures.py` |

### OpenJPEG

The reference JPEG 2000 decoder. `opj_decompress` produces the sample oracle;
`opj_dump` prints marker fields for header cross-checks; `opj_compress`
synthesizes the codestreams behind the isolated Tier-1 golden vectors and the
`scripts/gen-*-fixtures.py` synthetic corpus.

```sh
# Debian/Ubuntu (ships 2.5.0)
sudo apt-get install libopenjp2-tools

# macOS
brew install openjpeg   # may be newer than 2.5.0: build from source then
```

For a reproducible oracle independent of the distro package, build from a
pinned release tag:

```sh
sudo apt-get install liblcms2-dev   # LittleCMS 2.14 (Ubuntu 24.04), linked dynamically
git clone --depth 1 --branch v2.5.0 https://github.com/uclouvain/openjpeg
cmake -S openjpeg -B openjpeg/build -DCMAKE_BUILD_TYPE=Release
cmake --build openjpeg/build --target install
```

The generators in `scripts/` check the version (`scripts/oracle_tools.py`) and
exit on anything but OpenJPEG 2.5.0 (and, for the ICC oracles, LittleCMS 2.14),
so install exactly 2.5.0. To build it from source, use the `v2.5.0` tag.
A source build must link the system LittleCMS (`liblcms2-dev` installed, and
`-DBUILD_THIRDPARTY=ON` left off, which would bundle an older, static copy that
the version check cannot see).

### eccodes

ECMWF's GRIB decoder, for fixtures sourced from GRIB2 §5.40 (`grid_jpeg`)
messages.

```sh
# Debian/Ubuntu
sudo apt-get install libeccodes-tools   # 2.34.1

# macOS
brew install eccodes
```

**Sample-mapping gap.** A §5.40 message embeds a raw JPEG 2000 codestream. Our
decoder, and `opj_decompress` on the extracted codestream, emit the *raw integer*
samples. eccodes' `grib_get_data` instead emits the *scaled geophysical* values,
`Y = (R + X · 2^E) / 10^D`, where `X` are those integers and `R`, `E`, `D` come
from the data-representation template. So:

- For the sample-level oracle the harness compares against, extract the
  codestream and run `opj_decompress` — not `grib_get_data`.
- eccodes is a higher-level cross-check on the scaled values. Record the
  `R`/`E`/`D` scaling (`grib_dump` shows them) with the fixture so the
  integer ↔ geophysical mapping is pinned where the oracle is generated.

### cargo-deny

Runs the license and advisory gate from
[`conventions.md`](../.claude/rules/conventions.md).

```sh
cargo install cargo-deny
```

### cargo-fuzz (+ nightly)

libFuzzer-based fuzzing of the `decode` entry point. It needs a nightly
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
packages. Present by default on Linux and macOS.

## Regenerating an oracle snapshot

[`scripts/gen-oracle.sh`](../scripts/gen-oracle.sh) decodes one codestream with
`opj_decompress` and writes the sibling `<name>.expected.json` the harness reads,
echoing the exact command it ran so the snapshot's provenance is captured.
Run it from the repository root and spell the path `tests/fixtures/...`: the
snapshot's `oracle_command` records the input path exactly as typed, so
`./tests/...` changes its bytes.

```sh
# reversible (5/3, lossless): bit-exact agreement
scripts/gen-oracle.sh tests/fixtures/jpeg2000_regular_latlon.j2k \
    --source "fieldglass/jpeg2000_regular_latlon.grib2"
```

The irreversible 9/7 fixture has its own script, which re-encodes the Eta
grid and calls `gen-oracle.sh` with the committed provenance:
[`scripts/gen-irreversible-fixture.sh`](../scripts/gen-irreversible-fixture.sh).

For a GRIB2-sourced fixture, extract the §5.40 codestream first with
[`scripts/extract-grib2-codestream.py`](../scripts/extract-grib2-codestream.py)
and pass that `.j2k`. The committed corpus and its snapshots live under
`tests/fixtures/` — this page only sets up the tools that (re)generate them.
