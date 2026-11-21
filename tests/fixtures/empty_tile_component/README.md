# Empty tile-component fixtures

Used by `tests/empty_tile_component.rs`. Each codestream has a tile narrower
or shorter than a component's sub-sampling, so the tile holds no sample of
that component (ISO B.3, Eq. B-12 gives `tcx0 == tcx1` or `tcy0 == tcy1`).
SIZ allows it. The tile-component has no precinct, so no packet (#595).
OpenJPEG 2.5.0 decodes every file here but `opjfail_l1_lrcp` and
`probe_none_l1`, which it fails to decode whole (see below).

## Source

Written by `scripts/gen-empty-tile-component-fixtures.py` with OpenJPEG
2.5.0 (Ubuntu package `libopenjp2-tools` 2.5.0-2ubuntu0.5) on `PATH`. Every
file is synthetic: no outside image goes in.

- The encodes are `opj_compress` output from raw sources the script makes:
  one byte per sample from a fixed LCG, seed 595, each component at its
  sub-sampled size. The script checks that `opj_decompress` gives the source
  back, and, for `opjfail_l1_lrcp` and `probe_none_l1`, that it fails to
  decode the whole file.
- The probes are written byte by byte in 1x1 tiles, every packet empty
  (0x00): one packet for each component and resolution that holds a sample.

## Oracles

Each oracle is `opj_decompress` output, the components' samples one after
another, 8 bits each:

- `<name>.opj.raw`: the whole image.
- `<name>.tile<N>.opj.raw` (encodes only): `opj_decompress -t <N> -c 0`, the
  tile that holds no sample of component 1, component 0 alone.
  `opj_decompress -t <N>` of both components fails ("Failed to decode all
  used components"), as this decoder refuses it.
- `<name>.r1.opj.raw` (encodes under two levels): `opj_decompress -r 1`.
- `opjfail_l1_lrcp.tiles.opj.raw`: `opj_decompress -t <N> -c <the
  components tile N holds a sample of>` for each of the 49 tiles in turn.
  `opj_decompress` of the whole file fails ("failed to decode image"):
  with a wavelet level, `opj_j2k_update_image_data` reads resolution 0 of
  a tile-component with no sample, which starts before the output's
  origin, and returns false. The source cannot serve as the oracle, since
  under the image offset `opj_compress` reads the sub-sampled components
  at sizes that differ from the codestream's.
- `probe_none_l1.tiles.opj.raw`: `opj_decompress -t 0`, tile 0 alone.
  Tile 1 holds no sample of the one component, and `opj_decompress` of
  the whole file fails ("Failed to decode tile 2/2").

## Files

| File | Image, components (XRsiz x YRsiz), tiles | Levels | Order |
| ---- | ---------------------------------------- | ------ | ----- |
| `x_l0_{lrcp,rlcp,rpcl,pcrl,cprl}` | 24 x 20, (1x1, 8x1), 6 x 5: tile 3 of each row holds no sample of component 1 | 0 | each of the five |
| `x_l2_{lrcp,rpcl}` | 16 x 20, (1x1, 16x1), 6 x 5: tiles 1 and 2 of each row hold none | 2 | LRCP, RPCL |
| `x_l2_poc` | the same, with a POC in every tile: resolutions 0 and 1 in CPRL, then 2 in LRCP | 2 | POC |
| `y_l0_{lrcp,rpcl}` | 20 x 24, (1x1, 1x8), 5 x 6: the bottom row of tiles holds none | 0 | LRCP, RPCL |
| `y_l2_{lrcp,rpcl}` | 20 x 16, (1x1, 1x16), 5 x 6: rows 1 and 2 hold none | 2 | LRCP, RPCL |
| `xy_l0_{lrcp,rpcl}` | 24 x 24, (1x1, 8x8), 6 x 6 | 0 | LRCP, RPCL |
| `opjfail_l1_lrcp` | 35 x 12 at offset (1, 7), (1x1, 1x3, 5x5), 5 x 2 at offset (1, 6) | 1 | LRCP |
| `probe_1pkt` | 2 x 1, (1x1, 2x1), 1 x 1: tile 1 holds no sample of component 1 and carries one packet | 0 | LRCP |
| `probe_2pkt` | `probe_1pkt` with a byte after tile 1's packet, which OpenJPEG ignores and a strict decode here refuses | 0 | LRCP |
| `probe_ht_1pkt` | `probe_1pkt` with a `CAP` and HT code-blocks | 0 | LRCP |
| `probe_y_1pkt` | 1 x 2, (1x1, 1x2), 1 x 1 | 0 | LRCP |
| `probe_none_l1` | 2 x 1, (2x1), 1 x 1: tile 1 holds no sample of any component | 1 | LRCP |
| `probe_xy_l2` | 3 x 3, (1x1, 2x2), 1 x 1: column 1 and row 1 hold no sample of component 1 | 2 | LRCP |

OpenJPEG 2.5.0 does not give the source back where the image width is not a
multiple of `XRsiz`, nor for a component one sample wide at an odd origin
under wavelet levels, and `opj_compress` crashes on a 1x1 tile-component
under two levels. The encodes avoid those shapes; `probe_xy_l2` covers both
ways at two levels.
