# Project documentation

rust-j2k is a JPEG 2000 decoder written in pure Rust, with no C library
underneath and no `unsafe` code in the library. It decodes general Part 1
and HTJ2K codestreams, and JP2 and JPH files; it started with the part of
JPEG 2000 that GRIB2 weather grids use. For the API and a usage example,
see the crate [README](../README.md) and the rustdoc. The pages here cover
the project as a whole; code comments cite the section of the standard each
module implements.

Other ways to read JPEG 2000 from Rust exist. `jp2k`, `openjpeg-sys` and
`grokj2k-sys` bind a C or C++ library. `hayro-jpeg2000` is a pure-Rust decoder
whose image output is 8-bit (per-component samples are `f32`). By default
this crate keeps each sample at its coded depth, which a GRIB2 grid of more
than 8 bits per value needs.

## Which page to read

If you use the crate:

- [scope.md](scope.md) lists the JPEG 2000 features, by part of the standard,
  and says whether this decoder reads each one. It also lists the decode
  options, such as reduced resolution and partial decode.
- [openjpeg-departures.md](openjpeg-departures.md) lists every place this
  decoder's output differs from OpenJPEG 2.5.0 on purpose, and why. Start here
  if a file decodes differently from OpenJPEG.
- [resource-bounds.md](resource-bounds.md) gives the memory and work limits,
  how each is counted and what it costs a valid file. Start here if a file is
  refused with `Error::Limit`.
- [stability.md](stability.md) says which parts of the API stay the same
  within a minor version and which can change.
- [incremental-decoding.md](incremental-decoding.md) explains how
  `IncrementalDecoder` decodes a file while it arrives, and what each refine
  costs.
- [srgb-output.md](srgb-output.md) gives what sRGB output produces for each
  kind of input, and the evidence for each case.
- [correctness.md](correctness.md) explains why the output can be trusted: the
  conformance suite, the reference decoders it is compared against, per-stage
  golden tests and fuzzing.
- [performance.md](performance.md) compares decode speed and peak memory with
  OpenJPEG, Grok, OpenJPH and hayro-jpeg2000, with the method and commands.

If you work on the crate:

- [roadmap.md](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/roadmap.md) says what comes next, and the gate each phase must
  pass.
- [development.md](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/development.md) sets up the reference decoders and the
  extra tools, shows how to regenerate an oracle snapshot, and lists the
  checks for work on a decode stage. You don't need it to run the tests.
- [`tests/fixtures/README.md`](../tests/fixtures/README.md) describes each
  test fixture, where it came from and the command that made it.
- [`fuzz/README.md`](https://github.com/D0ubleD0uble/rust-j2k/blob/main/fuzz/README.md) shows how to run the fuzz targets.
- [release-validation.md](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/release-validation.md)
  lists the checks a release runs before it is tagged: what each catches, how
  to run it, its cost and when it runs.
- [roadmap-phase1.md](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/roadmap-phase1.md),
  [roadmap-phase2.md](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/roadmap-phase2.md),
  [roadmap-phase3.md](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/roadmap-phase3.md) and
  [roadmap-phase4.md](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/roadmap-phase4.md) are the plans of the finished phases,
  with each step tied to its module and the oracle that checked it. They are
  kept as the record of how the decoder was built.
- [roadmap-phase5.md](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/roadmap-phase5.md)
  is the plan for the Part 1 encoder, with each step tied to its module and the
  oracle that checks it.
