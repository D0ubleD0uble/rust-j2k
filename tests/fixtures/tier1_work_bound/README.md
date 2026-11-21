# Tier-1 work bound fixtures

Used by `tests/work_limit.rs` and the margin test in
`src/tier1/work_bound_tests.rs`. A codestream whose code-blocks ask the
block decoder for far more work than its bytes, and `opj_compress` files
that spend much work per byte, the most seen from an encoder among them,
which must decode.

## Source

`t1bomb_1024_64_88.j2k` is written from scratch by
`scripts/gen-tier1-work-bound-fixtures.py` (no encoder, no input image),
from the shape in issue #250. With `--check` it runs each file through
`opj_decompress` 2.5.0 (the system package) and prints the outcome and the
time. With `--big DIR` it writes the issue's 8192^2 shape to `DIR`
instead; that one is not committed, and `tests/work_limit.rs` builds it.

| File | Shape | Sample-passes | OpenJPEG 2.5.0 | Here |
| ---- | ----- | ------------- | -------------- | ---- |
| `t1bomb_1024_64_88.j2k` (1276 B, SHA-256 `4bdd8f69…b523`) | `t1bomb(1024, 6, 88)`: one 1024^2 tile of one 8-bit component, no wavelet levels, 64x64 code-blocks, one layer, LRCP. QCD gives the band 30 bit-planes and every block is included with no zero bit-plane, 88 passes and the one byte 0x00 | 9.2e7 | decodes (samples 0, 128 and 255), 0.14 s | `Limit`; decodes under `WorkLimit::Raised(2)` to the samples OpenJPEG decodes (SHA-256 of the 8-bit samples `a20e3ada…734d`) |

`htbomb_1024_64_3.j2k` (1330 B, SHA-256 `8bfe0262…75d1`) is the HT
counterpart, written by `htbomb(1024)` in `tests/work_limit.rs`, which checks
the committed bytes against it (no encoder, no input image): one 1024^2 tile
of one 8-bit component, no wavelet levels, `CAP` and a COD with the HT block
style, 64x64 code-blocks, one layer. QCD gives the band 30 bit-planes and
every block is included with no zero bit-plane, all three passes of an HT
set, a cleanup segment of the two bytes `0xF2 0x00` (`Scup = 2`, the fewest a
block decodes from) and a refinement segment of the byte `0x00`. That is
3.1e6 sample-passes, 4.7% of the bound, and about the most an HT file can ask
for its bytes (see `docs/resource-bounds.md`). It decodes here and in
`opj_decompress` 2.5.0 (4 ms) to a flat 128 (SHA-256 of the 8-bit samples
`36ff3497…3534e`). The flat image is not a check that the blocks decode
correctly: the work test shows the three passes are counted, and the HT
conformance corpus shows the decode is right.

## Encoder output

`opj_specks2048_16bit.j2k` (28079 B, SHA-256 `cdcaddcb…96ee`) is written by
`scripts/gen-tier1-work-bound-fixtures.py --opj`, which runs `opj_compress`
2.5.0 at its defaults (lossless 5/3, 64x64 code-blocks, 5 levels, one
layer) on a 2048^2 16-bit image of 1000 with a speck of 60000 at
`(i * 977 % 2048, i * 613 % 2048)` for each `i` below 128 (`specks_image`;
the PGM's SHA-256 is
`af4245123bfcbc68a88cc9f96a78fc3290c6e0bbefca5f3ab54a32b502fcce85`):

```
opj_compress -i specks.pgm -o opj_specks2048_16bit.j2k
```

Each speck makes its code-blocks run every pass for a few bytes, so it
spends about 3600 sample-passes per byte, 11% of the bound. It decodes
here and in `opj_decompress` 2.5.0 to exactly the image that was coded;
the test rebuilds that image and compares sample for sample.

`opj_flat2048_16bit_n1.j2k` (13968 B, SHA-256 `ba45005d…f189`) is written
by the same `--opj` run: `opj_compress` 2.5.0 with no wavelet level and
otherwise its defaults, on a 2048^2 16-bit image of 0 (`specks_image(2048,
0, 0)`; the PGM's SHA-256 is
`799684cb89c9575e382fe1471293b29f04dec810f588483cf62d4cd832c5dfdc`):

```
opj_compress -i flat.pgm -o opj_flat2048_16bit_n1.j2k -n 1
```

Every 64x64 code-block runs about 46 passes for about 14 bytes, so it
spends 13933 sample-passes per byte, 42.5% of the bound, the most seen
from an encoder. It decodes here and in `opj_decompress` 2.5.0 to the
flat 0 that was coded.

## A work bound in a lower tile

`lower_tile_t1bomb_over_cap.j2k` (1305 B, SHA-256 `1dd8c35d…bb47`) is written
from scratch by `scripts/gen-lower-tile-work-fixture.py` (no encoder, no input
image), from `t1bomb(1024, 6, 88)` above (#572). It is two 1024^2 tiles side
by side. The main `COD` gives 1x1 precincts, which puts a 1024^2 tile over the
2^18 precinct cap. Tile 0's tile-part has a `COD` of the maximal precincts,
which brings it within the caps, and the bomb's packet; tile 1 has a tile-part
with no `COD` and no data, so it has the main header's precincts.

`read_header_with` reads no tile data, so it refuses tile 1 for its 2^20
precincts (`Limit`, `TilePrecincts`). `decode_with` reaches tile 0 first and
refuses it for its 9.2e7 sample-passes (`Limit`, `CodeBlockWork`); with the
bound lifted it refuses tile 1 as the header does. `opj_decompress` 2.5.0
decodes tile 0 and fails on tile 1 ("Failed to decode tile 2/2"). It is the
work-bound case of the third same-error exception in `src/error.rs`; it is
listed in `LOWER_TILE_FAULTS` in `tests/read_header.rs` and checked by
`a_work_bound_in_a_lower_tile_comes_first_in_the_decode` in
`tests/tile_geometry_cap.rs`. It sits here, not in `header/`, because
`tests/incremental.rs` caps `header/` at its 16 shares and this file is a
work-bound one (it is fed in halves there, as the other files here are).
