# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Migrating from 0.3

The main changes that can break a 0.3 caller or change what it sees are
below. The first fails to compile; the others compile and behave differently.

- Pass options by reference: `decode_with(bytes, &options)`.
  `DecodeOptions` is no longer `Copy`, so call `.clone()` where you reuse
  one by value.
- Re-check every `match` arm on `Error::Unsupported`. `Error` is
  `#[non_exhaustive]`, so the match still compiles, but some inputs that
  were `Unsupported` now return another variant and fall to your wildcard
  arm. A `resolution_reduction` past the decomposition levels is
  `Error::InvalidOptions`. A resource guard is `Error::Limit`. A malformed
  JP2 file is `Error::FileFormat`. `Unsupported` now means only a valid
  feature that is not decoded. A "level not available, fall back to full
  resolution" path should match `InvalidOptions` (`Header::max_reduction`
  gives the largest reduction the main header allows; a decode can still
  return `InvalidOptions` when a tile-part header allows less).
- `decode` and `decode_with` accept JP2 files as well as raw codestreams.
  If only a raw codestream is valid for you (GRIB2 section 7, say), refuse
  input that does not start with SOC (`FF 4F`) before you decode. Or call
  `read_header` first, which reads both forms, and refuse the input when
  `header.jp2` is `Some`; that also lets you check `width`, `height` and
  the component count before anything is allocated. Without that, input you
  used to refuse now decodes. Some raw codestreams 0.3 refused decode too
  (see "Fixed" and "Changed"), so do not use `decode` alone to validate
  input.
- Do not match on error message text. Messages are not stable, and the
  spelling is now American ("color", not "colour").

Also, the decoder now bounds its work, so a file 0.3 decoded can come back as
`Error::Limit`. Raise the bound that was hit with
`DecodeOptions::packet_work_limit`, `code_block_work_limit` or
`sample_limit`, each set to a `WorkLimit` (`Unlimited` for trusted input). The
sample limit itself is higher than in 0.3 (2^28 samples, four times
2^26), so no image 0.3 decoded is refused on size. But the default now
admits four times the samples, which is about 1.4 GiB peak, estimated, for
a single-component tile, and about twice that for a JP2 palette.
`WorkLimit` cannot set a lower ceiling. To keep one, check `width`,
`height` and the component count from `read_header` before decoding.
Adding `Image::jp2` and `Image::report` breaks no caller, since `Image` is
`#[non_exhaustive]`. The other files that 0.3 decoded and that are now refused
are listed in "Changed" (a code-block size past the limit in any COD or COC,
and a POC marker that leaves out packets). Some decoded values changed with no
error (an RGN maxshift with the bypass style, a POC in a tile's first
tile-part header, and 9/7 output, which can move by 1 on some samples now
that β and δ are the nearest `f32`; see "Fixed"), so output snapshots from
0.3 can differ.

### Added

- `decode` and `decode_with` read JP2 files as well as raw codestreams,
  telling them apart by their first bytes. The boxes are checked (signature,
  `ftyp` with `jp2 ` compatibility, `jp2h` with an `ihdr` that matches the
  codestream) and the first `jp2c` codestream is decoded. Files that bend the
  box rules the way OpenJPEG tolerates decode as they do there: `ihdr`
  anywhere in `jp2h`, a second `jp2h`, and a `jp2c` whose length is wrong
  (past the end of the file, `LBox` 2..7, `XLBox` below 16, or shorter than
  its codestream), since the codestream ends where its tile-parts end. A file
  that is not JP2-compatible is `Unsupported`.
- In a JP2 file, a `Psot = 0` last tile-part ends at the end of its `jp2c`
  box, with or without `EOC`, when the box's length ends before the end of
  the file and the tile's packets end exactly there. Such a file decodes as
  in OpenJPEG, and its boxes after the codestream are reported. Otherwise
  the tile-part is read to the end of the file, as OpenJPEG reads it, and
  bytes after the last packet are refused. `read_header` makes the same
  check, walking that tile's packet headers within a fixed bound; a tile
  past the bound, or one whose POC volumes reach too many packets to walk,
  is read to the end of the file, so the boxes after `jp2c` are not
  reported. Bytes after a box that ends inside the codestream are never
  reported as metadata.
- The boxes after the codestream are read as OpenJPEG reads them after
  decoding. Unknown boxes there are skipped, but a malformed header box is
  `FileFormat`: a second signature or `ftyp`, a `jp2h` without `ihdr`, a bad
  `colr`, `bpcc`, `pclr`, `cmap` or `cdef`, or a box shorter than its header.
  That includes junk after the `EOC` of a `jp2c` with `LBox` 0, and padding
  inside a `jp2c` that reads as a bad box.
- A JP2 file with a palette (`pclr` + `cmap`) decodes to the palette's
  channels, one per palette column, with each channel's depth and sign from
  the palette. What is accepted and the output samples follow OpenJPEG, so a
  palette channel that reads another channel's column is `Unsupported`. Two
  differences: a palette column too deep for an `i32` sample is `Unsupported`,
  and channels from components of different sub-sampling each keep their own
  grid, where OpenJPEG fails. `Jp2Metadata::palette_applied` says whether the
  palette was applied. The expanded channels count against the same sample
  budget as the codestream (`Error::Limit`), so while the decoded components
  are still held a palette can briefly take about twice the budget: a
  one-column palette at the full budget peaks at about 515 MiB, measured.
- A JP2 file with channel definitions (`cdef`) puts its color channels in
  color order and labels every channel on `Jp2Metadata::channels` with its
  type and association. What is accepted and the output order follow OpenJPEG:
  each color entry moves its channel to its color's position, and other
  channels stay where the file put them. The entries describe the channels
  after the palette. Without a `cdef` (or under `raw_channels`), only the
  first N channels are colors, N being the color space's number of colors (3
  for sRGB and sYCC, 1 for grayscale, the profile's for an ICC profile); any
  channel past them is `Unspecified` with no association, since I.5.3.6 gives
  it no meaning. When the count is not known (no `colr`, an enumerated value
  without a listed count, or a profile too short to name its color space),
  every channel is labeled a color, in order. This also lets a file of two
  components, a grayscale ICC profile and no `cdef` convert under
  `ColorOutput::Srgb` as gray plus opacity, and an sRGB file of four
  components whose fourth differs in format from the colors passes through as
  coded, as OpenJPEG does. Under `Srgb`, a three-component sRGB or grayscale
  file at 4:2:0 or 4:2:2 is still refused, whatever its `cdef` labels or
  without one, where `opj_decompress` would convert it as sYCC.
