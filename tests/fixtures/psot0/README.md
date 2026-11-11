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
| `jp2_p0eoc_cut_in_com_xml` | `single_psot0_eoc` with a COM before SOD | ends inside the COM, whose text is an `xml ` box header | nothing (the rest of the codestream is not metadata) | decodes, equals `single` | the same, no `xml ` reported |
| `jp2_p0eoc_cut_in_com_uuid` | the same, with a `uuid` box header | ends inside the COM | nothing | decodes, equals `single` | the same, no `uuid` reported |
| `jp2_p0eoc_cut_in_com_rreq` | the same, with an `rreq` box header | ends inside the COM | nothing | decodes, equals `single` | the same, no reader requirements reported |
| `jp2_bypass_short989` | `bypass_psot0_eoc` | `LBox` 8 + 989, 510 bytes short | nothing (the rest of the codestream reads as boxes) | decodes, equals `bypass` | the same |
| `jp2_lbox0_p0eoc_xml` | `single_psot0_eoc` | `LBox` 0 | `xml ` (inside the box) | decodes, equals `single` | `Codestream` (left after the last packet) |
| `jp2_t2bomb_visits` | `t2bomb(512, 512, 300)`: 2^14 code-blocks, 300 layers | right, no EOC | an empty `free` box | decodes, all 128 | `Codestream` (left after the last packet); the walk stops at its visit bound |
| `jp2_t2bomb_blocks` | `t2bomb(520, 520, 1)`: 16900 code-blocks, 1 layer | right, no EOC | an empty `free` box | decodes, all 128 | `Codestream` (left after the last packet); the tile is past the geometry bound and not walked |
| `jp2_t2_many_layers` | `t2bomb(32, 28, 19000)`: 56 code-blocks, 19000 layers | right, no EOC | an empty `free` box | decodes, all 128 | the same; the walk spends about 1.08 million visits, within its 2^22 bound |
| `jp2_t2_sweep_rpcl`, `jp2_t2_sweep_pcrl` | `sweep(512, 64, RPCL)` and `sweep(512, 64, PCRL)`: 64 components, 1148 empty packets | right, no EOC | an empty `free` box | decodes, all 128 (64 components) | `Codestream` (left after the last packet); the sweep's 256^2 positions, each checking 64 components at 2 resolutions, pass the visit bound |
| `jp2_t2_sweep_cprl` | `cprl_sweep(130)`: one component, 1x1 precincts, 16900 empty packets | right, no EOC | an empty `free` box | decodes, all 128 | `Codestream` (left after the last packet); 16900 precincts, past the geometry bound |
| `jp2_t2_sweep_cprl_tall` | `cprl_steps(1, 1, 2^24, 1, [0x0F, 0xF1], 4 x 1024 code-blocks, 0)`: one 1-wide column, 512 precincts, 2^14 code-blocks, 512 empty packets | right, no EOC | an empty `free` box | decodes, all 128 | `Codestream` (left after the last packet); resolution 0 (empty in x) sets the y step to 2 and resolution 1 the x step to 2, so the sweep's 2^23 positions (about 25 million visits) pass the visit bound |
| `jp2_t2_sweep_cprl_poc` | `cprl_steps(0, 128, 128, 32, PP 15 then 1 on top, 64 x 64 code-blocks, 31)`: 31 POC volumes, 4128 empty packets | right, no EOC | an empty `free` box | decodes, all 128 | `Codestream` (left after the last packet); each volume sweeps 64^2 positions at 34 visits each, about 4.3 million visits in all, past the visit bound |

When the box ends at least two bytes before the end of the file and holds
the tile-part's `SOT`, the codestream ends at the box's end if tier-2 walks
that tile's packets to exactly there, and runs to the end of the file
otherwise. The `jp2_t2bomb_*` and `jp2_t2_many_layers` files are written by
`t2bomb` in the same script (one tile, one resolution, 4x4 code-blocks, each
packet the byte 0x80, which visits every code-block and includes none; their
`ihdr` matches their size). The `jp2_t2_sweep_rpcl` and `jp2_t2_sweep_pcrl`
files are written by `sweep`: two thin components set the sweep's step to 2 on
both axes, and 62 more are checked at every position but hold two packets
each. `jp2_t2_sweep_cprl` is written by `cprl_sweep` and the other
`jp2_t2_sweep_cprl_*` files by `cprl_steps`. A CPRL sweep's step on each axis
is the finest precinct span over all of the component's resolutions, empty
ones included, and each axis can take it from a different resolution, so the
sweep can visit many more positions than there are precincts. A POC marker
repeats the sweep once per volume.
The walk is bounded, and past a bound it answers no, so they run to the end of
the file, as they do without the check. The `Codestream` rows are a
known difference, recorded in `docs/correctness.md`: the box's end is not
where the codestream ends, so the tile-part runs to the end of the file and
the bytes after its last packet are refused, as for a raw codestream.

Not covered, because they hit other tile-part rules: a `Psot = 0` that is not
on the last tile of the grid, or on a tile-part before the tile's last
declared one. OpenJPEG decodes those with missing tiles or missing parts; this
crate rejects them (missing tile, `TNsot` mismatch).
