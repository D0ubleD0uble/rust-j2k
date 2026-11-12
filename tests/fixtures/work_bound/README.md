# Tier-2 work bound fixtures

Used by `tests/work_bound.rs`, and the JP2 one by `tests/psot_zero.rs`
and the `read_header` tests. Codestreams whose packets are a byte or less
each, yet could make tier-2 walk far more than their bytes, and two
`opj_compress` files of the same shape that must decode.

## Source

All but the `opj_*` files are written from scratch by
`scripts/gen-work-bound-fixtures.py` (no encoder, no input image). With
`--check` it runs each through `opj_decompress` 2.5.0 (the system package)
and prints the outcome and the time. With `--big DIR` it writes the larger
timing shapes to `DIR` instead, each as a raw codestream and as a
`Psot = 0` JP2 file; those are not committed.

Every packet there is 0x80 or 0xC0 (present, then tag-tree bits that
include no code-block) or 0x00 (empty). A decoder that walks them all reads
a flat image of 128.

| File | Shape | Steps | OpenJPEG 2.5.0 | Here |
| ---- | ----- | ----- | -------------- | ---- |
| `t2bomb_2048x1024_1000.j2k` | `t2bomb(2048, 1024, 1000)`: one tile, no wavelet levels, 4x4 code-blocks in one precinct (2^17 blocks), 1000 LRCP layers, every packet 0x80, so the inclusion root is past each layer | 2.0e3 | decodes, all 128, 6.6 s | decodes, all 128 |
| `t2skip_2048x1024_40000.j2k` | `t2skip(2048, 1024, 40000)`: the same on 512 x 256 code-blocks, 40000 layers, first packet 0xC0, so the root resolves and each layer reads a bit for each of its two children; the decode then steps over each child's part of each row | 2.0e7 | decodes, all 128, 6.1 s | `Limit` |
| `jp2_t2skip_512_20000.jp2` | `t2skip(512, 512, 20000)` on 128 x 128 code-blocks as a `Psot = 0` JP2 (`jp2_psot0`), then an empty `free` box | 5.1e6 | not run | `read_header`'s walk stops at its bound, so the codestream runs to the end of the file; `Codestream` (left after the last packet) |
| `sweep_rpcl_1024_64.j2k`, `sweep_pcrl_1024_64.j2k` | `sweep(1024, 64, RPCL)` and `PCRL`: two thin components set the sweep step to 2 on both axes; 62 more, subsampled 255 times both ways, are checked at each of the 512^2 positions | 3.4e7 | decodes, 64 components, all 128, 0.5 s | `Limit` |
| `cprl_tall.j2k` | `cprl_tall(0)`: one column of 2^24 rows on x in [1, 2), one level; resolution 0 sets the y step to 2 and resolution 1 the x step to 2, so the CPRL sweep visits 2^23 positions for 512 packets | 2.5e7 | decodes, all 128, 0.7 s | `Limit` |
| `cprl_tall_poc31.j2k` | `cprl_tall(31)`: the same, with a POC marker repeating the CPRL order 31 times | 7.8e8 | decodes, all 128, 20.6 s | `Limit` |

The same generator functions, with other arguments, write the shapes of
`tests/fixtures/psot0/jp2_t2bomb_*` and `jp2_t2_sweep_*` (those are JP2 files
from `scripts/gen-psot0-jp2-fixtures.py`, made for the walk `read_header`
makes).

## Encoder output

`opj_patch2048_b4_q64.j2k` (563 B) and `opj_patch2048_b4_q100.j2k` (794 B)
are written by `scripts/gen-work-bound-fixtures.py --opj`, which runs
`opj_compress` 2.5.0 on a 2048^2 8-bit image of 128 with one 8 x 8 patch of
noise in its top-left corner (`patch_image`; Python's `random` seeded with
1; the PGM's SHA-256 is
`276f0c279b6fde7eefeebd8c42c7fb246cb55d52f17473ed001a1bd22e391f80`):

```
opj_compress -i patch2048.pgm -o opj_patch2048_b4_q64.j2k -b 4,4 \
  -q 20,20.625,21.25,...,59.375      # 64 layers, 20 dB up by 0.625
opj_compress -i patch2048.pgm -o opj_patch2048_b4_q100.j2k -b 4,4 \
  -q 20,20.4,20.8,...,59.6           # 100 layers, 20 dB up by 0.4
```

OpenJPEG's encoder sets the "present" bit of every packet, so each layer's
header names all 262144 code-blocks, almost none ever included. Both decode
here to exactly what `opj_decompress` 2.5.0 decodes; `tests/work_bound.rs`
holds the SHA-256 of its 8-bit output (`80c5ce50…d529` for 64 layers,
`3f2603a0…0251` for 100).
