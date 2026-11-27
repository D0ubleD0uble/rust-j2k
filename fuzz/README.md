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
  fuzz_targets/retry_ladder.rs  # DecodeOptions::raised_for followed from each limit refusal
  fuzz_targets/header_split.rs  # IncrementalDecoder::header() fed in pieces, each call against read_header_with, strict off
  icc.dict                      # ICC tag and type signatures for icc_srgb
  ../tests/support/synth.rs     # std-only seg/bx builders; tile_body, jp2_boxes and icc_srgb include it by #[path]
  ../tests/support/tile_body.rs # tile_body's header and body wrapper; tests/tile_body_variants.rs pins its variant bytes
```

`decode` must mutate its way through a byte-perfect main header before anything
reaches tier-2 or tier-1, so most of its executions die in `parse_main_header`.
The other three start deeper: `tile_body` wraps the input in a valid main
header so every execution reaches the packet reader, and `tier1_block`/`mq`
call hidden hooks the library exposes (`rust_j2k::fuzz`, `#[doc(hidden)]`,
not public API, and built only with the private `__fuzz` feature, which this
workspace turns on) to drive the EBCOT block decoder and MQ coder directly.

Besides the plain decode, `decode` draws its options from a hash of the input
(`fuzz_targets/decode.rs`): a layer cap of 1 to 3, with `strict` off too, and
for one such input in four a packet work limit of `Raised(2)` or `Raised(4)` on
the decode with `strict` off only; a window, a tile or a component subset, at reduction 0 or 1; and,
for a JP2 file, `ColorOutput::Srgb` on one window or tile draw in four, which a
sub-sampled sYCC image refuses on part of the image. Seed the last with the
`jp2_sycc_*.jp2` files (below). The hash changes with any byte, so a seed
reaches the draws a mutation of it picks, not its own.

The work cap below prices code-block work only, and every decode may also
walk packets up to the packet work bound: 2^24 visits for a fuzz input, about
1 to 3 s under ASan. A geometry of many code-blocks with empty packets
reaches that bound in a few hundred bytes
(`tests/fixtures/incremental/at_most_work_bound_waits.seed`). So `decode` and
`incremental_decode` keep a packet ledger (`budget::PacketBounds` in
`common/budget.rs`): once a decode of the input is refused at the packet
bound, it is charged the factor it ran at, and a later decode runs only if
its factor fits in what is left. `decode` allows 5 bounds: the plain
decode, then the decode with `strict` off at up to `Raised(4)`; the strict
decode, which walks the same packets, is not run, and its place in the
check goes to the plain decode's refusal. `incremental_decode` allows 2: the
first due refine and one-shot decode refused there, whose wait is checked,
and the execution ends at the next decode. A decode that passes the bound
says nothing of what it spent, so it is not charged: an input whose every
decode walks just under the bound still runs each one.

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
box type. A body can carry a `jp2c` of its own, which wins over the pinned
one, so the target skips a file over its share of the sample budget
(`common/budget.rs`), as the other decoding targets do.

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
against `decode_with` on the same input, under seven fixed option sets
(defaults, `raw_channels`, reduction 1, sRGB, reduction 1 with sRGB, the
sample limit lifted, and `strict` off). A raw codestream leaves out
`raw_channels`, which repeats the defaults there. The
header is read under every set; a header that reads is compared with a full
decode under one of the sets that read, picked by a hash of the whole input,
since a decode costs far more than the header: a
`read_header_with` error must be `decode_with`'s error too, and a successful
decode must match the header field for field. The one exception is a tile
the header refuses, over a cap or for a reduction its own header has too
few levels for, where the decode first finds something only it checks: a
fault or a work bound in a lower tile's data, or, with `strict` off, the
packet work charged for tiles that have no tile-part. The target accepts that pair
only when three things hold: the decode of the refused tile alone
(`Region::Tile`), under the same options but with the color as coded (a
tile alone cannot give sRGB from sub-sampled sYCC chroma), gives exactly
the header's error,
so the refusal is real and not a header bug that a fault in another tile
hides (a tile-part `COD` that brings the tile within the caps, say); the
decode gives the same error with the header's refusal lifted (by the cap's
retry, or at reduction 0); and the refused tile decoded alone, with the
refusal lifted, does not give it. A fault in the tile-parts, which every
decode reads, still has to match, with `strict` on and off: where the
header read refuses a tile it finds that fault with a walk of its own that
keeps no tile record. Each input of 64 KiB or less also goes through
`rust_j2k::fuzz::tile_parts_walks_agree`, which checks that walk's error
against the decode's parse of the tile-parts directly, under both settings,
on the input and on up to 15 of its prefixes that end in the tile-parts, so
a whole seed also tests the walks on input that ends early.
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
two, plus one for each row of tiles a window reads), a tile with leftover bytes must leave `complete_reduction` at `None`,
and a complete report must hold no missing tiles. `tile_body` makes the same
check on each of its inputs, so the steered shapes (POC, packed headers,
tile-parts) reach the strict-off walk too. Seed `decode_partial` as
`decode`, plus the conformance codestreams of 300000 bytes or less (the
larger ones are cut to 1 MiB, and mutating those costs about a second a run)
and `tests/fixtures/partial/*.j2k`; the recipe is below.

