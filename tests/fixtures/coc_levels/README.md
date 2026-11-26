# COC levels fixture

Used by `tests/coc_levels_reduction.rs` (docs/openjpeg-departures.md, "A
reduction checked against the merged levels").

- `levels5.j2k`: a 64x64 three-component 8-bit reversible 5/3 image, one tile,
  no color transform, 5 decomposition levels. Source: a synthetic ramp plus
  noise (`random.seed(1)`; `(x*3 + y*2 + c*40 + randrange(16)) & 255`) written
  as a P6 PPM, SHA-256
  `0e312092f00ba76cdf31a14e708958ce0d348080bedfeed11c83bcecc5b08a20`. Encoder:
  OpenJPEG 2.5.0, `opj_compress -i in.ppm -o levels5.j2k -n 6 -mct 0`.
- `levels5_r3_opj.ppm`: `opj_decompress -i levels5.j2k -o out.ppm -r 3`, with
  the PPM comment line removed. The oracle for the reduced image.

The test builds the edited file itself: the `COD` is set to 1 level and a
`COC` per component restores 5. `opj_decompress -r 3` refuses that edit at the
`COD`; without `-r` it decodes it to the same image as `levels5.j2k`.
