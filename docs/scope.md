# Feature scope

The JPEG 2000 family is large. This document inventories the capabilities a
full codec carries, maps each to its part of the standard, and records two
things per feature: whether the **GRIB2** decode path needs it, and which
project **phase** delivers it. Phases are defined in [roadmap.md](roadmap.md).

Legend: ✓ required · — not needed · ○ optional/rare. "GRIB2" is the column for
template 5.40 (`grid_jpeg`) decode.

## How GRIB2 narrows the standard

GRIB2 §5.40 wraps a *raw codestream* (ISO/IEC 15444-1 Annex A) in the GRIB2
data section. That codestream is deliberately plain:

- one component (a single scalar grid), so no multi-component or color transform;
- integer samples, signed or unsigned, up to 32 bits;
- either wavelet — 5/3 reversible (lossless) or 9/7 irreversible (lossy). No
  operational GRIB2 producer ships lossy 9/7 (HRRR and NDFD are complex-packed,
  ECMWF is CCSDS), so the 9/7 path is graded by re-encoding a real grid with
  OpenJPEG's irreversible mode;
- typically a single tile, a single quality layer, LRCP progression, and no
  JP2 box wrapper, no region of interest, no precinct subdivision.

So the GRIB2 MVP is a strict, well-defined subset of Part 1's *decoder*. The
value of building it first is that the subset still exercises the whole vertical
pipeline (codestream → Tier-2 → Tier-1 → dequant → IDWT → image), so the engine
that the later phases widen is correct end-to-end before breadth work starts.

## Part 1 — core coding system (ISO/IEC 15444-1)

### Codestream syntax (Annex A)

| Feature | GRIB2 | Phase |
|---|---|---|
| SOC / SIZ / COD / QCD / SOT / SOD / EOC (minimum viable header + tile) | ✓ | 1 |
| COC / QCC (per-component coding/quant overrides) | — | 2 |
| RGN (region of interest, maxshift) | ○ | 2 |
| POC (progression order change) | — | 2 |
| PPM / PPT (packed packet headers) | ○ | 2 |
| PLM / PLT (packet-length markers) | ○ | 2 |
| TLM (tile-part-length marker) | ○ | 2 |
| CRG (component registration) | — | 2 |
| SOP / EPH (start-of-packet / end-of-packet-header, error resilience) | ○ | 2 |
| COM (comment) — recognise and skip | ✓ | 1 |
| Multiple tiles and multiple tile-parts per tile | — | 2 |

### Transform & reconstruction

| Feature | GRIB2 | Phase |
|---|---|---|
| 5/3 reversible inverse DWT (integer lifting, bit-exact) | ✓ | 1 |
| 9/7 irreversible inverse DWT (float lifting) | ✓ | 1 |
| Multiple resolution levels (decomposition) | ✓ | 1 |
| Scalar dequantization: derived & expounded, guard bits | ✓ | 1 |
| DC level shift + clamp to component depth | ✓ | 1 |
| Multiple components (up to 16384) with subsampling | — | 2 |
| Multiple component transform: RCT (reversible) / ICT (irreversible) color | — | 2 |

### Tier-1 — EBCOT block coding (Annexes C, D)

| Feature | GRIB2 | Phase |
|---|---|---|
| MQ arithmetic decoder | ✓ | 1 |
| Significance-propagation / magnitude-refinement / cleanup passes | ✓ | 1 |
| Context formation from the 3×3 neighbourhood | ✓ | 1 |
| Code-block styles: selective arithmetic-coding bypass (lazy) | ○ | 2 |
| Code-block styles: reset contexts, termination on each pass (restart) | ○ | 2 |
| Code-block styles: vertically causal context | ○ | 2 |
| Code-block styles: predictable termination, segmentation symbols | ○ | 2 |

### Tier-2 — packet decoding (Annex B)

| Feature | GRIB2 | Phase |
|---|---|---|
| Packet header parse: inclusion / zero-bitplane / pass-count / length tag-trees | ✓ | 1 |
| Single quality layer | ✓ | 1 |
| LRCP progression | ✓ | 1 |
| Remaining progressions: RLCP, RPCL, PCRL, CPRL | — | 2 |
| Multiple quality layers (quality scalability) | — | 2 |
| Precinct partition (non-maximal precincts) | ○ | 2 |

### JP2 file format (Annex I)

| Feature | GRIB2 | Phase |
|---|---|---|
| Box parsing: signature, ftyp, jp2h, jp2c (contiguous codestream) | — | 3 |
| Image header (ihdr), bits-per-component (bpcc) | — | 3 |
| Enumerated colorspaces (sRGB, greyscale) | — | 3 |
| Restricted ICC profile (colr) | — | 3 |
| Palette (pclr), component mapping (cmap), channel definition (cdef) | — | 3 |
| Resolution (res), capture/display metadata | — | 3 |

## Part 15 — High Throughput JPEG 2000 / HTJ2K (ISO/IEC 15444-15)

The modern high-value extension; OpenJPEG decodes it since 2.5. Replaces the
EBCOT Tier-1 arithmetic coder with the fast block coder (FBCOT), much faster,
no MQ coder. Everything outside Tier-1 (codestream, Tier-2, DWT, quant) is
shared with Part 1.

| Feature | GRIB2 | Phase |
|---|---|---|
| CAP marker + HTJ2K capability signalling | — | 4 |
| FBCOT block **decoder** (MEL + VLC + MagSgn) | — | 4 |
| FBCOT block **encoder** | — | 6 |

## Part 2 — extensions / JPX (ISO/IEC 15444-2)

| Feature | GRIB2 | Phase |
|---|---|---|
| Extended (.jpx) file format and boxes | — | 6 |
| Variable DC offset, nonlinear point transform | — | 6 |
| Trellis-coded quantization (TCQ) | — | 6 |
| Arbitrary decomposition structures & custom wavelet kernels | — | 6 |
| Extended / array-based multiple component transforms (multi/hyperspectral) | — | 6 |
| Extended region of interest | — | 6 |

## Later parts (long tail)

| Feature | GRIB2 | Phase |
|---|---|---|
| Encoder for Part 1 (forward DWT, MQ encoder, rate control, packetization) | — | 5 |
| Part 3 — Motion JPEG 2000 (MJ2 container) | — | 7 |
| Part 9 — JPIP interactive streaming protocol | — | 7 |
| Part 11 — JPWL wireless error protection | — | 7 |

## Explicit non-goals (for now)

- Performance parity with OpenJPEG/Grok before correctness parity. Correct
  first, then profile and optimise the measured hot paths.
- `unsafe` and SIMD before a benchmark justifies them. See conventions.

These are "not yet," not "never": the aim is OpenJPEG-level coverage. They are
listed so a contributor knows what *this* phase is not trying to do.
