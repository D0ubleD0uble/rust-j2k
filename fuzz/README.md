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
  fuzz_targets/decode.rs        # the public entry point: rust_j2k::decode(&[u8])
  fuzz_targets/tile_body.rs     # a pinned valid header (geometry steered) around fuzzed tile-part bytes
  fuzz_targets/tier1_block.rs   # one EBCOT code-block, shape and bytes fuzzed
  fuzz_targets/mq.rs            # the raw MQ arithmetic decoder
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

# Deeper periodic run over the widened surface; expected to find nothing.
cargo +nightly fuzz run decode -- -dict=fuzz/j2k.dict -max_total_time=3600 -rss_limit_mb=4096
cargo +nightly fuzz run tile_body -- -dict=fuzz/j2k.dict -max_total_time=1800 -rss_limit_mb=4096
```

A crash is written to `fuzz/artifacts/decode/`. Reproduce and minimize it with:

```sh
cargo +nightly fuzz run decode fuzz/artifacts/decode/crash-<hash>
cargo +nightly fuzz tmin decode fuzz/artifacts/decode/crash-<hash>
```

`corpus/`, `artifacts/`, `coverage/`, and `target/` are git-ignored; only the
target and its manifest are tracked, so the corpus stays reproducible from the
committed fixtures.
