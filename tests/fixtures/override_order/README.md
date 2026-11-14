# Override order fixtures

Used by `tests/override_order.rs`. A COC or QCC applies to its component
wherever it sits among the other markers of its header (A.6.1, A.6.2, A.6.4, A.6.5; the legal marker order is Figure A.3). Each file
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

- The COD sets the selective arithmetic bypass style (`SPcod` code-block style 0x01), and the QCD has two more
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
| `main_usual.j2k` | main header | COD QCD COC* QCC* | `ac823fbecfbac0c947fdee29335099b18a8b32efcec05fdbb383d7453645fade` | the source's image |
| `main_reordered.j2k` | main header | COC* QCC* COD QCD | `034a72684d89d126b9dfd9d2290c7833683dfecb2818aa51ca0399fe44b303a0` | a different image, 1152 of 1152 samples wrong, no error |
| `tile_usual.j2k` | tile-part header | COD QCD COC* QCC* | `663a5fb9228ccdd970c3d95b2e4bbf1a3fda0debc356f69e739a9c5531137929` | the source's image |
| `tile_reordered.j2k` | tile-part header | COC* QCC* COD QCD | `4d49b6deb5b1b9db0d14ee2b2c46030c2eeb7f044c6bbfa8519d219eed227042` | a different image, 1152 of 1152 samples wrong, no error |

The grading reference is the source's decode, not `opj_decompress` on the
`_reordered` files.
