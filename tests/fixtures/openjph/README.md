# OpenJPH test codestreams

The 16 HTJ2K test codestreams under 100 KB from OpenJPH's test set, with
OpenJPEG's decode of each as the reference. They are graded by
`tests/conformance_ht.rs` with the Part 1 comparator and ratchet.

| What | Source | Pinned to |
| ---- | ------ | --------- |
| Codestreams, `LICENSE` | `github.com/aous72/jp2k_test_codestreams`, `openjph/` | commit `1ccd349d` |
| References | `opj_decompress` 2.5.0 on the bare codestream | — |
| OpenJPH release | `github.com/aous72/OpenJPH`, tag `0.33.0` | commit `345a8bcd` |

**License.** BSD-2-Clause, in `LICENSE`, kept verbatim. The directory is
excluded from the published crate (`exclude` in `Cargo.toml`).

OpenJPH is pinned as a regeneration tool only, like `opj_compress`. No file here
was made with it, and it is never a build or test dependency.

```
openjph/
  files/        simple_{dec,enc}_*.{jph,j2c} — the 16 test codestreams
  references/   <name>_K.pgx                 — OpenJPEG's decode, one plane per component
  manifest.json                              — bounds, signaling, passes, digests
  LICENSE                                    — BSD-2-Clause (retain verbatim)
```

The `.jph` files are graded on their `jp2c` payload (`codestream_range`), the
`.j2c` files whole. OpenJPEG decodes the sub-sampled files one component at a
time (`-c K`), since it would otherwise convert them from sYCC.

**Bounds.** Agreement with OpenJPEG: exact for the 5/3 files. For 9/7, PAE 1 up
to 12 bits and PAE 5 at 16 bits, the bounds of the existing irreversible
fixtures (`tiles_odd_origin_irreversible`, `irreversible_16bit`), with the MSE
bound the square of the PAE bound.

`manifest.json` denies unknown fields. Each entry records `file`,
`codestream_range`, `graded_components`, `bit_exact`, `bounds`, `references`,
`features` and `ht` (as in `../conformance/ht/manifest.json`), `passes`
(measured with OpenJPEG as for the HT set), and the digests of every file.

Regenerate with `python3 ../conformance/regenerate.py openjph` (needs network
access and `opj_decompress` 2.5.0).
