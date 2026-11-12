# Tile header cost fixtures

Used by `tests/tile_header.rs`. Codestreams of many 1x1 tiles whose cost
lies in the tile headers, not the packets: many components, or a large TLM.
Before issue #256, every tile's header was a copy of the main header, so
these took time in proportion to the tiles times the components (or the TLM
entries) before any check ran.

## Source

Written from scratch by `scripts/gen-tile-header-fixtures.py` (no encoder,
no input image). With `--check` it runs each through `opj_decompress` 2.5.0
(the system package) and prints the outcome. With `--big DIR` it writes the
larger timing shapes from the issue to `DIR` instead; those are not
committed. `tests/tile_header_cost.rs` builds the same `tilecomp` layout in
code, at 4096 tiles.

| File | Shape | OpenJPEG 2.5.0 | Here |
| ---- | ----- | -------------- | ---- |
| `tilecomp_16_256.j2k` | `tilecomp(16, 256)`: the image `[1, 17)^2` in 1x1 tiles (256 tiles), component 0 full size and 255 more subsampled 255 times both ways, so empty on tile 0; no levels, one QCD step | fails, "failed to decode image" | `Codestream`, "tile 0 has an empty area on component 1" |
| `tilecomp_heavy_16_256.j2k` | `tilecomp(16, 256, 32)`: the same under a heavy main header, 32 levels, explicit precincts, 97 expounded QCD steps | fails, "failed to decode image" | the same |
| `tilecomp_override_16_256.j2k` | `tilecomp(16, 256, 0, True, True)`: the light header, with a COD and a QCD restating it in every tile, so each tile overrides every component | fails, "failed to decode image" | the same |
| `tlm_32_16k.j2k` | `tlm(32, 16)`: 1024 1x1 tiles of one component after 16 KB of TLM entries (ST = 1, SP = 0); every packet empty | decodes, all 128 | decodes, all 128 |
