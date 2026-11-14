# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

JP2 files now decode, with their header metadata, and can be converted to
sRGB. Whole-file decodes are several times faster than 0.3.0. Each
decode's work is bounded for its file's size and its image by a fixed sample
limit, so a small hostile file cannot take minutes or unbounded memory, and a
damaged codestream can be decoded in part on request. Some of these changes break 0.3 callers or change what they
see: read "Migrating from 0.3" first.

### Migrating from 0.3

Every change that can break a 0.3 caller or change what it sees is below.
The first fails to compile; the others compile but behave differently.

- Pass options by reference: `decode_with(bytes, &options)`.
  `DecodeOptions` is no longer `Copy`, so call `.clone()` where you reuse
  one by value.
- Re-check every `match` arm on `Error::Unsupported`. Some inputs that were
  `Unsupported` now return a new variant and fall to your wildcard arm: a
  `resolution_reduction` past the decomposition levels is
  `Error::InvalidOptions`, a resource guard is `Error::Limit`, and a
  malformed JP2 file is `Error::FileFormat`. `Unsupported` now means only a
  valid feature that is not decoded. A "level not available, fall back to
  full resolution" path should match `InvalidOptions`; `Header::max_reduction`
  gives the largest reduction the main header allows.
- If only a raw codestream is valid input for you (GRIB2 section 7, say),
  refuse JP2 files yourself, because `decode` and `decode_with` now accept
  them. Refuse input that does not start with `SOC` (`FF 4F`), or call
  `read_header` and refuse the input when `header.jp2` is `Some`. Some raw
  codestreams that 0.3 refused also decode now (see "Changed" and "Fixed"),
  so do not use `decode` alone to validate input.
- Match on the `Error` variant, not on its message. Messages changed
  throughout: they use American spellings ("color", not "colour"), backtick
  marker names, and give the value and the limit.
- Handle `Error::Limit` on files 0.3 decoded. The decoder now bounds the
  work a file may ask for. To decode such a file, raise the bound its
  message names with `DecodeOptions::packet_work_limit`,
  `code_block_work_limit` or `sample_limit`, each a `WorkLimit`
  (`Unlimited` for trusted input).
