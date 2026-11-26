# TNsot extra-part fixtures

Used by `tests/tnsot_extra_part.rs`. Source: a synthetic 32x32 8-bit gray ramp
plus noise (`random.seed(2)`, `(x*5 + y*3 + randrange(12)) & 255`) as a P5 PGM,
SHA-256 `a0293b731eaf1a1c6d2912978a64340943e2ffa8024a50ec2e0cfad23f83848e`.
Encoder: OpenJPEG 2.5.0.

- `tile1_tp3.j2k`: `opj_compress -i g.pgm -o tile1_tp3.j2k -n 3 -TP R` (one
  tile, three tile-parts).
- `tiles2_tp3.j2k`: the same with `-t 16,32` (two tiles, three tile-parts
  each, in file order tile 0 then tile 1).
- `image.pgm`: `opj_decompress` on either file (identical, and equal to the
  source), with the comment line removed.

The test edits `TNsot` itself.