`incremental_decode` feeds each input to an `IncrementalDecoder` in pieces
whose sizes a hash of the file picks, and refines after each piece. The first
six input bytes steer the options, so a small mutation of the file keeps them.
The rest is the file, so seed it with six zero bytes in front (the default
limit, no other option), or with a plain fixture: an input that starts with
`FF 4F` or the JP2 signature box is the whole file with all-zero control
bytes. The control bytes:

| Byte | Bits | Picks |
| ---- | ---- | ----- |
| 0 | 0-1 | the code-block work limit: default, unlimited, `Raised(2)`, `Raised(4)` |
| 0 | 2 | a layer cap of 1 + (byte 0 >> 3) mod 3 |
| 0 | 5 | a resolution reduction of 1 |
| 0 | 6 | change the options between refines (below) |
| 0 | 7 | (independent of the layer cap, which reads bits 3-6) a sample limit of `SampleLimit::AtMost(n)`, where `n` is the samples the file declares, or half of them (one hash of the file in four), so most draws admit the image: a refusal by it is the one-shot decode's sticky error, never a wait, except that a refine cut short by earlier work may wait on `BoundSpent` where a later tile's cap refusal ends the one-shot decode, ceiling or not |
| 1 | 0-1 | the region: whole, the window of bytes 2 to 5, or a tile |
| 1 | 2-4 | the tile: 0 for four of the eight values, then 1, 1, 2, 3 |
| 1 | 5-7 | a component subset of one or two distinct indices |
| 2-5 | | the window: `x0` and `y0` are bytes 2 and 3 mod 24, its width and height 1 + bytes 4 and 5 mod 40 |

For a JP2 file, a hash of the file also draws the output, which the decoder
cannot change once it is built: `ColorOutput::Srgb` for one file in four
(never with a component subset), else `raw_channels` for one in three of the
rest. A codestream keeps the control bytes' options. So the held conversion and the
channel-box faults in `refine` are checked against the one-shot decode under
the same options.

With bit 6 of byte 0, before some refines the target calls one of the
decoder's setters, as a hash of the file picks: `set_resolution_reduction`
(0 to 2, or 33, past the 32 levels a file may have), `set_max_quality_layers`
(none, or 1 to 3) or `set_region` (the whole image, a window, tile 0 or 1). A
layer cap of `Some(0)` or an empty window must be refused and leave the
options as they were. Each refine after a change is checked under the options
as they are then. Any real change of an option clears the error, even a
change back to the options that gave it. A walk kept across a change of the
layer cap would give a wrong image, which the target catches. The decoder
keeps its walks across a change of the reduction, which changes only the
code-blocks a walk charges for the resolutions it drops; a walk that still
charged those of the old reduction would differ from the one-shot decode only
at the packet bound.

Each refine that decodes goes on from the packet walks the earlier ones kept,
and must give what `decode_with` gives with `strict` off on the bytes fed so
far: the same image or the same error. After an error, each later refine must
give it again, and `finish` must be the decode of the whole input. The decoder
holds all its refines to the code-block limit together, and a one-shot decode
holds only itself. So each refine that was due is checked by the decoder's own
rule: it waits if and only if the code-block work a decoder with nothing spent
spends on the same bytes passes what is left of the bound after the work spent
before it. Under the fuzz cap the bound is 2^22 sample passes, which few
inputs of a few kilobytes come near, so each due refine also starts a probe
decoder with work already spent (`rust_j2k::fuzz`), drawn around the most that
leaves the refine room, and checks it by the same rule.

A refine that waits says why, and the target checks the reason. One that is
not due waits for `MoreBytes`, naming twice the bytes of the last try. A due
one waits for `RepeatLimit` exactly when the hook below reports that bound's
refusal. Otherwise a header cut short is `HeaderIncomplete`, and a refusal by
the sample limit, the charge for missing tiles or a work bound carries the
refusal the one-shot decode gives, hint included, and a code-block `WorkLimit`
never stands where the one-shot decode gives another error. `BoundSpent`, a
refusal by what earlier refines spent of the code-block bound, comes only
after work was spent, and carries no hint. Where the one-shot decode is
refused by that bound, the wait's `refused_at` gives the same tile and
work. The probe's reason is checked by the same rule; it is the path that
reaches a count past the cut. `tests/fixtures/incremental/segsym_wide_flipped.j2k`
is a seed that reaches a count past the whole bound, and
`t1bomb_row_128_8.seed` there one where the one-shot decode is refused by
that bound at the same tile. Only the first 4 refines that
are checked against a decode of their own (`MAX_CHECKED_TRIES`: a due one, or
one that gives an image or an error) are compared with the one-shot oracle;
the later ones only refine, and an error they give is still held to stick.
Every refine that is not due is still checked for its `MoreBytes`. An input
whose decodes reach the packet work bound ends sooner, at the packet ledger's
limit (above, under `decode`).

