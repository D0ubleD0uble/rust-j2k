# Override order fixtures

Used by `tests/override_order.rs`. A COC or QCC applies to its component
wherever it sits among the other markers of its header (A.6). Each file
carries a COD and QCD that are wrong for every component, and one COC and QCC
per component with the right values. The `_usual` files put the COC and QCC
after the COD and QCD. The `_reordered` files put them before. Every file
decodes to `../multicomponent_lossless.j2k`'s image here. `opj_decompress`
2.5.0 decodes the `_reordered` files to a different image (see
`docs/openjpeg-departures.md`, "COC and QCC apply wherever they sit in the
header").

## Source and tools

Written by `scripts/gen-override-order-fixtures.py` from
`../multicomponent_lossless.j2k` (three 24x16 8-bit reversible components, one
tile; itself a lossless OpenJPEG 2.5.0 encode, see `../README.md`). No encoder
runs. The script changes the markers by hand:

- The COD asks for 32x32 code-blocks instead of 16x16, and the QCD has two more
  guard bits and every exponent shifted by 4.
- Three COCs (`Scoc` 0, `SPcoc` the source's `SPcod`) and three QCCs (the
  source's QCD body) restore the source's values.
- In the `main_` files these replace the source's COD and QCD in the main
  header. In the `tile_` files the source's COD and QCD stay in the main header,
  the markers go in the tile-part header before `SOD`, and `Psot` grows by
  their length.

The script runs `opj_decompress` 2.5.0 (system package, checked by
`scripts/oracle_tools.py`) on each file and on the source, and prints whether
the output matches the source's.

| File | Markers in | Order | SHA-256 | `opj_decompress` 2.5.0 |
| ---- | ---------- | ----- | ------- | ---------------------- |
| `main_usual.j2k` | main header | COD QCD COC* QCC* | `c096390bcbe55011f86817fa642f672d2fa8100356e595d3064b061857c3b11a` | the source's image |
| `main_reordered.j2k` | main header | COC* QCC* COD QCD | `573caa4279357cc2c961c8a4037f314ca6a5ff9465cbdbee0b311916f7a0cb69` | a different image, 1138 of 1152 samples wrong, no error |
| `tile_usual.j2k` | tile-part header | COD QCD COC* QCC* | `cbc9f448cd35c67b0d443f709ef2754aaa1af98c5cfe499465391e3abd8cfb17` | the source's image |
| `tile_reordered.j2k` | tile-part header | COC* QCC* COD QCD | `a0b09247e5eb15499230cb1564dcad0a56acdb38503876333309b7a5d9fbd41a` | a different image, 1138 of 1152 samples wrong, no error |

The grading reference is the source's decode, not `opj_decompress` on the
`_reordered` files.
