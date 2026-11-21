# Feature scope

This page lists the JPEG 2000 features, grouped by part of the standard, and
says whether this decoder reads each one today. The [roadmap](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/roadmap.md)
says which features are planned. A file that
needs a feature this decoder doesn't read is refused with
`Error::Unsupported` rather than decoded wrongly.

"Yes, with limits" means the feature decodes, but a few inputs inside it are
refused or handled partly. The text under each table names them.

## Part 1: core coding system (ISO/IEC 15444-1)

### Codestream syntax (Annex A)

| Feature | Decoded |
|---|---|
| SOC / SIZ / COD / QCD / SOT / SOD / EOC (main header and tile-parts) | Yes, with limits |
| COC / QCC (per-component coding and quantization overrides) | Yes, with limits |
| RGN (region of interest) | Yes, with limits |
| POC (progression order change) | Yes |
| PPM / PPT (packed packet headers) | Yes |
| PLM / PLT (packet-length markers) | Yes |
| TLM (tile-part-length marker) | Yes |
| CRG (component registration) | Yes, with limits |
| SOP / EPH (start-of-packet and end-of-packet-header markers) | Yes |
| COM (comment), read and skipped | Yes |
| Multiple tiles and multiple tile-parts per tile | Yes, with limits |
| Non-zero image offset (XOsiz/YOsiz) | Yes |
| Non-zero tile offset (XTOsiz/YTOsiz) | Yes |
| CAP (capabilities) and CPF (corresponding profile) | Yes, with limits |
| Part 2 markers (MCT, MCC, NLT, MCO, CBD) | No |

Limits inside the decoded rows:

- RGN decodes only the maxshift style of Annex H (`Srgn = 0`). Any other style
  is refused.
- CAP may signal only Part 15 (HTJ2K). A capability of any other part, and
  more than one HT set per code-block (MULTIHT), are refused. The code-block
  style of each COD and COC decides the block coder. A MIXED style lets each
  code-block be an HT or a Part 1 block; it is refused with bypass or
  termination on each pass, and under a CAP whose `Ccap15` bits 15-14 are
  `10`.
- A reserved marker 0xFF30 to 0xFF3F, which carries no segment, is skipped in
  the main header but refused in a tile-part header, as OpenJPEG refuses it.
- A tile-part COD or COC that gives a component a different wavelet from the
  main header is refused: tiles must agree on each component's wavelet.
- CRG is read and checked, but its offsets are not applied to the samples and
  are not returned by the API. The decoded components are the same with or
  without it.

### Transform and reconstruction

| Feature | Decoded |
|---|---|
| 5/3 reversible inverse DWT (integer lifting, bit-exact) | Yes |
| 9/7 irreversible inverse DWT (floating-point lifting) | Yes |
| Multiple resolution levels | Yes |
| Scalar dequantization: derived and expounded, guard bits | Yes |
| DC level shift and clamp to the component depth | Yes |
| Component depth up to 31 bits unsigned or 32 bits signed | Yes |
| Component depth past that (an unsigned 32-bit component, 33 to 38 bits) | No |
| Multiple components (up to 16384) with sub-sampling | Yes |
| Multiple component transform: RCT (reversible color) | Yes, with limits |
| Multiple component transform: ICT (irreversible color) | Yes, with limits |

Each sample is held as an `i32`, which is why the depth stops at 31 bits
unsigned and 32 signed. No phase plans deeper components.

The color transform runs over components 0 to 2, and the wavelet picks which
one applies (5/3 means RCT, 9/7 means ICT). If COD asks for the transform and
those three components don't all use the same wavelet, the codestream is
refused.

### Tier-1: EBCOT block coding (Annexes C, D)

| Feature | Decoded |
|---|---|
| MQ arithmetic decoder | Yes |
| Significance-propagation, magnitude-refinement and cleanup passes | Yes |
| Context formation from the 3×3 neighborhood | Yes |
| Code-block style: selective arithmetic-coding bypass (lazy) | Yes |
| Code-block style: termination on each pass (restart) | Yes |
| Code-block style: reset context probabilities | Yes |
| Code-block style: vertically causal context | Yes |
| Code-block style: predictable termination | Yes |
| Code-block style: segmentation symbols | Yes |
| Code-block style: HTJ2K block coding | Yes (see Part 15) |
| Code-block style: HTJ2K mixed-mode block coding | Yes |