A due refine may also wait (`Waiting::RepeatLimit`) when the bound on the
walks of tiles not kept, which all the refines spend together, refuses it
(`rust_j2k::fuzz::incremental_repeat_work`), even where the one-shot decode
gives an image or an error. The visits those walks were charged must then be
past `REPEAT_BOUNDS` times the refine's packet bound, which the hook reports;
the refusal's message is not read. A decoder with nothing spent must give the
one-shot result, and when it kept every tile's walk it must have charged
nothing: a single refine walks each tile once, and the first walk of a tile
kept from its start is not charged. So a charge on a kept walk, which would
turn a refine into a wait past the bound, fails the target. The inputs
that reach that bound are larger than `-max_len=4096`: the smallest known is
`restarted_poc_tile(4, 8, 299)` from `tests/support/synth.rs`, 16 KB, whose
fifth due refine an overcharge refuses. The unit test
`fuzz::tests::incremental_target_repeat_check_holds_on_a_restarted_walk` runs
the check on it in the normal suite, and writes it out as a seed (see
Running).

Each input is decoded several times, so seed it with small files, such as the
fixtures under 3 KB, and cap the input length (`-max_len=4096`). Even so it
can be slow, since almost all its time goes to its own `decode_with` oracle
decodes: from the fixtures under 3 KB it runs about 230 inputs a second, but
on a corpus grown by earlier runs it has dropped to about 7 a second under
ASan, and one 598-byte input took 13 s. The name `incremental` is one Cargo
reserves.

`window_crop` checks a decode of part of the image against the whole
decode. Eight control bytes pick a resolution reduction (0 to 2), one of
four modes (a window, a tile, a component subset decoded with
`raw_channels`, as a subset names codestream components, or a subset
through a window), and the window, tile or subset; the rest is the file. So
libFuzzer can keep a file and try other areas of it by mutating the control
bytes. An input that starts with `FF 4F` or the JP2 signature box is the
whole file, with a hash of it as the control bytes, so plain fixtures are
valid seeds. The window and the tile are worked out on the reference grid
from the first `SOC SIZ` in the file. That `SIZ` may not be the
codestream's, since a JP2 file can hold those bytes in another box, so an
input is skipped unless its image area gives the whole decode's component
sizes. When the whole decode gives an image, the part must give one too,
unless its options are `InvalidOptions` (an area with no sample of a
component at the reduction, or a component past the image's), and its size,
and each of its components, must equal the whole decode's, cut to the area. A
window may start one point before the image area's origin (windows are cut to
the image area), so an offset image gets one that overlaps its edge. Taking
the area from `read_header` rather than the first `SOC SIZ` is a possible
follow-up. Seed it as
`decode`, with the window fixtures, the OpenJPH `.jph` files and the JP2
fixtures under 200 KB added (the JP2 ones from the top of `tests/fixtures`,
`jp2_real` and the conformance JP2 files; the `psot0` and work-bound JP2
files are bombs, slow to decode twice).

What `window_crop` cannot catch, by design:

- A bug the whole decode and the part share (tier-1, dequantization, the
  DWT, color): both are wrong the same way. Only the oracle tests catch that
  (the whole-image oracles, and the `-d`, `-t` and `-c` comparisons in
  `tests/decode_window.rs` for the part).
- A convention both the target and the decoder hold that OpenJPEG does not:
  its `at()` is the same ceil model as the decoder's, so a shared rounding
  passes.
- Windows over the good tiles of a file whose whole decode fails: the input
  is skipped, and `strict` is never turned off.
- `ColorOutput::Srgb`, reductions above 2, tiles past 15, windows wider than
  96 points, and component subsets other than `[0]`, `[1]`, `[2, 0]` and
  `[1, 0]` are never generated.

`retry_ladder` follows `DecodeOptions::raised_for` from each limit refusal.
The first input byte picks where each of the three limits starts
(`Default`, or a factor of 1 or 2), a reduction of 0 or 1, and `strict`;
the rest is the file. An input that starts with `FF 4F` or the JP2
signature box is the whole file with a zero control byte: the default
limits, reduction 0 and `strict` on. Each step must raise the option the
refusal names to `retry_with`, strictly above where it was, and leave every
other option as it was (compared by their `Debug` text, so every field
counts). A limit no option raises names no factor, and gives no retry.
Raising a limit never turns an image into an error: the options start at
or above the default limits, so when those decode the first decode must
too. Otherwise a ladder that reaches an image must have the samples of a
decode with no limits. A decode past a factor of 4 can take longer than the
timeout, so the ladder stops when a refusal asks for more, once that step
is checked. A packet work refusal names twice the factor it ran under, so a
ladder takes about log2 of the factor it needs; more than 4 steps on one
limit (log2 of that factor cap, plus 2) is a failure. The per-tile caps on
code-blocks and precincts are raised through the sample limit, so a sample
limit refusal and the two cap refusals share one step count. Each step must
raise the option strictly and the ladder stops past a factor of 4, so one
option takes at most 4 steps (2, 3, 4, then past 4) whatever the hints name;
the bound catches a ladder that does not end. That the hints double is held by
`tests/work_limit.rs`, the `limit_hint` unit tests and
`tests/tile_geometry_cap.rs`.

