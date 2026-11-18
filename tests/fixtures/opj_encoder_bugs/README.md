# OpenJPEG encoder bug fixtures

Used by `tests/opj_encoder_bugs.rs`. Two `opj_compress` encodes whose streams
`opj_decompress` decodes, with exit 0, to a wrong image. This decoder refuses
both. Each oracle (`*.opj.raw`) is OpenJPEG's wrong output, so a change to
either behavior shows.

## Source and tools

`scripts/gen-opj-encoder-bug-fixtures.py` writes the sources, runs OpenJPEG
2.5.0 `opj_compress` and `opj_decompress` (Ubuntu 2.5.0-2ubuntu0.5) on them,
and checks that each oracle differs from its source and that a control (the
same encode without the option that triggers the bug) decodes to the source.
Each source is one byte per sample from a fixed LCG (`state * 1103515245 +
12345` mod 2^31, taking `(state >> 16) & 0xFF`, seed 443), each component at
its sub-sampled size, one component after another.

| Source | Shape |
| ------ | ----- |
| `tp_mixed_subsampling.source.raw` | 32 x 32, 2 components, 8-bit: component 0 sub-sampled `1x2` (512 samples), component 1 `2x1` (512) |
| `tp_over255.source.raw` | 68 x 64, 1 component, 8-bit |

## Files

| File | Command | `opj_decompress` 2.5.0 | Here |
| ---- | ------- | ---------------------- | ---- |
| `tp_mixed_subsampling.j2k` | `opj_compress -i tp_mixed_subsampling.source.raw -o tp_mixed_subsampling.j2k -F 32,32,2,8,u@1x2:2x1 -n 1 -p CPRL -c [16,16] -TP R` | 4 tile-parts, the last two empty; component 1 is the constant 128, so 509 of 1024 samples are wrong | strict: `Codestream`; strict off: the same image, with `MissingPackets` for tile 0 |
| `tp_over255.j2k` | `opj_compress -i tp_over255.source.raw -o tp_over255.j2k -F 68,64,1,8,u@1x1 -n 1 -p RPCL -c [4,4] -TP C` | 272 tile-parts, `TNsot` 16; warns `Non conformant codestream TPsot==TNsot`; 4062 of 4352 samples wrong | strict and strict off: `Codestream` |
| `*.opj.raw` | `opj_decompress -i <name>.j2k -o <name>.pgx`, the components' samples one after another | the oracles | |

The controls are in the script, not committed: the first encode without `-TP
R` and the second without `-TP C` decode to the source.
