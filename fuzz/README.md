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
  fuzz_targets/decode_partial.rs # decode_with, strict off, whole and cut, against the strict decode
  common/partial.rs             # the strict-off check decode_partial and tile_body share
  fuzz_targets/incremental_decode.rs # IncrementalDecoder fed in pieces, each refine against decode_with, strict off
  fuzz_targets/tile_body.rs     # a pinned valid header (geometry steered) around fuzzed tile-part bytes
  fuzz_targets/tier1_block.rs   # one EBCOT code-block, shape and bytes fuzzed
  fuzz_targets/ht_block.rs      # one HT code-block: shape to 4096 samples (13 header bytes), passes, causal flag, segments
  fuzz_targets/mq.rs            # the raw MQ arithmetic decoder
  fuzz_targets/jp2_boxes.rs     # fuzzed JP2 boxes around a pinned valid codestream, also decoded as sRGB
  fuzz_targets/header.rs        # rust_j2k::read_header_with, checked against decode_with
  fuzz_targets/icc_srgb.rs      # a fuzzed ICC profile under ColorOutput::Srgb
  fuzz_targets/window_crop.rs   # a window, tile or component subset against the crop of the whole decode
  fuzz_targets/retry_ladder.rs  # DecodeOptions::with_retry followed from each limit refusal
  icc.dict                      # ICC tag and type signatures for icc_srgb
  ../tests/support/synth.rs     # std-only seg/bx builders; tile_body, jp2_boxes and icc_srgb include it by #[path]
  ../tests/support/tile_body.rs # tile_body's header and body wrapper; tests/tile_body_variants.rs pins its variant bytes
```

`decode` must mutate its way through a byte-perfect main header before anything
reaches tier-2 or tier-1, so most of its executions die in `parse_main_header`.
The other three start deeper: `tile_body` wraps the input in a valid main
header so every execution reaches the packet reader, and `tier1_block`/`mq`
call hidden hooks the library exposes (`rust_j2k::fuzz`, `#[doc(hidden)]` and
not public API) to drive the EBCOT block decoder and MQ coder directly.

`tile_body`'s second input byte steers a region of interest: with bit 7 set
it writes an `RGN` marker (a maxshift of 0 to 19 from the low 5 bits, on
component 0 only when bit 5 is set, else on every component), so the packets
are read under a shifted bit-plane count; with bit 7 clear it writes none, and
the HT variants never write one. The body starts after both bytes.

`tile_body`'s first input byte also steers the codestream shape, so one run
reaches the widened tier-2 paths, not only the single-component single-layer
default: three components with the color transform, three quality layers, an
explicit precinct partition, the bypass (lazy) code-block style combined with
the other style flags, fuzzed POC progression volumes over the widened
geometry, a 2×2 tile grid with interleaved tile-parts, and PPM/PPT packed
packet headers stitched from out-of-order marker segments. The bypass shape is
deliberately a `tile_body` variant rather than a `rust_j2k::fuzz` hook: the
lazy `10, 2, 1, 2, 1, …` codeword-segment split belongs to tier-2, so setting
the style byte in the pinned COD drives `decode_block` through the same split
production decodes use instead of a hand-built one. For the same reason
`tier1_block` leaves the bypass style out (`src/fuzz.rs`), so the bypass
code paths in `decode_block` (`terminated`, `raw_from`) are reached only
through `tile_body`.

`jp2_boxes` does for the JP2 file format what `tile_body` does for tile
data: the codestream is pinned (one of eight small committed fixtures) and the
input becomes the boxes around it, so every execution reaches the box reader.
Its first byte picks how the `jp2c` box is written (32-bit length, 64-bit
`XLBox`, length 0, or followed by more boxes), whether the signature,
`ftyp`, and a matching `ihdr` are pinned, so mutations can land on the whole
top level or only on the `jp2h` children, which codestream, and whether every
decode is one resolution level down. `jp2.dict` holds the signature and every
box type.

