# Psot = 0 last tile-part fixtures

Used by `tests/psot_zero.rs`. Small synthetic codestreams whose last tile-part
has `Psot = 0`, with and without the closing EOC.

## Source and tools

`source.pgm` is a 48x40 8-bit image, row-major. Sample (x, y) is
`x*5 + y*3 + n` mod 256, with `n = random.randint(0, 7)` drawn per sample after
`random.seed(7)`. The three base codestreams were made from it with
OpenJPEG 2.5.0 (`opj_compress`/`opj_decompress` from the system package):

| File | Command |
| ---- | ------- |
| `single_base.j2k` | `opj_compress -i source.pgm -o single_base.j2k -r 1` (one tile, one tile-part) |
| `tiled_base.j2k` | `opj_compress -i source.pgm -o tiled_base.j2k -t 32,32` (four tiles) |
| `tileparts_base.j2k` | `opj_compress -i source.pgm -o tileparts_base.j2k -t 32,32 -TP R -r 1` (four tiles, six tile-parts each) |

Every other `.j2k` is a byte edit of its base. In all of them the SOT `Psot`
of the last tile-part is set to 0, then:

| File | Edit | OpenJPEG 2.5.0 |
| ---- | ---- | -------------- |
| `*_psot0_eoc` | none | decodes; equals the base's decode |
| `*_psot0_junk_tail` | EOC replaced by `41 42` | warns "Stream does not end with EOC", decodes the same |
| `*_psot0_no_eoc` | EOC removed | `segment too long`, no output |
| `single_psot0_no_eoc_trunc100` | EOC and the last 100 bytes removed | `segment too long`, no output |
| `single_psot0_one_byte_after_sod` | cut one byte after SOD | `Tile part length size inconsistent with stream length` |

`*.expected.pgm` are `opj_decompress` outputs of the bases. Each shape that
decodes gives exactly that output.

Not covered, because they hit other tile-part rules: a `Psot = 0` that is not
on the last tile of the grid, or on a tile-part before the tile's last
declared one. OpenJPEG decodes those with missing tiles or missing parts; this
crate rejects them (missing tile, `TNsot` mismatch).
