# Real-world JP2 files

Nine small `.jp2` files from GDAL's test data, used by `tests/jp2_real.rs` to
check the JP2 box reader against the box layouts real encoders write (Kakadu,
the ECW JPEG 2000 SDK, OpenJPEG). The test decodes each file and compares it
with its oracle (below). `rgb16_ecwsdk.jp2` is the exception: its last
tile-part has `Psot = 0` and no `EOC`, so the codestream loses its last two
data bytes, as in OpenJPEG, and fails with the same `Codestream` error as its
extracted codestream. OpenJPEG 2.5.0 does not decode it either (it fails in
tile decoding, "segment too long").

## Rights

**The redistribution rights of these files are not confirmed.** GDAL's
`LICENSE.TXT` says the project is "in general" under the MIT license and does
not mention its test data, and several files are output of third-party
encoders. They are not labelled MIT here, and the crate's own `MIT OR
Apache-2.0` license does not cover them. The directory is excluded from the
published crate (`exclude` in `Cargo.toml`). Committing them was an owner
decision (Decision 2 in `docs/roadmap-phase3.md`); if a rights holder objects,
delete the file and its row in `tests/jp2_real.rs`.

## Source

- Repository: <https://github.com/OSGeo/gdal>
- Commit: `18642a1c44c884bbfaf5442ea9a25e25d0e8113e` (master, 2026-10-31)
- Directory: `autotest/gdrivers/data/jpeg2000/`
- Fetched with: `curl -sfLO https://raw.githubusercontent.com/OSGeo/gdal/<commit>/autotest/gdrivers/data/jpeg2000/<file>`

The encoder column is what the codestream's `COM` marker says; a file whose
comment does not name an encoder says so.

| File | GDAL path | Encoder (`COM` marker) | Bytes | SHA-256 | Notable boxes |
| ---- | --------- | ---------------------- | ----: | ------- | ------------- |
| `3_13bit_and_1bit.jp2` | `autotest/gdrivers/data/jpeg2000/3_13bit_and_1bit.jp2` | ECW JPEG 2000 SDK v3.3.0.160 | 27029 | `f67d0b8fa68634659a74e0c7e1768348ed64b588e835bb8efbf6f8308acc72f7` | `bpcc` (13, 13, 13, 1 bit), `cdef` with alpha, `xml `, `uuid` |
| `byte.jp2` | `autotest/gdrivers/data/jpeg2000/byte.jp2` | Kakadu-v3.4 | 10247 | `ef330c025133f062b4cca8c8171f8aef4f245be975c5e534dc9ce463689c3aa0` | greyscale `colr`, `uuid` (GeoJP2) |
| `byte_tlm_plt.jp2` | `autotest/gdrivers/data/jpeg2000/byte_tlm_plt.jp2` | not named (`COM` reads `my_comment`) | 1267 | `c2bccd9839cd4cb24ac6bc84fd888d707bbf1d873af07616fa020e05ab491a49` | `res ` with `resc`, `uuid`; TLM and PLT markers |
| `byte_with_xmp.jp2` | `autotest/gdrivers/data/jpeg2000/byte_with_xmp.jp2` | ECW JPEG 2000 SDK v3.3.0.161 | 6114 | `cda36211b536322d28b96f17123ee2154e14826b69533e44ade7242906cc0b7d` | `cdef`, `asoc`/`lbl `/`xml `, two `uuid` |
| `int16.jp2` | `autotest/gdrivers/data/jpeg2000/int16.jp2` | Kakadu-v3.4 | 1282 | `62fef13b5f49de6302a86fe1f18663626c0b7e5f2470893d32136bc16af690e3` | signed 16-bit, `uuid` |
| `rgb16_ecwsdk.jp2` | `autotest/gdrivers/data/jpeg2000/rgb16_ecwsdk.jp2` | ECW JPEG 2000 SDK v3.1.0.188 | 7911 | `86cf3c0a005b640e1cc9bd8dcf9f5955897b6576551383fc89f9d18adf32a507` | `cdef` with every type unspecified (65535), `xml `, `uuid` |
| `rgbwcmyk01_YeGeo_kakadu.jp2` | `autotest/gdrivers/data/jpeg2000/rgbwcmyk01_YeGeo_kakadu.jp2` | Kakadu-v6.1.1 | 2803 | `85a6382b972a5f5b39edf70282235663c3be596c430e4d2b208b5570611cbdfe` | `res ` with `resd`, `uuid` |
| `small_200ppcm.jp2` | `autotest/gdrivers/data/jpeg2000/small_200ppcm.jp2` | Kakadu-v6.2.1 | 3166 | `27a1cc037485e8e575ee57cf0f85e66146d9bc88420d49afcd874c5db6839838` | `res ` with `resd`, `asoc`/`lbl `/`xml `, `uuid` |
| `stefan_full_rgba.jp2` | `autotest/gdrivers/data/jpeg2000/stefan_full_rgba.jp2` | Created by OpenJPEG version 2.5.0 | 9605 | `53ecd23500a0f7921f8af6ef39b101a776503a797462ed10dcde77790b0916d5` | `cdef` with alpha |

`tests/jp2_real.rs` checks each file's SHA-256 against the same values, so an
edited or swapped file fails there.

## Oracle

For the five files without a `cdef` box the oracle is the file's own
codestream: `decode` of the `.jp2` must equal `decode` of its first `jp2c`
box's contents. They were also checked by hand against `opj_decompress` 2.5.0
(`-i <file>.jp2 -o <file>.pgx`, one PGX per component): every sample matched.

The three files with a `cdef` box (`3_13bit_and_1bit.jp2`,
`byte_with_xmp.jp2`, `stefan_full_rgba.jp2`) are graded against
`references/<stem>_<k>.pgx`, `opj_decompress` 2.5.0's output for the `.jp2`
(`opj_decompress -i <file>.jp2 -o <stem>.pgx`), and their channel labels
against their `cdef` boxes. Two match bit-exactly. `stefan_full_rgba.jp2` is
9/7 and is graded within 1: 10 of its 97200 samples differ from OpenJPEG's by
1, from 9/7 float rounding, mostly OpenJPEG's 2/K constant (see
`docs/correctness.md`). The references are OpenJPEG output for files whose
rights are not confirmed, so they share the status in Rights above.

`metadata.json` records what each file's header says (colour, channels,
resolution, top-level `xml ` and `uuid` boxes), in the `jp2` snapshot schema of
`tests/fixtures/README.md`, for every file whose header reads cleanly. The
values come from a box dump of each file (the `colr` `EnumCS`, the
`cdef` entries, the `resc`/`resd` fields, the top-level `xml ` and `uuid` box
sizes and identifiers), not from this decoder. `read_header` must report them,
and so must `decode` for each file it decodes.
