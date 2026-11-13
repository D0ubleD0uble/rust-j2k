# Partial decode fixtures

Used by `tests/partial_decode.rs`, which decodes these with
`DecodeOptions::strict` off and compares the samples with
`opj_decompress -allow-partial` on the same bytes.

## Source and tools

`scripts/gen-partial-fixtures.py` writes the two sources, runs OpenJPEG
2.5.0 `opj_compress` and `opj_decompress` (system package) on them, and
checks each result it relies on. Both sources come from a fixed LCG
(`state * 1103515245 + 12345` mod 2^31, taking `state >> 16`):

| Source | Shape |
| ------ | ----- |
| `source.pgm` | 64 x 64, 8-bit: `3x + 2y` plus noise from -4 to 4 (seed 214), clamped |
| `tp_r_source.raw` | 2 x 99, 1-bit, one byte per sample (seed 167) |

## Files

| File | Command | Content |
| ---- | ------- | ------- |
| `lrcp.j2k` | `opj_compress -i source.pgm -o lrcp.j2k -n 6 -p LRCP` | one tile, one layer, five levels, 2075 bytes |
| `lrcp.prefixes` | the script | for each reduction r, the shortest prefix of `lrcp.j2k` whose `opj_decompress -allow-partial -r r` output equals the whole file's |
| `lrcp_r<r>.pgm` | `opj_decompress -i lrcp.j2k -r <r>` | the whole file at reduction r |
| `lrcp_r<r>_short.pgm` | `opj_decompress -allow-partial -r <r>` of the prefix one byte shorter than `lrcp.prefixes` gives | differs from `lrcp_r<r>.pgm` (the script checks) |
| `tiles.j2k` | `opj_compress -i source.pgm -o tiles.j2k -t 32,32 -n 3` | four 32 x 32 tiles |
| `tiles.cut` | the script | 971: the middle of tile 1's tile-part |
| `tiles_cut.pgm` | `opj_decompress -allow-partial` of the first 971 bytes of `tiles.j2k` | tiles 2 and 3 are 0 |
| `overlap.j2k` | `opj_compress -i source.pgm -o overlap.j2k -n 1 -POC T1=0,0,1,1,1,LRCP/T1=0,0,1,1,1,PCRL` | 5512 bytes: every packet written twice; `opj_decompress` gives the source |
| `tp_r.j2k` | `opj_compress -i tp_r_source.raw -o tp_r.j2k -F 2,99,1,1,u -n 6 -t 32,64 -d 22,14 -TP R` | a stray byte in each tile-part of an empty resolution |
| `tp_r.pgm` | `opj_decompress -i tp_r.j2k` | not the source: 117 of 198 samples differ |
| `psot0_single_no_eoc.pgm` | `opj_decompress -allow-partial -i ../psot0/single_psot0_no_eoc.j2k` | `opj_decompress` refuses the file without `-allow-partial` |
| `psot0_tileparts_no_eoc.pgm` | `opj_decompress -allow-partial -i ../psot0/tileparts_psot0_no_eoc.j2k` | the same |

The shortest prefixes the script found, by reduction:

| r | 0 | 1 | 2 | 3 | 4 | 5 |
| - | - | - | - | - | - | - |
| bytes | 2073 | 648 | 294 | 196 | 161 | 141 |

The prefix for r = 0 lacks only the `EOC` marker. The tests check that this
decoder reports each prefix complete at r, and the prefix one byte shorter
not complete at r but complete at r + 1 when the file has that level.
