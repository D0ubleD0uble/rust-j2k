# POC coverage fixtures

Used by `tests/poc_coverage.rs`. `opj_compress` encodes whose POC volumes
leave out packets of the tile, and controls whose volumes cover it.

## Source and tools

`scripts/gen-poc-coverage-fixtures.py` writes the two sources, runs OpenJPEG
2.5.0 `opj_compress` (system package) on them, and checks each file with
`opj_decompress` 2.5.0. Both sources are a gradient plus noise from a fixed
LCG, as raw big-endian 16-bit samples, component after component:

| Source | Shape |
| ------ | ----- |
| `res_source.raw` | 32 x 32, 1 component, 10-bit unsigned |
| `comp_source.raw` | 32 x 16, 4 components, 12-bit unsigned |

`RES` is `-F 32,32,1,10,u@1x1 -n 2`. `COMP` is
`-F 32,16,4,12,u@1x1:1x1:1x1:1x1 -n 3 -p LRCP -r 10,1 -M 33` (two layers,
the last lossless). All are lossless, so a full decode is the source.

| File | Command | `opj_decompress` 2.5.0 | Here |
| ---- | ------- | ---------------------- | ---- |
| `poc_res_missing.j2k` | `opj_compress -i res_source.raw -o poc_res_missing.j2k RES -POC T1=0,0,1,1,1,LRCP` | resolution 1 left empty: all 1024 samples wrong, by up to 993 | `Codestream` |
| `poc_res_covered.j2k` | `... RES -POC T1=0,0,1,1,1,LRCP/T1=1,0,1,2,1,LRCP` | the source | the source |
| `poc_comp_missing.j2k` | `opj_compress -i comp_source.raw -o poc_comp_missing.j2k COMP -POC T1=0,3,20,3,4,CPRL` | components 0 to 2 left empty (0 and 2048), by up to 3200 | `Codestream` |
| `poc_comp_covered.j2k` | `... COMP -POC T1=0,0,20,3,3,LRCP/T1=0,3,20,3,4,CPRL` | the source | the source |
| `poc_res_tileparts.j2k` | `poc_res_covered.j2k`, with its second POC volume moved to the second tile-part's header | the source | the source |

`opj_compress` writes only the packets its volumes reach, and a tile-part
per volume with the whole POC in the first tile-part's header. For
`poc_res_tileparts.j2k` the script moves the second volume's entry to the
second tile-part and fixes both `Psot`; the packets are unchanged.

The two refused shapes are the repros of #260 (`poc_10024`, `poc_30027`),
on smaller sources.
