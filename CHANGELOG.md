# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `IncrementalDecoder::header` reads the header of the bytes fed so far
  (`Ok(None)` until the main header is whole), and
  `IncrementalDecoder::bytes` borrows them.
- `IncrementalDecoder::set_resolution_reduction`, `set_max_quality_layers`
  and `set_region` change the options between refines. Each later image is
  the strict-off decode of its bytes under the new options. A change of
  region keeps the packet walks, unless the walks kept are past the room of
  the new region's decode: then walks of tiles the region does not read are
  let go until the rest fit. A change of the reduction keeps the walks
  too, as they do not depend on it. A change of the layer cap walks the
  tiles again, charged to the bound on the walks of tiles not kept. The
  cadence of refines does not reset, and an error the old options gave is
  cleared.
- `IncrementalDecoder::options` returns the decoder's options as the
  caller set them. To get an image right after a change of options,
  without waiting for the next refine, call
  `decode_with(decoder.bytes(), &decoder.options().clone().with_strict(false))`.
  That decode spends nothing from the decoder's bounds.
- `WorkLimit::AtMost(n)` on `DecodeOptions::sample_limit` sets a fixed
  ceiling of `n` samples, whatever the input's length or padding: at most
  `4·n` bytes of output, and sample buffers of no more than about 3.2 times
  that. The input, the tile-part parse (under 1 KiB for each tile the
  input carries, at most about 52 times the input plus 128 KiB) and one
  tile's code-block and precinct records are on top. The records do not
  shrink with `n`: a tile may hold at least 2^20 code-blocks and 2^18
  precincts at any ceiling, estimated at about 0.8 GiB. Below about
  `n` = 2^26 they, not the samples, set the peak. Code-block segment
  records add up to about 65 bytes for each byte of input.
  It holds every count the default limit holds (the tiles a region reads, in
  every component the codestream codes, a palette's expanded channels and
  sRGB output). `read_header_with` refuses the sample count as the decode
  would, and the tile caps where the main header shows them. A refusal under
  it gives no retry, and in an `IncrementalDecoder` it is an error, not a
  wait, except a later tile's cap refusal met while a refine counts the
  code-block work, which ends the count as `BoundSpent`.
  `AtMost(0)`, and `AtMost` on either work bound, is `InvalidOptions`. It
  replaces counting the samples by hand from the header (#475).
- For a refusal by the code-block work bound or by a tile's cap on
  code-blocks or precincts, `LimitDetail::refused_at` gives the tile refused
  and the amount it needs, as a `RefusedAt` with `tile()` and `amount()`, so
  a caller need not parse the message. The amount is in sample-passes for
  the work bound, code-blocks for `TileCodeBlocks` and precincts for
  `TilePrecincts`. The refusal `decode_with` and `read_header_with` give
  carries it, and so do `Waiting::WorkLimit` and a `Waiting::BoundSpent`
  where the whole bound refuses a tile. For other kinds it is `None` today
  and a later release may set it. It is also `None` for a `BoundSpent` whose
  count found no tile the whole bound refuses, where only the work the
  earlier refines spent refused it (#493, #513).

### Changed

- The refusal of a Part 2 marker names `DCO`, `VMS`, `DFS`, `ADS` and `ATK`
  (0xFF70 to 0xFF73 and 0xFF79) as it does the others, where it called them
  unrecognized. The error variant is unchanged (`Unsupported`) (#579).
- `docs/openjpeg-departures.md` records two refusals OpenJPEG does not make:
  an unknown marker in the main header, and a color transform over mixed
  wavelets (#579).
- `IncrementalDecoder::refine` returns `Result<Refined>` in place of
  `Result<Option<Image>>`. To migrate, call `.image()`:
  `decoder.refine()?.image()` is the `Option<Image>` it returned before.
  `Refined::Waiting` says why there is no image: `MoreBytes { until, .. }`,
  `HeaderIncomplete { .. }`, `SampleLimit { limit, .. }`,
  `WorkLimit { limit, .. }`, `BoundSpent { limit, .. }` and
  `MissingTiles { limit, .. }` (each with its `LimitDetail`), or
  `RepeatLimit { .. }`.
  `BoundSpent` is a refine cut short by the code-block work earlier
  refines spent, at a tile whose work fits the whole bound, so its
  `retry_with` is `None`. Such a refine reads the packets of the
  later tiles without decoding their code-blocks, to tell `BoundSpent`
  from `WorkLimit`. An error it meets there ends that count, and the
  refine waits on `BoundSpent`: the code-blocks it skipped can hold the
  error `decode_with` gives first (#485). Where the count passes the whole
  bound, the refine also waits on `BoundSpent`, whose `refused_at` gives
  the tile the whole bound refuses: `decode_with` decodes the tiles counted
  first and can fail in one of them (#487). A `WorkLimit` of the
  code-block bound comes only where the refine decoded every tile before
  the one refused, and carries the refusal a decode of the bytes fed
  gives, hint included. Both enums and their struct variants are
  `#[non_exhaustive]`, so a `match` needs a `_` arm and `..` in each
  variant (#466, #487). `HeaderIncomplete` is a struct variant too, so
  match it as `HeaderIncomplete { .. }` (#493).
- The default sample limit grows with the input: it is the larger of 2^29
  samples and 8 samples per byte of input. Up to 64 MiB of input nothing
  changes. The sample limit no longer refuses a larger file that holds at least
  one byte for every 8 samples: a lossless 23171 x 23171 file of 234 MB, refused
  before, now decodes bit-exact against `opj_decompress` with a peak of 2.84
  GiB. `WorkLimit::Raised(n)` on `DecodeOptions::sample_limit` is n times that
  default, so past 64 MiB each factor allows more than before, and a refusal may
  name a smaller factor. A 100 MB file may now ask for about 3 GiB of output,
  and a decode of it may peak at about 4.1 GiB for a square tile and up to about
  9.6 GiB for a tile one sample wide, input included. Padding counts as input,
  so bound the size of the input you accept, measured in memory after any
  decompression. `IncrementalDecoder::refine` now waits
  (`Waiting::SampleLimit`) when the sample limit refuses, as it does for a work
  bound, since more bytes raise the limit; `finish` reports the refusal (#437).
- HTJ2K decoding is about 1.5x to 2x faster on large files on one thread.
  The output is unchanged.
- The code-blocks and precincts one tile may hold grow with the sample
  limit: max(2^20, the samples it allows / 512) code-blocks and max(2^18,
  those samples / 2048) precincts. At the defaults, up to 64 MiB of input
  the caps stay 2^20 and 2^18; `WorkLimit::Raised` and `WorkLimit::AtMost`
  on `sample_limit` move them at any size. `LimitKind::TileCodeBlocks` and
  `LimitKind::TilePrecincts` are now adjustable: a refusal names a
  `WorkLimit::Raised` factor of `DecodeOptions::sample_limit` as its
  `retry_with`, which `with_retry` sets, and under `WorkLimit::AtMost(n)`
  the caps are set from `n` and a refusal names no retry. An
  `IncrementalDecoder` waits on such a refusal (`Waiting::SampleLimit`)
  rather than failing, unless under `AtMost`. `read_header_with` now counts
  the tiles it would read under the main header and refuses a tile past a
  cap that the decode refuses too, at any input size, where it used to
  return the header; it reads a tile's tile-part header only for a tile
  over a cap there. A 40000 x 40000 one-tile file in 32x32 code-blocks
  padded to 210 MB, refused before
  at every option, now decodes at the defaults, bit-exact against
  `opj_decompress` (17.5 s and 7.65 GiB at full resolution, 0.43 s at
  reduction 3). The bookkeeping of a tile at the caps is at most about a
  third of the output the same limit allows. A tile whose geometry needs a
  sample limit past what the target can address is `LimitKind::AddressSpace`,
  not adjustable, and an `IncrementalDecoder` waits only on a refusal with a
  `retry_with` (#476, #494).
- `read_header_with` follows the tile-parts once, and no further than the
  last tile over a cap under the main header, to find the first tile-part
  of each such tile, where it used to follow them all once for each batch
  of 1, 2, 4 and so on such tiles. On 65535 tiles of 255 tile-parts each
  (235 MB), a window of 1530 tiles over a cap took 1.2 s and now takes
  45 ms at most, 1.8 ms where the first tile-parts come first. It counts
  those tiles in index order, so where its budget runs out no longer
  depends on the order of the tile-parts, and it never refuses a file the
  decode admits (#495).
- `IncrementalDecoder::header` goes on from what its last call found for
  the tiles over a cap, so calls after each feed walk the tile-parts about
  once in all. On the window above, calls after each 256 KiB fed took
  5.9 s in all and now take 66 ms. The header of a tile-part not yet fed
  in full, or of a last tile-part whose `Psot` is 0, is still read again
  on each call. Each call still gives what `read_header_with` gives with
  `strict` off on the bytes fed (#502).
- `IncrementalDecoder::header` keeps no copy of the main header: a
  key of it (two integers), and nothing for a file whose tiles' sizes show them
  within the caps (a few hundred bytes otherwise, plus the per-tile counts).
  On a 16.8 MB main header of `PPM` data it held 100 MB after a call and
  now holds none, and the decoder is 464 bytes, not 896. Each call
  parses the main header again, so it costs the main header plus the
  tile-parts fed since (#556).
- A marker fault gets the same error variant in the main header and in a
  tile-part header, and a file the standard forbids is no longer reported
  as a feature not decoded. Each of these files was refused before and
  still is, with a new variant; one file that decoded before is now
  refused (#563):
  - A Part 2 marker (`MCT`, `MCC`, `NLT`, `MCO`, `CBD`) or an unrecognized
    marker in a tile-part header is `Error::Unsupported`, as in the main
    header. It was `Error::Codestream`. A code read where a damaged length
    led into the packet data is still `Error::Codestream`: a code is taken
    for a marker only when the header's markers go on from it to `SOD`
    within the tile-part.
  - A reserved marker 0xFF30 to 0xFF3F in a tile-part header is
    `Error::Unsupported`, on the same condition. It was
    `Error::Codestream`. The main header still skips one.
  - `PPT`, `SOP` or `EPH` in the main header is `Error::Codestream`, as
    `PLT` there is. It was `Error::Unsupported`.
  - A `PPT` marker in a codestream that has a `PPM` is `Error::Codestream`.
    It was `Error::Unsupported`. A `PPT` that holds no packet-header bytes
    (only its `Zppt`) beside a `PPM` decoded before, and is now refused
    too, as the standard (A.7.4) and OpenJPEG 2.5.0 refuse it.
  - A color transform over components 0 to 2 that do not share a wavelet is
    `Error::Marker`, naming the tile where a tile's markers cause it. It was
    `Error::Unsupported`.
  - Bytes that are not a marker where a tile-part header needs one stay
    `Error::Codestream`, with a message that says so.

### Fixed

- A code-block's codeword segments cost about 15 bytes of heap each, or
  about 110 with bytes, where they cost about 130 with none. A file of
  many pass-terminated (`termall`) segments peaked at up to about 350
  bytes for each byte of input, past the documented bound: a 37 MB
  4096 x 4096 file peaked at 11.2 GiB at the defaults. It now peaks at
  1.48 GiB, and such files stay within about 65 bytes for each byte of
  input. The segment records are also freed before the inverse wavelet.
  Decoded images do not change (#585).
- With `strict` off, a fault in the header of a tile-part that runs to the
  end of the input, a last one with `Psot` 0 or one whose `Psot` runs past
  the end, is refused with the error a strict decode gives when the input
  holds the marker segment the fault is in. The decode returned `Ok`
  without the tile-part, as if the input had been cut inside its header.
  An unrecognized, Part 2 or reserved marker is refused once the input
  holds the header's markers through `SOD`, as the error depends on them.
  A header the input ends inside still decodes, with the loss in the
  report. Files that change: those with such a header holding a malformed
  segment, a marker not allowed there, or a segment length that runs past
  `SOD` within the input. Each was refused with `strict` on, and is now
  refused with `strict` off too, as `opj_decompress -allow-partial`
  refuses it (#574).
- With `strict` off, a last tile-part with `Psot` 0 that the input ends
  inside the header of is reported as `ReportEntry::TruncatedTilePart`,
  with `declared` 0. It was dropped with only `MissingEoc`, and that
  `MissingEoc` is no longer given when the input ends in `FF D9` (#574).
- A tile over both the precinct and the code-block cap is still refused for
  its precincts (`LimitKind::TilePrecincts`), but `retry_with` now names a
  `WorkLimit::Raised` factor that admits its code-blocks too, and the
  message says the code-blocks are over. It named the factor for the
  precincts alone, so a tile whose code-blocks need more took a second
  refusal and retry. Where the code-blocks need a sample limit past what
  the target can address, the tile is refused as `LimitKind::AddressSpace`
  at once. A tile refusal that needs more than `Raised(u32::MAX)` allows
  names `WorkLimit::Unlimited`, where it named `Raised(u32::MAX)`, which
  did not admit the tile. `read_header_with`, `decode_with` and
  `IncrementalDecoder` name the same factor (#575).
- The `LimitKind::PocVolumes` docs no longer name a case no caller sees
  (more packets than the decoder tracks): the kind is for more than 32
  `POC` volumes in a tile (#575).
- With `strict` on, `read_header_with` gives the decode's error for a file
  where a tile over a code-block or precinct cap under the main header has
  no tile-part: the decode's error for the tile-parts, which is the first
  fault its parse finds (a cut or bad `SOT`, or an illegal tile-part
  header, say), or the missing tile. It gave `Error::Limit` for the cap,
  while `decode_with` refuses the file for the tile-parts before it counts
  any tile. To find the error it walks the tile-parts as the decode's
  parse does, keeping up to 10 bytes a tile (14 on an input of 4 GiB or
  more) in place of the decode's tile records, plus the coding of each
  first tile-part header over 1 KiB that a later tile-part restates, up to
  about 13 times those headers' bytes: on a 235 MB file of
  16.7 million tile-parts that refusal takes 240 ms and 640 KiB, where the
  decode's parse takes 1.7 s and 290 MiB (#498, #545).
- With `strict` off, `read_header_with` gives the decode's error for the
  tile-parts where it refuses a tile or stops at a budget and the
  tile-parts hold a fault, as it does with `strict` on. It gave the tile's
  `Error::Limit` or `Error::InvalidOptions`, while `decode_with` refuses
  the file for the fault first. It now runs the same walk of the
  tile-parts as with `strict` on (#545).
- `read_header_with` refuses the tile the decode refuses where a tile-part
  `COD` or `COC` takes a lower tile over a code-block or precinct cap. It
  refused the first tile over a cap under the main header instead, so its
  `Error::Limit` could name a higher tile, another kind and a smaller
  `retry_with` than `decode_with`'s, and a retry sized from it was refused
  again. Before it refuses a tile, it now counts each lower tile it would
  read that the tile's own header could refuse, under that header, in index
  order, as the decode does: each whose first tile-part carries a `COD` or
  `COC` and whose size does not rule out a tile-part header taking it over
  the caps, and under a resolution reduction each with a `COD` or `COC`
  whatever its size, as one with fewer levels than the reduction takes is
  refused too. With `strict` on it first looks up the tiles it would read
  up to the tile over a cap, and gives the decode's error for the
  tile-parts where one of them has none. As the strict decode parses every
  tile-part before it counts any tile, whatever the selection, a refusal or
  a stop at the count's budget now looks for the error of that parse too,
  with `strict` on or off, and gives its fault where it finds one.
  That reads the tile-part headers of those tiles in the same single walk;
  a file with no tile over a cap under the main header is still read no
  further than the first `SOT`. The decode can still refuse
  first for what only decoding finds in a lower tile (its packets or
  code-blocks), before a tile over a cap or one its own header gives too
  few levels for, or, with `strict`
  off, for the charge for the tiles read that have no tile-part; the
  `read_header_with` docs list these cases (#501).
- `DecodeOptions::with_retry` no longer climbs a work bound one factor at a
  time. A refusal of the packet work or code-block work bound, including the
  charge for missing tiles, names the larger of what the tiles seen so far
  need and twice the factor it ran under (`Raised(2)`, `Raised(4)`,
  `Raised(8)`, ...), so a loop of retries takes about log2 of the factor the
  file needs, not that many steps, and no longer a step for each tile of a
  multi-tile file. The message no longer says "at least": the factor can be
  more than the file needs. Measured on a release build, a loop from the
  defaults, one thread:
  `cprl_tall_poc31` (needs 32) took 31 steps and 70 s and now takes 5 steps
  and 8.5 s; `pocvol_1024x512_1000_2` (strict off) was still refused after
  46 steps and 120 s and now takes 5 steps and 8.0 s;
  `pocspent_2tiles_60000_559` (strict off) took 25 steps and 9.8 s and now
  takes 5 and 1.9 s; a 982-byte file of 64 tiles that each reach the bound
  (strict off) took 16 steps and 35.8 s and now takes 5 and 11 s. No default
  changes, and the hint only advises what to raise. A bound already under
  `Raised(u32::MAX)` names `Unlimited`.
- `IncrementalDecoder` holds the packet walks it keeps between calls to the
  larger of 1 MiB and 32 bytes per byte fed, as well as to twice the image
  and the largest walk. A file of 1228 bytes that declares a 16384 x 16384
  image of 4 x 4 code-blocks held 640 MiB between calls and peaked at
  1716 MiB in a refine; it now holds no walk, and a refine peaks at the
  1076 MiB one decode takes. A tile whose walk does not fit is walked from
  the start by each refine until more bytes make room. Every image, report
  and refusal is unchanged (#415).
- `IncrementalDecoder` counts the packet visits of the walks that read a
  tile from the start again: of tiles walked anew on each refine, and of
  walks started again by a later tile-part's `POC` volumes or packed
  packet headers, or built again once there is room. Once they pass four
  times the packet bound of one decode of the bytes fed, a refine that would
  make such a walk waits (`Waiting::RepeatLimit`), and `finish` decodes the
  file. Two files of 525 KB made for it, fed in small pieces with a refine
  after each, took 8.4 and 7.3 times one decode; they now take 4.6 and 3.5
  times. No
  committed fixture gives another result (#416).
- The tile-part parse keeps a record only for each tile that has a
  tile-part, and 2 bytes for each tile the SIZ grid declares. A file that
  declares 65535 tiles and carries one tile-part allocated 53 MB, and as much
  before a strict decode refused it; it now peaks at 132 KB either way,
  however long the tile-part. A file of 65535 tiles, each in one tile-part,
  peaks at 47 MB, down from 57 MB (#484).
- After a change of the layer cap, `IncrementalDecoder` sizes the room for the
  walks it keeps from the walks built under the new cap, as a decoder made
  with it does. It used the largest walk of the old cap, so the room, and
  which walks fit, could differ from a fresh decoder's (#519).
- A JP2 file's header is checked closer to OpenJPEG's order (see
  `docs/openjpeg-departures.md`), and a codestream and its JP2 wrapper give
  the same error for the same options, except under sRGB output and where
  the sample limit and the tile caps, which count a JP2 file's own bytes,
  differ past 64 MiB (#419):
  - The layout of the `pclr`, `cmap` and `cdef` boxes (a second box, a
    `cmap` without a `pclr` or before it, a malformed payload) is checked
    before the codestream, as OpenJPEG checks it while it reads `jp2h`. A
    file with such a fault and a codestream fault, an `ihdr` that disagrees
    with SIZ or a refused option gave the other error first. Between these
    boxes, the first fault in file order wins. An `IncrementalDecoder` now
    refuses such a file once the `jp2c` box header is in, even when the main
    header is still cut.
  - The resolution reduction, then the selection, are checked right after
    `ihdr` against SIZ, as a raw codestream checks them, and before what the
    palette and `cdef` say about the components, the header boxes after the
    codestream and the sRGB step. A file with a refused reduction or
    selection and a fault in those steps (a palette over the sample limit, a
    color space the sRGB step does not convert, a `cdef` or `cmap` that names
    a missing channel or column) gave that fault, while its codestream gave
    `InvalidOptions`. OpenJPEG refuses the reduction as it reads `COD`, and
    the selection when the decoded area is set, both before it checks the
    channels, which it does after decoding.
  - An `IncrementalDecoder` reading a paletted file under the default sample
    limit, with a reduction the file refuses, now refuses at once with
    `InvalidOptions`. `refine` used to wait on the palette's sample limit
    refusal, which more bytes would raise.
  - sRGB output is the one difference left: a raw codestream declares no
    color space, so it refuses sRGB output of three or more components
    before the reduction and the selection, while a JP2 file plans the sRGB
    step last, from its color space and its channels. OpenJPEG has no sRGB
    refusal to follow.

## [0.5.0] - 2026-11-17

### Migrating from 0.4

- The default sample limit is 2^29 samples (2 GiB as `i32`), up from 2^28, so
  a 10980 x 10980 Sentinel-2 true-color image (361.7 M samples) decodes at the
  defaults. `WorkLimit::Raised(n)` on `DecodeOptions::sample_limit` now
  means n x 2^29 samples, not n x 2^28: a `Raised(n)` you set to admit one
  file admits twice as much, and needs half the factor for the same file. A
  decode may now allocate up to about 2.7 GiB for a square tile, and up to
  about 6.4 GiB for a tile one sample wide, before it is refused.
- `ChannelType` and `Association` have a new variant each,
  `ChannelType::ApplicationDefined` and `Association::ApplicationDefined(u16)`.
  Both enums are now `#[non_exhaustive]`, so a `match` on either needs a
  wildcard arm, and a later variant will not break it. They are used only
  for JPH files; a JP2 file's `cdef` channel of type 3 is still
  `ChannelType::Other(3)` with an `Association` read as a color.
- `Error`'s variants no longer hold a `String`. `Error::Limit` holds a
  `LimitDetail`, and every other variant an `ErrorDetail`. Both give the
  message with `message()` and `Display`, and `Error`'s `Display` is
  unchanged. A `LimitDetail` also says which limit refused the input,
  `kind()`, a `LimitKind`, and what to retry with, `retry_with()`: the
  `WorkLimit` the message names, or `None` for a guard that no option
  raises (`LimitKind::is_adjustable` is false for those).
  `DecodeOptions::with_retry` sets the option a `LimitDetail` names to its
  `retry_with`. `Error::message()` gives any error's text without its
  prefix. A match on `Error::Codestream(_)` and the like needs no change.
  Code that bound the `String` reads `message()` instead:

  ```rust
  // 0.4
  Err(Error::Limit(why)) => {
      // The factor was only in the text.
      eprintln!("refused: {why}");
  }
  // 0.5
  Err(Error::Limit(limit)) => {
      eprintln!("refused: {}", limit.message());
      if let Some(raised) = options.with_retry(&limit) {
          options = raised;
      }
  }
  ```
- `Error` no longer implements `PartialEq` or `Eq`. Equality compared the
  message, whose wording can change between releases. Test the variant with
  `matches!`, and a limit by its kind:

  ```rust
  // 0.4
  assert_eq!(result, Err(Error::Marker("reserved progression order 9".into())));
  // 0.5
  assert!(matches!(result, Err(Error::Marker(_))));
  assert!(matches!(&result, Err(Error::Limit(l)) if l.kind() == LimitKind::Samples));
  ```

  `Error` is still `Clone`, `Send`, `Sync` and unwind-safe.
- For the same reason, a type of your own that derives `PartialEq` or `Eq`
  and holds a `rust_j2k::Error` no longer compiles. Drop the derive, or
  compare that field by its variant or by `message()` in a hand-written
  `PartialEq`.
- A codestream with HT code-blocks (`COD` code-block style bit `0x40`) used
  to give `Error::Unsupported`. It now decodes, or fails as another variant
  if it is damaged. Code that matched `Unsupported` to detect or refuse
  HTJ2K must change.

### Added

- `IncrementalDecoder` decodes a file while it arrives. `feed` appends bytes,
  `refine` returns an image of what has come in, and `finish` decodes all of
  it once the input ends. Each image `refine` returns is the one `decode_with`
  gives with `strict` off on the same bytes, with a report of what they do not
  hold yet. `refine` decodes at least each time the bytes fed have doubled
  since it last decoded or tried to, and may decode more often. Each refine
  goes on from where the last one stopped reading packets;
  `docs/incremental-decoding.md` lists the packets it reads again. Each refine
  is held to the packet bound of a decode of its bytes, and the code-block
  bound holds across all the refines together; each refine that returns an
  image also rebuilds the whole image, as one decode does. A work limit is
  reported only by `finish`, since more bytes raise the work bounds; a limit
  that does not depend on the bytes, such as the sample limit, is an error
  from `refine`, and every later `refine` returns it again. The decoder is
  `Send` and `Sync`. A main header still cut short at `finish` is
  `Error::Codestream`, or `Error::FileFormat` inside a JP2 file's boxes.
  `finish` borrows the decoder and can be called again with the same result,
  and `into_bytes` returns the bytes fed, so a file that `finish` refuses for
  a work limit can be decoded again with the limit raised.
- A tile-component whose code-block style is MIXED (ISO/IEC 15444-15 A.4)
  decodes: each code-block is read as an HT block or a Part 1 block, told
  apart by its first nonzero length field. The three MIXED files of the
  ISO/IEC 15444-4 HT set decode within their bounds. A MIXED style with
  bypass or termination on each pass is `Error::Marker`, and a MIXED style
  under a `CAP` that says each tile-component holds one kind of code-block is
  `Error::Codestream`.
- JPH files (ISO/IEC 15444-15 Annex D) decode, and `read_header` reads them:
  a file whose File Type box has `jph ` as its brand or in its compatibility
  list is read as a JP2 file is. In a JPH file, a `cdef` channel of type 3
  is reported as `ChannelType::ApplicationDefined`, with its `Asoc` as
  `Association::ApplicationDefined`. Its `Asoc` is not checked against the
  channel count, and it moves no channel. OpenJPEG 2.5.0 refuses a file
  whose type 3 `Asoc` is past the last channel. In a JP2 file, type 3 is
  reserved and read as before.
- `Header::quality_layers`: the number of quality layers the main header's
  `COD` declares, as `read_header` reads it. A
  `DecodeOptions::max_quality_layers` cap at or past it decodes every layer of
  a tile that keeps that `COD`. A tile-part `COD` can declare another count
  for its tile. `Header` is `#[non_exhaustive]`, so the new field is not a
  breaking change.
- `Header::image_area`, a `Window`: the image area on the reference grid,
  `XOsiz..Xsiz` by `YOsiz..Ysiz`, so a window can be placed relative to the
  image.
- `Header::tiles`, a `TileGrid`: the tile grid the `SIZ` marker declares
  (ISO/IEC 15444-1 Eq. B-5), with its columns and rows, the tile size and
  the grid's offset on the reference grid. `TileGrid::count` is how many
  tiles `Region::Tile` can name, and `TileGrid::tile` gives a tile's
  rectangle, cut to the image area: the area `Region::Tile` decodes. On
  ordinary files the values match what `opj_dump` prints.
- HTJ2K codestreams decode: the cleanup, SigProp and MagRef passes of each
  code-block, in any number of quality layers, with or without placeholder
  passes before them. The output matches OpenJPEG 2.5.0 on every HT file it
  decodes, OpenJPH's test codestreams included: bit for bit on the reversible
  files, and within 1 on the irreversible ones. OpenJPEG 2.5.0 refuses the
  other 31 HT codestreams (placeholder passes, HT region of interest, MIXED
  code-blocks), so those are graded against the ISO/IEC 15444-4 reference
  images alone. Region of interest decodes on HT code-blocks as on Part 1
  ones. An `RGN` marker in a codestream whose `CAP` says it has none, and an
  HT magnitude past the bound `CAP` signals, are `Error::Codestream`. More
  than one HT set per code-block is `Error::Unsupported`.
- `Error::Incomplete`: the input ends before the codestream's main header is
  whole, that is, before its first `SOT` marker is complete. It begins at the
  empty input and, in a JP2 file, covers the signature box, the top-level
  boxes before `jp2c` and the `jp2c` box header. A caller whose input streams
  in can wait for more bytes instead of treating the file as damaged.
  `read_header` and `decode_with` return it whether or not `strict` is set.
  These inputs were `Error::Codestream` or `Error::FileFormat` before. The
  boxes and segments the input holds in full are checked first, so a damaged
  one gives an error, not `Incomplete`, usually the one the whole file gives.
  The same check changes one `Error::FileFormat` message. A JP2 file whose
  first box after the signature is cut short and is not `ftyp` now gives
  `the ftyp box must follow the signature, not ...`, where it gave `box ... is
  N bytes but only M remain`. The variant is the same. Inputs in the fuzz
  corpus changed this way.
- `DecodeOptions::max_quality_layers` and `with_max_quality_layers`: decode
  only the first `n` quality layers of each tile, as `opj_decompress -l n`
  does. The image keeps its size and loses fidelity, and the code-block
  decoder does less work: at one layer of eight, a 2048 x 2048 RGB file
  spends 16% of the full decode's block work. The later layers' packets are
  still read, so damage in them is still an error. `None`, the default,
  decodes every layer; `Some(0)` is `Error::InvalidOptions`.
- `DecodeOptions::region` (`with_region`, and the `Region` and `Window`
  types) and `DecodeOptions::components` (`with_components`): decode a
  rectangle of the reference grid (`Region::Window`), one tile
  (`Region::Tile`), or some of the components, as `opj_decompress -d`, `-t`
  and `-c` do. One `Region` holds the window or the tile, so both cannot be
  asked for. Only the
  tiles the rectangle reaches are decoded, and in each of them only the
  code-blocks and wavelet samples it needs: a 256 x 256 window of a
  2048 x 2048 image of 256 x 256 tiles decodes in about 2.4 ms against 54 ms
  for the whole. The components left out are not decoded past the packet headers.
  Under the color transform, asking for any of components 0, 1 and 2 decodes
  all three and returns those components of the whole decode; OpenJPEG skips
  the transform there instead. An empty window, one outside the image, and a tile
  or component past the image's are `Error::InvalidOptions`. `read_header_with` describes the part the options
  ask for. Under a window or a tile, the sample limit counts the samples of
  the tiles read, in every component, so a small window of a large tiled image
  decodes at the default limit. A palette's expanded channels count only the
  window. A component subset alone counts the whole image. With `strict` off,
  the report describes what the decode returns: the tiles it reads, the
  components it reconstructs, and the code-blocks the window's samples depend
  on. Data missing elsewhere does not make it incomplete. `ColorOutput::Srgb`
  from sYCC with sub-sampled chroma is `Error::Unsupported` when the area is
  not the whole image, as the chroma replication depends on where the area
  starts.

### Changed

- The default sample limit is 2^29 samples (2 GiB as `i32`), up from 2^28,
  so a Sentinel-2 true-color image of 10980 x 10980 x 3 (361.7 M samples)
  decodes at the defaults, as does an RGB image of up to about 179 MP. The
  limit is still counted in decoded samples, so `Raised(n)` now allows
  n x 2^29 samples (#436). A tile may now hold 2^20 code-blocks, up from 2^19,
  so a one-tile image under 2^29 samples and at least 72 samples wide decodes
  in 32 x 32 blocks; a tile
  of more than 2^20 is still `Error::Limit` with `LimitKind::TileCodeBlocks`.
- Some complete inputs now give `Error::Incomplete`, since their bytes look
  the same as a cut: empty input (was `Error::Codestream`), and a JP2 file
  with no `jp2c` box or with a top-level box before `jp2c` that runs past the
  end of the file (was `Error::FileFormat`). A caller that matched those
  variants should treat `Incomplete` with no more bytes coming as truncated.
- The `CAP` and `CPF` markers are read. A `CAP` that
  signals a part other than Part 15 is `Unsupported`. A malformed `CAP` or
  `CPF` is `Error::Marker`; a repeated one, or either one in a tile-part
  header, is `Error::Codestream`.
- A tile with many components and wavelet levels but few samples decodes
  up to 1.7 times faster (64 tiles of 16384 components, now 1.2 times
  OpenJPEG's time, which includes writing its output): a resolution with no
  precinct costs no allocation and no tier-1 or inverse DWT work (#272).
- Code-block decoding reuses one set of buffers per component, and no
  longer copies a code-block's bytes when one quality layer carried them. A
  128x128 image of 4x4 code-blocks makes 2246 allocations where it made
  6338, and decodes about 5% faster (#299).
- An HT code-block of a subband with more than 30 bit-planes decodes when
  its zero bit-planes keep the cleanup plane's shift under 30, as Part 1
  already did, including under a maxshift. It was refused as `Error::Limit`. OpenJPEG refuses any such
  HT block.

### Fixed

- With `strict` off, a `POC` sequence's packets past the end of the data
  no longer take memory that grows with the packet work bound. The packets
  emitted past the bitmap are now held as a count per precinct, at most
  1 MB, not as a set of every packet. A 200-byte file at
  `WorkLimit::Raised(8)` took 3.6 GB before its `Error::Limit` and now
  takes 33 MB; a 10 MB file at the default bound took 7.1 GB and now
  takes 16 MB. Strict decodes of such sequences use less memory too. Every
  decoded image, report and refusal is unchanged (#438).
- A region-of-interest maxshift now treats a coefficient as region only from
  `2^s`, as ISO/IEC 15444-1 H.1 says. It was one plane lower, so a background
  coefficient with bit `s - 1` set decoded as 0 or a fraction of its value.
  This applies to both Part 1 and HT code-blocks. OpenJPEG makes the same
  mistake, so on such a stream the output now differs from
  `opj_decompress` (see `docs/openjpeg-departures.md`). No conformance ROI
  codestream is affected. A maxshift of 31 or more used to decode the block
  to zeros; it now keeps every coefficient as background, as H.1 says
  (#396).
- With `strict` off, a codestream under `predictable termination` that is
  cut short, or whose `POC` volumes leave packets out, decodes to a partial
  image. It was `Error::Codestream` when a missing packet of a later layer
  would have added passes to a code-block's last segment, with or without
  `max_quality_layers`. That segment is no longer held to the termination
  check; the block's earlier segments still are.
- A window, tile or `components` selection that does not fit the image is
  `Error::InvalidOptions` on an image over the sample limit. It was
  `Error::Limit`, with a hint to raise the limit that could not help.

## [0.4.0] - 2026-11-14

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
  limit ([the sample limit](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.4.0/docs/resource-bounds.md#the-sample-limit)).
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
  on `decode` and in [the JP2 box layer notes](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.4.0/docs/openjpeg-departures.md#the-jp2-box-layer).
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
- [Resource bounds](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.4.0/docs/resource-bounds.md) gives each limit
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

[Unreleased]: https://github.com/D0ubleD0uble/rust-j2k/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/D0ubleD0uble/rust-j2k/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/D0ubleD0uble/rust-j2k/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/D0ubleD0uble/rust-j2k/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/D0ubleD0uble/rust-j2k/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/D0ubleD0uble/rust-j2k/releases/tag/v0.1.0
