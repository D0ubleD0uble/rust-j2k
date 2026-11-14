# sRGB output

`ColorOutput::Srgb` converts sYCC and the two restricted ICC profile forms
(I.5.3.3) to sRGB. The default output is as coded and is unaffected by
everything here. The measured error against the ISO references and against
`opj_decompress -p 8S -force-rgb` is tabulated per channel in
[`tests/fixtures/conformance/README.md`](../tests/fixtures/conformance/README.md).

Every outcome is decided by the three rules in
[correctness.md](correctness.md#when-the-oracle-and-the-standard-disagree): the
standard first, then the error correction a user would expect, with OpenJPEG
usually the guide, and otherwise an error rather than a wrong image. Every
refusal is a row of `srgb_reject_matrix_maps_each_input_to_its_typed_error` in
`src/jp2/srgb_tests.rs`. OpenJPEG source lines are cited against tag v2.5.0
([openjpeg-departures.md](openjpeg-departures.md) explains the pin).

## Every input and its result

This table is the complete list of what `Srgb` does. The rustdoc and the README
link here and summarize it. The checks run in the order of the rows, and the
first row that matches decides. "Channels" are the output channels, after the
palette and the channel definitions; the color channels are the ones the `cdef`
labels color 1, 2 and 3 ([rows 8 and 21](#rows-8-and-21-color-channels-are-picked-by-their-labels)).
OpenJPEG means `opj_decompress -force-rgb` 2.5.0 with LittleCMS 2.14, which
follows `opj_decompress.c:1620-1716` and `color.c`. Each row number links to
the evidence for that row.

| # | Input | OpenJPEG | `Srgb` here | Why |
| - | ----- | -------- | ----------- | --- |
| [1](#row-1-raw_channels) | `raw_channels` set | (no such option) | `InvalidOptions`, before anything is read | option check |
| [2](#rows-2-and-4-one-or-two-channels) | A raw codestream of one or two components | read as gray, or gray and opacity | as coded | matches OpenJPEG |
| [3](#rows-3-and-5-420-and-422-are-not-guessed-to-be-sycc) | A raw codestream of three or more components | at 4:2:0 or 4:2:2, converted as sYCC; otherwise fails | `Unsupported` | rule 3 |
| [4](#rows-2-and-4-one-or-two-channels) | One or two channels, any color space or none, unless a monochrome profile converts them (rows 18 to 20) | read as gray, or gray and opacity | as coded, metadata as declared | matches OpenJPEG |
| [5](#rows-3-and-5-420-and-422-are-not-guessed-to-be-sycc) | Three channels at 4:2:0 or 4:2:2 (luma 1x1, both chroma 2x1 or both 2x2), declared sRGB, grayscale or by an ICC profile (any profile, any `cdef`) | converted as sYCC, before the profile is read | `Unsupported` | rule 3 |
| [6](#rows-6-and-7-srgb-and-grayscale) | sRGB or grayscale (`EnumCS` 16, 17), color channels of one format | written as coded | as coded | matches OpenJPEG |
| [7](#rows-6-and-7-srgb-and-grayscale) | sRGB or grayscale, color channels that differ in depth, sign or sub-sampling | written as coded | `Unsupported` | rule 3 |
| [8](#rows-8-and-21-color-channels-are-picked-by-their-labels) | sYCC (`EnumCS` 18) without colors 1, 2 and 3, or with colors that differ in depth or sign | converts positions 0 to 2 | `Unsupported` | rule 3 |
| [9](#row-9-signed-sycc) | Signed sYCC | converted as unsigned, then clamped | `Unsupported` | rule 3 |
| [10](#row-10-other-sycc-layouts) | sYCC other than luma at 1x1 with both chroma channels at 1x1, 2x1 or 2x2 | "CAN NOT CONVERT", written as coded | `Unsupported` | rule 3 |
| [11](#row-11-replicated-chroma-and-the-sample-limit) | sYCC whose replicated chroma passes the sample limit | | `Limit` | resource bound |
| [12](#row-12-sycc-is-converted) | sYCC otherwise | converted, constants shortened and truncated | converted, IEC constants rounded | rule 1 |
| [13](#rows-13-14-and-16-a-profile-littlecms-refuses-passes-through) | An ICC profile LittleCMS refuses to open | written as coded | as coded, `RestrictedIcc` kept | rule 2 |
| [14](#rows-13-14-and-16-a-profile-littlecms-refuses-passes-through) | A profile in a color space other than RGB, gray or YCbCr | written as coded (no transform) | as coded, `RestrictedIcc` kept | rule 2 |
| [15](#row-15-the-afl-test) | An RGB profile whose first four channels (all, if fewer) differ from the first in depth, sign or sub-sampling (the AFL test) | written as coded | as coded, `RestrictedIcc` kept | rule 2 |
| [16](#rows-13-14-and-16-a-profile-littlecms-refuses-passes-through) | A profile LittleCMS refuses to build a transform from | written as coded | as coded, `RestrictedIcc` kept | rule 2 |
| [17](#row-17-lookup-tables-and-the-lab-connection-space) | A profile LittleCMS applies through a lookup table or the Lab connection space | converted | `Unsupported` | rule 3 |
| [18](#row-18-a-monochrome-profile-on-more-than-two-channels) | A monochrome profile without exactly one color channel, or on more than two channels | more than two: the planes go to a gray transform as RGB | `Unsupported` | rule 3 |
| [19](#rows-19-and-23-sample-depths-littlecms-would-scale-wrongly) | A monochrome profile on samples other than 8-bit unsigned | scaled from the wrong range | `Unsupported` | rule 3 |
| [20](#rows-20-and-24-profiles-that-convert) | A monochrome profile otherwise | converted | the same; a tinted output gives R, G, B in place of the gray channel (`Limit` past the sample limit) | matches OpenJPEG |
| [21](#rows-8-and-21-color-channels-are-picked-by-their-labels) | An RGB profile without colors 1, 2 and 3 | converts positions 0 to 2, whatever they are | `Unsupported` | rule 3 |
| [22](#row-22-a-differing-color-past-the-first-four-positions) | An RGB profile whose colors differ in depth, sign or sub-sampling (possible only with a color past the first four positions) | converts positions 0 to 2 | `Unsupported` | rule 3 |
| [23](#rows-19-and-23-sample-depths-littlecms-would-scale-wrongly) | An RGB profile on samples other than 8- or 16-bit unsigned | scaled from the wrong range | `Unsupported` | rule 3 |
| [24](#rows-20-and-24-profiles-that-convert) | An RGB profile otherwise | converts positions 0 to 2 | the same samples when the colors come first; the colors are picked by label | rule 1 |
| [25](#row-25-ycbcr-profiles) | A YCbCr profile on three or more channels | converted as 16-bit YCbCr (8-bit crashes) | `Unsupported` | rule 3 |
| [26](#rows-26-to-29-color-spaces-without-a-conversion) | CMYK (`EnumCS` 12) on four or more channels of one sampling | converted to three RGB planes | `Unsupported` | rule 3 |
| [27](#rows-26-to-29-color-spaces-without-a-conversion) | CIELab (`EnumCS` 14) on three channels of one size | converted to 16-bit RGB | `Unsupported` | rule 3 |
| [28](#rows-26-to-29-color-spaces-without-a-conversion) | e-sYCC (`EnumCS` 24) on three channels of one sampling | converted | `Unsupported` | rule 3 |
| [29](#rows-26-to-29-color-spaces-without-a-conversion) | Any other enumerated space, or no `colr`, on three or more channels | fails, written as coded | `Unsupported` | rule 3 |

In rows whose OpenJPEG column does not mention 4:2:0 or 4:2:2, OpenJPEG
converts such a three-channel file as sYCC (row 5).

## One rule for files OpenJPEG writes as coded

When `opj_decompress` leaves the samples as coded (and `-force-rgb` fails),
`Srgb` goes by what the file declares, not by what OpenJPEG does:

- sRGB or grayscale need no conversion, so they pass through (rows 6, 4);
- a profile OpenJPEG's color step declines passes through, with the profile
  kept in `Jp2Metadata::color`, so a caller can tell that nothing was
  converted (rows 13 to 16; rule 2, #199);
- anything else is refused: an enumerated space this decoder has no
  conversion for, no color space on three or more channels, or color
  channels the conversion cannot take (rows 7, 10 and 29; rule 3).

## What the metadata reports

After a conversion `Image::jp2` reports what the samples now are, as OpenJPEG
does (#165, 2026-11-01): `ColorSpace::Srgb`, or `ColorSpace::Grayscale` for a
monochrome profile, with no ICC profile. A tinted monochrome output is reported
as `ColorSpace::Srgb` ([rows 20 and 24](#rows-20-and-24-profiles-that-convert)).
A pass-through, the default `AsCoded` decode and `read_header` report the
color space the file declares.

## Evidence for each row

### Row 1: `raw_channels`

`raw_channels` with `Srgb` is `InvalidOptions`, before anything is read.
OpenJPEG has no such option.

### Rows 2 and 4: one or two channels

Any color space on one or two channels passes through as gray (and gray plus
opacity), as OpenJPEG reads any image of two or fewer components
(`opj_decompress.c:1624-1625` sets such an image to gray): from a raw
codestream, or a JP2 file with no `colr`, any enumerated space (sRGB included,
#211, 2026-11-02, even when the two channels differ in depth or sign), or an
RGB or YCbCr profile. Only a monochrome profile converts them, on 8-bit
unsigned samples only (row 20). The metadata stays as declared, since nothing
was converted (tests
`a_gray_jp2_without_a_converted_color_space_passes_through`,
`srgb_on_one_or_two_channels_passes_through`).

### Rows 3 and 5: 4:2:0 and 4:2:2 are not guessed to be sYCC

For a three-component 4:2:0 or 4:2:2 file, whatever `colr` declares (sRGB,
grayscale, another space, a profile, or none) and whatever its `cdef` labels,
or a raw codestream, `opj_decompress` guesses sYCC and converts it. This
decoder does not make the guess (rule 3: the file does not say it is sYCC).
`opj_decompress` guesses sYCC whenever the second component is sub-sampled in x
and the first has equal x and y sampling, whatever the file declares
(`opj_decompress.c:1620-1623`), but `color_sycc_to_rgb` converts only those two
layouts (`color.c:403-436`).

Checked: `jp2_sycc_420_odd.jp2` with its `EnumCS` patched to 17 (grayscale)
gives byte-identical `opj_decompress` output to the sYCC original, while `Srgb`
here refuses it (a row of the reject matrix). The refusal holds on every
channel, whatever the `cdef` labels or without one: a file declared sRGB or
grayscale on three components laid out 4:2:0 or 4:2:2 (luma at 1x1, both
chroma channels at 2x2 or both at 2x1) is `Unsupported`. Checked on such sRGB
files with a `cdef` of color 1 and two opacity or unspecified channels, of no
color, and of colors 2 and 3 only: `opj_decompress` 2.5.0, with and without
`-force-rgb`, writes three full-size planes byte-identical to those of the file
with no `cdef` (test `srgb_4_2_0_or_4_2_2_is_refused_whatever_cdef_labels`).

Any other three-component grayscale file passes through as coded, as OpenJPEG
writes it (row 6): one whose third channel is signed or the only one
sub-sampled, whose luma is sampled 1x2 with chroma 2x1, or whose layout is in
the guess shape but not one of the two (1x1, 2x1, 1x1, say), where OpenJPEG
prints "CAN NOT CONVERT". Checked with `opj_decompress` on 1x1:2x1:1x1,
1x1:2x2:1x1, 1x1:2x1:2x2, 1x1:2x2:2x1 and 2x2:2x2:2x2, as grayscale and under
an RGB profile: each is written as coded, except 2x2:2x2:2x2 under the profile,
which the profile converts. A grayscale palette of three columns over a 2x2
index is written as coded too.

The guess also comes before a profile LittleCMS refuses. `opj_decompress`
converts the file as sYCC before `color_apply_icc_profile` runs
(`opj_decompress.c:1620-1629`), so a profile lcms refuses does not leave it as
coded. Checked on `jp2_sycc_420_odd` and `jp2_sycc_422_odd` under
`lcms_intent_4`, a refused gray profile (`gray_lcms_intent_4`), a profile cut
short (`bad_short_header`) and a CMYK profile (`space_cmyk`): `-force-rgb`
writes the sYCC original's planes each time. `Srgb` makes no sYCC guess, so it
refuses all of them, before the profile is read, with the sub-sampling message.
Tests: `an_lcms_refused_profile_on_4_2_0_or_4_2_2_is_refused`, and
`an_lcms_refused_profile_passes_through_outside_the_sycc_layouts` for the
layouts that stay as coded.

### Rows 6 and 7: sRGB and grayscale

sRGB and grayscale need no conversion, so color channels of one format pass
through. An enumerated sRGB file of four components and no `cdef`, whose fourth
differs in format from the colors (signed 8-bit, say), passes through as coded,
as with a `cdef` and as in OpenJPEG (test
`an_srgb_file_with_a_differently_formatted_fourth_channel_passes_through`); the
fourth channel is unspecified ([Default labels without
`cdef`](openjpeg-departures.md#default-labels-without-cdef)).

Color channels that differ in depth, sign or sub-sampling, other than the sYCC
chroma layouts, are refused (2026-11-01; rule 3). For an enumerated sRGB or
grayscale file there is no conversion to skip: `opj_decompress -force-rgb`
writes the file as coded. Under an RGB profile such channels pass through
instead (row 15).

### Rows 8 and 21: color channels are picked by their labels

I.5.3.6 makes the `cdef` association say which channel is which color, so
`Srgb` converts the channels labeled color 1, 2 and 3 wherever they sit
(2026-11-02; rule 1). OpenJPEG converts the first three positions. The two
agree whenever the colors come first, which is every file in the corpus but
this one: `jp2_cdef_rgba` with the `cdef` (0,0,1), (1,0,2), (3,0,3), (2,0,3),
(2,1,0) puts color 3 after an opacity channel. Here planes 0, 1 and 3 are
converted; `opj_decompress` converts planes 0, 1 and 2. Test:
`srgb_picks_the_color_channels_by_their_cdef_labels` in `tests/jp2_icc.rs`.

A file with fewer than three color labels under sYCC or an RGB profile is
refused (rule 3): three channels with an RGB profile and a `cdef` of (color 1,
color 2, opacity) are `Unsupported`, where OpenJPEG converts all three planes
and treats the opacity channel as blue.

### Row 9: signed sYCC

Signed sYCC is refused (#165, 2026-11-01; rule 3). OpenJPEG converts it without
a message, as if it were unsigned, and clamps: on an 8-bit signed 4:4:4 file,
as-coded R `[-7, -62, 61, 114, ...]` comes out `[0, 0, 15, 0, ...]`. sYCC is
defined on unsigned samples, so there is no right answer to match.

### Row 10: other sYCC layouts

sYCC other than full-resolution luma with chroma at 1x1, 2x1 or 2x2 is refused
(rule 3). OpenJPEG prints "CAN NOT CONVERT" (`color.c:432`) and writes the
samples as coded.

### Row 11: replicated chroma and the sample limit

Chroma replication grows the output, so it is checked against the decode's
[sample limit](resource-bounds.md#the-sample-limit) before the tiles are read
(`Limit`), as palette expansion is.

### Row 12: sYCC is converted

sYCC uses the IEC constants and rounds (#165, 2026-11-01; rule 1). OpenJPEG's
`sycc_to_rgb` computes `G = Y - (int)(0.344 Cb + 0.714 Cr)`,
`R = Y + (int)(1.402 Cr)`, `B = Y + (int)(1.772 Cb)`: shortened constants and
each product truncated toward zero. This decoder uses IEC 61966-2-1 Amd 1's
`0.344136` and `0.714136` and rounds to nearest. Both changes (the constants
and the rounding) meet the evidence rule of
[correctness.md](correctness.md#when-the-oracle-and-the-standard-disagree). The
proof is the conformance references for files 2 and 3; the result is an MSE
against them about 20 to 2000 times lower:

| File | This decoder vs ISO (MSE R/G/B) | OpenJPEG vs ISO (MSE R/G/B) | This decoder vs OpenJPEG (MSE R/G/B) |
| ---- | ------------------------------- | --------------------------- | ------------------------------------ |
| file2 | 0.0003 / 0.0155 / 0.0242 | 0.3853 / 0.4748 / 0.5135 | 0.3850 / 0.4803 / 0.4895 |
| file3 | 0.0002 / 0.0151 / 0.0239 | 0.3862 / 0.4724 / 0.5109 | 0.3860 / 0.4774 / 0.4872 |

The peak error is 1 in every column. OpenJPEG's formula, reproduced in
`src/jp2/srgb_tests.rs` (`sycc_sample_openjpeg`), is bit-exact against
`opj_decompress` on both files and on the two synthetic sYCC fixtures, so
the difference is the formula alone. The IEC constants with truncation fit
neither (green off OpenJPEG by up to 2), so the two changes go together.

Chroma replication follows OpenJPEG, including where it is a convention
rather than the standard. Sub-sampled chroma is replicated to the luma grid
in the order of `sycc422_to_rgb` and `sycc420_to_rgb` in the oracle
build, Ubuntu's OpenJPEG 2.5.0-2ubuntu0.5. That build carries upstream
master's `comp12w` guard (below), which tag v2.5.0 (`a589155`) does not
have; the committed fixtures are its output. At an odd image
origin (`XOsiz` or `YOsiz` odd, on the full-resolution grid even under
resolution reduction) OpenJPEG sets chroma to zero, which comes out tinted,
in the first column of each row (4:2:2), or in the first row and in the
first column of the upper row of each pair and of a last unpaired row
(4:2:0). A last odd column whose pair index reaches the chroma width also takes
zero (`comp12w`). `jp2_sycc_420_odd` and `jp2_sycc_422_odd` pin this against
`opj_decompress` sample for sample, and `tests/jp2_sycc_replication.rs`
grades them and the even-origin `jp2_sycc_420_even` and `jp2_sycc_422_even`
against `opj_decompress -r 1` too: there the reduced origin's parity
differs from the full one's in y (and in x for the even fixtures), so a
replication keyed on the reduced origin fails. The only change is where
OpenJPEG would read past the chroma plane: that sample takes chroma zero
here.

### Rows 13, 14 and 16: a profile LittleCMS refuses passes through

A profile lcms refuses passes through, as in OpenJPEG (#199, 2026-11-01;
rule 2). When lcms will not open the profile or build a transform from it,
`color_apply_icc_profile` returns without converting and `opj_decompress`
writes the samples as coded. `Srgb` does the same, and `Jp2Metadata::color`
stays `RestrictedIcc`, so a caller can tell.

To refuse what lcms refuses, `src/jp2/lcms.rs` makes the checks lcms makes when
it opens a profile (`_cmsReadHeader`): the `acsp` signature, a version of at
most 5.0, a device class lcms knows, at most 100 tags, no two kept entries with
the same signature. Like lcms, it first skips a tag entry with a zero offset or
size, or one that ends past the size the header declares (capped at the
profile's length), so a tag it needs is then missing, and a skipped entry does
not count as a duplicate.

When building the transform it makes lcms's checks too: a named color profile,
a device link or abstract profile without a lookup table, or a connection space
other than XYZ or Lab builds none, and of the lookup tables lcms would read for
the intent (`D2Bx`, then `A2Bx`, then `A2B0`) it checks the type, the channel
counts, the grid and the table's length as lcms's reader does. It reads tags as
`cmsReadTag` does: a tag under 8 bytes or of a type lcms does not allow for it
fails, lcms's legacy types are read (an old Monaco tool's curve type for
`rTRC`, `gTRC` and `bTRC`, an old Corbis tool's XYZ type), and a value is read
without checking the tag's size, so one that runs past its tag takes the bytes
after it and fails only past the end of the profile. A media white point lcms
can't read is D50 (`_cmsReadMediaWhitePoint`).

Each case was checked with `opj_decompress` 2.5.0 on a crafted file, and is a
case of `tests/fixtures/icc/profiles.json` and a test in
`src/jp2/srgb_tests.rs`:

| Case | OpenJPEG + lcms 2.14 | `Srgb` here |
| ---- | -------------------- | ----------- |
| profile under 132 bytes, no `acsp`, version above 5.0, unknown class (`zzzz`), more than 100 tags, tag table past the end, two kept entries with one signature | unconverted (lcms won't open it) | as coded |
| a duplicate whose first copy lcms skips | converted from the second | the same |
| class `mntr`, `prtr`, `spac` or 0 with a matrix and curves | converted as `scnr` (a version 2 `mntr` takes D50 as its white point) | the same |
| class `nmcl`; class `link` or `abst` with no lookup table | unconverted | as coded |
| intent 4 to 9 or above 15 | unconverted | as coded |
| connection space other than XYZ or Lab | unconverted | as coded |
| color space other than RGB, gray or YCbCr | unconverted (no transform for it) | as coded |
| lookup table lcms would read of the wrong type, wrong channel counts, a grid of 1, or cut short | unconverted | as coded |
| lookup table for another intent only | converted from the matrix | the same |
| lookup table lcms reads | converted through it | `Unsupported` (row 17) |
| Lab connection space with a matrix and curves | converted | `Unsupported` (row 17) |
| intent 10 to 15 | converted as intent 1 | converted as intent 1 |
| needed tag missing or skipped, under 8 bytes, of the wrong type, or with a value past the profile's end; `para` function above 4; `curv` of more than 32767 entries | unconverted | as coded |
| value past its tag but inside the profile | converted from the bytes after it | the same |
| Monaco curve type, Corbis XYZ type | converted | converted |
| `wtpt` unreadable at intent 3 | converted with D50 | converted with D50 |

The order follows OpenJPEG's too (#199, 2026-11-02): every refusal above,
whether lcms makes it opening the profile or building the transform, comes
before this decoder's own checks (rows 17 to 23). So a profile lcms refuses
passes through whatever the samples are, bar the 4:2:0 and 4:2:2 layouts of
[rows 3 and 5](#rows-3-and-5-420-and-422-are-not-guessed-to-be-sycc): an intent
of 4 on 12-bit RGB, 16-bit gray, or a monochrome profile on three channels
gives the samples as coded, as `opj_decompress` does. This decoder's own
refusals (#165, #197) apply only where lcms would build the transform. Test:
`refusals_are_ordered_as_in_openjpeg`.

### Row 15: the AFL test

An RGB profile is skipped when the first four channels differ, as in OpenJPEG
(#206, 2026-11-02; rule 2). In `color_apply_icc_profile`
(`src/bin/common/color.c`), lcms opens the profile first
(`cmsOpenProfileFromMem`, `color.c:468-477`). For an RGB profile OpenJPEG then
returns on fewer than three components (`color.c:491-494`), and otherwise runs
its "AFL test" (`color.c:495-519`): it compares `comps[1..min(numcomps, 4)]`
with `comps[0]` by `dx`, `dy`, `prec` and `sgnd`, and returns without
converting if any differs. Only then does lcms build the transform
(`cmsCreateTransform`, `color.c:586`). The gray (`color.c:530-534`) and YCbCr
(`color.c:535-543`) branches have no such test. The components are the image
after the palette and `cdef`, by position, so a differing opacity channel
counts and a fifth channel does not.

`Srgb` runs the same test in the same place: after the profile is opened and
before the transform is built, so before every check of this decoder's own (a
lookup table, the Lab connection space, a `cdef` without colors 1 to 3, an
unsupported depth). One case is refused before the test: a 4:2:0 or 4:2:2
image, which `opj_decompress` converts as sYCC before it reads the profile
(rows 3 and 5). Other layouts in its sYCC-guess shape stay as coded there, so
the test applies to them: 1x1:2x1:1x1 and 1x1:2x2:1x1 pass through, and
2x2:2x2:2x2 matches and converts.

Checked against `opj_decompress` 2.5.0 with lcms 2.14. With and without a
`cdef`, a signed, a 12-bit, a 2x1 and a 1x2 sub-sampled fourth channel,
and a signed or sub-sampled third color, pass through as coded
(`-force-rgb` fails, as nothing was converted). A lookup-table or Lab
profile, or a `cdef` of two colors, passes through with a signed fourth
channel. A matching fourth channel, a sub-sampled fifth, and a gray
profile with a signed opacity channel convert. Tests (`tests/jp2_icc.rs`):
`an_rgb_profile_passes_through_when_the_first_four_channels_differ`,
`the_afl_test_comes_before_this_decoders_refusals`,
`the_afl_test_covers_only_rgb_profiles_and_four_channels`,
`the_afl_test_applies_where_openjpeg_cannot_convert_as_sycc`.

### Row 17: lookup tables and the Lab connection space

An ICC profile lcms would apply through a lookup table (`D2Bx` or `A2Bx`,
which it prefers over the matrix) or the Lab connection space is refused
(rule 3). OpenJPEG converts these; this decoder ports only the matrix/TRC
pipeline into XYZ. A table whose header passes lcms's checks but whose
contents lcms rejects (an `mpet` with no elements, say) is refused here too,
where OpenJPEG writes the samples as coded.

### Row 18: a monochrome profile on more than two channels

A monochrome (`GRAY`) ICC profile on more than two channels is refused (#197,
2026-11-01; rule 3), for example one color channel and two opacity channels in
`cdef`, or a palette of three columns. OpenJPEG takes its `numcomps > 2`
branch, which passes the planes interleaved to a gray transform as if they
were RGB, so its three-channel output is not a conversion. Checked on a 24x16
file with a tinted gray profile and a `cdef` of (color 1, opacity, opacity):
`opj_decompress` writes an RGB PNG.
One gray channel (1 to 1 or 3) and gray plus opacity (2 to 2 or 4) match
OpenJPEG and are unchanged. Test:
`a_gray_profile_on_more_than_two_channels_is_unsupported`.

### Rows 19 and 23: sample depths LittleCMS would scale wrongly

An RGB profile on samples other than 8- or 16-bit unsigned, or a gray profile
on samples other than 8-bit unsigned, is refused (#165, 2026-11-01; rule 3).
OpenJPEG picks an lcms format from the precision alone (`TYPE_RGB_8` up to 8
bits, else `TYPE_RGB_16`, `color.c:521-527`; `TYPE_GRAY_8` for gray,
`color.c:531`) and so scales other depths wrongly, past the samples' own
range. A 12-bit 4095 comes out 7413 (#165). With file5's profile on a 4-bit
RGB image, gray 15 comes out 18, gray 12 comes out 13, and (15, 0, 0) comes
out with red 29, all written into 4-bit planes.

### Rows 20 and 24: profiles that convert

The output matches `opj_decompress -force-rgb` sample for sample
(`tests/jp2_icc.rs`). The reference is OpenJPEG 2.5.0 linked against LittleCMS
2.14 on x86-64 Linux (glibc 2.39, on a CPU with FMA and AVX2).

OpenJPEG hands the profile to LittleCMS 2 (2.14 in the oracle build) with the
profile's rendering intent. `src/jp2/lcms.rs` ports the part of lcms these
profiles reach, step for step at lcms's precision:

- the pipeline lcms builds: the tone curves and matrix into XYZ, the
  white-point scaling of absolute colorimetric, black point compensation
  under perceptual and saturation (lcms's sRGB profile is ICC v4), then the
  inverse sRGB matrix and curves;
- the optimization lcms picks for that pipeline: the 8-bit matrix-shaper in
  1.14 fixed point, prelinearization curves with a 33-point table, or
  resampling into a 33-point table, with the white fix-up; for 16-bit RGB on a
  profile of gamma below 1.6, no optimization;
- lcms's 16-bit interpolation.

lcms's black-preserving intents 10 to 15 on a profile that is not CMYK become
perceptual, relative colorimetric or saturation without the black point
compensation lcms forces for intents 0 and 2, which for these profiles is
relative colorimetric in every case; other intents above 3 build no transform
(rows 13, 14 and 16).

On a monochrome profile (row 20), two components with a grayscale ICC profile
and no `cdef` convert as gray plus opacity, and match `opj_decompress
-force-rgb` sample for sample (test
`a_gray_profile_on_two_channels_without_cdef_is_gray_plus_opacity`). A
monochrome profile whose output is not neutral (absolute colorimetric with a
tinted media white point) gives three channels, R, G and B in place of the gray
one, as `ColorSpace::Srgb`; this is checked against the sample limit too.

LittleCMS calls the C library's `pow` and `log`, and a platform's own can
differ in the last bit, which moves an output level. So `src/jp2/math.rs`
ports glibc 2.39's `pow` and `log` (Arm's optimized-routines code), and the
output is the same on every platform whose `fma` is correctly rounded. CI
checks this on x86-64 Linux (glibc), macOS on aarch64 and wasm32-wasip1, which
does `fma` in software; Windows is not run. MinGW's `fma` (`*-pc-windows-gnu`)
may round twice (rust-lang/rust#140515), so the output there can differ. i586
targets use x87 arithmetic, which is not exact IEEE, so the port's own
arithmetic can differ there too. On x86-64, glibc picks a variant built with
FMA when the CPU has FMA and AVX2, and a plain SSE2 one otherwise. The port
follows the FMA variant, the one the reference machine runs: each multiply-add
GCC fused there is a `mul_add` in the port. The two variants differ in the
last bit on 296 of 5.2 million `log` inputs and 3,251 of 3.7 million `pow`
inputs of an earlier test sweep, so oracles regenerated on an x86-64 CPU
without FMA could differ. The current ones do not: regenerating them with
glibc held to its SSE2 variant (`GLIBC_TUNABLES=glibc.cpu.hwcaps=-AVX2,-FMA`)
gives the same hashes. The default test (`src/jp2/math_tests.rs`) checks the
port's results on a sweep of inputs against digests recorded from glibc, on
every platform. An ignored, larger sweep compares the port bit for bit with
the platform's `f64::ln` and `f64::powf` on x86-64 Linux with glibc and FMA.

The cases:

- files 5, 7 and 8 at rendering intents 0 to 3, and at intent 3 with the
  media white point set to D65, (1.2, 1.1, 1.0) and (0.9, 0.95, 0.7): 21
  variants, file7 at its native 16 bits;
- 63 synthetic RGB profiles at 8 and 16 bits and 10 monochrome ones: edge
  cases (degenerate, descending and noisy curves, every parametric type,
  extreme matrices and white points, nonzero black points, both sides of
  the gamma 1.6 threshold) and 24 malformed profiles (#184);
- 15 RGB profiles at 8 and 16 bits on either side of each header check
  ([rows 13, 14 and 16](#rows-13-14-and-16-a-profile-littlecms-refuses-passes-through))
  (a bad signature; versions 5.0, 5.1, 4.F and FF; 100 and 101 tags;
  duplicate `rTRC` and `rXYZ`, and a duplicate lcms skips; declared sizes of
  0, 140 and 0xFFFFFFFF, and ones that cut off `wtpt` or an `A2B0` table);
- 25 RGB profiles at 8 and 16 bits and 4 monochrome ones on how lcms reads
  intents, legacy tag types and tags that run short (the table under rows 13,
  14 and 16);
- 58 RGB profiles at 8 and 16 bits and 13 monochrome ones on device
  classes, connection and color spaces, lookup tables and missing tags
  (the same table).

During development the port also matched lcms 2.14 itself on every 8-bit
RGB input (2^24) and a million random 16-bit pixels for each of these
profiles. Against the ISO references the result is OpenJPEG's, inside the
class bounds (PAE 4, MSE 1):

| File | This decoder vs ISO (MSE) | This decoder vs OpenJPEG |
| ---- | ------------------------- | ------------------------ |
| file5 (RGB, gamma 1.8) | 0.0897 / 0.1750 / 0.2050 | identical |
| file7 (RGB, 16-bit, 2048-entry curves; after `>> 8`) | 0.1442 / 0.1770 / 0.1900 | identical at 16 bits |
| file8 (gray, `kTRC`) | 0.2739 | identical |

Peak error against ISO is 1 everywhere. The floating-point evaluation this
replaced was slightly closer to ISO on files 5 and 8 (MSE 0.0846 / 0.1723 /
0.2025 and 0.2115). OpenJPEG is the reference decoder, and nothing in the
standard asks for a closer match to these display references, so the
evidence rule keeps OpenJPEG's output.

Two output shapes differ from `opj_decompress`, by choice of the grading
harness and not of the samples: file7 stays 16-bit, and the harness shifts
it down by 8 to compare with the 8-bit reference (its `shift_right`, as for
the as-coded grade), and file8 stays one gray channel where OpenJPEG writes
three identical ones (graded on that one channel).

### Row 22: a differing color past the first four positions

Under an RGB profile, color channels that differ pass through as in OpenJPEG
(row 15), except where a `cdef` leaves a differing color past the first four
positions. There the AFL test passes and OpenJPEG converts whatever sits at
positions 0 to 2, color or not, so this decoder refuses the file rather than
give a wrong image (2026-11-01; rule 3). Checked: the fuzz seed
`icc_rgb_uniform_after_afl` (five palette channels, the 4-bit color 3 moved to
position 4 by a second color-3 entry) comes out of `opj_decompress` with
positions 0 to 2 converted and the 4-bit channel as coded.

### Row 25: YCbCr profiles

A YCbCr (`YCbr`) ICC profile on three or more channels that LittleCMS would
build a transform from is refused (#199, 2026-11-02; rule 3). OpenJPEG runs it
as 16-bit YCbCr whatever the depth (`color.c:535-543`): `opj_decompress` 2.5.0
converts 16-bit samples and crashes on 8-bit ones. The refusal is a deviation at
16 bits, kept because YCbCr is not one of Annex I's restricted forms. On one or
two channels, or where LittleCMS refuses the profile (intent 4, a missing tag),
it passes through as in OpenJPEG (`refusals_are_ordered_as_in_openjpeg`).

### Rows 26 to 29: color spaces without a conversion

Three or more channels with an enumerated color space other than sRGB,
grayscale or sYCC, or with no `colr`, and a raw codestream of three or more
components, which declares no color space, are refused (rule 3). For most such
spaces at one sampling `opj_decompress -force-rgb` fails too; at 4:2:0 or 4:2:2
it converts them as sYCC, which rows 3 and 29 refuse. Three spaces it does
convert: CMYK (12) on four channels gives three RGB planes
(`color_cmyk_to_rgb`), CIELab (14) on three gives 16-bit RGB
(`color_cielab_to_rgb`), and e-sYCC (24) on three converts
(`color_esycc_to_rgb`). This decoder has no such conversion (they are out of
scope for JP2, [roadmap-phase3.md](roadmap-phase3.md)); without one, sRGB
output would be a wrong image.