- `DecodeOptions::raw_channels` returns a JP2 file's codestream components as
  coded, without the palette or channel definitions. The index plane is then
  labeled `ChannelType::Unspecified`, and only the boxes' layout is checked.
- `read_header_with` reads the header for a given `DecodeOptions`: it fails
  exactly when `decode_with` does for a fault in the boxes or the main header
  (a palette `cmap` that `raw_channels` leaves unchecked, a palette that fits
  the sample budget once the resolution is reduced, a code-block size past the
  limit in the main-header COD or COC, a reduction past the main header's
  decomposition levels), and its `width` and `height` follow the resolution
  reduction. `read_header` is the same at the default options.
- `ComponentInfo::width` and `ComponentInfo::height` give each output
  channel's size in samples, equal to the decoded `Component`'s under the same
  options: they follow the image origin (`ceil(x1/dx) - ceil(x0/dx)`, not
  `ceil(width/dx)`), the resolution reduction, the palette and channel
  definitions, and the sRGB conversion (sYCC chroma on the luma grid).
- `Image::jp2: Option<Jp2Metadata>` says what a JP2 file's header says about
  the samples (`None` for a raw codestream), with `ColorSpace`,
  `ChannelDefinition`, `ChannelType`, and `Association` (`WholeImage` for
  `Asoc` 0, `Color(n)` for 1 to 65534, `None` for 65535). The color space
  comes from the first usable `colr` box (`METH` 1 or 2); the ICC profile
  bytes of a `METH` 2 box are returned as they are, not applied. A `colr`
  shorter than 3 bytes, a `METH` 1 box without its `EnumCS`, a `METH` 2 box
  without a profile, and a `bpcc` box that is not one byte per component are
  `FileFormat` errors, as in OpenJPEG for boxes before the codestream.
- `Error::FileFormat` for a malformed JP2 box structure.
- All nine JP2 files of the ISO/IEC 15444-4 conformance suite decode within
  their compliance class, alongside its 23 codestreams.
- `read_header` returns a codestream's or JP2 file's `Header` (image area,
  each output channel's `ComponentInfo`, and the same `Jp2Metadata` as
  `decode`) without decoding any tile data, so a file whose tile data is cut
  short still has a readable header. A header fault is the same error
  `decode` returns.
- `Jp2Metadata` reports the `res ` box's capture and display resolutions
  (`Resolution`, `Res`, `Ratio`, with `Ratio::grid_points_per_meter`), and
  every top-level `xml ` and `uuid` box verbatim (`XmlBox`, `UuidBox`),
  including those after the codestream. A malformed `res ` never fails a
  decode, as in OpenJPEG.
- `Jp2Metadata::ipr` returns every top-level intellectual property box
  (`jp2i`) verbatim as an `IprBox`, and `Jp2Metadata::uuid_info` every
  top-level UUID info box (`uinf`) as a `UuidInfo`: the UUIDs of its `ulst`
  box and the version, flags and location of its `url ` box, with
  `UuidInfo::location_str` for the location as text. Both are collected before
  and after the codestream, like `xml ` and `uuid`. A malformed `uinf` (not
  exactly one `ulst` and one `url `, a `ulst` whose count does not match its
  length, a `url ` under 4 bytes, or a bad box inside it) is left out and
  never fails a decode, as OpenJPEG skips the box.
- `Jp2Metadata` reports the rest of the JP2 header: `image_header`
  (`ImageHeader`: the `ihdr` compression type, unknown-color-space flag and
  `IPR` flag as `ipr_flag`), `color_specifications` (every `colr` box in file
  order as a `ColorSpecification`, with its method, precedence, approximation
  and the bytes after them as `payload`; the first usable one still sets
  `color`), `file_type` (`FileType`: the `ftyp` brand, minor version and
  compatibility list) and `reader_requirements` (`ReaderRequirements`, with
  `StandardFeature` and `VendorFeature`: the first top-level `rreq` box's
  masks and features). A malformed `rreq` is left out and never fails a
  decode, as OpenJPEG skips the box.