Every input is decoded with `raw_channels`, and a file that decodes as coded
is decoded again with `ColorOutput::Srgb`, which reads the `colr` ICC
profile and converts sYCC. Without a `pclr`, `cmap` or `cdef` the
`raw_channels` image must equal the default one, or the raw decode may fail
only on the boxes after the codestream (a `cmap`-less `pclr` that the
default decode drops, seed `pclr_no_cmap_twice`); otherwise the results are
only checked to be `Ok` or a typed error. The codestreams are chosen so each
conversion path is reachable: an 8-bit single-component one (the monochrome
profile, `kTRC`), an 8-bit three-component one without sub-sampling (the RGB
profile, `rXYZ`, `rTRC`, ... with `curv` and `para` curves), a 16-bit
four-component one (the RGB profile at 16 bits, and the four-channel `cdef`
seeds), and the sYCC 4:2:0 and 4:2:2 codestreams of `jp2_sycc_*_odd.jp2` and
the 4:2:0 one of `jp2_sycc_420_even.jp2` (chroma replication at both origin
parities). The RGB profile on the three-component codestream sub-sampled 2x2
in every component (`icc_rgb_subsampled`) converts, since its channels
agree. `gen-jp2-fuzz-seeds.py` adds one seed for each refusal and
pass-through of `src/jp2/srgb.rs` a pinned codestream reaches: a `pclr`
gives channels of mixed depth, sign or sub-sampling, a `cdef` too few
colors, and a profile built with one header field, tag or curve changed
reaches each of the profile reader's refusals. Not reached: the sample
budget refusals (the pinned images are tiny), an even-origin 4:2:2
conversion (no pinned codestream; `tests/jp2_sycc_replication.rs` grades one
against OpenJPEG). The RGB profile's `uniform` refusal after the "AFL test"
needs a `cdef` that moves a differing color past the first four channels
(`icc_rgb_uniform_after_afl`). `tests/fuzz_seed_outcomes.rs` runs the script
into a scratch directory and checks that every seed decodes as coded and that
its `ColorOutput::Srgb` outcome matches `tests/fixtures/jp2_fuzz_seed_outcomes.tsv`
(130 convert or pass through, 30 are refused as `Unsupported`; regenerate the
table with `J2K_BLESS_SEED_OUTCOMES=1`). The CI smoke generates these seeds before it
runs `jp2_boxes` for 30 s, and seeds `header` from the committed fixtures.
`gen-jp2-fuzz-seeds.py` builds the matching ICC and sYCC `colr` seeds, and
seeds with `jp2i` and `uinf` boxes before and after the codestream.

`header` drives the header-only entry point, `read_header_with`, and checks it
against `decode_with` on the same input, under five fixed option sets
(defaults, `raw_channels`, reduction 1, sRGB, reduction 1 with sRGB). A raw
codestream leaves out `raw_channels`, which repeats the defaults there. The
header is read under every set; a header that reads is compared with a full
decode under one of the sets that read, picked by a hash of the whole input,
since a decode costs far more than the header: a
`read_header_with` error must be `decode_with`'s error too, and a successful
decode must match the header field for field.
A decode costs far more than the header, so once the header reads, inputs
over 64 KiB or images over 2^16 samples are not decoded. For those inputs the
target checks `read_header` alone: it does not compare a successful header
with a successful decode field by field. Seed it with the
small synthetic codestreams and JP2 files, and give it the JP2 box
dictionary.

`icc_srgb` pins a small codestream (RGB at 8 or 16 bits, or gray, by the
first input byte) and its boxes, and the rest of the input is the ICC profile
in the `colr` box. Every execution reads the profile with
`rust_j2k::fuzz::icc_profile`. Building the transform to sRGB costs
milliseconds, far more than reading, so only one profile in 16, chosen by a
hash of the profile, is also decoded with `ColorOutput::Srgb`, which builds
and runs the LittleCMS port. On the old corpus that gate took the target from
about 21 to about 560 exec/s (the gate's speeds, not this smoke run's); the 696
seeds of the current corpus take about 8 s to initialize on CI. Seed it with
the profiles in `tests/fixtures/icc/profiles.json`;
`gen-icc-oracles.py --fuzz-seeds` also writes, for each distinct codestream
and profile pair with a profile of 128 bytes or more, a `*_build` copy with a
2-byte nonce in the reserved header bytes 100 to 101 that makes the hash
select it (the hash skips the first byte, so each codestream needs its own
copy). So every such profile is selected and decoded with
`ColorOutput::Srgb` under each codestream kind, and the profiles the
converter accepts reach the transform builders: 82 of 160 for RGB 8-bit, 82
of 160 for RGB 16-bit and 15 of 27 for gray. The script exits if the target's
`BUILD_EVERY` or hash constants differ from its own, or if the target no
longer has the lines `let Some((&variant, profile)) = data.split_first()` and
`if fnv1a(profile) % BUILD_EVERY != 0 {`.