An input over the fuzz sample budget (`common/budget.rs`) is only read for its
header, and a sample limit refusal there is checked as one step with no decode.
So a sample limit retry of the image area is never decoded: the budget is at most
2^27 samples and the default limit at least 2^29, so only an input over the budget is
refused for its area, and that input is never decoded. The exception is an input
whose header fails to read: the budget counts it as under budget, so its decode
can still be refused for its area.

The budget bounds memory. Time is bounded by the fuzz cap on code-block
work (`rust_j2k::fuzz::with_code_block_work_cap`), which every decode here
starts from. Under the cap `Raised(n)` allows `n` times the capped bound, so
the ladder's factors mean what they say, and no limit, `Unlimited` included,
allows more than 4 times the cap, the most the ladder decodes at.

Seed it as `decode`, with 26 seeds made from the work-bound, sample-limit
and `psot0` bomb fixtures, each behind a control byte: `00` (the defaults),
`80` (`strict` off), `40` (reduction 1), `0f` (the sample and packet work
limits at a factor of 2) or `30` (the code-block work limit at a factor of
2). Of the 125 pairs of the five bytes and those 25 files, these are the
ones that take a step or compare an image and run in under about 2 s under
the sanitizer build, plus two slower ones: a code-block work step that a
factor of 2 admits (`00` and `t1bomb_1024_64_88`, about 6 s) and a packet
work step that does (`40` and `cprl_tall`, about 5 s). The others are left
out: 62 take no step and compare nothing, 22 take or compare but run 5 to
22 s, and the 15 of `cprl_tall_poc31`, `sweep_pcrl_1024_64` and
`sweep_rpcl_1024_64` run 17 to 27 s. libFuzzer replays every seed before it
starts, so slow seeds eat the run. Of the 93
seeds, 12 take a step (3 sample limit refusals of the header, 8 packet work
and 1 code-block work) and 16 compare an image.

`header_split` feeds a file to an `IncrementalDecoder` in pieces and checks
every `header()` call. `header()` keeps the walk of the tile-parts and what
each whole first tile-part gave between calls, so a call on more bytes goes on
where the last one stopped; the other targets never call it. After each feed
the call must give what `read_header_with` gives with `strict` off on the
bytes fed so far, under the decoder's options: the same `Header` or the same
error, and `None` where that read finds the main header incomplete (compared
by `Debug` text). On one feed in five a second call must give the same again.

The first 12 bytes are control bytes and the rest is the file: the reduction
(0 to 2), `strict` (the oracle reads with it off, as `header()` documents), the
sample limit (`Default`, `Raised`, `Unlimited` or `AtMost`), the region (the
whole image, a tile or a window), `ColorOutput::Srgb`, raw channels, a
component subset and a layer cap, then six bytes that seed the sizes of the
pieces (about 24 on average, so a tile-part is often cut in the middle) and
the changes of the region, the reduction and the layer cap between them. A
change the decoder refuses leaves the options as they were, and the oracle
reads them back from the decoder.

Random bytes seldom reach a tile over a cap under the main header, which is
what the cache is for, so seed it with `scripts/gen-header-split-seeds.py`:
the files of `tests/incremental_header.rs` (tiles rescued by a tile-part
`COD`, refused, of no levels at a reduction of 1, or missing; a file of 1024
components; the same in a JP2 file, and with a box layout fault) behind four
sets of control bytes, plus four seeds whose pieces end inside the `COD` of a
tile of no levels. The fixtures go in behind a control prefix too (see the
recipe below).