- `DecodeOptions::color` takes a `ColorOutput`. The default, `AsCoded`,
  returns the samples as the file codes them. `Srgb` converts a JP2 file's
  color channels, picked by their labels, to sRGB:
  - sYCC to RGB, with sub-sampled chroma replicated to full resolution first.
    This uses the IEC 61966-2-1 constants and rounds, where OpenJPEG shortens
    the constants and truncates, so the two differ by an MSE of about 0.38 to
    0.49 a channel; the ISO references agree with it (MSE under 0.03);
  - a restricted ICC profile (monochrome, or three-component matrix/TRC)
    applied at its rendering intent, with the same output as OpenJPEG 2.5.0
    with LittleCMS 2.14, sample for sample. That holds on x86-64 Linux
    (glibc 2.39) and on every platform whose `fma` is correctly rounded
    (MinGW's, on `*-pc-windows-gnu`, may not be: rust-lang/rust#140515).
    i586 targets use x87 arithmetic, which is not exact IEEE, so the output
    there can differ too.

  sRGB and grayscale pass through. So do the files OpenJPEG leaves as coded:
  one or two channels in any color space but a monochrome profile, a profile
  LittleCMS refuses, a profile in a color space other than RGB, gray or YCbCr,
  and an RGB profile whose first four channels differ in format. The metadata
  then keeps the profile. A three-channel 4:2:0 or 4:2:2 image in any color
  space but sYCC is refused, where `opj_decompress` would convert it as sYCC.
  Other color spaces and channel layouts are `Unsupported`, and `Srgb` with
  `raw_channels` is `InvalidOptions`. After a conversion `Jp2Metadata::color`
  says sRGB, or grayscale for a monochrome profile whose output is neutral. A
  monochrome profile with a tinted white point gives three channels;
  `read_header` doesn't predict that, and `read_header_with` with the same
  options does. The table in `docs/correctness.md` ("sRGB output") lists every
  case, what OpenJPEG does with it, and why this decoder follows or differs.
- `DecodeOptions::with_raw_channels` and `DecodeOptions::with_color` set
  those options like `with_resolution_reduction` does; the fields stay public.
- Every public type added in this release is `#[non_exhaustive]` except
  three, so later variants and fields (a JPX color method, an output ICC
  profile, where an `xml ` box sat) are not breaking changes. The enums
  `ColorSpace`, `ColorOutput` and `MappingKind` need a `_` arm in a
  `match`. The structs `Header`, `ComponentInfo`, `Jp2Metadata`,
  `ImageHeader`, `ColorSpecification`, `FileType`, `ReaderRequirements`,
  `StandardFeature`, `VendorFeature`, `Resolution`, `Res`, `Ratio`,
  `XmlBox`, `IprBox`, `UuidBox`, `UuidInfo`, `Palette`, `PaletteColumn`,
  `ChannelMapping` and `ChannelDefinitionEntry` can't be built outside the
  crate, and a pattern on them needs `{ field, .. }`. `ChannelDefinition`,
  `ChannelType` and `Association` are exhaustive: they mirror the `cdef`
  box's `Typ` and `Asoc`, whose every 16-bit value they already hold, so a
  caller may build a `ChannelDefinition` and match on both enums without a
  catch-all arm.
- `Jp2Metadata::palette` and `Jp2Metadata::channel_mapping` give the file's
  palette table (`Palette`, `PaletteColumn`) and component mapping
  (`ChannelMapping`, `MappingKind`). They are set whenever a palette and a
  `cmap` are present, also under `raw_channels`, so a caller can expand the
  index planes itself. `Palette::entry` returns one entry's columns, or
  `None` past the last entry.
- `Jp2Metadata::channel_definitions` gives the `cdef` entries as written
  (`ChannelDefinitionEntry`), and `channel_mapping` is the mapping the default
  decode applies in both modes, OpenJPEG's single-component correction
  included. With `raw_channels`, a caller can rebuild the default decode in
  order from the raw one: expand the index planes through `palette` with
  `channel_mapping`, then order them by the rule on `channel_definitions`. The
  `raw_channels` documentation gives the recipe.
- `src/jp2/lcms.rs` is a Rust port of parts of LittleCMS 2.14 (MIT). Its
  notice is at the top of the file and in the README.
- `src/jp2/math.rs` and `src/jp2/math_data.rs` are a Rust port of `pow` and
  `log` from Arm's optimized-routines (MIT), so the ICC output does not
  depend on the platform's `libm`. Its notice is at the top of `math.rs` and
  in the README.
- `DecodeOptions::packet_work_limit` and
  `DecodeOptions::code_block_work_limit` raise or lift the decode's two
  work bounds, for a valid file that goes past one: `WorkLimit::Default`
  (the default), `WorkLimit::Raised(n)` for n times the bound, or
  `WorkLimit::Unlimited`. The packet limit also bounds the walk
  `read_header_with` makes of a `Psot = 0` tile. Raising it does not raise
  the memory a POC sequence's packets are held in.
- `DecodeOptions::sample_limit` (and `with_sample_limit`) raises or lifts
  the limit on the decoded image's size, with the same `WorkLimit` values:
  `Raised(n)` allows n times 2^28 samples, and `Unlimited` allocates what
  the header declares, refusing only an image too large for the target to
  address. `Raised(0)` is `InvalidOptions`. The refusal names the option and
  the smallest `Raised` factor that admits the file.
- The `Error::Limit` message for a work bound names a
  `WorkLimit::Raised(n)`, worked out from the refusal alone, and points to
  `WorkLimit::Unlimited` for trusted input. For the code-block bound it is
  what the tiles seen so far need (exact for a file of one tile). For a POC
  sequence that reaches too many packets it is what that reach needs; the
  walk after the check can still run out. For the packet walk it is the
  next factor up, and the file may need more, so double `n` on a retry. The
  variant stays `Limit`.
- `DecodeOptions::strict` (and `with_strict`), `true` by default. With it
  off, the decode takes missing data and bytes left after a tile's last
  packet, as `opj_decompress -allow-partial` does, and says what it left
  out on `Image::report`, a `DecodeReport`. The input may end anywhere after
  the first `SOT` marker: missing tiles are 0, packets never read add
  nothing, a cut packet keeps each code-block's whole layers, and a `TNsot`
  that disagrees with the tile-parts found is advisory. Every other fault,
  and every work and sample bound, is refused as when strict. The report
  lists `ReportEntry` values (`MissingTiles`, `MissingPackets`,
  `TruncatedTilePart`, `TilePartCountMismatch`, `LeftoverBytes`,
  `MissingEoc`), at most five per tile-part plus two, and
  `complete_reduction`, the finest reduction the data holds in full, so a
  codestream can be decoded as it streams in. A strict decode is unchanged
  and its report is `DecodeReport::default()`.
- `Header::max_reduction`, the largest `resolution_reduction` the main
  header allows.

### Fixed

- The code-block count guard now rejects before allocating the per-band
  geometry, matching the precinct guard. The budget is threaded through the
  geometry walk and spent per subband, so a header pairing a large tile with
  tiny code-blocks is refused up front instead of after its block and
  precinct-index vectors have been materialized (~160 MiB transient in the
  worst case).
- A tile-part with `Psot = 0` (the last one) now takes its data up to the last
  two bytes of the codestream without checking that they are `EOC`, as
  OpenJPEG does. A codestream that ends in two other bytes now decodes. One
  with no `EOC` loses its last two data bytes and fails in the tile
  ("overruns the tile-part"); it used to be rejected as not terminated by
  `EOC`.
- Tile headers are checked in time proportional to their own markers. Each
  tile's header used to be a copy of the main header, made for every tile
  before any check, so the time grew as the tiles times the components or
  the TLM entries. 65025 1x1 tiles over 16384 components took 46 s to be
  refused and now take 17 ms; 65025 tiles behind 4 MB of TLM entries took
  85 s to decode and now take 63 ms.
- A code-block with an RGN maxshift and the bypass style now switches to raw
  coding at its fifth coded bit-plane, counting the shifted planes, as the
  standard and OpenJPEG's encoder do. It switched `SPrgn` planes late, as
  OpenJPEG's decoder does, and decoded such streams wrong or refused them.
  Streams without a maxshift decode as before.
- A wrong segmentation symbol no longer fails the decode when the MQ decoder
  read it from past the end of its segment on a pass the encoder did not
  terminate. On a terminated pass (every pass under `restart`, the block's
  last cleanup, and the bypass cleanups from the fourth coded plane down) it
  is still an error. Rate control can cut an unterminated pass a few bytes
  short, and `opj_compress` output with segmentation symbols and `-r` was
  refused about once in 50 encodes. Such streams now decode as in OpenJPEG. A
  wrong symbol read from the segment's own bytes is still a `Codestream`
  error.
- A POC in a tile's first tile-part header now replaces the main header's
  POC for that tile, as the standard requires. The tile's volumes were
  appended after the main header's, as OpenJPEG does, so a file with both
  read its packets in the wrong order and was refused or decoded wrong. A
  tile whose first POC is in a later tile-part, which the standard does not
  allow, is still read as OpenJPEG reads it. The 32-volume limit counts the
  volumes the tile walks, so the main header's no longer count against a
  tile that replaces them.
- A single-tile decode no longer holds two image-sized buffers at once. The
  image plane takes the tile's buffer instead of a copy, and the inverse
  wavelet frees each level's subbands as it merges them. At the full sample
  budget, one 9/7 tile peaked at about 580 MiB and now peaks at about
  340 MiB.
- A code-block whose coding passes never get a byte now decodes, as the
  standard allows (B.10.7) and as OpenJPEG decodes it: the passes read the
  `0xFF` fill the decoder appends (D.4.1). It was refused with "included
  code-block has coding passes but no coded bytes". The passes still count
  against the code-block work bound.
- The 9/7 lifting constants β and δ are now the nearest `f32` values to the
  standard's (Table F.4), as OpenJPEG's are. Each was one `f32` step off. 9/7
  output moves by at most 1 on a few samples.
- A packet header packed in a `PPM` or `PPT` marker that reads past the end
  of its packed bytes is now a `Codestream` error, with `strict` on or off.
  The missing bits were read as 1s and the decode returned a wrong image
  with no error.

### Changed

- The decoded image may hold up to 2^28 samples (1 GiB as `i32`), up from
  2^26, so a Sentinel-2 10 m band (10980 x 10980) and RGB images up to
  about 89 MP decode at the defaults. The samples are counted at the
  decode's resolution reduction, so a reduced decode of a larger image is
  admitted when the reduced image fits. The image is now refused after the
  whole main header is read, so a header with another fault reports that
  fault first. The `Error::Limit` message changed: it gives the samples
  needed and names `DecodeOptions::sample_limit`.
- At a resolution reduction, the packet work bound
  (`DecodeOptions::packet_work_limit`) also counts each code-block of the
  resolutions the reduction drops, which every tile still builds, so a small
  file of many tiles cannot take minutes to decode at a reduction. A decode
  at full resolution spends what it did before.
- The docs say what a caller can do about each failure. The crate docs and
  README gained "Damaged input" and "Limits" sections, `Error` says which
  variant to expect for non-JPEG-2000 input and that messages are not
  stable, and `WorkLimit`, `decode_with` and `ColorOutput::Srgb` have
  runnable examples (a retry after `Error::Limit`, a reduction with sRGB
  output, and an 8-bit RGBA buffer).
- The crate uses American spellings: color, gray, grayscale, -ize, behavior,
  center. Every public name added in this release is spelled that way
  (`ColorSpace`, `ColorSpace::Grayscale`, `ColorOutput`,
  `ColorSpecification`, `DecodeOptions::color` and `with_color`,
  `Jp2Metadata::color` and `color_specifications`,
  `ImageHeader::unknown_color_space`, `ChannelType::Color`,
  `Association::Color`, `Ratio::grid_points_per_meter`); none of these
  existed in 0.3.0. Error messages use the same spellings, so text such as
  "the color transform needs three components" changed from 0.3.0's
  "colour". Box types, field names and spec titles stay as ISO writes them
  (`colr`, `EnumCS`, "Colour Specification box").
- Breaking: `decode_with` takes `&DecodeOptions` instead of `DecodeOptions`,
  so one set of options serves many calls: `decode_with(bytes, &options)`.
- Breaking: `DecodeOptions` is no longer `Copy`, so a later option can
  carry data (an output ICC profile). It stays `Clone`; clone it where you
  reuse one. `ColorOutput`, new in this release, is likewise `Clone` but
  not `Copy`. For the same reason the new
  `ComponentInfo`, `ChannelMapping`, `MappingKind`, `PaletteColumn` and
  `ChannelDefinitionEntry` are `Clone` but not `Copy`.
- A code-block size past the limit in any COD or COC, in the main header or
  a tile-part header, is now refused when that header is read, even when a
  later COC overrides it, as OpenJPEG does. `p0_02.j2k` with byte 55 set to
  56 decoded in 0.3.0 and is now `Error::Marker`. Likewise a resolution
  reduction past the main header's decomposition levels is checked before
  the tile data, so a truncated file with an impossible reduction reports
  the reduction rather than the truncation.
- Three encoder mistakes that OpenJPEG corrects on purpose now decode as
  they do there; each was an error in 0.3.0. A tile whose last tile-part has
  `TPsot` equal to `TNsot` (one part more than declared, as Adobe Photoshop
  writes them): when the first tile to have all its declared parts has one
  more, every tile's count is raised by one. A later tile-part that repeats
  the COD, COC, QCD or QCC already in force (as BAP Image Systems writes
  them): it is accepted and changes nothing, while one that differs is
  still `Error::Codestream`. And the color transform flag on an image of
  one or two components, which is ignored.
- A JP2 file used to be `Unsupported`; it now decodes, or fails with
  `FileFormat` when its boxes are malformed.
- A JP2 file whose `ihdr` component count differs from its codestream's
  decodes, as in OpenJPEG; the codestream decides the image. An `ihdr` with a
  zero width, height or component count, or more than 16384 components, is
  `FileFormat`.
- A JP2 file's header is checked before its tile data: a file with a header
  fault and a broken codestream reports the header fault.
- A codestream component too deep for an `i32` sample is refused as
  `Unsupported` when the main header is read, before any tile data.
- Packet headers that name many code-blocks and include none read faster:
  where the inclusion tag tree already puts a group of code-blocks past the
  layer, the decode steps over the group instead of each code-block. A
  1 KB file whose one-byte packet headers each name 2^17 code-blocks took
  4.6 s and now takes 14 ms. `opj_compress` writes such headers for every
  layer, so its output with small code-blocks and many layers gains too.
- Reading the packet headers is bounded: `decode` may take 2^24 steps that
  read no header bit, or 32 per byte of tile data when that is more,
  counting packets, steps over code-blocks a header reads no bit for, and
  the positions a positional progression order sweeps, for the components
  that hold a precinct at each resolution. Past it the decode is
  `Error::Limit`. Encoder output seen spends under 13% of it, but a
  valid file of many layers that each include a few scattered code-blocks
  can exceed it (see `docs/correctness.md`). A POC-repeated CPRL sweep of
  819 bytes took 6.5 s and a 9 KB PCRL sweep 29 s; both are now refused in
  a fraction of a second. OpenJPEG has no such bound and decodes them.
  `DecodeOptions::packet_work_limit` raises or lifts the bound.
- Decoding the code-blocks is bounded: `decode` may run 2^26 sample-passes
  (coding passes times code-block samples), or 2^15 per byte of tile data
  when that is more. A tile past it is `Error::Limit` before any of its
  code-blocks is decoded. Each pass scans its whole code-block however few
  bytes it carries, so a 76 KB file of 8192^2 samples, every 64x64
  code-block asking for 88 passes, took 19 s and is now refused in 6 ms.
  A file just under the bound can still take about 0.5 s, or 0.25 ms a
  byte. Encoder output seen spends up to 43% of the bound
  (`opj_compress -n 1` on a flat 16-bit image). OpenJPEG has no such bound
  and decodes the 76 KB file in 9 s.
  `DecodeOptions::code_block_work_limit` raises or lifts the bound.
- A resolution that holds no precinct costs less: it gets no band or
  code-block geometry, and the packet walk checks no component there. A tile
  of one sample at an odd origin has 32 such resolutions per component, so
  64 such tiles of 16384 components (1.1 MB) decode in 10.5 s instead of
  0.3.0's 20.9 s, and one such tile in 0.18 s instead of 0.41 s.
- A POC marker whose volumes reach the same packets again costs a bit per
  packet instead of about 80 bytes, and a few ns per repeat instead of
  about 1 µs, in `decode` and in `read_header`. A 1 MB file of 31 identical
  volumes took 5.7 s and 105 MB and now takes 0.15 s and 3.5 MB; a 10.5 MB
  one of 160 components took 4.5 s and 824 MB and now takes 0.32 s and
  14 MB. When every volume is LRCP or RLCP and they reach more packets than
  the decode's work bound allows, the decode is `Error::Limit` before it
  reads one, where 0.3.0 read on until the data ran out. With a positional
  volume, each repeat skipped past that point counts 12 steps against the
  bound.
- A POC marker whose volumes leave out packets of a tile is refused as
  `Error::Codestream`, naming the tile, component, resolution and first
  missing layer: each resolution of each component, counting the
  component's own levels, must be in a volume that reaches every layer. The
  main header's and every tile-part's volumes count, and a resolution with
  no precincts needs none. These files used to decode with the missing
  packets left empty. OpenJPEG decodes them that way, and `opj_compress`
  writes them when its `-POC` volumes do not cover the tile. The refusal
  holds under a resolution reduction too. `read_header` does not check
  coverage, so for such a file in JP2 it still reports the boxes after the
  codestream.
- Breaking: two new error variants split what `Error::Unsupported` used to
  conflate. `Error` is `#[non_exhaustive]`, so a `match` still compiles,
  but an `Unsupported` arm no longer sees these inputs; they reach your
  other arms. Re-check each such arm. `Error::Limit` is returned when an input
  exceeds a decoder resource guard (image/component sample budget, precinct and
  code-block counts, bit-plane depth, progression volumes), and
  `Error::InvalidOptions` when the caller's `DecodeOptions` are invalid for
  the codestream (a `resolution_reduction` that consumes a whole pyramid).
  `Error::Unsupported` now means only a valid-but-not-decoded feature.
- The inverse wavelet transform is faster. It applies the boundary
  extension only at the two ends of each row and column, and it runs the
  vertical pass on eight columns at a time. The output is unchanged, and
  9/7 images decode about 1.3 times faster.
- Code-block decoding is faster. The coding passes keep the arithmetic
  decoder's registers in locals, read the state of a four-row column with
  one load, and skip a column that has nothing to code with one test. The
  output is unchanged, and the benchmark set decodes 1.2 to 1.4 times
  faster.

## [0.3.0] - 2026-07-18

General ISO/IEC 15444-1 codestream decoding: all 23 entries of the ISO/IEC
15444-4 conformance suite decode within their class-1 error bounds. This
release also carries the v0.3.0 pre-release review's security hardening and
API future-proofing.

### Added

- PPM (main-header packed packet headers, A.7.4): every tile-part's packet
  headers may be relocated into the main header, framed by 4-byte `Nppm`
  lengths whose chunks can straddle marker boundaries. The PPM markers are
  joined in `Zppm` order and each tile-part reads its headers from its own
  chunk, exactly as PPT feeds them per tile-part; a codestream carrying both
  PPM and PPT is rejected. This completes packed packet headers — and with
  them the corpus: all 23 ISO/IEC 15444-4 conformance entries decode within
  their class-1 bounds. Along the way the predictable-termination check was
  relaxed to OpenJPEG's tolerance: a terminated segment that synthesises
  marker bytes past its end is valid (`p1_05` carries one); only unconsumed
  padding still signals a wrong length.
- The bypass (lazy) code-block style (D.5): from the fifth coded bit-plane on,
  significance and refinement passes are packed as raw bits rather than
  MQ-coded decisions. Tier-2 splits the codeword into the lazy segment
  pattern so every segment is a homogeneous raw or MQ run, and Tier-1 decodes
  the raw runs with MSB-first, bit-unstuffed reads. All six Part 1 code-block
  styles now decode; only the two HTJ2K bits stay rejected.
- PPT (packed packet headers, tile-part): a single-tile codestream may move its
  packet headers out of the bitstream into `PPT` marker segments, leaving only the
  bodies inline. Tier-2 now reads headers from that packed buffer while bodies (and
  SOP markers) stay inline, with EPH moving into the packed buffer alongside the
  headers. Grades conformance entry `p1_02`.
- Two code-block coding styles: **vertically causal context** (the significance
  context of a stripe's bottom row ignores the next stripe) and **reset context
  probabilities** (the MQ context model reinitialises after every coding pass).
  Both are graded bit-exact against OpenJPEG.
- Inverse ICT (irreversible colour transform): a three-component 9/7 codestream
  that signals the multiple-component transform now reconstructs colour by
  inverting the float YCbCr→RGB transform (Annex G.3), applied after the IDWT and
  before the level shift. The wavelet picks the transform — 5/3 is RCT, 9/7 is
  ICT — read off the first colour component; a mix across the three is rejected.
  The inverse matrix matches OpenJPEG's constants, so the decode lands within the
  compliance-class bounds. Grades conformance entry `p0_04`.
- CRG (component registration): the per-component `(Xcrg, Ycrg)` sub-pixel
  registration offsets are parsed and recorded on the decoded header. They are
  informational — how a display should align the components, not a change to the
  samples — so they are recorded, not resampled. Grades conformance entries
  `p0_03` and `p0_15`.
- POC (progression-order change): a codestream may now carry a `POC` marker that
  replaces `COD`'s single progression with a sequence of volumes, each an order
  over its own layer/resolution/component sub-range. The packet iterator walks the
  volumes in turn and emits each packet once, by the first volume that reaches it.
  A `POC` is decoded in the main header and in any tile-part header, where its
  volumes accumulate across the tile's parts. Grades conformance entries `p0_07`
  and `p0_13`.
- Non-zero image and tile origins (`XOsiz`/`YOsiz`, `XTOsiz`/`YTOsiz`): the
  canvas no longer has to start at the reference-grid origin. Subband extents,
  the code-block partition, and the inverse DWT's interleave parity already read
  absolute coordinates, so the origin flows through unchanged; the SIZ validation
  now enforces the Table A-9 constraints a non-zero origin makes load-bearing
  (`XOsiz < Xsiz`, `XTOsiz <= XOsiz`, `XTOsiz + XTsiz > XOsiz`) and bounds the
  decode by the image area `(Xsiz − XOsiz)·(Ysiz − YOsiz)` rather than
  `Xsiz·Ysiz`. Grades conformance entries `p1_01` (odd offset) and `p1_07`.
- Precinct partition: codestreams that subdivide each resolution into precincts
  (`Scod`/`Scoc` bit 0 and the `SPcod`/`SPcoc` precinct sizes) now decode, with
  one packet and one pair of tag trees per precinct. Because a resolution can now
  hold more than one precinct, the position axis of the packet order stops being
  a degenerate counter: RPCL, PCRL and CPRL now enumerate precincts by sweeping
  the reference grid, which is also what tells PCRL and CPRL apart for the first
  time. Precinct-partitioned codestreams are capped at 2^18 precincts.
- Multiple tiles and tile-parts: the reference grid may be tiled, and each tile
  decodes independently in its own coordinate frame — its tile-part headers
  resolved over the main header's markers in the standard's precedence order
  (a tile COD outranks a main-header COC), its own packets and wavelet
  pyramid — before writing its rectangle of the canvas. Tile-parts interleave
  across tiles in codestream order.
- `decode_with` and `DecodeOptions`: decode at a resolution reduction. Each
  dropped level halves the output in both axes (rounding up) using the wavelet
  pyramid's own lower resolutions — the natural way to get a thumbnail or a
  quick preview without paying for the full image. `decode` is unchanged and
  equals `decode_with` at the default options. A reduction that would consume
  some component's whole pyramid is rejected as `Error::Unsupported`.
- The length markers TLM, PLM, and PLT (A.7): informational pointers into the
  codestream the decode does not depend on. TLM is decoded, validated, and
  recorded on the header for callers; PLM and PLT are validated and skipped, as
  OpenJPEG does, since their length chains may split mid-entry across markers.
- Region of interest (the RGN marker and the inverse maxshift, A.6.3 / H.2): a
  region lifted above its background by `SPrgn` bit-planes starts its code-blocks
  that much higher, and any magnitude at or above `1 << SPrgn` is shifted back
  down after the passes. A tile-part RGN replaces the main header's for its
  tile. The packet header's zero-bit-plane count is signalled against the
  maxshift-raised plane count (H.2's Kmax), which is also where a
  background-only block's starting plane is measured from. Grades conformance
  entry `p0_06` (maxshift 11 in the main header, 9 in the tile-part's
  override).
- Per-component coding and quantization overrides (COC, A.6.2; QCC, A.6.5),
  resolved override-else-default onto each component: COC over COD, QCC over
  QCD. The wavelet, decomposition depth, and code-block geometry become
  per-component. `Ccoc`/`Cqcc` widen to two bytes at 257 components — `p0_13`
  sits exactly on that boundary. A COC that moves one of the colour-transform
  components off the shared wavelet is rejected.
- The `predictable termination` and `segmentation symbols` code-block styles
  (D.4.2, D.5). Segmentation symbols are part of the codeword: four decisions
  in the uniform context close every cleanup pass, consumed first and judged
  second — anything but 1010 reports the coded data corrupt. Predictable
  termination adds OpenJPEG's per-segment residue check.
- The `restart` code-block style (termination on each coding pass, Annex D).
  Each coding pass is a terminated codeword segment: Tier-2 reads a length field
  per pass rather than one per contribution, and Tier-1 re-initialises the MQ
  decoder at each segment while carrying the context states, the significance
  map, and the bit-plane counter across. Conformance codestream `p0_12` now
  decodes bit-exact. A style byte that mixes `restart` with styles that are not
  yet decoded names only the parts that block it.
- The packet delimiters SOP and EPH (Annex A.8). `COD`'s `Scod` bits 1 and 2 are
  honoured independently. SOP *may* precede each packet even when signalled, so
  its absence is tolerated; its `Lsop` and `Nsop` are validated, which is
  stricter than OpenJPEG (whose source reads `/* TODO : check the Nsop value */`).
  EPH *shall* follow every packet header, an empty packet's included, and its
  absence is a `Codestream` error. A reserved `Scod` bit is a `Marker` error.
- The remaining progression orders: RLCP, RPCL, PCRL, and CPRL (Annex B.12.1).
  The packet walk is now driven by `COD`'s progression code rather than assuming
  LRCP. Under maximal precincts the position axis has one value, so PCRL and CPRL
  enumerate the same sequence; they are distinguishable only once the precinct
  partition lands. A reserved progression code is a `Marker` error.
- Multiple quality layers (Annex B.10). A code-block's coding passes and byte
  contributions now accumulate across the layers that include it, and its
  inclusion tag tree, zero-bitplane tag tree, and `Lblock` length indicator
  persist across the packets of a precinct instead of being rebuilt per packet.
  Tier-1 decodes the concatenated contributions as one MQ codeword. `COD`
  declaring zero layers is a `Marker` error.
- The inverse reversible color transform (RCT, Annex G.2). When `COD` signals
  the multiple-component transform on the 5/3 path, the first three components
  are recombined after the inverse DWT and before the DC level shift. The
  wavelet selects the transform: 5/3 means RCT, 9/7 means ICT (decoded above);
  Part 2's array MCT (`Smct = 2`) stays rejected. A codestream that signals the
  transform without three components of matching depth, sign, and sub-sampling
  is rejected as a `Marker` error rather than silently decoded without it.
  Conformance codestream `p0_14` now decodes bit-exact.
- Multi-component decoding. Every component a codestream declares reconstructs
  onto its own sample grid, honoring its bit depth, sign, and `XRsiz`/`YRsiz`
  sub-sampling. The component axis runs through Tier-2 packet enumeration
  (LRCP's resolution-major, component-minor order), Tier-1, dequantization, and
  the inverse DWT.
- Two synthetic multi-component fixtures, generated and graded against OpenJPEG
  by `scripts/gen-multicomponent-fixtures.py`. The ISO/IEC 15444-4 corpus cannot
  grade multi-component decoding on its own: every one of its multi-component
  entries also needs a feature that is not decoded yet.
- A grading harness over the vendored ISO/IEC 15444-4 conformance corpus
  (`tests/conformance_part4.rs`). It decodes all 23 entries and grades each
  graded component against its class-1 reference by peak absolute error and
  mean-squared error, reading the `.pgx` reference images directly. Entries
  using features the decoder does not implement yet report as *not yet
  decoded*; `p0_09` decodes and matches its reference exactly.
- SIZ now parses every component a codestream declares (`Csiz` up to 16384),
  with each component's bit depth, sign, and `XRsiz`/`YRsiz` sub-sampling, and
  validates them: a zero or oversized component count, a zero sub-sampling
  factor, a bit depth above 38, and a component-record count that disagrees with
  `Csiz` are all typed errors. `Siz::component_extent` derives a component's own
  sample-grid dimensions from the reference-grid equations.

### Changed

- **Breaking.** `Image`, `Component`, and `DecodeOptions` are now
  `#[non_exhaustive]`, so adding a field — decoded metadata on the image, a new
  decode option — stops being a semver break. Construct options with
  `DecodeOptions::default()` and the new `with_resolution_reduction` method;
  construct images by hand (a test harness's expected values, say) with the new
  `Image::new` and `Component::new`. `DecodeOptions` also drops its
  `PartialEq`/`Eq` derives, which a future non-comparable option would forbid.
- **Breaking.** The never-constructed `Error::Packet` and `Error::Tier1`
  variants are gone — tier-2 and tier-1 corruption reports as
  `Error::Codestream`, as it always did — and `Error` is now
  `#[non_exhaustive]`, so future failure classes are not semver-major breaks.
- **Breaking.** `Image` now carries the image area on the reference grid
  (`width`, `height`) and a `components` vector, instead of describing a single
  component inline. The per-component fields moved to the new `Component` type,
  which also records the `x_sampling`/`y_sampling` sub-sampling factors, and
  `Image::sample` moved to `Component::sample`. Reach a component with
  `image.component(0)`. This settles the output shape that multi-component
  decoding threads the pipeline toward.
- The main header is now walked before it is judged. Marker segments are located
  first, then interpreted, so a codestream carrying a feature outside the decoded
  subset is still traversed end to end rather than abandoned mid-header. An
  unrecognized marker segment is stepped over by its length during the walk, and
  the reserved `0xFF30`–`0xFF3F` range is treated as carrying no segment.
- A marker the decoder does not recognize is now reported as `Unsupported`
  instead of `Codestream`. It is walked past, not decoded past: every marker code
  is allocated by some part of the standard, so an unknown one may change what
  the packet data means. `CAP` (which an HTJ2K codestream carries), `PPM`, `PPT`,
  `PLM`, and `CRG` are named for a clearer message, and `PPT` now rejects in a
  tile-part header where it previously read as a structural error.

### Fixed

- The 9/7 subband gain is applied per high-pass filtering inside the inverse
  DWT (an exact `2/K` on each high-pass scale) instead of baked into the
  quantization step as `2^gain`. The two agree only while a coefficient is
  high-pass exactly its nominal-gain many times, which an empty coarse
  resolution breaks: `p1_06`'s small odd-offset tiles decoded with a constant
  DC offset per component. Bit-for-bit identical on the ordinary case, so no
  other fixture moves.
- The progression-volume list is capped at 32, matching OpenJPEG's `pocs`
  array, and the cap binds everywhere volumes accumulate: per marker, across a
  tile's parts, and on the combined main-plus-tile list a tile's walk runs.
  Uncapped, the walk re-enumerates the packet space once per volume and a
  duplicate emission consumes no bitstream bytes, so a crafted megabyte of POC
  markers over one-byte packets could buy minutes of compute — quadratic work
  from linear input. Found in the v0.3.0 pre-release security review. A POC
  volume declaring zero layers (`LYEpoc == 0`, illegal per Table A-33) is also
  now rejected at parse as `Error::Marker` instead of surfacing later as a
  packet desync.
- Four resource and desync guards from a whole-repo review. A QCD/QCC step
  table keeps at most the 97 entries a 32-level decomposition can use —
  excess is dropped the way OpenJPEG caps at `J2K_MAXBANDS` — so a crafted
  65535-byte segment can no longer multiply into gigabytes through the
  per-component parameter clones. The total code-block count is capped at 2^19
  before any per-block state is allocated, closing the counterpart bomb built
  from legal 4×4 code-blocks; the precinct count is capped at 2^18 alongside it,
  which the block count does not bound (a band empty in one axis has no blocks
  while its resolution still has a full column of precincts). And the fixture
  harness now grades a panicking decode as a failure — the skeleton-era `Pending`
  outcome had let one pass CI silently.
- Arithmetic hardening against hostile coefficients: the 5/3 lifting sums run
  in `i64` and saturate (they could leave `i32` near the Tier-1 magnitude
  ceiling), and the MQ decoder's BYTEIN adds wrap explicitly (matching the C
  reference's `OPJ_UINT32` semantics in every build profile).
- The 9/7 dequantization applied the interval mid-point reconstruction twice:
  Tier-1 already hands over magnitudes reconstructed at the mid-point, and the
  half-step bias dequantization added on top canceled only against the integer
  halving's truncation on odd double-scale magnitudes. The irreversible path
  now scales the double-scale value by half the step with no added bias,
  matching OpenJPEG's fold of the halving into the float multiply.
- Error classification now matches the crate's own variant conventions:
  reserved decomposition-level counts (33–255), short quantization step tables,
  and the 9/7-wavelet-with-no-quantization pairing reject at parse as
  `Error::Marker` (the latter two previously surfaced deep in dequantization
  as `Inconsistent`; both are documented stricter-than-oracle calls — see
  `docs/correctness.md`); `0xFF00`/`0xFFFF` in marker position and TLM/SOP/EPH
  in a tile-part header reject as `Error::Codestream`; standard-legal bit
  depths 33–38 report `Unsupported` rather than `Marker`.
- A codestream that sets any `SPcod` code-block style flag — selective
  arithmetic coding bypass, reset context probabilities, termination on each
  coding pass, vertically causal context, predictable termination, segmentation
  symbols, or either HTJ2K block-coding flag — decoded to wrong samples instead
  of being rejected. The flag was parsed, passed to Tier-1, and discarded. It
  was first rejected as `Unsupported`, naming the flags that are set; each Part
  1 style has since been decoded (see above), leaving only the HTJ2K flags
  rejected.
- A JP2 file is now rejected as `Unsupported`, naming the wrapper and pointing at
  the contained codestream, instead of reporting a missing `SOC` marker. JP2 is
  valid JPEG 2000; it is simply not the bare codestream this decoder reads.

## [0.2.0] - 2026-06-21

### Changed

- Reduced the public API to its intended surface: `decode`, `Image`, `Error`,
  and `Result`. The pipeline modules (`codestream`, `tier1`, `tier2`, `dwt`,
  `quant`) are now crate-internal. Nothing outside the crate referenced them;
  this keeps the documented surface to what a caller can use and lets the stages
  evolve without churning the committed API. Breaking for anyone who imported a
  pipeline module directly.
- Error messages and rustdoc describe the supported subset directly instead of
  internal roadmap phase numbers.

### Added

- `#![warn(missing_docs)]` to keep the public surface documented, a tested
  crate-level usage example, and `[package.metadata.docs.rs]` configuration.

## [0.1.0] - 2026-06-21

First release. Implements Phase 1: the GRIB2 template 5.40 (`grid_jpeg`) decode
path, end to end, gated against the OpenJPEG/eccodes oracle.

### Added

- Public API: `rust_j2k::decode(&[u8]) -> Result<Image>`, decoding a raw JPEG
  2000 codestream (Annex A, no JP2 boxes) into a single integer-component image.
- Codestream parsing (Annex A): SOC, SIZ, COD, QCD, SOT, SOD, EOC, with COM
  skipped. Out-of-subset markers and fields are rejected with `Error::Unsupported`;
  truncated or malformed input with `Error::Codestream` / `Error::Marker`.
- MQ arithmetic decoder (Annex C), verified against the standard's worked vectors.
- Tier-1 EBCOT bit-plane decoding (Annex D): significance, refinement, and
  cleanup passes with context formation.
- Tier-2 packet parsing (Annex B): single tile, single quality layer, LRCP
  progression, tag-tree decoding, no precinct subdivision.
- Inverse discrete wavelet transform (Annex F): 5/3 reversible integer lifting
  (bit-exact) and 9/7 irreversible float lifting, 2-D as 1-D over rows then
  columns with symmetric boundary extension.
- Dequantization (Annex E), DC level shift, clamping, and image assembly.
- Conformance harness (`tests/conformance.rs`) grading decodes against committed
  `expected.json` oracle snapshots: bit-exact for 5/3, within a stated tolerance
  for 9/7. Runs with no external tools.
- `cargo-fuzz` target over `decode` for robustness against malformed input.
- Minimum supported Rust version of 1.87, declared via `rust-version` and
  verified in CI.
- Project documentation: README install/usage/supported-subset sections,
  `CONTRIBUTING.md`, `SECURITY.md`, and `CODE_OF_CONDUCT.md`.

### Scope

This release decodes only the GRIB2 §5.40 subset: a single integer component,
one tile, one quality layer, LRCP progression, no precincts, no ROI, no JP2 box
wrapper, and no multi-component or color transform. Anything outside the subset
is rejected cleanly rather than half-decoded. Wider Part 1 coverage, the JP2 file
format, HTJ2K, and an encoder are later-phase work; see `docs/roadmap.md`.

[Unreleased]: https://github.com/D0ubleD0uble/rust-j2k/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/D0ubleD0uble/rust-j2k/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/D0ubleD0uble/rust-j2k/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/D0ubleD0uble/rust-j2k/releases/tag/v0.1.0
