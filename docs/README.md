# Project documentation

These pages cover the project as a whole: what the decoder covers, in what
order the work was built, and how its output is checked. Code comments cite
the ISO section each module implements, so the detail of a single stage lives
next to its code.

## What this project is

rust-j2k is a JPEG 2000 decoder written in pure Rust, with no C library
underneath and no `unsafe` code in the library. The long-run aim is the
coverage of [OpenJPEG](https://www.openjpeg.org/), the JPEG 2000 reference
software, including an encoder. For the API and a usage example, see the
crate [README](../README.md) and the rustdoc.

It was built GRIB2-decode-first. GRIB2 weather files can store each grid as a
raw JPEG 2000 codestream (template 5.40, `grid_jpeg`), and the first release
decoded exactly the slice of the standard those grids use. That slice had a
waiting user, the fieldglass GRIB2 reader, and it forced the hardest parts to
be right early: the EBCOT block decoder and the wavelet transform. Later work
widened the same engine. It now decodes general Part 1 codestreams (all 23
ISO/IEC 15444-4 conformance codestreams decode within their compliance-class
error bounds) and JP2 files (all nine JP2 reader files in the same suite do
too). HTJ2K (the High Throughput block coder of Part 15), an encoder and the
later parts of the standard come next.

Other ways to read JPEG 2000 from Rust exist. `jp2k`, `openjpeg-sys` and
`grokj2k-sys` bind a C or C++ library. `hayro-jpeg2000` is a pure-Rust decoder
whose image output is 8-bit (per-component samples are `f32`). By default
this crate keeps each sample at its coded depth, which a GRIB2 grid of more
than 8 bits per value needs.

## The documents

- [scope.md](scope.md) lists every JPEG 2000 capability OpenJPEG implements,
  which part of the standard it belongs to, whether GRIB2 needs it, and which
  phase delivers it.
- [roadmap.md](roadmap.md) is the phased plan, from the GRIB2 decode MVP
  (Phase 1, v0.1.0) through the general Part 1 decoder (Phase 2, v0.3.0) and the JP2
  file format (Phase 3) to full-codec parity, with the gate for each phase.
- [correctness.md](correctness.md) explains why the output can be trusted:
  the conformance suite, the OpenJPEG oracle, per-stage golden tests and
  fuzzing.
- [openjpeg-departures.md](openjpeg-departures.md) records every place this
  decoder deliberately differs from OpenJPEG 2.5.0, and why.
- [resource-bounds.md](resource-bounds.md) states the decoder's resource
  limits, how each is counted and what it costs a valid file.
- [srgb-output.md](srgb-output.md) gives what sRGB output produces for each
  kind of input, and the evidence for each case.
- [development.md](development.md) lists the tools that regenerate the oracle
  snapshots and run the extra quality gates (cargo-deny, cargo-fuzz), with a
  version and an install command for each. You don't need them to run the
  tests. It also lists the checks for work on a decode stage.
- [roadmap-phase1.md](roadmap-phase1.md), [roadmap-phase2.md](roadmap-phase2.md)
  and [roadmap-phase3.md](roadmap-phase3.md) are the milestone plans for the
  first three phases, each step tied to its module and its oracle. All three
  phases are done, so these are kept as the record of how the work was built.

## Reading order

If you're new, read this page, then [roadmap.md](roadmap.md) for the shape of
the work, then [scope.md](scope.md) when you need to know whether a feature is
supported or which phase it belongs to. Read [correctness.md](correctness.md)
and [what "done" means](development.md#what-done-means) before you claim a
stage is done.