`decode_partial` decodes each input with `strict` off, whole and cut at a
point a hash of the input picks, at reduction 0 or 1. `common/partial.rs`
checks each result against the strict decode of the same bytes: when the
strict decode gives an image, the decode with `strict` off gives an equal
`Image`, report included: `DecodeReport::default()`, at any reduction. Every report must
stay within its bound (five entries per `SOT` marker code in the input, plus
two), a tile with leftover bytes must leave `complete_reduction` at `None`,
and a complete report must hold no missing tiles. `tile_body` makes the same
check on each of its inputs, so the steered shapes (POC, packed headers,
tile-parts) reach the strict-off walk too. Seed `decode_partial` as
`decode`.

`incremental_decode` feeds each input to an `IncrementalDecoder` in pieces
whose sizes a hash of the file picks, and refines after each piece. The first
six input bytes steer the options, so a small mutation of the file keeps them:
a layer cap, a resolution reduction, a window, a tile, a component subset of
one or two distinct indices, and a code-block work limit. The rest is the
file, so seed it with six zero bytes in front (the default limit, no other
option), or with a plain fixture: an input that starts with `FF 4F` or the JP2
signature box is the whole file with all-zero control bytes. Each
refine that decodes goes on from the packet walks the earlier ones kept, and
must give what `decode_with` gives with `strict` off on the bytes fed so far:
the same image or the same error. After an error, each later refine must give
it again, and `finish` must be the decode of the whole input. The decoder holds
all its refines to the code-block limit together, and a one-shot decode holds
only itself. So each refine that was due is checked by the decoder's own rule:
it refuses (`Ok(None)`) if and only if the code-block work a decoder with
nothing spent spends on the same bytes passes what is left of the bound after
the work spent before it. The limit is at least 2^26 sample passes, which
inputs of a few kilobytes do not come near, so each due refine also starts a
probe decoder with work already spent (`rust_j2k::fuzz`), drawn around the most
that leaves the refine room, and checks it by the same rule. Each input is
decoded several times, so
seed it with small files, such as the fixtures under 3 KB, and cap the input
length (`-max_len=4096`). Even so it can be slow, since almost all its time
goes to its own `decode_with` oracle decodes: from the fixtures under 3 KB
it runs about 230 inputs a second, but on a corpus grown by earlier runs it
has dropped to about 7 a second under ASan, and one 598-byte input took 13
s. The name `incremental` is one Cargo reserves.