### Tier-2: packet decoding (Annex B)

| Feature | Decoded |
|---|---|
| Packet headers: inclusion, zero-bitplane, pass-count and length tag-trees | Yes |
| All five progression orders: LRCP, RLCP, RPCL, PCRL, CPRL | Yes |
| Multiple quality layers | Yes |
| Precinct partitions (non-maximal precincts) | Yes |

The decoder decodes every quality layer unless a decode caps them (see
[Decode options](#decode-options)).

### JP2 file format (Annex I)

| Feature | Decoded |
|---|---|
| Box parsing: signature, ftyp, jp2h, jp2c (contiguous codestream) | Yes, with limits |
| Image header (ihdr), bits-per-component (bpcc) | Yes |
| Enumerated color spaces (sRGB, grayscale, sYCC), reported as metadata | Yes |
| Restricted ICC profile (colr), passed through as bytes | Yes |
| Opt-in sRGB output: sYCC to RGB, restricted ICC to sRGB | Yes, with limits |
| Palette (pclr), component mapping (cmap), channel definition (cdef) | Yes, with limits |
| Resolution (res), capture and display | Yes |
| xml and uuid boxes, passed through as bytes | Yes |
| IPR (jp2i) passed through as bytes; UUID info (uinf) read | Yes |
| Reader requirements (rreq), file type (ftyp) and every colr box, reported | Yes |

Limits inside the decoded rows:

- A file that claims neither `jp2 ` nor `jph ` compatibility (a pure JPX,
  JPM or MJ2 file) is refused.
- A palette channel that looks up another palette column is refused, and so
  is a palette column deeper than 31 bits unsigned or 32 bits signed.
- sRGB output handles sYCC and the restricted ICC profiles. With three or
  more color channels, other color spaces (CMYK, CIELab, e-sYCC and so on) are
  refused under that option; one or two channels pass through as gray. An ICC
  profile that LittleCMS would apply through a lookup table or the Lab
  connection space is refused: a monochrome profile always, an RGB profile
  only on three or more channels (on one or two it passes through, as above).
  [srgb-output.md](srgb-output.md) lists every case.

The nine JP2 reader files of ISO/IEC 15444-4 decode within their compliance
class, and a set of real-world `.jp2` files matches OpenJPEG.

## Decode options

These are the ways to change what a decode returns, set on `DecodeOptions` and
passed to `decode_with`. The OpenJPEG equivalent is the `opj_decompress` flag
that does the same job.

| Option | Supported | How | OpenJPEG |
|---|---|---|---|
| Decode at a lower resolution | Yes | `with_resolution_reduction(n)` | `-r` |
| Decode a cut codestream and report what is missing | Yes | `with_strict(false)`, then `Image::report` | `-allow-partial` |
| Decode a file while it arrives, refining the image as bytes come in | Yes | `IncrementalDecoder` ([incremental-decoding.md](incremental-decoding.md)) | none |
| Return a JP2 file's palette indexes without applying the palette | Yes | `with_raw_channels(true)` | none |
| Convert the color channels to sRGB | Yes | `with_color(ColorOutput::Srgb)` | `-force-rgb` |
| Raise or lift the sample limit and the two work limits | Yes | `with_sample_limit`, `with_packet_work_limit`, `with_code_block_work_limit` | none |
| Read the header without decoding any pixels | Yes | `read_header`, `read_header_with` | `opj_dump` |
| Stop after a given number of quality layers | Yes | `with_max_quality_layers(Some(n))` | `-l` |
| Decode one area of the image | Yes, with limits | `with_region(Region::Window(Window::new(x0, y0, x1, y1)))` | `-d` |
| Decode one tile | Yes | `with_region(Region::Tile(index))` | `-t` |
| Decode some of the components | Yes, with limits | `with_components(Some(list))` | `-c` |

A window is on the reference grid, as OpenJPEG takes it, and the part of it
outside the image is left out. Only the tiles it reaches are decoded, and in
each of them only the code-blocks whose wavelet support reaches the window,
with the wavelet run over only the samples the window needs, as OpenJPEG does.
A code-block that straddles the window's edge is still decoded whole, so a
window inside one large tile costs only part of that tile's decode. The packets
of the whole tile are still read. Under a window or a tile the sample limit
counts the samples of the tiles read; a component subset alone counts the
whole image. Under the color transform, asking for one of components 0, 1
and 2 decodes all three. On a JP2 file a component subset returns the
codestream components as coded, without the palette or channel definitions.
sRGB output from sYCC with sub-sampled chroma is refused with a window or a
tile that does not cover the whole image, as the chroma replication depends
on where the decoded area starts. A layer cap still reads the packets of the
later layers, so damage in them is still an error. A partial decode relaxes
only missing data and leftover bytes: a damaged packet header or
code-block is still an error. The limits and what they cost a valid file are
in [resource-bounds.md](resource-bounds.md).

## GRIB2 template 5.40

GRIB2 weather files can store each grid as a raw JPEG 2000 codestream
(template 5.40, `grid_jpeg`). That codestream uses a small part of Part 1, all
of which decodes:

- one component (a single scalar grid), so no color transform;
- integer samples, signed or unsigned, up to 32 bits (an unsigned 32-bit grid
  is refused, as above);
- either wavelet: 5/3 reversible (lossless) or 9/7 irreversible (lossy);
- typically one tile at the canvas origin, one quality layer, LRCP progression,
  no JP2 boxes, no region of interest and no precinct partition.

The template allows 9/7, but no operational producer we know of ships it: HRRR
and NDFD grids are complex-packed, and ECMWF uses CCSDS.

## Part 15: High Throughput JPEG 2000 (HTJ2K, ISO/IEC 15444-15)

HTJ2K replaces the Tier-1 block coder with a much faster one (FBCOT) that has
no MQ coder. Everything outside Tier-1 (codestream, Tier-2, DWT,
quantization) is the same as Part 1. This decoder reads HTJ2K codestreams
whose code-blocks each have one HT set, including tile-components that mix
HT and Part 1 code-blocks, and refuses more than one HT set with
`Error::Unsupported`. OpenJPEG 2.5.0 refuses mixed-mode codestreams.

| Feature | Decoded |
|---|---|
| CAP marker and HTJ2K capability signaling | Yes, read; MULTIHT refused |
| HT cleanup pass (MEL, VLC and MagSgn) | Yes |
| HT placeholder passes | Yes |
| HT SigProp and MagRef passes | Yes |
| Mixed-mode (HT and Part 1 code-blocks in one tile-component) | Yes |
| Region of interest on HT code-blocks | Yes |
| JPH file format (`jph ` brand, Annex D) | Yes |
| JPH application-defined channels (`cdef` `Typ` 3) | Yes, reported |
| JPH `colr` methods 3 and 5 | No (skipped, as an unknown method is) |

## Part 2: extensions and JPX (ISO/IEC 15444-2)

| Feature | Decoded |
|---|---|
| Extended (.jpx) file format and boxes | No |
| Variable DC offset, nonlinear point transform | No |
| Trellis-coded quantization (TCQ) | No |
| Arbitrary decomposition structures and custom wavelet kernels | No |
| Extended and array-based multiple component transforms (multi- and hyperspectral) | No |
| Extended region of interest | No |

## Encoding and later parts

| Feature | Supported |
|---|---|
| Part 1 encoder (forward DWT, MQ encoder, rate control, packetization) | No |
| HTJ2K (FBCOT) block encoder | No |
| Part 3: Motion JPEG 2000 (MJ2 container) | No |
| Part 9: JPIP interactive streaming protocol | No |
| Part 11: JPWL wireless error protection | No |

## Speed

The decoder aims for correct output first and OpenJPEG's speed second.
Optimizations go only into hot paths a profile has measured, and `unsafe` code
and SIMD wait until a benchmark shows they're needed.
