# Tier-2 work bound fixtures

Used by `tests/work_bound.rs`. Raw codestreams whose packets are a byte or
less each, yet make tier-2 walk far more than their bytes. Each is past the
decode's tier-2 work bound and refused with `Error::Limit`.

## Source

All are written from scratch by `scripts/gen-work-bound-fixtures.py` (no
encoder, no input image). With `--check` it runs each through
`opj_decompress` 2.5.0 (the system package) and prints the outcome and the
time. With `--big DIR` it writes the larger timing shapes of issue #235 to
`DIR` instead, each as a raw codestream and as a `Psot = 0` JP2 file; those
are not committed.

Every packet is either 0x80 (present, then an inclusion tag-tree root bit of
0, so every code-block of the precinct is visited and none is included) or
0x00 (empty). A decoder that walks them all reads a flat image of 128.

| File | Shape | Visits | OpenJPEG 2.5.0 | Unbounded here (release) |
| ---- | ----- | ------ | -------------- | ------------------------ |
| `t2bomb_2048x1024_1000.j2k` | `t2bomb(2048, 1024, 1000)`: one tile, no wavelet levels, 4x4 code-blocks in one precinct (2^17 blocks), 1000 LRCP layers | 1.3e8 | decodes, all 128, 6.6 s | decodes, 4.6 s |
| `sweep_rpcl_1024_64.j2k`, `sweep_pcrl_1024_64.j2k` | `sweep(1024, 64, RPCL)` and `PCRL`: two thin components set the sweep step to 2 on both axes; 62 more, subsampled 255 times both ways, are checked at each of the 512^2 positions | 3.4e7 | decodes, 64 components, all 128, 0.5 s | decodes, 0.4 s |
| `cprl_tall.j2k` | `cprl_tall(0)`: one column of 2^24 rows on x in [1, 2), one level; resolution 0 sets the y step to 2 and resolution 1 the x step to 2, so the CPRL sweep visits 2^23 positions for 512 packets | 2.5e7 | decodes, all 128, 0.7 s | decodes, 0.4 s |
| `cprl_tall_poc31.j2k` | `cprl_tall(31)`: the same, with a POC marker repeating the CPRL order 31 times | 7.8e8 | decodes, all 128, 20.6 s | decodes, 6.5 s |

The same generator functions, with other arguments, write the shapes of
`tests/fixtures/psot0/jp2_t2bomb_*` and `jp2_t2_sweep_*` (those are JP2 files
from `scripts/gen-psot0-jp2-fixtures.py`, made for the walk `read_header`
makes).