`window_crop` checks a decode of part of the image against the whole
decode. A hash of the input picks a resolution reduction (0 to 2) and one of
four modes: a window, a tile, a component subset (decoded with
`raw_channels`, as a subset names codestream components), or a subset
through a window. The window and the tile are worked out on the reference
grid from the first `SOC SIZ` in the input. That `SIZ` may not be the
codestream's, since a JP2 file can hold those bytes in another box, so an
input is skipped unless its image area gives the whole decode's component
sizes. When the whole decode gives an image, the part must give one too,
unless its options are `InvalidOptions` (an area with no sample of a
component at the reduction, or a component past the image's), and each of
its components must equal the whole decode's, cut to the area. Seed it as
`decode`, with the window fixtures added.

`retry_ladder` follows `DecodeOptions::with_retry` from each limit refusal.
The first input byte picks where each of the three limits starts
(`Default`, or a factor of 1 or 2), a reduction of 0 or 1, and `strict`;
the rest is the file. An input that starts with `FF 4F` or the JP2
signature box is the whole file with a zero control byte: the default
limits, reduction 0 and `strict` on. Each step must
raise the option the refusal names to `retry_with`, strictly above where it
was, and leave every other option as it was; a limit no option raises gives
no retry. Raising a limit never turns an image into an error: when the
default options decode, the ladder ends at an image with their samples, and
otherwise at the image of a decode with no limits. A decode past a factor of
4 can take longer than the timeout, so the ladder stops when a refusal asks
for more, after that step is checked; more than 8 steps is a failure. An
input over the fuzz sample budget (`common/budget.rs`) is only read for its
header, and a sample limit refusal there is checked as one step with no
decode. Seed it as `decode`, with the work-bound and sample-limit fixtures
that the default limits refuse in under a second added: a packet work
refusal that asks for a factor of 3 and one that asks for 8, a code-block
work refusal that a factor of 2 admits, and a sample limit refusal of the
header. The slower work-bound fixtures take 9 to 26 s each under the
sanitizer build, so they are left out. `t1bomb_1024_64_88` takes about 7 s,
over its default decode, the factor of 2 and the decode with no limits.

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

# Time-boxed run (CI-friendly: no panics, no OOM, no hangs). A small input
# may declare an image at the 2^29-sample default limit, which would peak
# at about 2.7 GiB for a square tile and up to about 6.4 GiB for a tile one
# sample wide. The `decode`, `decode_partial` and `incremental_decode` targets
# skip an input that declares more than 2^28 samples (`common/budget.rs`), so
# what they decode peaks at about half that, and 4 GiB is enough, as CI gives.
cargo +nightly fuzz run decode -- -dict=fuzz/j2k.dict -max_total_time=300 -rss_limit_mb=4096

# The same with strict off, against the strict decode.
mkdir -p fuzz/corpus/decode_partial
cp fuzz/corpus/decode/* fuzz/corpus/decode_partial/
cargo +nightly fuzz run decode_partial -- -dict=fuzz/j2k.dict -max_total_time=300 -rss_limit_mb=4096

# The JP2 box target, seeded from the ISO/IEC 15444-4 JP2 reader files and
# the synthetic JP2 fixtures.
python3 scripts/gen-jp2-fuzz-seeds.py
cargo +nightly fuzz run jp2_boxes -- -dict=fuzz/jp2.dict -max_total_time=300

# The header target, seeded with the small synthetic codestreams and JP2 files.
mkdir -p fuzz/corpus/header
cp tests/fixtures/*.j2k tests/fixtures/jp2_*.jp2 fuzz/corpus/header/
cargo +nightly fuzz run header -- -dict=fuzz/jp2.dict -max_total_time=300

# The ICC target, seeded with the profiles of the ICC oracle cases (the
# first byte picks the codestream: 0 RGB 8-bit, 1 RGB 16-bit, 2 gray). The CI
# smoke runs it for 30 s with the same seeds.
python3 scripts/gen-icc-oracles.py --fuzz-seeds
cargo +nightly fuzz run icc_srgb -- -dict=fuzz/icc.dict -max_total_time=300

# The HT block target, seeded with the code-blocks of the HT corpus (3714
# seeds from six conformance codestreams). `-max_len` lets the cleanup and
# refinement segments run to the 5000 bytes each the hook allows.
RUST_J2K_HT_SEED_DIR=fuzz/corpus/ht_block cargo test --lib ht_block_seeds
cargo +nightly fuzz run ht_block -- -max_total_time=60 -max_len=8192 -rss_limit_mb=4096

# `decode` and `decode_partial` seeded with small HT files: the OpenJPH
# codestreams and the HT conformance codestreams under 6 KB, and the smallest
# MIXED file (`ds0_hm_15_b8.j2k`, 17 KB; the other two are larger). `tile_body`
# needs no seed: its header is pinned, bit 7 of its first byte makes the body's
# code-blocks HT, and bit 6 with it makes them MIXED.
for t in decode decode_partial; do
  mkdir -p fuzz/corpus/$t
  cp tests/fixtures/*.j2k tests/fixtures/openjph/files/*.j2c fuzz/corpus/$t/
  find tests/fixtures/conformance/ht/codestreams -size -6k -name '*.j2k' -exec cp {} fuzz/corpus/$t/ \;
  cp tests/fixtures/conformance/ht/codestreams/ds0_hm_15_b8.j2k fuzz/corpus/$t/
done
cargo +nightly fuzz run decode -- -dict=fuzz/j2k.dict -max_total_time=60 -rss_limit_mb=4096

# The incremental target, seeded with the fixtures under 3 KB.
mkdir -p fuzz/corpus/incremental_decode
find tests/fixtures -maxdepth 1 -size -3k \( -name '*.j2k' -o -name '*.jp2' \) \
  -exec cp {} fuzz/corpus/incremental_decode/ \;
cargo +nightly fuzz run incremental_decode -- -dict=fuzz/j2k.dict -max_len=4096 -max_total_time=300 -rss_limit_mb=4096

# The window target, seeded as decode plus the window fixtures.
mkdir -p fuzz/corpus/window_crop
cp fuzz/corpus/decode/* tests/fixtures/window/*.j2k fuzz/corpus/window_crop/
cargo +nightly fuzz run window_crop -- -dict=fuzz/j2k.dict -max_total_time=60 -rss_limit_mb=4096 -timeout=30

# The retry target, seeded as decode plus the quick files the limits refuse.
mkdir -p fuzz/corpus/retry_ladder
cp fuzz/corpus/decode/* fuzz/corpus/retry_ladder/
for f in work_bound/pocspent_2tiles_60000_559 work_bound/pocvol_1024x512_1000_2 \
  work_bound/t2bomb_2048x1024_1000 tier1_work_bound/t1bomb_1024_64_88 \
  tier1_work_bound/htbomb_1024_64_3 sample_limit/flat_32769x16385_tiled_lossless; do
  cp tests/fixtures/$f.j2k fuzz/corpus/retry_ladder/
done
cargo +nightly fuzz run retry_ladder -- -dict=fuzz/j2k.dict -max_total_time=60 -rss_limit_mb=4096 -timeout=30

# Deeper periodic run over the widened surface; expected to find nothing.
cargo +nightly fuzz run decode -- -dict=fuzz/j2k.dict -max_total_time=3600 -rss_limit_mb=4096
cargo +nightly fuzz run tile_body -- -dict=fuzz/j2k.dict -max_total_time=1800 -rss_limit_mb=4096
```

## Recorded runs

| Date | Target | Seeds | Length | Result |
| ---- | ------ | ----- | ------ | ------ |
| 2026-11-01 | `jp2_boxes` | `gen-jp2-fuzz-seeds.py` (46 seeds), `jp2.dict` | 61 s, 785 605 runs, under `nice` | no crash, leak, or timeout |
| 2026-11-01 | `jp2_boxes` with the `Srgb` decode | `gen-jp2-fuzz-seeds.py` (72 seeds), `jp2.dict` | 61 s, 86 856 runs (1 423 exec/s), under `nice` | no crash, leak, or timeout |
| 2026-11-01 | `jp2_boxes` with `jp2i` and `uinf` seeds | `gen-jp2-fuzz-seeds.py` (75 seeds), `jp2.dict` | 61 s, 35 821 runs, under `nice` | no crash, leak, or timeout |
| 2026-11-10 | `jp2_boxes` with the `raw_channels` decode, reduction (bit 7), and the `srgb.rs` branch seeds | `gen-jp2-fuzz-seeds.py` (154 seeds; the script now writes 160), `jp2.dict` | 61 s, 24 058 runs (394 exec/s), `nice -n 19`, one job | no crash, leak, or timeout |
| 2026-11-01 | `header` | `tests/fixtures/*.j2k` and `jp2_*.jp2` (53 seeds), `jp2.dict` | 61 s, 9 928 runs, under `nice` | no crash, leak, or timeout |
| 2026-11-01 | `icc_srgb` | `gen-icc-oracles.py --fuzz-seeds` (136 seeds), `icc.dict` | 61 s, 11 137 runs, under `nice` | no crash, leak, or timeout |
| 2026-11-11 | `icc_srgb`, profile read on every input and the transform built for one in 16 | `gen-icc-oracles.py --fuzz-seeds` (349 seeds and 185 `_build` copies), `icc.dict` | 31 s, 17 471 runs (563 exec/s, coverage 3344; `main` on its 349 seeds: 21 exec/s, coverage 3278), `nice -n 19`, one job | no crash, leak, or timeout |
| 2026-11-12 | the pre-release smoke: `decode`, `tile_body`, `tier1_block`, `mq`, `jp2_boxes`, `header`, `icc_srgb`, one after another with the CI step's options | the CI seeding (`gen-jp2-fuzz-seeds.py`, 160 seeds; the fixtures for `header`; `gen-icc-oracles.py --fuzz-seeds`) | 61, 46, 31, 16, 31, 31 and 31 s; 4 433 646, 182 668, 19 952, 242 767, 14 211, 9 216 and 1 620 runs; `nice -n 19`, one job | no crash, leak, or timeout |
| 2026-11-13 | `decode_partial`, and `tile_body` with the strict-off check | `decode`'s seeding (923 inputs after earlier runs), and `tile_body`'s corpus (1 478 inputs), `j2k.dict` not passed | 61 s each; 7 256 and 27 382 runs; `nice`, one job | no crash, leak, or timeout. An earlier `decode_partial` run saved a slow unit: a 1 MiB codestream of a 2048 x 2048 image whose tile 37 is cut, which the partial decode reads in 0.54 s (release) and the instrumented build in 12 s over its four decodes |
| 2026-11-13 | `decode_partial` after the review fixes (packed headers, `Image` equality) | the same corpus | 61 s, 6 042 runs; `nice`, one job | no crash, leak, or timeout |
| 2026-11-15 | `ht_block` (new) | the code-blocks of six HT conformance codestreams (3466 seeds) | 61 s, 739 458 runs; `nice -n 19`, one job, `-max_len=8192`; the 1-minute load was 10.6 at the start | no crash, leak, or timeout |
| 2026-11-16 | the pre-release smoke: all ten targets, one after another: `decode`, `decode_partial`, `incremental_decode`, `header`, `tile_body`, `tier1_block`, `ht_block`, `mq`, `jp2_boxes`, `icc_srgb` | the fixtures, OpenJPH files and HT files under 6 KB for `decode` and `decode_partial` (68 seeds); the fixtures under 3 KB for `incremental_decode` (49, `-max_len=4096`); the fixtures for `header` (68); `ht_block_seeds` (3 719, `-max_len=8192`); `gen-jp2-fuzz-seeds.py` (162); `gen-icc-oracles.py --fuzz-seeds` (696); none for the other three | 61 s each; 18 279, 10 017, 13 826, 42 007, 69 498, 94 377, 724 122, 3 276 003, 34 884 and 40 010 runs; `nice -n 19`, one job; the 1-minute load was 4.8 to 10.2 | no crash, leak, or timeout |
| 2026-11-15 | `decode` with the strict-off capped decode | `tests/fixtures/*.j2k` (49 seeds), `j2k.dict` | 61 s, 21 641 runs; `nice -n 19`, one job; the 1-minute load was 7.6 | no crash, leak, or timeout |
| 2026-11-15 | `decode` with the strict-off capped decode checked against the strict one (`common/partial.rs`), and the HT seeds | the 49 fixtures, the OpenJPH codestreams and the HT conformance codestreams under 6 KB, and what earlier runs kept (1197 inputs), `j2k.dict` with `CPF` and the HT style byte | 61 s, 24 746 runs; `nice -n 19`, one job; the 1-minute load was 5.8 | no crash, leak, or timeout |
| 2026-11-15 | `tile_body` with the MIXED variant (bit 7 and bit 6 of the first byte) | one MIXED seed, `j2k.dict` | 56 s, 64 703 runs; `nice -n 19`, one job; the 1-minute load was 8.5; 281 of 1 556 corpus inputs pick MIXED | no crash, leak, or timeout |

A crash is written to `fuzz/artifacts/decode/`. Reproduce and minimize it with:

```sh
cargo +nightly fuzz run decode fuzz/artifacts/decode/crash-<hash>
cargo +nightly fuzz tmin decode fuzz/artifacts/decode/crash-<hash>
```

`corpus/`, `artifacts/`, `coverage/`, and `target/` are git-ignored; only the
target and its manifest are tracked, so the corpus stays reproducible from the
committed fixtures. No seed corpus is committed: the commands above, and the CI
smoke, build each one. The CI smoke (`.github/workflows/ci.yml`) seeds `decode`
with the fixtures and small HT files, `decode_partial` and `incremental_decode`
from the `decode` corpus, `ht_block` with the HT corpus's code-blocks, and the
JP2, header and ICC targets as above. It builds every target and runs ten
of them for 15 to 60 s each, with `-timeout=30` and `-rss_limit_mb=4096`;
it does not run `window_crop` or `retry_ladder`.