What the seeds catch, measured with the file put back after each change:
with `FirstPart::is_whole` (`src/codestream/mod.rs`) returning `true`, so a
tile-part cut short is taken for whole and its header never read again, a
seed fails on replay. With the tile index of the `known` lookup in
`Rescue::count` (`src/pipeline/decode.rs`) shifted by one, no seed fails on
replay but the target fails within a minute. The reduction, area, start and
length that key the cache are not reached by this target: a change of options
drops the cache before the key is checked, and the input only grows. The unit
test that reuses one `HeaderCache` across two inputs covers them.

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
# Seed the corpus once, as the CI smoke does (67 files): the fixtures, the
# OpenJPH codestreams, and the HT conformance codestreams of 6144 bytes or
# less. Size tests here are in bytes (`-size -6145c`): the older `-size -6k`
# rounds up to 6 KiB units and meant 5 KiB or less. The HT files reach the HT
# paths; `tile_body` needs no seed (its header is pinned, bit 7 of its first
# byte makes the body's code-blocks HT, and bit 6 with it makes them MIXED).
mkdir -p fuzz/corpus/decode
cp tests/fixtures/*.j2k tests/fixtures/openjph/files/*.j2c fuzz/corpus/decode/
# Not in the CI smoke yet: the sYCC JP2 files, which the sRGB window draw needs.
cp tests/fixtures/jp2_sycc_*.jp2 fuzz/corpus/decode/
find tests/fixtures/conformance/ht/codestreams -size -6145c -name '*.j2k' -exec cp {} fuzz/corpus/decode/ \;
# Optional, not in the CI smoke: the ISO/IEC 15444-4 codestreams carry
# multi-tile, multi-component, multi-layer, precinct-partitioned geometry that
# reaches far more of tier-2 than the synthetic single-tile fixtures.
cp tests/fixtures/conformance/codestreams/*.j2k fuzz/corpus/decode/

# Time-boxed run (CI-friendly: no panics, no OOM, no hangs). An input under
# 64 MiB, as every fuzz input is, may declare an image at the 2^29-sample
# default limit (larger files get 8 samples a byte), which would peak
# at about 2.7 GiB for a square tile and up to about 6.4 GiB for a tile one
# sample wide. The targets that decode whole images (`decode`, `decode_partial`,
# `incremental_decode`, `jp2_boxes`, `retry_ladder`, `window_crop`) skip an
# input that declares more than 2^28 samples divided by the most images the
# target holds at once (`common/budget.rs`): 2^27 for most, 2^26 for
# `jp2_boxes`, 2^25 for `incremental_decode`. They also cap the work their
# decodes do, block passes and samples reconstructed, at 2^28 divided by how
# many decodes' worth one execution can run, so an input past it is refused
# with `Error::Limit` before the work is done. `decode` and
# `incremental_decode` also stop decoding an input whose decodes the packet
# work bound refuses (`budget::PacketBounds`). That keeps each target under
# the 4 GiB and the 30 s timeout CI gives.
cargo +nightly fuzz run decode -- -dict=fuzz/j2k.dict -max_total_time=300 -rss_limit_mb=4096

# The same with strict off, against the strict decode. Seeded as the CI smoke
# does (93 files): not from the `decode` corpus, which a run grows.
mkdir -p fuzz/corpus/decode_partial
cp tests/fixtures/*.j2k fuzz/corpus/decode_partial/
find tests/fixtures/conformance/codestreams -name '*.j2k' -size -300001c -exec cp {} fuzz/corpus/decode_partial/ \;
cp tests/fixtures/openjph/files/*.j2c tests/fixtures/partial/*.j2k fuzz/corpus/decode_partial/
find tests/fixtures/conformance/ht/codestreams -size -6145c -name '*.j2k' -exec cp {} fuzz/corpus/decode_partial/ \;
cp tests/fixtures/conformance/ht/codestreams/ds0_hm_15_b8.j2k fuzz/corpus/decode_partial/
cargo +nightly fuzz run decode_partial -- -dict=fuzz/j2k.dict -max_total_time=300 -rss_limit_mb=4096

# The JP2 box target, seeded from the ISO/IEC 15444-4 JP2 reader files and
# the synthetic JP2 fixtures.
python3 scripts/gen-jp2-fuzz-seeds.py
cargo +nightly fuzz run jp2_boxes -- -dict=fuzz/jp2.dict -max_total_time=300

# The header target, seeded with the small synthetic codestreams and JP2 files,
# the header seeds of tile-part and refusal shapes, and the `PPM` and `PPT`
# files, whose prefixes reach the tile-part walks' handling of a cut.
mkdir -p fuzz/corpus/header
cp tests/fixtures/*.j2k tests/fixtures/jp2_*.jp2 tests/fixtures/header/* \
  tests/fixtures/packed_overrun/*.j2k fuzz/corpus/header/
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

# The smallest MIXED file, `ds0_hm_15_b8.j2k` (17 KB; the other two are larger),
# is in the `decode_partial` seeding above. Add it to `decode` the same way:
cp tests/fixtures/conformance/ht/codestreams/ds0_hm_15_b8.j2k fuzz/corpus/decode/
cargo +nightly fuzz run decode -- -dict=fuzz/j2k.dict -max_total_time=60 -rss_limit_mb=4096

# The incremental target (120 files), seeded with the fixtures of 3072 bytes or
# less, the same behind control bytes that change the options between refines,
# and the files of `tests/fixtures/incremental` of 4096 bytes or less (the
# target runs with `-max_len=4096`, which would truncate larger ones). The
# old `-size -3k` meant 2 KiB or less.
mkdir -p fuzz/corpus/incremental_decode
find tests/fixtures -maxdepth 1 -size -3073c \( -name '*.j2k' -o -name '*.jp2' \) \
  -exec cp {} fuzz/corpus/incremental_decode/ \;
for f in $(find tests/fixtures -maxdepth 1 -size -3073c \( -name '*.j2k' -o -name '*.jp2' \)); do
  { printf '\100\0\0\0\0\0'; cat "$f"; } > "fuzz/corpus/incremental_decode/set_$(basename "$f")"
done
find tests/fixtures/incremental -type f -size -4097c -exec cp {} fuzz/corpus/incremental_decode/ \;
cargo +nightly fuzz run incremental_decode -- -dict=fuzz/j2k.dict -max_len=4096 -max_total_time=300 -rss_limit_mb=4096

# The incremental target on an input that reaches the bound on the walks read
# again (16 KB), written by a unit test. One run takes about 30 s.
RUST_J2K_INCREMENTAL_SEED_DIR=fuzz/corpus/incremental_decode_large \
  cargo test --lib fuzz::tests::incremental_target_repeat_check_holds_on_a_restarted_walk
cargo +nightly fuzz run incremental_decode fuzz/corpus/incremental_decode_large -- \
  -dict=fuzz/j2k.dict -max_len=32768 -max_total_time=300 -rss_limit_mb=4096

# The window target, seeded as decode plus the window, JPH and small JP2 files
# (as the CI smoke does; it copies the `decode` corpus as it stands).
mkdir -p fuzz/corpus/window_crop
cp fuzz/corpus/decode/* fuzz/corpus/window_crop/
cp tests/fixtures/window/*.j2k tests/fixtures/openjph/files/*.jph fuzz/corpus/window_crop/
find tests/fixtures/conformance tests/fixtures/jp2_real -name '*.jp2' -size -200k -exec cp {} fuzz/corpus/window_crop/ \;
find tests/fixtures -maxdepth 1 -name '*.jp2' -size -200k -exec cp {} fuzz/corpus/window_crop/ \;
cargo +nightly fuzz run window_crop -- -dict=fuzz/j2k.dict -max_total_time=60 -rss_limit_mb=4096 -timeout=30

# The retry target (79 files), seeded as the CI smoke does: the fixtures, its own
# `tests/fixtures/retry_ladder` files, and limit fixtures behind a control byte.
mkdir -p fuzz/corpus/retry_ladder
cp tests/fixtures/*.j2k tests/fixtures/retry_ladder/*.j2k fuzz/corpus/retry_ladder/
seed() { { printf "\\x$1"; cat "tests/fixtures/$2"; } > "fuzz/corpus/retry_ladder/$1-${2##*/}"; }
for c in 0f 30; do
  for f in tier1_work_bound/htbomb_1024_64_3.j2k tier1_work_bound/opj_specks2048_16bit.j2k \
    work_bound/opj_patch2048_b4_q100.j2k work_bound/opj_patch2048_b4_q64.j2k \
    work_bound/pocdup_4x4_1024_31.j2k work_bound/pocspent_2tiles_60000_559.j2k \
    work_bound/pocvol_1024x512_1000_2.j2k work_bound/t2bomb_2048x1024_1000.j2k \
    psot0/jp2_t2bomb_visits.jp2; do
    seed $c $f
  done
