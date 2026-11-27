# Stability

This page says what a caller can rely on from one release to the next, and
what can change.

The crate follows [Semantic Versioning](https://semver.org/). While the
version is `0.x`, a change that breaks a caller needs a new minor version,
and a patch release does not break one. The CHANGELOG lists each breaking
change in a "Migrating from" section.

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
- The JP2 metadata types: [`Jp2Metadata`], [`ImageHeader`],
  [`ColorSpecification`], [`ColorSpace`], [`FileType`],
  [`ReaderRequirements`], [`StandardFeature`], [`VendorFeature`],
  [`Resolution`], [`Res`], [`Ratio`], [`ChannelDefinition`],
  [`ChannelDefinitionEntry`], [`ChannelType`], [`Association`],
  [`Palette`], [`PaletteColumn`], [`ChannelMapping`], [`MappingKind`],
  [`XmlBox`], [`IprBox`], [`UuidBox`] and [`UuidInfo`].
- The incremental decoder: [`IncrementalDecoder`] (`new`, `feed`, `refine`,
  `header`, `finish`, `set_resolution_reduction`, `set_max_quality_layers`,
  `set_region`, `options`, `bytes`, `bytes_fed`, `bytes_decoded` and
  `into_bytes`), [`Refined`] with `image`, and [`Waiting`] and its variants.
- Their public fields, constructors and methods. The fields are on
  `Header`, `TileGrid`, `ComponentInfo`, `Image`, `Component`,
  `DecodeReport` and the JP2 types. The constructors and methods include
  `Image::new` and `component`, `Component::new` and `sample`,
  `TileGrid::count` and `tile`, `Window::new`, `DecodeReport::is_complete`,
  `LimitDetail::kind`, `retry_with` and `refused_at`,
  `LimitKind::is_adjustable`, and `RefusedAt::tile`, `amount` and `unit`.
- The settings of `DecodeOptions::default()` other than the limits: full
  resolution, every quality layer, the whole image, the components as the
  file's boxes shape them, color as coded, and `strict` on.
- The traits each type implements, the derived ones included. Every public
  type is `Send`, `Sync`, `UnwindSafe` and `RefUnwindSafe`
  (`tests/auto_traits.rs` checks this).

A type, variant or field marked `#[non_exhaustive]` can gain variants or
fields without a breaking change, so match on it with a `_` arm or `..`.

### Cargo features

The decoder needs no Cargo feature. It is in every build, including one
with `default-features = false`. A feature added later only adds API:
turning a feature on never removes or changes the decoder's API. The
hidden `__fuzz` feature is not covered (see "What is not covered").

### What the types mean

- **Which variant a fault gives.** Each kind of fault maps to one variant,
  and moving it to another, such as from `Error::Codestream` to
  `Error::Marker`, is a breaking change. The promise is about the fault, not
  the file: when a file has more than one fault, or a fix finds a fault that
  was missed before, the error that file gives can change, variant included
  (see "Which fault wins" below).
- **Which `Waiting` or `ReportEntry` variant a case gives.** The same
  holds for these. Each kind of case maps to one `Waiting` variant and to one
  `ReportEntry` variant, and moving it to another is a breaking change. When
  a refine has more than one reason to wait, or a gap in the data has more
  than one cause, which one is reported can still change.
- **Which limit refused.** The `LimitKind` a refusal names, which kinds an
  option raises (`LimitKind::is_adjustable`), and which option that is.
- **`retry_with`.** What `LimitDetail::retry_with` means, as its rustdoc
  and [`Raise`] describe it: the limit to retry with, a hint rather than a
  measure of the file, and `None` when no option raises the limit or the
  refusal shows no factor. `DecodeOptions::raised_for` applies it.

### The security promise

[SECURITY.md](../SECURITY.md#what-counts-as-a-vulnerability) promises that,
at the default limits, the public functions and the `IncrementalDecoder`
methods return a result or a typed `Error` for any input: they never panic,
read out of bounds or hang, and allocate at most what the limits allow.
This promise is part of the covered API. An input that breaks it is a bug,
fixed as a security fix, and dropping or narrowing the promise needs a
new minor version.

### Decoded output

The samples a file decodes to, and whether it decodes or is refused, are
covered, except for fixes and for the changes to the default limits
described below. The crate decides each case by three rules, in
order: follow the standard; otherwise give the error correction a user would
expect, with OpenJPEG as the usual guide; and return an error rather than a
wrong image ([correctness.md](correctness.md#when-the-oracle-and-the-standard-disagree)).

- A fix that brings the output, or the choice to decode or refuse, closer to
  those rules can ship in a patch release. Examples: output corrected to
  match the standard, or OpenJPEG where the standard does not decide; a file
  that gave a wrong image now refused; a file that was refused now decoded
  as OpenJPEG decodes it. The CHANGELOG lists each one.
- Any other change to the output needs a minor release, such as a new
  default for color handling, or, where the standard leaves room, a choice
  that follows neither the standard nor OpenJPEG.

[openjpeg-departures.md](openjpeg-departures.md) records each place the
output differs from OpenJPEG on purpose, and the rule behind it. Changing
one of those entries is not a fix; it needs a minor release.

### `DecodeOptions` equality

`DecodeOptions` implements `PartialEq` and `Eq`, and that is covered:
removing `Eq` would break a caller. So every new option must implement
`Eq` too. A float setting, such as a gamma or a color tolerance, would need
a wrapper type.

Two other trait promises rule things out the same way. `Error` is `Clone`,
so a source error it carries must be `Clone` too, or held in an `Arc`.
Every public type is `Send` and `Sync`, so none can hold an `Rc`, a `Cell`
or a raw pointer.

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

`Waiting::Held` carries an error that more bytes can still replace. One
variant covers several cases, and they resolve differently:

- In a codestream, the first refine that decodes once the `EOC` is in gives
  the final answer.
- In a JP2 file, the same holds once no box after the codestream can still
  be read. Until then, `finish` reports the error.
- A last tile-part whose `Psot` is 0 runs to the end of the input, so it
  never closes, and only `finish` reports the error.
- A refusal of the sRGB step is final or held, depending on whether a box
  after the codestream can still be read.
- A fault in a box before the codestream: in a box of length 0, only
  `finish` reports it, as the box runs to the end of the input. In a box
  cut inside its `XLBox`, it resolves once the bytes that complete the
  `XLBox` arrive.

The value does not say which, and that is deliberate: `Held` is
`#[non_exhaustive]`, so a field that says why can be added without a
break.

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
- `refine` never returns these. It waits with `Waiting::SampleLimit`
  instead, or, where a work bound refused first, with that bound's wait:
  `Waiting::WorkLimit`, `MissingTiles`, `BoundSpent` or `RepeatLimit`.

Every other error from `header` and `refine` is final. `finish` decodes
under the decoder's own options, so with `strict` on it can give a
different error from the one `refine` gave.

### What `header` returning `Ok(None)` means

`IncrementalDecoder::header` returns `Result<Option<Header>>`, and
`Ok(None)` has two meanings:

- The main header is not complete yet.
- In a JP2 file, a fault in a box before the codestream is held, because
  more bytes could change it, such as a `jp2h` or `ftyp` box of length 0,
  or one cut inside its `XLBox`.

`refine` tells the two apart: it waits with `Waiting::HeaderIncomplete` for
the first and with `Waiting::Held { error }` for the second. `finish`
reports the held fault once the input ends.

### The minimum Rust version

The crate needs Rust 1.87 or newer (`rust-version` in `Cargo.toml`). The
minimum rises only in a minor release, so a patch release never needs a
newer Rust than its minor release did.

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
  reported, including its variant. The rustdoc of `Error` lists what holds
  today.
- **The hidden `fuzz` module** and the `__fuzz` feature it needs. They exist
  for the crate's own fuzz targets and tests, and are not part of the API.

The default values of the limits are not fixed either. Only the types, the
variants and what they mean are stable. A value changes by direction:

- Raising one, so that more files pass, can ship in any release.
- Lowering one, so that some files are refused, needs a minor release,
  except as a security fix. A security fix that lowers a value can ship in
  a patch release, and the CHANGELOG lists it (see
  [SECURITY.md](../SECURITY.md#supported-versions)).

The values today:

- The sample limit: 2^29 samples, or 8 samples per byte of input when that
  is more.
- The caps on a tile: the larger of 2^20 code-blocks and one for each 512
  samples the sample limit allows, and the larger of 2^18 precincts and one
  for each 2048.
- The packet work bound: 2^24 steps, or 32 per byte of tile data when that
  is more.
- The code-block work bound: 2^26 sample-passes, or 2^15 per byte of tile
  data when that is more.
- The guards no option raises: at most 32 `POC` volumes in a tile, and at
  most 30 bit-planes in a code-block.
- For an `IncrementalDecoder`: the packet walks it keeps take at most the
  larger of 1 MiB and 32 bytes per byte fed. Once the walks it does again
  pass four times the packet bound of one decode, a refine waits with
  `Waiting::RepeatLimit`.

The memory and time figures in the rustdoc, [SECURITY.md](../SECURITY.md)
and [resource-bounds.md](resource-bounds.md) follow from these values, so
they can change with them. Among them: the tile-part records take under
1 KiB for each tile, and in all at most about 52 times the input plus
128 KiB.

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
[`Jp2Metadata`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.Jp2Metadata.html
[`ImageHeader`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.ImageHeader.html
[`ColorSpecification`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.ColorSpecification.html
[`ColorSpace`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.ColorSpace.html
[`FileType`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.FileType.html
[`ReaderRequirements`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.ReaderRequirements.html
[`StandardFeature`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.StandardFeature.html
[`VendorFeature`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.VendorFeature.html
[`Resolution`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.Resolution.html
[`Res`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.Res.html
[`Ratio`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.Ratio.html
[`ChannelDefinition`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.ChannelDefinition.html
[`ChannelDefinitionEntry`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.ChannelDefinitionEntry.html
[`ChannelType`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.ChannelType.html
[`Association`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.Association.html
[`Palette`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.Palette.html
[`PaletteColumn`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.PaletteColumn.html
[`ChannelMapping`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.ChannelMapping.html
[`MappingKind`]: https://docs.rs/rust-j2k/latest/rust_j2k/enum.MappingKind.html
[`XmlBox`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.XmlBox.html
[`IprBox`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.IprBox.html
[`UuidBox`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.UuidBox.html
[`UuidInfo`]: https://docs.rs/rust-j2k/latest/rust_j2k/struct.UuidInfo.html
