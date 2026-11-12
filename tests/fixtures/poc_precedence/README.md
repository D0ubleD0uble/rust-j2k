# POC precedence fixtures

Used by `tests/poc_precedence.rs`. Each codestream has a POC in the main
header and one in its tile-part headers. In the first two, the tile-part POC
overrides the main one (A.6.6), and the packets follow it, so the decode must
equal `source.raw`. The grading reference is `source.raw` itself:
`opj_decompress` 2.5.0 appends the tile volumes after the main one and decodes
those two files wrong (see `docs/correctness.md`, "A tile-part POC overrides the
main header's").

## Source and tools

`source.raw` is three 24x16 8-bit unsigned components, one plane after
another, with sample `(37c + 5x + 11y + ((xy) mod 7) * 13) mod 256` (SHA-256
`77d4b5fe9512bb0c20e75805b38f52960e4d4a99c9c973bd34406d3c0a50a11f`).
`scripts/gen-poc-precedence-fixtures.py` writes it and both codestreams, with
OpenJPEG 2.5.0 `opj_compress` and `opj_decompress` (system package).

The base encode is

```sh
opj_compress -i source.raw -o tile_only.j2k -F 24,16,3,8,u -mct 0 -n 3 \
  -b 16,16 -r 20,10,1 -POC T1=0,0,3,1,3,CPRL/T1=1,0,3,3,3,RLCP
```

It is lossless, has one tile in two tile-parts (one per volume), and carries
both volumes in one POC in the first tile-part header. `opj_decompress`
round-trips it exactly; the script checks that before it edits anything.

| File | Edit to the base encode | `opj_decompress` 2.5.0 |
| ---- | ----------------------- | ---------------------- |
| `poc_main_and_tile.j2k` | a main-header POC before the first SOT: one LRCP volume over resolution 0, components 0-2, layers 0-2 | 380, 383 and 384 of 384 samples wrong per component |
| `poc_main_and_split_tile.j2k` | the same, and the second tile volume moved into its own POC in the second tile-part header (both Psot values fixed) | the same |

The script also checks that the split without the main POC round-trips under
`opj_decompress`, so the split itself is valid.

`poc_main_then_later_tile_part.j2k` starts from a second encode, with
`-POC T1=0,0,3,1,3,LRCP/T1=1,0,3,3,3,RLCP` and otherwise the same flags (it
round-trips too). Its first volume, LRCP over resolution 0, moves to the main
header, where it is byte for byte the main POC above. The second moves into
its own POC in the second tile-part header. The first tile-part is left with
no POC, which A.6.6 does not allow, so its packets can only follow the main
POC. `opj_decompress` 2.5.0 returns the source for this file, and so does
this crate.

`poc_tile_leaves_out_packets.j2k` pins which volumes the coverage check reads.
It is the base encode with a main POC of one LRCP volume over every
resolution, component and layer, and the first tile-part's POC cut to its
CPRL volume over resolution 0 (its Psot fixed). The tile's own
volumes replace the covering main one and leave out resolutions 1-2, so this
crate refuses it as `Codestream` before reading a packet. `opj_decompress`
2.5.0 walks the main volume first and returns 381, 384 and 384 of 384 samples
wrong per component.
