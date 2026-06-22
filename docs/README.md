# Project documentation

This directory holds the durable design docs: what we're building, why, in what
order, and how we know it's correct. Code comments cite the ISO section each
module owns; these docs cover the project as a whole.

## What this project is

A pure-Rust JPEG 2000 codec aiming, over time, at the same coverage as
[OpenJPEG](https://www.openjpeg.org/) (the ISO/IEC reference implementation),
but with no C dependency, no `unsafe` outside justified hot paths, and clean
cross-compilation to every target.

We build it **GRIB2-decode-first**. The first deliverable is the exact slice of
the standard that GRIB2 template 5.40 (`grid_jpeg`) needs, because that path
unblocks a real downstream consumer (the fieldglass GRIB2 reader) and forces the
hardest core (the EBCOT block coder and the wavelet transform) to be correct
before any breadth work begins. Everything after that widens the same engine
outward toward full Part 1, the JP2 file format, HTJ2K, an encoder, and the
later parts of the standard.

This is a real gap to fill: as of 2026 there is no production-grade pure-Rust
JPEG 2000 codec. The usable options (`jp2k`, `openjpeg-sys`, `grokj2k-sys`) all
bind to a C/C++ library; the one native attempt (`iszak/jpeg2000`) is partial
and stale.

## The documents

- [scope.md](scope.md) — the full feature inventory. Every JPEG 2000 capability
  OpenJPEG implements, which part of the standard it belongs to, whether GRIB2
  needs it, and which project phase delivers it.
- [roadmap.md](roadmap.md) — the phased plan, from the initial skeleton through
  the GRIB2 decode MVP (Phase 1, shipped in v0.1.0) to full-codec parity, with
  the gate for each phase.
- [roadmap-phase1.md](roadmap-phase1.md) — the GRIB2 decode MVP broken into
  ordered milestones, each tied to its module, its oracle, and what it unblocks.
- [roadmap-phase2.md](roadmap-phase2.md) — the general Part 1 decoder broken into
  ordered milestones, gated on the ISO/IEC 15444-4 conformance suite.
- [correctness.md](correctness.md) — how we know a stage is right: the oracle
  cross-check, the ISO 15444-4 conformance suite, per-stage golden vectors,
  fuzzing, and round-trip testing once the encoder lands.

## Reading order

New here? Read this file, then [roadmap.md](roadmap.md) for the shape of the
work, then [scope.md](scope.md) when you need to know whether a given feature is
in or out of the phase you're working on. Reach for [correctness.md](correctness.md)
before you claim a stage is done.
