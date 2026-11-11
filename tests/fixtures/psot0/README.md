# Psot = 0 last tile-part fixtures

Used by `tests/psot_zero.rs`. Small synthetic codestreams whose last tile-part
has `Psot = 0`, with and without the closing EOC.

## Source and tools

`source.pgm` is a 48x40 8-bit image, row-major. Sample (x, y) is
`x*5 + y*3 + n` mod 256, with `n = random.randint(0, 7)` drawn per sample after
`random.seed(7)`. The four base codestreams were made from it with
OpenJPEG 2.5.0 (`opj_compress`/`opj_decompress` from the system package):

| File | Command |
| ---- | ------- |
| `single_base.j2k` | `opj_compress -i source.pgm -o single_base.j2k -r 1` (one tile, one tile-part) |
| `tiled_base.j2k` | `opj_compress -i source.pgm -o tiled_base.j2k -t 32,32` (four tiles) |
| `tileparts_base.j2k` | `opj_compress -i source.pgm -o tileparts_base.j2k -t 32,32 -TP R -r 1` (four tiles, six tile-parts each) |
| `bypass_base.j2k` | `opj_compress -i source.pgm -o bypass_base.j2k -M 1` (one tile, one tile-part, BYPASS code-blocks) |

Every other `.j2k` is a byte edit of its base. In all of them the SOT `Psot`
of the last tile-part is set to 0, then:

| File | Edit | OpenJPEG 2.5.0 |
| ---- | ---- | -------------- |
| `*_psot0_eoc` | none | decodes; equals the base's decode |
| `*_psot0_junk_tail` | EOC replaced by `41 42` | warns "Stream does not end with EOC", decodes the same |
| `*_psot0_no_eoc` | EOC removed | `segment too long`, no output |
| `single_psot0_no_eoc_trunc100` | EOC and the last 100 bytes removed | `segment too long`, no output |
| `single_psot0_one_byte_after_sod` | cut one byte after SOD | `Tile part length size inconsistent with stream length` |
| `bypass_psot0_eoc` | none (written by `scripts/gen-psot0-jp2-fixtures.py`) | decodes; equals the base's decode |

`*.expected.pgm` are `opj_decompress` outputs of the bases. Each shape that
decodes gives exactly that output.

## JP2 files

The `jp2_*.jp2` files wrap these codestreams in JP2 boxes:
`scripts/gen-psot0-jp2-fixtures.py` writes them, and with `--check` runs each
through `opj_decompress` 2.5.0 and compares the output with the base's
`*.expected.pgm`. Each is a signature, `ftyp`, a `jp2h` (`ihdr` 48x40, one
8-bit component, grayscale `colr`), a `jp2c` box, then the boxes listed. A
"right" length ends the box where the codestream bytes end; `LBox` 108 and
200 end it inside the codestream (the PR #186 shapes), and the `cut_in` and
`short989` files end it inside the codestream where the bytes after the box
read as whole boxes (from the PR #227 review).

| File | Codestream | `jp2c` length | After it | OpenJPEG 2.5.0 | This crate |
| ---- | ---------- | ------------- | -------- | -------------- | ---------- |
| `jp2_p0noeoc_xml` | `single_psot0_no_eoc` | right | `xml ` | decodes, equals `single` | the same, `xml ` reported |
| `jp2_p0noeoc_badcolr` | `single_psot0_no_eoc` | right | 2-byte `colr` | decodes, equals `single` | the same |
| `jp2_p0noeoc_tileparts_xml_uuid` | `tileparts_psot0_no_eoc` | right | `xml `, `uuid` | decodes, equals `tileparts` | the same, both reported |
| `jp2_p0noeoc_xml_junk` | `single_psot0_no_eoc` | right | `xml `, then `abc` | decodes, equals `single` | the same, `xml ` reported |
| `jp2_short108_p0eoc_xml` | `single_psot0_eoc` | `LBox` 108 | `xml ` | decodes, equals `single` | `Codestream` (left after the last packet) |
| `jp2_p0eoc_short_badcolr` | `single_psot0_eoc` | `LBox` 108 | 2-byte `colr` | decodes, equals `single` | `Codestream` (left after the last packet) |
| `jp2_t2_last0_short` | `tileparts_psot0_eoc` | `LBox` 200 | 2-byte `colr` | decodes, equals `tileparts` | `Codestream` (left after the last packet) |
| `jp2_p0eoc_cut_in_com` | `single_psot0_eoc` with a COM after SIZ | ends inside the COM, whose text is a box header | one whole `free` box (the rest of the codestream) | decodes, equals `single` | the same |
| `jp2_p0eoc_cut_in_tile_part_com` | `single_psot0_eoc` with a COM before SOD | ends inside the COM, whose text is a box header | one whole `free` box (the rest of the codestream) | decodes, equals `single` | the same |
| `jp2_bypass_short989` | `bypass_psot0_eoc` | `LBox` 8 + 989, 510 bytes short | nothing (the rest of the codestream reads as boxes) | decodes, equals `bypass` | the same |
| `jp2_lbox0_p0eoc_xml` | `single_psot0_eoc` | `LBox` 0 | `xml ` (inside the box) | decodes, equals `single` | `Codestream` (left after the last packet) |

When the box ends at least two bytes before the end of the file and holds
the tile-part's `SOT`, the codestream is decoded to the box's end first, and
read to the end of the file only if that fails. The `Codestream` rows are a
known difference, recorded in `docs/correctness.md`: the box's end is not
where the codestream ends, so the tile-part runs to the end of the file and
the bytes after its last packet are refused, as for a raw codestream.

Not covered, because they hit other tile-part rules: a `Psot = 0` that is not
on the last tile of the grid, or on a tile-part before the tile's last
declared one. OpenJPEG decodes those with missing tiles or missing parts; this
crate rejects them (missing tile, `TNsot` mismatch).
