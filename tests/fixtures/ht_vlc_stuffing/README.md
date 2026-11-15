# HT VLC stuffing-bit fixtures

Used by `tests/ht_vlc_stuffing.rs`. Each pair is one HT code-block, cleanup
pass only, alone in a one-tile, one-component image with no wavelet level:

- `<name>.set.j2k` has one or two set stuffing bits in its VLC stream.
- `<name>.clear.j2k` is the same block with those bits cleared.

T.814 7.1.4 skips a stuffing bit, so this decoder reads the two files the
same. OpenJPEG 2.5.0 ORs a set bit into the next byte, so it reads the `.set`
file differently. See `docs/openjpeg-departures.md`, "Set stuffing bits in the
HT VLC and MagRef streams".

`manifest.tsv` holds the SHA-256 of OpenJPEG's samples for each file (each
sample a little-endian `i32`), `refused` when it fails on the file, or
`abort` when the process aborts on an assertion. The `abort` rows need an
`opj_decompress` built with assertions enabled, such as the Debian/Ubuntu
package; a Release build decodes those two files as this decoder does.

## Source and tools

`scripts/gen-ht-vlc-stuffing-fixtures.py` writes every file byte by byte from
its row (size, bit depth, quantization, zero bit-planes, code-block style and
cleanup segment). It runs `opj_decompress` 2.5.0 (system package) on each and
writes `manifest.tsv`. It checks that the two files of a pair differ only in
the stuffing bits and that OpenJPEG decodes them differently.

The cleanup segments come from a random differential against
`opj_decompress` 2.5.0 (issue #368). It used two generators of crafted
cleanup-only blocks, each block with a twin whose set VLC stuffing bits were
cleared. "Both decode" counts the blocks as generated that both decoders
decode; the last column counts the cleared twins both decode, which is a
different set, since clearing a bit can make a block decode or stop it:

| Generator | Blocks | Both decode | Samples differ | Differ, bits cleared |
| --------- | ------ | ----------- | -------------- | -------------------- |
| The PR #367 review's: widths 1, 2, 3, 4, 6; a 0xFF never followed by a byte with its top bit set | 17,936 | 4,371 | 81 | 0 of 4,376 |
| Random bytes: widths 1 to 9, 8-, 12- and 16-bit | 24,000 | 5,515 | 84 | 0 of 5,454 |

All 165 blocks whose samples differed had a set VLC stuffing bit, and this
decoder read each the same as its cleared twin. OpenJPEG would not decode
121 blocks this decoder reads, and each had a set VLC stuffing bit too. With
the bits cleared, the two decoders gave the same samples on every block both
decode.

The `nibble_` pairs have the bit in the first VLC nibble, and the `byte_`
pairs in a byte after one above 0x8F. On the `refused_` pairs OpenJPEG
refuses the `.set` file as malformed. On the `abort_mel_` pairs the set
bit's 0xFF is also a MEL byte followed by one above 0x8F, and OpenJPEG's
`mel_init` assertion (`ht_dec.c:319`) aborts the process on the `.set` file,
in a build with assertions enabled; on these two blocks a Release build
decodes as this decoder does. They are 2 of the 33 such blocks in the random-bytes sweep. Their test
pins only that each decodes, as its cleared twin does: the MEL decode stops
before the 0xFF, and the VLC never reads a bit the skip would change, so
these pairs cannot tell skipping the bit from ORing it in.
`nibble_2x17_i12` and `nibble_2x17_r12_causal` are the cleanup passes of
`r2_230_w2h17mb9z6p3s40i` and `r5_217_w2h17mb7z3p2s48r`, the blocks the issue
names.

| Pair | Block | Stuffing bits | `opj_decompress` on `.set` |
| ---- | ----- | ------------- | -------------------------- |
| `nibble_2x17_i12` | 2x17, 12-bit, 9/7 | first nibble | 12 of 34 samples differ |
| `nibble_2x17_r12_causal` | 2x17, 12-bit, 5/3, style 0x48 | first nibble | 14 of 34 differ |
| `nibble_3x8_r12_causal` | 3x8, 12-bit, 5/3, style 0x48 | first nibble | 4 of 24 differ |
| `nibble_5x2_r12` | 5x2, 12-bit, 5/3 | first nibble | 1 of 10 differ |
| `nibble_4x8_i12` | 4x8, 12-bit, 9/7 | first nibble | 8 of 32 differ |
| `nibble_4x8_r8` | 4x8, 8-bit, 5/3 | first nibble | 9 of 32 differ |
| `nibble_4x8_r16` | 4x8, 16-bit, 5/3 | first nibble | 24 of 32 differ |
| `nibble_2x2_i16` | 2x2, 16-bit, 9/7, a 3-byte segment | first nibble | 2 of 4 differ |
| `byte_8x2_r8` | 8x2, 8-bit, 5/3 | one byte | 5 of 16 differ |
| `byte_4x8_r16` | 4x8, 16-bit, 5/3 | one byte | 4 of 32 differ |
| `byte_1x12_r16` | 1x12, 16-bit, 5/3 | one byte | 1 of 12 differ |
| `refused_outside_2x17_i16` | 2x17, 16-bit, 9/7 | first nibble | significant samples outside the block |
| `refused_uq_4x8_r16` | 4x8, 16-bit, 5/3 | first nibble | `U_q` too large |
| `refused_uq_two_bits_4x18_i16` | 4x18, 16-bit, 9/7 | first nibble and one byte | `U_q` too large |
| `abort_mel_4x16_r12` | 4x16, 12-bit, 5/3 | one byte | aborts in `mel_init` |
| `abort_mel_2x17_r8` | 2x17, 8-bit, 5/3 | one byte | aborts in `mel_init` |

The style is 0x40 (HT) unless the table says 0x48 (HT, vertically causal).

A cleared twin reads the same as its block under T.814 unless its stuffing
bytes chain, as after a 0xFF stuffing byte (`12 34 7F FF A0 25 00`) or a
stuffing first nibble (`12 34 56 7F F4 00`): clearing the first bit changes
`VLC_last`, so the next byte is no longer a stuffing byte. The generator
refuses such blocks.