done
for c in 00 30 80; do seed $c sample_limit/flat_32769x16385_tiled_lossless.j2k; done
for c in 00 80; do seed $c work_bound/pocvol_1024x512_1000_2.j2k; done
seed 00 work_bound/pocspent_2tiles_60000_559.j2k
seed 00 tier1_work_bound/t1bomb_1024_64_88.j2k
seed 40 work_bound/cprl_tall.j2k
seed 00 retry_ladder/tile_precincts.j2k
seed 00 retry_ladder/tile_code_blocks.j2k
cargo +nightly fuzz run retry_ladder -- -dict=fuzz/j2k.dict -max_total_time=60 -rss_limit_mb=4096 -timeout=30

# The header_split target: 44 synthetic files with tiles over a cap (the script
# needs only Python 3 and writes the same files on every run), and the fixtures
# behind the control prefix
# `00 00 00 00 00 00 01 02 00 00 00 00` (the defaults, one stream of
# piece sizes). `-max_len` must be over the largest seed, 36,832 bytes.
mkdir -p fuzz/corpus/header_split
python3 scripts/gen-header-split-seeds.py fuzz/corpus/header_split
for f in tests/fixtures/*.j2k tests/fixtures/header/*.j2k tests/fixtures/header/*.jp2 tests/fixtures/*.jp2; do
  { printf '\0\0\0\0\0\0\1\2\0\0\0\0'; cat "$f"; } > "fuzz/corpus/header_split/fx_$(basename "$f")"
done
cargo +nightly fuzz run header_split -- -dict=fuzz/j2k.dict -max_len=65536 -max_total_time=60 -rss_limit_mb=4096 -timeout=30

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
| 2026-11-17 | `window_crop` (new) | `decode`'s seeding and `tests/fixtures/window/*.j2k` (73 inputs), `j2k.dict` | 61 s, 33 062 runs; `nice -n 19`, one job; the 1-minute load was 7.6 | no failure. 709 of the 1 298 inputs it kept reach the comparison: 231 windows, 138 tiles, 121 subsets and 219 subsets through a window. With `Window` shifted one column right in the decoder, a seed fails on replay |
| 2026-11-17 | `retry_ladder` (new) | `decode`'s seeding and six limit fixtures (73 inputs), `j2k.dict` | 61 s, 23 891 runs; `nice -n 19`, one job; the 1-minute load was 7.2 | no failure. Of the 875 inputs it kept, 11 take a step and 4 compare the ladder's image with the default or unlimited one. With `with_retry` also raising `code_block_work_limit` on a packet work refusal, the seed `pocvol_1024x512_1000_2` fails on replay |
| 2026-11-17 | `window_crop` with eight control bytes | `decode`'s seeding, the window fixtures, the `.jph` files and the JP2 files under 200 KB (109 seeds; 84 reach the comparison, 27 of the 36 JP2 and JPH ones among them), `j2k.dict` | 61 s, 18 074 runs; `nice -n 19`, one job; the 1-minute load was 5.5 | no failure. Coverage 9 464 (8 145 in the run above). 557 of the 1 126 inputs it kept reach the comparison: 182 windows, 123 tiles, 99 subsets and 153 subsets through a window. With `Window` shifted one column right, a seed fails on replay |
| 2026-11-17 | `retry_ladder` with every option compared and the seeds behind control bytes | `decode`'s seeding and the 26 control-byte seeds (93 seeds; 12 take a step, 16 compare), `j2k.dict` | 61 s, 4 081 runs (the seeds take about 22 s to replay); `nice -n 19`, one job; the 1-minute load was 5.4 | no failure. Coverage 7 993 (7 836 in the run above). Of the 406 inputs it kept, 16 take a step (7 sample, 8 packet work, 1 code-block work) and 19 compare. With `with_retry` also setting `ColorOutput::Srgb` on a packet work refusal, the seed `0f-pocvol_1024x512_1000_2` fails on replay |
| 2026-11-17 | `retry_ladder` with the packet work hint doubling and a per-limit step bound (`MAX_STEPS` 4) | `decode`'s seeding and the 26 control-byte seeds (98 inputs), `j2k.dict` | 61 s, 1 476 runs; `nice -n 19`, one job; the 1-minute load was 4.4 | no failure |
| 2026-11-19 | `retry_ladder` with the tile caps on the sample limit's step count | the fixtures under `tests/fixtures/`, the 26 control-byte seeds and `retry_ladder/tile_precincts.j2k` (76 inputs; 503 with every fixture and codestream replayed), `j2k.dict` | 61 s, 7 877 runs; `nice`, one job; the 1-minute load was 3 | no failure. With `TilePrecincts` left out of the sample limit's option, the seed `tile_precincts.j2k` fails on replay |
| 2026-11-18 | `window_crop` with windows from one point before the origin, and the image size compared | the README's seeding (114 seeds: `decode`'s, the window fixtures, the `.jph` files and the JP2 files under 200 KB), `j2k.dict` | 61 s, 21 419 runs; `nice -n 19`, one job; the 1-minute load was 4.6 | no failure. Coverage 10 427. With `Footprint::rect` shifted one column right, a seed fails on replay; with the image's reported width one too many, a seed fails on its size assertion |
| 2026-11-18 | `incremental_decode` with the repeat-charge oracle (`incremental_repeat_work`) and the setters between refines (bit 6) | the fixtures under 3 KB, and the same behind `40 00 00 00 00 00` (98 seeds), `j2k.dict`, `-max_len=4096` | `-max_total_time=60` (66 s with loading), 18 228 runs; `nice -n 19`, one job; the 1-minute load was 1.5 | no failure |
| 2026-11-18 | `incremental_decode` with the reason a refine waits checked (`check_waiting`, and `MoreBytes { until }` on each refine not due) | the fixtures under 3 KB, and the same behind `40 00 00 00 00 00` (110 seeds), `j2k.dict`, `-max_len=4096` | `-max_total_time=60`, 22 818 runs; `nice -n 19`, one job; the 1-minute load was 1.4 | no failure |
| 2026-11-20 | `header_split` (new) | the README's seeding (118 inputs: 36 from `gen-header-split-seeds.py` and the 82 fixtures behind a control prefix), `j2k.dict`, `-max_len=65536` | 61 s, 25 711 runs; `nice -n 19`, one job; the 1-minute load was 1 | no failure. Coverage 3 107. With `FirstPart::is_whole` always true, a seed (`rescued_tile30_nolevels_cut*`) fails on replay; with the `known` lookup's tile index shifted by one, no seed fails on replay and the target fails within 60 s |
| 2026-11-22 | `header_split` and `incremental_decode` with `held.rs` counting a settled JP2 file as closed (#654) | `header_split`: the README's seeding plus an ad hoc input, not committed: a JP2 file of two tiles whose tile 0 a reduction of 1 refuses, then a second `jp2c` (122 inputs; the script's `jp2_*_then_jp2c` seeds now cover the shape); `incremental_decode`: `tests/fixtures/incremental/` (15 seeds), then its 7 `tile_error_*` and `held_box_*` seeds alone; `j2k.dict` | `header_split` 65 s, 5 877 runs; `incremental_decode` 63 s, 82 runs, then 61 s, 112 455 runs; `nice -n 19`, one job at a time; the 1-minute load was 5 to 7 | no failure, once `header_split`'s `replaceable` completed a cut `SOT` and the box after the codestream the input ends inside (the target found each gap within seconds). With `Jp2Prefix::settled` always false, both new `tile_error_lblock_jp2_then_*` seeds and the ad hoc `header_split` input fail on replay |
| 2026-11-22 | the same, after review: `held.rs` following #655 and reading a `jp2c` of `LBox` 0, `header_split`'s padding capped | `header_split`: the README's seeding (128 inputs, the script's `jp2_*_then_jp2c` among them), then the corpus that run kept; `incremental_decode`: `tests/fixtures/incremental/` (15 seeds); `j2k.dict` | `header_split` 61 s, 12 926 runs (`-malloc_limit_mb=1024`); `incremental_decode` 74 s, 1 058 runs; `nice -n 19`, one job at a time; the 1-minute load was 7 to 11 | no failure, once `header_split` allowed a held error where the input ends one byte, not `0xFF`, into where the next marker belongs (`Completion::LoneByte`; the target found it within seconds). With `Jp2Prefix::settled` always false, both `jp2_*_then_jp2c_c0` seeds and both `tile_error_lblock_jp2_then_*` seeds fail on replay |
| 2026-11-23 | `header_split` without `Completion::LoneByte`, and with `Completion::Far` trying the input with the far box or tile-part dropped; `incremental_decode` (#657) | `header_split`: the README's seeding (128 inputs); `incremental_decode`: `tests/fixtures/incremental/` (15 seeds); `j2k.dict` | `header_split` 61 s, 15 714 runs (`-malloc_limit_mb=1024`); `incremental_decode` 74 s, 278 runs; `nice -n 19`, one job at a time; the 1-minute load was 1.4 to 1.5 | no failure. With the lone byte not taken as breaking the chain (`codestream_end` back to `End::Unknown` there), `header_split` fails on the corpus the first run kept, right after it loads |
| 2026-11-23 | `header_split`, after review: a tile-cap refusal more bytes lift held where the codestream is fixed, and allowed by the target | the README's seeding (128 inputs), `j2k.dict` | 61 s, 11 298 runs (`-malloc_limit_mb=1024`); `nice -n 19`, one job; the 1-minute load was 10.7 | no failure |
| 2026-11-27 | `decode` and `incremental_decode` with the new draws: sRGB on a window or tile, a raised packet work limit, and sRGB or `raw_channels` on the incremental decoder | `decode`: the 68 `tests/fixtures` codestreams and JP2 files, then the five `jp2_sycc_*.jp2` files alone; `incremental_decode`: `tests/fixtures/incremental/` and the JP2 fixtures behind six zero bytes (34 seeds), `j2k.dict`, `-max_len=4096` | 40 to 60 s each; `nice -n 19`, one job at a time | no failure. Each new draw reached under a temporary counter: raised packet limits on thousands of runs, the sYCC window and tile refusal (`sRGB output from sub-sampled sYCC chroma on an area smaller than the image`) within 60 s on the sYCC seeds, and all three outputs in `incremental_decode`. One `incremental_decode` input of 604 bytes (control byte `a0`) takes 16 to 20 s under ASan with or without the new draws |
| 2026-11-27 | `decode` and `incremental_decode` with the packet ledger (`budget::PacketBounds`) | `decode`: the 68 `tests/fixtures` codestreams and JP2 files; `incremental_decode`: `tests/fixtures/incremental/`, the JP2 fixtures behind six zero bytes and `restarted_poc_tile(4, 8, 299)` (35 seeds), `-max_len=4096`; `j2k.dict` | 61 and 79 s; 18 938 and 1 190 runs; `nice -n 19`, one job at a time; the 1-minute load was 12 to 15 | no failure. Replayed under a temporary counter of every decode's result and every wait's reason, the seeds give the same on `main` and with the ledger, except the two that reach the packet bound: `at_most_work_bound_waits.seed` still checks its first due refine's `WorkLimit` wait against the one-shot refusal, then ends (27 to 31 s before, 5 to 6 s after, CPU under ASan), and a `decode` input of that shape 8 times taller drawing `Raised(4)` skips the strict decode and the part (17 to 21 s before, 14 to 17 s after). The `incremental_decode` run saved a slow unit the ledger does not reach: a 4096-byte codestream of 20 components and 65 535 layers whose decodes pass the packet bound, 0.24 s each in release, which takes 32 to 34 s under ASan on `main` and with the ledger alike |

A crash is written to `fuzz/artifacts/decode/`. Reproduce and minimize it with:

```sh
cargo +nightly fuzz run decode fuzz/artifacts/decode/crash-<hash>
cargo +nightly fuzz tmin decode fuzz/artifacts/decode/crash-<hash>
```

`corpus/`, `artifacts/`, `coverage/`, and `target/` are git-ignored; only the
target and its manifest are tracked, so the corpus stays reproducible from the
committed fixtures. No seed corpus is committed: the commands above, and the CI
smoke, build each one. The CI smoke (`.github/workflows/ci.yml`) seeds each
target as the recipes above do, builds every target, and runs them for 15 to
60 s each, with `-timeout=30` and `-rss_limit_mb=4096`.