- If you need a memory ceiling, check `width`, `height` and the component
  count from `read_header` before decoding. The default sample limit is now
  2^28 samples, four times 0.3's, so no image 0.3 decoded is refused for its
  size, but one decode can take more memory. At the limit the peak is an
  estimated 1.4 GiB for a square single-component tile, up to about 3.2 GiB
  for a tile one sample wide, and about 2 GiB while a JP2 palette expands.
  These are ratios measured on images of 2^24 to 2^26 samples, scaled to the
  limit ([the sample limit](docs/resource-bounds.md#the-sample-limit)).
  `WorkLimit` cannot set a lower limit.
- Expect an error on three kinds of malformed file that 0.3 decoded: a
  code-block size past the 4096-sample limit in any `COD` or `COC`, even
  one a later `COC` overrides (`Error::Marker`); a `POC` marker whose
  volumes leave out packets of a tile, which 0.3 decoded with those packets
  empty (`Error::Codestream`); and a packet header that reads past the end of
  its `PPM` or `PPT` bytes, which 0.3 decoded to a wrong image
  (`Error::Codestream`).
- Regenerate output snapshots taken with 0.3 for three kinds of file, whose
  samples change with no error: a code-block with an RGN maxshift and the
  bypass style, a tile with a `POC` in its first tile-part header, and any
  9/7 file, which can move by 1 on a few samples (see "Fixed").

### Added

- `decode` and `decode_with` read JP2 files as well as raw codestreams,
  telling them apart by their first bytes, and decode the first `jp2c`
  codestream. The boxes are checked; files that bend the box rules the way
  OpenJPEG tolerates decode as they do there. A file that is not
  JP2-compatible is `Error::Unsupported`. The rules for what is accepted are
  on `decode` and in [the JP2 box layer notes](docs/openjpeg-departures.md#the-jp2-box-layer).
- A JP2 file with a palette (`pclr` and `cmap`) decodes to the palette's
  channels, and one with channel definitions (`cdef`) puts its color
  channels in color order. `DecodeOptions::raw_channels` skips both and
  returns the codestream's components as coded.
- `Image::jp2: Option<Jp2Metadata>` returns what a JP2 file's header says:
  the color space and any ICC profile (as bytes, not applied), a label for
  each channel, the palette, component mapping and channel definitions, the
  `ihdr`, `colr`, `ftyp`, `rreq` and `res ` boxes, and every top-level
  `xml `, `uuid`, `jp2i` and `uinf` box. It is `None` for a raw codestream.
- `DecodeOptions::color` takes a `ColorOutput`: `AsCoded` (the default) or
  `Srgb`, which converts sYCC and restricted ICC profiles to sRGB. ICC output
  matches OpenJPEG 2.5.0 with LittleCMS 2.14 sample for sample on platforms
  with a correctly rounded `fma`. What
  converts, what passes through and what is refused is listed on
  `ColorOutput::Srgb`.
- `read_header` and `read_header_with` return a codestream's or JP2 file's
  `Header` (image area, each output channel's `ComponentInfo`,
  `max_reduction` and the same `Jp2Metadata`) without decoding any tile
  data. A header fault is the same error `decode_with` returns.
- `DecodeOptions::strict`, `true` by default. With it off, a decode takes
  input that is cut short or has bytes left after a tile's last packet, as
  `opj_decompress -allow-partial` does, and says what it left out on
  `Image::report`, a `DecodeReport` of `ReportEntry` values.
  `DecodeReport::complete_reduction` gives the finest reduction the data
  holds in full, so a codestream can be decoded as it streams in, and
  `DecodeReport::is_complete` says whether anything is missing.
- `WorkLimit`, with `DecodeOptions::packet_work_limit`,
  `code_block_work_limit` and `sample_limit`, raises or lifts each of the
  decode's three limits (see "Changed" and "Security"). The `Error::Limit` message names
  the option and a `WorkLimit::Raised` factor to retry with.
- `Error::FileFormat` for a malformed JP2 file, `Error::Limit` for an input
  past a resource guard, and `Error::InvalidOptions` for options that are
  invalid or do not fit the file.
- `DecodeOptions::with_raw_channels`, `with_color`,
  `with_packet_work_limit`, `with_code_block_work_limit`,
  `with_sample_limit` and `with_strict` set those options, as
  `with_resolution_reduction` does.
- Every public type added in this release is `#[non_exhaustive]`, so match
  it with `_` and `{ .. }`, except `ChannelDefinition`, `ChannelType` and
  `Association`, which cover every `cdef` value and can be built and matched
  exhaustively.
- All nine JP2 files of the ISO/IEC 15444-4 conformance suite decode within
  their compliance class, alongside its 23 codestreams.
- The crate now includes MIT-licensed code ported from LittleCMS 2.14 and
  from Arm's optimized-routines (`pow` and `log`, so ICC output does not
  depend on the platform's `libm`). The notices are in the README.

### Changed

- Whole-file decodes are 1.7 to 5.3 times faster than 0.3.0, depending on
  the file, measured on the conformance and benchmark files with the first
  three changes below. Very small files gain little. The last three were
  each measured against the code before it, not against 0.3.0. None of them changes the output.
  - The inverse wavelet extends only at the ends of each row and column,
    lifts each row as its two halves, and runs the vertical pass eight
    columns at a time on tiles at least 16 samples wide.
  - Code-block decoding uses a padded flag plane with a neighbor mask, so
    contexts come from table lookups, and skips a column with nothing to
    code in one test.
  - Packet headers that name many code-blocks and include none step over
    each group the inclusion tree excludes, instead of each code-block.
    `opj_compress` writes such headers for every layer.
  - 9/7 samples are rounded, level-shifted and clamped in one pass, which
    makes a whole 9/7 decode about 4 to 9% faster (#321).
  - Reading packet headers costs less per code-block. On a file of 4x4
    code-blocks and 100 layers the whole decode takes about 16% less time
    (#322).
  - 9/7 dequantization runs as each code-block is placed instead of in a
    separate pass over each band; the speed change is within measurement
    noise (#324).
- The decoded image may hold up to 2^28 samples (1 GiB as `i32`), up from
  2^26, so a Sentinel-2 10 m band (10980 x 10980) and RGB images up to about
  89 MP decode at the defaults. The samples are counted at the decode's
  resolution reduction, and the image is refused only after the whole main
  header is read, so another header fault is reported first.
- `decode_with` takes `&DecodeOptions`, and `DecodeOptions` is no longer
  `Copy`, so a later option can carry data (see "Migrating from 0.3").
- `Error::Unsupported` no longer covers resource guards or invalid options,
  which now have their own variants (see "Migrating from 0.3").
- A code-block size past the limit in any `COD` or `COC` is refused when
  that header is read, as OpenJPEG does, and a resolution reduction past the
  main header's levels is checked before the tile data.
- Three encoder mistakes that OpenJPEG corrects on purpose now decode as
  they do there; each was an error in 0.3.0: a tile with one more tile-part
  than its `TNsot` declares (Adobe Photoshop), a later tile-part that repeats
  the coding parameters already in force (BAP Image Systems), and the color
  transform flag on an image of one or two components.
- A codestream component too deep for an `i32` sample is refused as
  `Unsupported` when the main header is read, before any tile data.
- The crate uses American spellings: color, gray, grayscale, -ize, behavior,
  center. Box types, field names and spec titles stay as ISO writes them
  (`colr`, `EnumCS`, "Colour Specification box").
- Error messages give the value and the limit it broke, name the tile in
  more cases, and put marker and field names in backticks.
  `Error::Inconsistent` displays as "decoder bug (please report it)".
- The crate docs and README have "Damaged input" and "Limits" sections, and
  `WorkLimit`, `decode_with` and `ColorOutput::Srgb` have runnable examples.

### Fixed

- A last tile-part with `Psot = 0` takes its data up to the last two bytes
  of the codestream without checking that they are `EOC`, as OpenJPEG does.
  A codestream that ends in two other bytes now decodes.
- A code-block with an RGN maxshift and the bypass style switches to raw
  coding at its fifth coded bit-plane, counting the shifted planes, as the
  standard and OpenJPEG's encoder do. Such streams decoded wrong or were
  refused.
- A wrong segmentation symbol read from past the end of its segment, on a
  pass the encoder did not terminate, no longer fails the decode. Rate
  control can cut such a pass short; `opj_compress` output with
  segmentation symbols and `-r` was refused about once in 50 encodes.
- A `POC` in a tile's first tile-part header replaces the main header's
  `POC` for that tile, as the standard requires. It was appended, so a file
  with both read its packets in the wrong order.
- A code-block whose coding passes get no bytes decodes, as the standard
  allows (B.10.7) and OpenJPEG does. It was refused.
- The 9/7 lifting constants β and δ are the nearest `f32` values to the
  standard's (Table F.4), as OpenJPEG's are. Each was one `f32` step off.
- A packet header in a `PPM` or `PPT` marker that reads past the end of its
  bytes is a `Codestream` error. The missing bits were read as 1s, and the
  decode returned a wrong image with no error.
- A single-tile decode no longer holds two image-sized buffers at once. At
  2^26 samples, one 9/7 tile peaked at about 580 MiB and now peaks at about
  340 MiB.

### Security

- Reading the packet headers is bounded at 2^24 steps, or 32 per byte of
  tile data when that is more. A packet header of one byte can step over
  every row of code-blocks in its precinct, and a progression order can
  sweep positions that hold no packet, so a file of a few hundred bytes
  could take minutes. At a resolution reduction, each code-block of the
  dropped resolutions counts too, since every tile still builds them.
- Decoding the code-blocks is bounded at 2^26 sample-passes, or 2^15 per
  byte of tile data when that is more, checked before a tile's code-blocks
  are decoded. Each pass scans its whole code-block however few bytes it
  carries, so a 76 KB file could take 19 s.
- A header that pairs a large tile with tiny code-blocks is refused before
  the code-block geometry is allocated (up to about 160 MiB), as the
  precinct count already was.
- Tile headers are checked in time proportional to their own markers. Each
  was a copy of the main header, so a header of many tiles and components,
  or of many `TLM` entries, could take over a minute.
- A `POC` marker whose volumes reach the same packets again costs a bit per
  packet instead of about 80 bytes. A 10.5 MB file of 160 components took
  824 MB and now takes 14 MB.
- With `strict` off, each missing tile-component and each tile whose data
  ends is charged against the packet work bound, so a short input cannot ask
  for unbounded work.
- [Resource bounds](docs/resource-bounds.md) gives each limit
  and its measurements.

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
