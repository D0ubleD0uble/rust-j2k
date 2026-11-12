# Tier-1 work bound fixtures

Used by `tests/work_limit.rs` and the margin test in
`src/tier1/work_bound_tests.rs`. A codestream whose code-blocks ask the
block decoder for far more work than its bytes, and an `opj_compress` file
that spends the most work per byte seen from an encoder, which must decode.

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
