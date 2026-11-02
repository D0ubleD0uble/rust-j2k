# Fuzzing the decoder

`decode` parses bytes it did not produce, so malformed input is a first-class
case (see [`docs/correctness.md`](../docs/correctness.md) → Robustness). This
crate stands up [`cargo fuzz`](https://github.com/rust-fuzz/cargo-fuzz)
(libFuzzer) targets over the decoder, from the public entry point down to the
deep state machines. The bar:

- no panics, aborts, or out-of-bounds access;
- no unbounded allocation (a malformed SIZ cannot steer the buffers into an
  overflowing or out-of-memory allocation — `Xsiz * Ysiz` is bounded at parse
  time);
- no infinite loops;
- every rejected input returns a typed [`rust_j2k::Error`], never a crash.

## Layout

This is a **detached workspace** (note the empty `[workspace]` table in
`Cargo.toml`), so `libfuzzer-sys` never enters the library's dependency graph:
`cargo build`, `cargo test`, `cargo clippy --all-targets`, and `cargo deny
check` at the repo root do not see it, and the no-C-dependency / clean
cross-compile guarantee for `rust-j2k` is preserved. `libfuzzer-sys` links
LLVM's libFuzzer at build time through the nightly compiler's sanitizer
support; it is a build/test tool here, not a runtime dependency of the codec.

```
fuzz/
  Cargo.toml                    # detached crate, depends on libfuzzer-sys + rust-j2k
  j2k.dict                      # J2K marker dictionary for the codestream-reading targets
  jp2.dict                      # JP2 box-type dictionary for jp2_boxes
  fuzz_targets/decode.rs        # the public entry point: rust_j2k::decode(&[u8])
  fuzz_targets/tile_body.rs     # a pinned valid header (geometry steered) around fuzzed tile-part bytes
  fuzz_targets/tier1_block.rs   # one EBCOT code-block, shape and bytes fuzzed
  fuzz_targets/mq.rs            # the raw MQ arithmetic decoder
  fuzz_targets/jp2_boxes.rs     # fuzzed JP2 boxes around a pinned valid codestream, also decoded as sRGB
  fuzz_targets/header.rs        # rust_j2k::read_header_with, checked against decode_with
  fuzz_targets/icc_srgb.rs      # a fuzzed ICC profile under ColourOutput::Srgb
  icc.dict                      # ICC tag and type signatures for icc_srgb
```

`decode` must mutate its way through a byte-perfect main header before anything
reaches tier-2 or tier-1, so most of its executions die in `parse_main_header`.
The other three start deeper: `tile_body` wraps the input in a valid main
header so every execution reaches the packet reader, and `tier1_block`/`mq`
call hidden hooks the library exposes (`rust_j2k::fuzz`, `#[doc(hidden)]` and
not public API) to drive the EBCOT block decoder and MQ coder directly.

`tile_body`'s first input byte also steers the codestream shape, so one run
reaches the widened tier-2 paths, not only the single-component single-layer
default: three components with the colour transform, three quality layers, an
explicit precinct partition, the bypass (lazy) code-block style combined with
the other style flags, fuzzed POC progression volumes over the widened
geometry, a 2×2 tile grid with interleaved tile-parts, and PPM/PPT packed
packet headers stitched from out-of-order marker segments. The bypass shape is
deliberately a `tile_body` variant rather than a `rust_j2k::fuzz` hook: the
lazy `10, 2, 1, 2, 1, …` codeword-segment split belongs to tier-2, so setting
the style byte in the pinned COD drives `decode_block` through the same split
production decodes use instead of a hand-built one.

`jp2_boxes` does for the JP2 file format what `tile_body` does for tile
data: the codestream is pinned (one of six small committed fixtures) and the
input becomes the boxes around it, so every execution reaches the box reader.
Its first byte picks how the `jp2c` box is written (32-bit length, 64-bit
`XLBox`, length 0, or followed by more boxes), whether the signature,
`ftyp`, and a matching `ihdr` are pinned, so mutations can land on the whole
top level or only on the `jp2h` children, and which codestream. `jp2.dict`
holds the signature and every box type.

A file that decodes as coded is decoded again with `ColourOutput::Srgb`, which
reads the `colr` ICC profile and converts sYCC. The result is only checked to
be `Ok` or a typed error. The codestreams are chosen so each conversion path
is reachable: an 8-bit single-component one (the monochrome profile, `kTRC`),
an 8-bit three-component one without sub-sampling (the RGB profile, `rXYZ`,
`rTRC`, ... with `curv` and `para` curves), the sYCC 4:2:0 and 4:2:2
codestreams of `jp2_sycc_*_odd.jp2` (chroma replication). The RGB profile
on the three-component codestream sub-sampled 2x2 in every component
(`icc_rgb_subsampled`) converts, since its channels agree. Three seeds are
refused: the 16-bit one-component codestream (monochrome profiles need 8
bits) and an RGB profile on the 4:2:0 codestream (mixed sub-sampling) after
the profile is read, and sYCC on the all-2x2 codestream
(`sycc_subsampled_all`, sYCC wants luma at full size), which has no profile
and is refused from the sampling alone. The CI smoke generates these seeds before it runs `jp2_boxes` for 30 s, and
seeds `header` from the committed fixtures.
`gen-jp2-fuzz-seeds.py` builds the matching ICC and sYCC `colr` seeds.

`header` drives the header-only entry point, `read_header_with`, and checks it
against `decode_with` on the same input, under five fixed option sets
(defaults, `raw_channels`, reduction 1, sRGB, reduction 1 with sRGB): a
`read_header_with` error must be `decode_with`'s error too, and a successful
decode must match the header field for field.
A decode costs far more than the header, so once the header reads, inputs
over 64 KiB or images over 2^16 samples are not decoded. For those inputs the
target checks `read_header` alone: it does not compare a successful header
with a successful decode field by field. Seed it with the
small synthetic codestreams and JP2 files, and give it the JP2 box
dictionary.

`icc_srgb` pins a small codestream (RGB at 8 or 16 bits, or grey, by the
first input byte) and its boxes, and the rest of the input is the ICC profile
in the `colr` box, decoded with `ColourOutput::Srgb`. Every execution reaches
the profile parser, and an accepted profile builds and runs the LittleCMS
port. Seed it with the profiles in `tests/fixtures/icc/profiles.json`.

`j2k.dict` is a libFuzzer dictionary of the marker codes (plus an SOP and an
SOT composite), for the targets that read codestream structure (`decode`,
`tile_body`): mutation almost never synthesizes a two-byte marker code, and
with the dictionary the fuzzer splices one in a single step.

## Running

Requires the nightly toolchain and `cargo fuzz`:

```sh
rustup toolchain install nightly
cargo +nightly install cargo-fuzz --locked
```

Run the target. Seeding from the committed conformance fixtures gives libFuzzer
real codestreams to mutate, which reaches far more of the pipeline than random
bytes:

```sh
# Seed the corpus once. The conformance codestreams carry the widened Phase 2
# geometry — multi-tile, multi-component, multi-layer, precinct-partitioned —
# so they reach far more of tier-2 than the synthetic single-tile fixtures.
mkdir -p fuzz/corpus/decode
cp tests/fixtures/*.j2k tests/fixtures/conformance/codestreams/*.j2k fuzz/corpus/decode/

# Time-boxed run (CI-friendly: no panics, no OOM, no hangs).
cargo +nightly fuzz run decode -- -dict=fuzz/j2k.dict -max_total_time=300 -rss_limit_mb=2048

# The JP2 box target, seeded from the ISO/IEC 15444-4 JP2 reader files and
# the synthetic JP2 fixtures.
python3 scripts/gen-jp2-fuzz-seeds.py
cargo +nightly fuzz run jp2_boxes -- -dict=fuzz/jp2.dict -max_total_time=300

# The header target, seeded with the small synthetic codestreams and JP2 files.
mkdir -p fuzz/corpus/header
cp tests/fixtures/*.j2k tests/fixtures/jp2_*.jp2 fuzz/corpus/header/
cargo +nightly fuzz run header -- -dict=fuzz/jp2.dict -max_total_time=300

# The ICC target, seeded with the profiles of the ICC oracle cases (the
# first byte picks the codestream: 0 RGB 8-bit, 1 RGB 16-bit, 2 grey).
python3 scripts/gen-icc-oracles.py --fuzz-seeds
cargo +nightly fuzz run icc_srgb -- -dict=fuzz/icc.dict -max_total_time=300

# Deeper periodic run over the widened surface; expected to find nothing.
cargo +nightly fuzz run decode -- -dict=fuzz/j2k.dict -max_total_time=3600 -rss_limit_mb=4096
cargo +nightly fuzz run tile_body -- -dict=fuzz/j2k.dict -max_total_time=1800 -rss_limit_mb=4096
```

## Recorded runs

| Date | Target | Seeds | Length | Result |
| ---- | ------ | ----- | ------ | ------ |
| 2026-11-01 | `jp2_boxes` | `gen-jp2-fuzz-seeds.py` (46 seeds), `jp2.dict` | 61 s, 785 605 runs, under `nice` | no crash, leak, or timeout |
| 2026-11-01 | `jp2_boxes` with the `Srgb` decode | `gen-jp2-fuzz-seeds.py` (72 seeds), `jp2.dict` | 61 s, 86 856 runs (1 423 exec/s), under `nice` | no crash, leak, or timeout |
| 2026-11-01 | `header` | `tests/fixtures/*.j2k` and `jp2_*.jp2` (53 seeds), `jp2.dict` | 61 s, 9 928 runs, under `nice` | no crash, leak, or timeout |
| 2026-11-01 | `icc_srgb` | `gen-icc-oracles.py --fuzz-seeds` (136 seeds), `icc.dict` | 61 s, 11 137 runs, under `nice` | no crash, leak, or timeout |

A crash is written to `fuzz/artifacts/decode/`. Reproduce and minimize it with:

```sh
cargo +nightly fuzz run decode fuzz/artifacts/decode/crash-<hash>
cargo +nightly fuzz tmin decode fuzz/artifacts/decode/crash-<hash>
```

`corpus/`, `artifacts/`, `coverage/`, and `target/` are git-ignored; only the
target and its manifest are tracked, so the corpus stays reproducible from the
committed fixtures.
