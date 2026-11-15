# Window, tile and component fixtures

Used by `tests/decode_window.rs`, which decodes these with
`DecodeOptions::window`, `DecodeOptions::tile` and `DecodeOptions::components`
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

## Oracles

| File | Content |
| ---- | ------- |
| `<name>.sha256` | one line per case: the `opj_decompress` arguments with the spaces taken out, the output size, and the SHA-256 of the samples, every component in order, each a little-endian `i32` |
| `tiled_97_r<R>_<c>.pgx` | component `c` of `opj_decompress -i tiled_97.j2k -o out.pgx -r R`, R from 0 to 2 |

The cases are windows off the tile grid, to the image's right and bottom
edges, from before its origin and past its far edges (which OpenJPEG clips),
one tile's rectangle, one sample, and windows at `-r 1` and `-r 2`; tiles 0, 4
and 8, and tile 4 at `-r 1`; and the components `1`, `2,0` and `0,1,2`, alone,
with a window and with a tile at a reduction.

The script checks that each `-d` and `-t` output is the matching crop of the
whole decode at the same reduction, for all three codestreams, 9/7 included.
The 9/7 test grades each case against that crop of the PGX, within 1, the
tolerance of the whole decode.

On `tiled_53.j2k` and `tiled_97.j2k`, OpenJPEG skips the color transform under
`-c` and writes the components before it. The script checks that it does, and
records the whole decode's components for those cases instead, which is what
this decoder returns (see `docs/openjpeg-departures.md`). On
`tiled_53_nomct.j2k` the `-c` output is OpenJPEG's own.

## Benchmark fixture

`../bench/tiled_2048_gray.j2k` is written by `python3
scripts/gen-window-fixtures.py --bench`: a 2048 x 2048 8-bit gray image,
`((3x + 2y) >> 3) mod 256` plus noise from -4 to 4 (seed 2160), coded by
`opj_compress -t 256,256 -n 5 -r 20`, so 64 tiles of 256 x 256.

The sources are synthetic and the encoder is OpenJPEG, so these files are
under the repository's license.
