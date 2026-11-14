# Roadmap

Next is Phase 4, decoding HTJ2K (Part 15) codestreams. After it come a Part 1
encoder (Phase 5), then the Part 2 extensions and an HTJ2K encoder (Phase 6),
then the rest of the standard's parts (Phase 7). Phases 0 to 3, which built a
decoder for Part 1 codestreams and JP2 files, are finished, and Phase 3 ships
as the 0.4.0 release.

Each phase has a goal, the work it contains and a gate: the test that must
pass before the phase counts as done. A gate always compares against an
external reference, never against the crate's own output, for the reasons in
[correctness.md](correctness.md). [scope.md](scope.md) lists every feature,
whether it decodes today, and the phase that plans each one that doesn't. This
page gives the order and the gates.

## Finished phases

The work started with the GRIB2 decode path because it was a hard,
end-to-end slice with a waiting user, the fieldglass GRIB2 reader, and then
widened the same engine. Phase 0 laid out one stubbed module per pipeline
stage. Phase 1 decoded the single-component codestream GRIB2 template 5.40
produces, bit-exact against the eccodes and OpenJPEG oracle for 5/3 and within
tolerance for 9/7, and shipped in v0.1.0 ([plan](roadmap-phase1.md)). Phase 2
decoded any Part 1 codestream: its gate was all 23 ISO/IEC 15444-4 Part 1
conformance codestreams within their compliance-class bounds, with a clean
fuzzing run. It shipped in v0.3.0 ([plan](roadmap-phase2.md)). Phase 3 added
the JP2 file format; its gate was the nine ISO/IEC 15444-4 JP2 reader files
within their compliance bounds and a set of real `.jp2` files matching
OpenJPEG ([plan](roadmap-phase3.md)). Each plan ties every step to its module
and the oracle that checked it.

## Phase 4: HTJ2K decode (Part 15)

The goal is to decode High Throughput (Part 15) codestreams. HTJ2K swaps the
Tier-1 block coder for a faster one, so the work is in two places:

- the CAP marker and HTJ2K capability signaling;
- the FBCOT block decoder (MEL, VLC and MagSgn) as a second Tier-1, with the
  rest of the pipeline shared with Part 1.

The gate is every ISO/IEC 15444-4 HT codestream and JPH file within its ISO
bounds, MIXED code-blocks included, plus bit-exact agreement with OpenJPEG
2.5.0 wherever OpenJPEG decodes the file. OpenJPEG decodes only part of the
HT set; where it and the standard disagree, the standard wins. The phase ships
as 0.5.0. The [plan](roadmap-phase4.md) breaks it into steps, each with its
oracle.

## Phase 5: Part 1 encoder

The goal is to encode Part 1 codestreams and JP2 files. The work:

- forward 5/3 and 9/7 DWT, and forward scalar quantization;
- the MQ arithmetic encoder and EBCOT pass encoding;
- rate control (PCRD-opt), Tier-2 packetization, and codestream and JP2
  writing.

The gate has two halves. The round trip encodes, decodes and compares against
the original within the path's tolerance. The cross-decode has OpenJPEG read
this encoder's output and this decoder read OpenJPEG's, within the target rate
and quality. The round trip alone would be self-consistency; the
cross-decode is what makes this gate external.

## Phase 6: Part 2 extensions and HTJ2K encode

The goal is to decode JPX and the Part 2 coding extensions, and to encode
HTJ2K. The work:

- the JPX extended file format; trellis-coded quantization; arbitrary
  decompositions and custom wavelet kernels; extended and array-based multiple
  component transforms (multi- and hyperspectral); extended region of
  interest;
- the FBCOT block encoder.

The gate is a cross-check against OpenJPEG and Grok on Part 2 and HTJ2K encode
corpora.

## Phase 7: the remaining parts

Motion JPEG 2000 (Part 3, the MJ2 container), JPIP interactive streaming
(Part 9) and JPWL wireless error protection (Part 11). They are sequenced by
demand, and none blocks the phases above.

## Not in a phase yet

Three decode options that `opj_decompress` has are not planned in any phase:
stopping after a given number of quality layers (`-l`), decoding one area of
the image (`-d`) and decoding one tile (`-t`). The decoder always decodes
every layer of every tile over the whole image.

## Rules for every phase

Every phase keeps the crate pure Rust, with no C or native dependency, and
treats its input as untrusted: a malformed file ends in a typed `Error`, never
a panic. [CONTRIBUTING.md](../CONTRIBUTING.md) has the quality gates a change
must pass.
