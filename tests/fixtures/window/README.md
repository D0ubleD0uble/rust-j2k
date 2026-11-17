# Window, tile and component fixtures

Used by `tests/decode_window.rs`, which decodes these with
`DecodeOptions::region` (a window or a tile) and `DecodeOptions::components`
set and compares the samples with `opj_decompress -d`, `-t` and `-c` on the
same bytes.

## Source and tools

`scripts/gen-window-fixtures.py` writes the source, runs OpenJPEG 2.5.0
`opj_compress` and `opj_decompress` (system package) on it, and checks each
result it relies on. `rgb.ppm` is 70 x 54, 8-bit, three components:
`(x(3 + c) + y(5 - c) + 40c) mod 256` plus noise from -8 to 8, from a fixed LCG
(`state * 1103515245 + 12345` mod 2^31, taking `state >> 16`, seed 216),
clamped.

## Codestreams

Each puts the image at (5, 4) on the reference grid, so the area is 5..75 by
4..58, with 24 x 20 tiles from (3, 2): 3 x 3 tiles, the ones on the image's
edges partial. Three decomposition levels.

| File | Command |
| ---- | ------- |
| `tiled_53.j2k` | `opj_compress -i rgb.ppm -o tiled_53.j2k -t 24,20 -T 3,2 -d 5,4 -n 3` (5/3, RCT) |
| `tiled_97.j2k` | the same with `-I` (9/7, ICT) |
| `tiled_53_nomct.j2k` | the same as `tiled_53.j2k` with `-mct 0` (no color transform) |

`sub.raw` is 67 x 51, 8-bit, three components sampled 1x1, 2x2 and 3x2, from
the same LCG (seed 2161): component c at (x, y) of its own grid is
`(x(5 + 2c) + y(3 + c) + 60c) mod 256` plus noise from -8 to 8, clamped, each
`ceil(67 / dx)` x `ceil(51 / dy)` samples, the layout `opj_compress -F` reads.
It puts the image at (7, 5), so the area is 7..74 by 5..56, with 16 x 14 tiles
from (2, 3): 5 x 4 tiles. Neither codestream has a color transform.

| File | Command |
| ---- | ------- |
| `sub_53.j2k` | `opj_compress -i sub.raw -o sub_53.j2k -F 67,51,3,8,u@1x1:2x2:3x2 -t 16,14 -T 2,3 -d 7,5 -n 3 -mct 0` (5/3) |
| `sub_97.j2k` | the same with `-I` (9/7) |

## Oracles

| File | Content |
| ---- | ------- |
| `<name>.sha256` | one line per case: the `opj_decompress` arguments with the spaces taken out, the output size, and the SHA-256 of the samples, every component in order, each a little-endian `i32` |
| `<name>_r<R>_<c>.pgx` | component `c` of `opj_decompress -i <name>.j2k -o out.pgx -r R`, R from 0 to 2, for `tiled_97` and `sub_97` |

The cases are windows off the tile grid, to the image's right and bottom
edges, from before its origin and past its far edges (which OpenJPEG clips),
one tile's rectangle, one sample, and windows at `-r 1` and `-r 2`; tiles 0, 4
and 8, and tile 4 at `-r 1`; and the components `1`, `2,0` and `0,1,2`, alone,
with a window and with a tile at a reduction. The `sub_*` cases are windows
with odd and even bounds, so each sub-sampled component's first and last
samples move, clipped windows, one at each reduction, tiles 0, 7 and 19 and
tile 12 at `-r 1`, and the components `2,0` and `1`, alone, with a window and
with a tile at a reduction. The size on each line is the first output
component's, as OpenJPEG writes it.

The script checks that each `-d` and `-t` output is the matching crop of the
whole decode at the same reduction, on each component's own grid, for every
codestream, 9/7 included. The 9/7 tests grade each case against that crop of the PGX, within 1, the
tolerance of the whole decode.

On `tiled_53.j2k` and `tiled_97.j2k`, OpenJPEG skips the color transform under
`-c` and writes the components before it. The script checks that it does, and
records the whole decode's components for those cases instead, which is what
this decoder returns (see `docs/openjpeg-departures.md`). On
`tiled_53_nomct.j2k` the `-c` output is OpenJPEG's own.

`a_tile_of_a_cut_codestream_matches_openjpeg_allow_partial` in
`tests/decode_window.rs` holds three more hashes, made by hand on 2026-11-16
and not by the script: `head -c 7000 tiled_53.j2k > cut.j2k`, then
`opj_decompress -i cut.j2k -o t.pgx -allow-partial -t N` for tiles 2, 4 and
6 (the cut one),
hashed as the `.sha256` lines are. `-allow-partial -d` refuses that prefix
("Stream too short"), so it has no window oracle.

## Benchmark fixture

`../bench/tiled_2048_gray.j2k` is written by `python3
scripts/gen-window-fixtures.py --bench`: a 2048 x 2048 8-bit gray image,
`((3x + 2y) >> 3) mod 256` plus noise from -4 to 4 (seed 2160), coded by
`opj_compress -t 256,256 -n 5 -r 20`, so 64 tiles of 256 x 256.

The sources are synthetic and the encoder is OpenJPEG, so these files are
under the repository's license.
