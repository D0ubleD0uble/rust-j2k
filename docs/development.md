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
| OpenJPEG | 2.5.0 (apt) / 2.5.4 (source) | `opj_decompress`, `opj_dump`, `opj_compress` | oracle snapshots, header checks, Tier-1 vectors |
| eccodes | 2.34.1 | `grib_dump`, `grib_get_data` | GRIB2-sourced fixtures |
| cargo-deny | current release | `cargo deny` | license/advisory quality gate |
| cargo-fuzz | current release (+ nightly) | `cargo fuzz` | robustness fuzzing |
| Python 3 | 3.8+ | `.pgx` → `expected.json` conversion | `scripts/gen-oracle.sh` |

### OpenJPEG

The reference JPEG 2000 decoder. `opj_decompress` produces the sample oracle;
`opj_dump` prints marker fields for header cross-checks (issue #5);
`opj_compress` can synthesize small codestreams for isolated Tier-1 golden
vectors (issue #10).

```sh
# Debian/Ubuntu (ships 2.5.0)
sudo apt-get install libopenjp2-tools

# macOS
brew install openjpeg
```

For a reproducible oracle independent of the distro package, build from a
pinned release tag (latest stable is `v2.5.4`):

```sh
git clone --depth 1 --branch v2.5.4 https://github.com/uclouvain/openjpeg
cmake -S openjpeg -B openjpeg/build -DCMAKE_BUILD_TYPE=Release
cmake --build openjpeg/build --target install
```

The reversible 5/3 path is bit-exact across these patch releases, so the apt
2.5.0 build is fine for lossless fixtures. Pin the source build when an
irreversible 9/7 oracle must be reproduced exactly.

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
aarch64 Unix only. This page installs the tool; the fuzz *target* is issue #18.

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
```

### Python 3

Used only by `scripts/gen-oracle.sh` to turn an OpenJPEG `.pgx` decode into the
`expected.json` sample array. Standard library only, no third-party packages.
Present by default on Linux and macOS.

## Regenerating an oracle snapshot

[`scripts/gen-oracle.sh`](../scripts/gen-oracle.sh) decodes one codestream with
`opj_decompress` and writes the sibling `<name>.expected.json` the harness reads,
echoing the exact command it ran so the snapshot's provenance is captured.

```sh
# reversible (5/3, lossless): bit-exact agreement
scripts/gen-oracle.sh tests/fixtures/sample.j2k \
    --source "fieldglass/jpeg2000_regular_latlon.grib2"

# irreversible (9/7, lossy): an absolute per-sample bound
scripts/gen-oracle.sh tests/fixtures/hrrr.j2k \
    --tolerance 2.0 --source "hrrr/sample.grib2" --notes "9/7 lossy path"
```

For a GRIB2-sourced fixture, extract the §5.40 codestream first and pass that
`.j2k` (codestream extraction is part of sourcing the fixture; see issue #4).
The committed corpus and its snapshots are added in issue #4 — this page only
sets up the tools that produce them.
