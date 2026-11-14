# RGN maxshift + bypass fixtures

Used by `tests/roi_bypass.rs`. Lossless encodes of `source.pgm` with an RGN
maxshift (`SPrgn = 3`) and a bypass code-block style. The grading reference is
`source.pgm` itself: `opj_decompress` 2.5.0 decodes all four wrong (see
`docs/openjpeg-departures.md`, "Bypass under a maxshift").

## Source and tools

`source.pgm` is a 64x64 8-bit gray image (SHA-256
`cfb1141cf83df86279cf608cc6766d87df37b5afe32835f52d41b5f457f03d06`): a
diagonal gradient plus noise from a fixed LCG. `scripts/gen-roi-bypass-fixtures.py`
writes it and the codestreams, with OpenJPEG 2.5.0 `opj_compress` (system
package). It checks each codestream for the RGN and the style byte, and that a
control encode without bypass round-trips under `opj_decompress`.

| File | Command | `opj_decompress` 2.5.0 |
| ---- | ------- | ---------------------- |
| `roi_bypass_m1.j2k` | `opj_compress -i source.pgm -o roi_bypass_m1.j2k -n 3 -ROI c=0,U=3 -M 1` | 3814 of 4096 samples wrong, max error 30 |
| `roi_bypass_m5.j2k` | `opj_compress -i source.pgm -o roi_bypass_m5.j2k -n 3 -ROI c=0,U=3 -M 5` | 3811 wrong, max error 21 |
| `roi_bypass_m33.j2k` | `opj_compress -i source.pgm -o roi_bypass_m33.j2k -n 3 -ROI c=0,U=3 -M 33` | 3817 wrong, max error 26 |
| `roi_bypass_m17.j2k` | `opj_compress -i source.pgm -o roi_bypass_m17.j2k -n 3 -ROI c=0,U=3 -M 17` | 3814 wrong, max error 30 |

`-M 1` is bypass, `5` adds termination on every pass, `33` segmentation
symbols, `17` predictable termination. Of these four, `-M 5` is the one that
pins the exact plane: every pass is its own segment there, so a segment opens
at the significance pass of the fourth coded plane, which must stay MQ-coded.
