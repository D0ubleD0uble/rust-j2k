# Stability

This page says what a caller can rely on from one release to the next, and
what can change.

The crate follows [Semantic Versioning](https://semver.org/). While the
version is `0.x`, a change that breaks a caller needs a new minor version
(`0.6` to `0.7`), and a patch release (`0.6.0` to `0.6.1`) does not break
one. The CHANGELOG lists each breaking change under "Migrating from".

## What is covered

A change to any of these that breaks a caller needs a new minor version.

### The functions and types

- The functions [`decode`], [`decode_with`], [`read_header`] and
  [`read_header_with`].
- The errors: [`Error`] and its variants, [`Result`], [`ErrorDetail`],
  [`LimitDetail`], [`LimitKind`] and its kinds, [`Raise`], [`RefusedAt`] and
  [`AmountUnit`].
- The options: [`DecodeOptions`] with its getters, its `with_*` builders and
  `raised_for`, and [`ColorOutput`], [`Region`], [`Window`],
  [`SampleLimit`] and [`WorkLimit`].
- The output: [`Image`], [`Component`], [`Header`], [`ComponentInfo`],
  [`TileGrid`], [`DecodeReport`] and [`ReportEntry`].
- The JP2 metadata types: `Jp2Metadata`, `ImageHeader`,
  `ColorSpecification`, `ColorSpace`, `FileType`, `ReaderRequirements`,
  `StandardFeature`, `VendorFeature`, `Resolution`, `Res`, `Ratio`,
  `ChannelDefinition`, `ChannelDefinitionEntry`, `ChannelType`,
  `Association`, `Palette`, `PaletteColumn`, `ChannelMapping`,
  `MappingKind`, `XmlBox`, `IprBox`, `UuidBox` and `UuidInfo`.
- The incremental decoder: [`IncrementalDecoder`] (`new`, `feed`, `refine`,
  `header`, `finish`, `set_resolution_reduction`, `set_max_quality_layers`,
  `set_region`, `options`, `bytes`, `bytes_fed`, `bytes_decoded` and
  `into_bytes`), [`Refined`] with `image`, and [`Waiting`] and its variants.
- The accessors on these types, such as `LimitDetail::kind`,
  `LimitDetail::retry_with`, `LimitDetail::refused_at`,
  `LimitKind::is_adjustable`, and `RefusedAt::tile`, `amount` and `unit`.
- The traits each type implements, the derived ones included. Every public
  type is `Send`, `Sync`, `UnwindSafe` and `RefUnwindSafe`
  (`tests/auto_traits.rs` checks this).

A type, variant or field marked `#[non_exhaustive]` can gain variants or
fields without a breaking change, so match on it with a `_` arm or `..`.

### What the types mean

- **Which variant a fault gives.** The variant a given fault maps to is part
  of the API: moving a fault to another variant, such as from
  `Error::Codestream` to `Error::Marker`, is a breaking change. Which fault
  is reported when an input has more than one is not covered (see below).
- **Which limit refused.** The `LimitKind` a refusal names, which kinds an
  option raises (`LimitKind::is_adjustable`), and which option that is.
- **`retry_with`.** What `LimitDetail::retry_with` means, as its rustdoc
  and [`Raise`] describe it: the limit to retry with, a hint rather than a
  measure of the file, and `None` when no option raises the limit or the
  refusal shows no factor. `DecodeOptions::raised_for` applies it.

### `DecodeOptions` equality

`DecodeOptions` implements `PartialEq` and `Eq`, and that is covered:
removing `Eq` would break a caller. So an option held as a float, such as a
gamma or a color tolerance, can only be added as a type that implements
`Eq`.

Equality is structural: it compares the settings as stored, not the decodes
they give. `Region::Whole` and a `Region::Window` that covers the whole
image compare unequal, and so do `components` of `None` and a list of every
component. The builders store a limit of `Raised(1)` as `Default`, so two
options built with those compare equal.

### `RefusedAt`

`RefusedAt` implements `Eq` and `Hash` over its tile, amount and unit. It
does not hold the limit's kind, so two refusals of different kinds with the
same tile, amount and unit compare equal: a `TileCodeBlocks` refusal and an
`AddressSpace` refusal of the same tile's code-blocks, for example. Compare
`LimitDetail::kind` too when the kind matters. `RefusedAt` is
`#[non_exhaustive]` with private fields, so a `kind` accessor can be added
later without a break.

`LimitDetail::refused_at` is set today for `CodeBlockWork`,
`TileCodeBlocks` and `TilePrecincts` refusals, and for an `AddressSpace`
refusal of a tile. A later release can set it for more refusals, so handle
`None` for any kind.

### `Waiting::Held`

One `Waiting::Held` covers three ways a held error resolves:

- In a codestream, the first refine that decodes once the `EOC` is in gives
  the final answer.
- In a JP2 file, the same holds once no box after the codestream can still
  be read. Until then, only `finish` resolves it.
- A refusal of the sRGB step is final or held, depending on whether a box
  after the codestream can still be read.

The value does not say which. It is covered without a field that says why,
on purpose: `Held` is `#[non_exhaustive]`, so a `reason` field can be added
later without a break.

### When an error from the incremental decoder is final

Once `refine` returns an error, every later `refine` returns it again until
a change of options. An error from `header` is final too: more bytes do not
change it.

There is one exception. `header` can return a size-limit refusal that more
bytes would lift: an `Error::Limit` of kind `Samples`, `TileCodeBlocks` or
`TilePrecincts` whose `retry_with` is `Raised` or `Unlimited`.

- A `Samples` refusal comes as soon as the main header is in, so a server
  can refuse a huge image early.
- A tile-cap refusal comes once the `EOC` closes the tile-parts, and in a
  JP2 file once no box after the codestream can still be read.
- `refine` never returns these: it waits with `Waiting::SampleLimit`
  instead, or with `Waiting::BoundSpent` for a later tile's cap met while it
  counts the code-block work.

Every other error from `header` and `refine` is final.

### What `header` returning `Ok(None)` means

`IncrementalDecoder::header` returns `Result<Option<Header>>`, and
`Ok(None)` has two meanings:

- The main header is not complete yet.
- In a JP2 file, a fault in a box before the codestream is held, because
  more bytes could change it, as with a `jp2h` or `ftyp` box of length 0.

`refine` tells the two apart: it waits with `Waiting::HeaderIncomplete` for
the first and with `Waiting::Held { error }` for the second. `finish`
reports the held fault once the input ends.

## What is not covered

These can change in any release:

- **Text.** The wording of every message: `Error::message`,
  `ErrorDetail`, `LimitDetail::message`, the `Display` text of `Error`
  (with the prefix it adds for the variant), `LimitDetail` and
  `ReportEntry`, and all `Debug` output. These are diagnostics for a log,
  not a stable format: do not parse them or compare them. Match on the
  variant, and read a limit's data from `kind`, `retry_with` and
  `refused_at`.
- **Which fault wins.** When an input has more than one fault, which one is
  reported. The rustdoc of `Error` lists what holds today.
- **The minimum Rust version.** It is 1.87 (`rust-version` in
  `Cargo.toml`). No policy is set for raising it.
- **The hidden `fuzz` module** and the `__fuzz` feature it needs. They exist
  for the crate's own fuzz targets and tests, and are not part of the API.

The default values of the limits are not covered either: they can change in
a minor release. Only the types, the variants and what they mean are
stable. The values today:

- The sample limit: 2^29 samples, or 8 samples per byte of input when that
  is more.
- The caps on a tile: one code-block for each 512 samples the sample limit
  allows and one precinct for each 2048, and at least 2^20 code-blocks and
  2^18 precincts.
- The packet work bound: 2^24 steps, or 32 per byte of tile data when that
  is more.
- The code-block work bound: 2^26 sample-passes, or 2^15 per byte of tile
  data when that is more.
- The guards no option raises: at most 32 `POC` volumes in a tile, and at
  most 30 bit-planes in a code-block.
- For an `IncrementalDecoder`: the walks it keeps hold at most the larger of
  1 MiB and 32 bytes per byte fed, and the walks it does again spend at most
  about four times the packet bound of one decode before a refine waits with
  `Waiting::RepeatLimit`.

The memory and time figures in the rustdoc, [SECURITY.md](../SECURITY.md)
and [resource-bounds.md](resource-bounds.md) follow from these values, so
they can change with them. Among them: the tile-part records take under 1
KiB for each tile, at most about 52 times the input plus 128 KiB.

## 32-bit targets

On a 32-bit target a sample buffer can hold at most 2^29 - 1 samples, one
below the 2^29 floor of the default sample limit. So:

- The default sample limit is that address-space cap, at any input size.
  `SampleLimit::Raised` and `SampleLimit::Unlimited` cannot lift it.
- An image over it is refused as `LimitKind::AddressSpace`, with no
  `retry_with`. A lower `SampleLimit::AtMost` ceiling still refuses as
  `Samples`, also with no `retry_with`.
- A tile past a code-block or precinct cap is refused as `AddressSpace` too,
  with no `retry_with`. Its `refused_at` gives the tile and the count.
- No refusal is one that more bytes lift, so an `IncrementalDecoder` never
  waits with `Waiting::SampleLimit`, and the `header` exception above does
  not arise.

[`decode`]: https://docs.rs/rust-j2k/latest/rust_j2k/fn.decode.html
[`decode_with`]: https://docs.rs/rust-j2k/latest/rust_j2k/fn.decode_with.html
[`read_header`]: https://docs.rs/rust-j2k/latest/rust_j2k/fn.read_header.html
[`read_header_with`]: https://docs.rs/rust-j2k/latest/rust_j2k/fn.read_header_with.html
[`Error`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.Error.html
[`Result`]: https://docs.rs/rust-j2k/latest/rust_j2k/type.Result.html
[`ErrorDetail`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.ErrorDetail.html
[`LimitDetail`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.LimitDetail.html
[`LimitKind`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.LimitKind.html
[`Raise`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.Raise.html
[`RefusedAt`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.RefusedAt.html
[`AmountUnit`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.AmountUnit.html
[`DecodeOptions`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.DecodeOptions.html
[`ColorOutput`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.ColorOutput.html
[`Region`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.Region.html
[`Window`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.Window.html
[`SampleLimit`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.SampleLimit.html
[`WorkLimit`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.WorkLimit.html
[`Image`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.Image.html
[`Component`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.Component.html
[`Header`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.Header.html
[`ComponentInfo`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.ComponentInfo.html
[`TileGrid`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.TileGrid.html
[`DecodeReport`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.DecodeReport.html
[`ReportEntry`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.ReportEntry.html
[`IncrementalDecoder`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.IncrementalDecoder.html
[`Refined`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.Refined.html
[`Waiting`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.Waiting.html
