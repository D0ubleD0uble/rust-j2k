# Maxshift background in the H.1 band

Used by `tests/roi_h1_band.rs`. A lossless 8x8 codestream with an RGN maxshift
of `SPrgn = 7` and no region of interest. Every coefficient is background, and
half have bit 6 (`s - 1`) set, the band where H.1 and OpenJPEG differ (see
`docs/openjpeg-departures.md`, "Region of interest on HT code-blocks").

## Provenance

- `source.pgm`: 8x8 8-bit gray, SHA-256
  `a9885e4bf39ac3936a4db35fa7da82581b17cd208795a3ea1b405365b2f5c4a5`. Every
  `|sample - 128|` is below 128. The values 192, 64, 228, 28, 255, 1, 200 and
  50 have `|sample - 128| >= 64`, which is half the image.
- `roi_band_s7.j2k`: SHA-256
  `2e5d29b1499b6e776a1bbfb1134be7939efd19e4629640e9a3c0ab4bae76a069`.
  `scripts/gen-roi-h1-band-fixture.py` runs `opj_compress -i source.pgm -n 1`
  (OpenJPEG 2.5.0, lossless, one resolution, so the coefficients are the
  level-shifted samples), then adds an `RGN` marker segment (`Srgn = 0`,
  `SPrgn = 7`) and raises the packet header's zero bit-planes by 7. That is
  what a maxshift encoder signals for a block with no region (H.2 counts them
  against `Mb + s`). The coded bytes are unchanged. `opj_compress -ROI` cannot
  make this stream: it shifts every coefficient of the component, so it has
  no background.

## Expected output

The source image. It does not come from this decoder: the codestream is
lossless and every coefficient is background (`q < 2^7`), so H.1 gives back
each sample. `opj_decompress` 2.5.0 returns 128 for the eight band values
(32 of the 64 samples): it takes them for region and shifts them away.
