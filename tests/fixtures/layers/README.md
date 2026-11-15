# Layer-cap fixtures

Used by `tests/max_quality_layers.rs`, which decodes these with
`DecodeOptions::max_quality_layers` set and compares the samples with
`opj_decompress -l N` on the same bytes.

## Source and tools

`scripts/gen-layer-fixtures.py` writes the two sources, runs OpenJPEG 2.5.0
`opj_compress` and `opj_decompress` (system package) on them, and checks each
result it relies on. Both sources come from a fixed LCG
(`state * 1103515245 + 12345` mod 2^31, taking `state >> 16`):

| Source | Shape |
| ------ | ----- |
| `gray.pgm` | 64 x 64, 8-bit: `3x + 2y` plus noise from -8 to 8 (seed 215), clamped |
| `rgb.ppm` | 40 x 32, 8-bit, three components: `(x(3 + c) + y(5 - c) + 40c) mod 256` plus noise from -8 to 8 (seed 2150), clamped |

## Codestreams

| File | Command | Layers | Tolerance |
| ---- | ------- | ------ | --------- |
| `lrcp.j2k` | `opj_compress -i gray.pgm -o lrcp.j2k -n 4 -p LRCP -r 80,20,5,1` | 4, the last lossless | exact |
| `rpcl.j2k` | `opj_compress -i gray.pgm -o rpcl.j2k -n 3 -p RPCL -c [32,32],[16,16] -b 16,16 -r 40,10,1` | 3 | exact |
| `bypass_pterm.j2k` | `opj_compress -i gray.pgm -o bypass_pterm.j2k -n 3 -p LRCP -M 17 -r 20,6,2,1` | 4 | exact |
| `cprl_irreversible.j2k` | `opj_compress -i rgb.ppm -o cprl_irreversible.j2k -n 3 -p CPRL -I -r 60,20,6` | 3, 9/7 and the color transform | 1 |
| `pterm_cut.j2k` | written by hand, see below | 2 | exact |

`pterm_cut.j2k` is two 8 x 8 code-blocks under `predictable termination`, 16 x
8, no wavelet level. Each block is `opj_compress -n 1 -b 8,8 -M 16` of one 8 x
8 block of `gen-zero-length-fixtures.py`'s source, and the codestream is built
with that script's writer. Layer 0 brings 3 passes of each block with every
byte of its one codeword segment; layer 1 brings the rest of the passes with a
length of zero. A cap of 1 ends the segment 3 passes in with bytes those
passes do not reach. `predictable termination` forbids that of a whole
segment, so the check must not apply to a segment the cap cuts short;
OpenJPEG does not make it under `-l`. The full decode is the source (the
script checks). No `opj_compress` encode the script tried leaves a cut segment
with more than the 2 bytes the check allows, so this one is built by hand.

The 9/7 tolerance is the most a sample may differ from OpenJPEG's: 1, as for
the whole codestream. The 5/3 ones are exact.

## Oracles

| File | Command |
| ---- | ------- |
| `<name>_l<N>_<c>.pgx` | component `c` of `opj_decompress -i <name>.j2k -o out.pgx -l N -r 0`, for each N from 1 to the layer count |
| `lrcp_l<N>_r1_<c>.pgx` | the same at `-r 1`, N from 1 to 4 |
| `rpcl_l2_r2_0.pgx` | `-l 2 -r 2` |
| `cprl_irreversible_l1_r1_<c>.pgx` | `-l 1 -r 1` |
| `part4.sha256` | for ISO/IEC 15444-4 `p0_02`, `p0_03` (also at `-r 1`), `p0_10`, `p0_16` and `p1_01`, each N from 1 to the layer count: the SHA-256 of the `opj_decompress -l N -r R` samples, every component in order, each sample a little-endian `i32` |
| `damaged.sha256` | the same hash of `opj_decompress -l 2` on `bypass_pterm.j2k` with byte 403 xor 0x10, a flip in a segment the cap keeps whole; OpenJPEG decodes it without a warning |
| `prefixes.sha256` | the same hash of `opj_decompress -allow-partial` on every prefix of `pterm_cut.j2k` and `bypass_pterm.j2k`, with no cap (`0`) and at `-l 1`; a run of prefixes that decode alike is one line, `<name> <cap> <first length> <last length> <hash> <flag>`. The hash is `refused` where OpenJPEG writes no image. The flag is `cut` where OpenJPEG warns `segment too long`, that is, the prefix ends inside a code-block's contribution, and `whole` otherwise. `python3 scripts/gen-layer-fixtures.py --prefixes` writes only this file |

The Part 4 oracles are hashes because `layers/` ships in the crate and the
conformance corpus does not: its license is for conformance testing only, so
`Cargo.toml` excludes it (see [`../conformance/README.md`](../conformance/README.md)).
Decoded PGX of it here would ship what the exclusion keeps out. The
codestreams are read from `../conformance/codestreams/`. All five are 5/3, so the hash is
graded bit-exactly.

The script checks that each N decodes to other samples than N + 1, so every
cap is graded on samples it changes. The sources are synthetic and the
encoder is OpenJPEG, so these files are under the repository's license.
