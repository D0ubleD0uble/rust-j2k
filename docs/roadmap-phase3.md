# Phase 3 roadmap — JP2 file format decode

> **Status: in progress.** P3.0 (the harness), P3.1 (the box reader) and
> P3.3 (the palette) have landed; the rest is planned. The milestones are the build order to follow,
> each with the oracle that gates it.

The overall [roadmap.md](roadmap.md) sequences the whole project; this file
zooms into Phase 3 alone. It breaks the line item "JP2 file format decode" into
ordered milestones, each tied to the module it adds, the oracle that proves it,
and the condition that lets the next milestone start.

The feature rows this phase covers are the "JP2 file format (Annex I)" entries
in [scope.md](scope.md). The gate stays at the Part 4 level of authority: the
**ISO/IEC 15444-4 JP2 reader test files** (`file1.jp2` .. `file9.jp2`, Annex G
of 15444-4) and their reference images, graded by the same PAE/MSE machinery
Phase 2 built. The day-to-day oracle stays `opj_decompress`.

## Decisions

Settled while planning, on 2026-10-31:

1. **Colour conversion is in this phase, and the phase ships as one release.**
   The opt-in sRGB output (P3.6: sYCC to RGB, restricted ICC to sRGB) ships
   with the rest of JP2 support in 0.4.0, and the phase gate is all nine Part 4
   JP2 files. There is no interim JP2 release.
2. **The GDAL real-world samples are committed, with per-file provenance.**
   The nine small files from GDAL `autotest/gdrivers/data/jpeg2000/` go under
   `tests/fixtures/jp2_real/`, excluded from the published crate like the
   conformance corpus. Their rights are not confirmed: GDAL's `LICENSE.TXT`
   says the project is "in general" MIT and does not mention test data, and
   some files are third-party encoder output (Kakadu, ECW SDK). The fixtures
   README says so plainly and records, per file, the GDAL path, the pinned GDAL
   commit, and the encoder that wrote it. It does not label them MIT.
3. **P3.1 rejects `pclr`, `cmap` and `cdef` with `Unsupported`** until P3.3
   and P3.4 lift their own guards, as Phase 2 did in #78. `main` never decodes
   a JP2 it would present wrongly.
4. **The box reader is fuzzed from P3.1.** P3.1 adds the `jp2_boxes` fuzz
   target, its dictionary, seeds and the CI smoke run, plus tests for the three
   length hazards (a length shorter than the header, an `XLBox` of 0 or past
   the end of the file, a child box past its parent). P3.7 keeps the full
   reject matrix and the long fuzz session.
5. **`read_header` is in this phase (P3.5).** A header-only entry point
   returns dimensions, component depths and the same `Jp2Metadata` that
   `decode` produces, for raw codestreams and JP2 alike, with its own fuzz
   target. Two known consumers read headers by hand today.

## What Phase 3 decodes

Phase 2 decodes any conformant Part 1 *codestream* (Annex A). Phase 3 wraps that
decoder in the JP2 file format (Annex I) so a `.jp2` file decodes directly, and
exposes what the wrapper says about the samples:

- the **box structure**: signature, `ftyp`, the `jp2h` superbox, `jp2c`;
  64-bit `XLBox` lengths; a last box of unknown length; unknown boxes skipped;
- the **image header** (`ihdr`) and per-component depths (`bpcc`), cross-checked
  against SIZ;
- the **colour specification** (`colr`): enumerated sRGB / greyscale / sYCC, and
  a restricted ICC profile carried through as bytes;
- **palette expansion** (`pclr` + `cmap`): a one-component index plane becomes
  the palette's channels, the way every JP2 reader (and OpenJPEG) presents it;
- **channel definitions** (`cdef`): colour channels put into colour order, and
  opacity / premultiplied-opacity channels labelled;
- **resolution** (`res`: `resc`, `resd`), and the `xml ` / `uuid` metadata boxes
  passed through as bytes;
- an opt-in **sRGB output** step: sYCC → RGB with chroma replication, and the
  restricted ICC profile (matrix/TRC and monochrome forms) applied to produce
  sRGB. This is what the Part 4 JP2 reader references are graded against, so it
  is in scope, but it is off by default. The default output is the samples as
  coded plus the metadata to interpret them.

The contract is unchanged: a feature this phase does not own (JPX-only boxes,
a file with no `jp2 ` compatibility) is rejected with `Error::Unsupported`,
never half-decoded, and a malformed box is a typed `Error`, never a panic.

### What the Part 4 JP2 files actually test

The nine files sit in `uclouvain/openjpeg-data` `input/conformance/` at the
same pinned commit (`39524bd3`) and under the same conformance-only grant as the
vendored Part 1 corpus; their references are `baseline/conformance/jp2_N.tif`,
8-bit RGB. Measured with `opj_decompress` 2.5.0 (built with lcms2):

| File | Boxes beyond the minimum | Codestream | What it grades | Raw jp2c decode vs reference |
| ---- | ------------------------ | ---------- | -------------- | ---------------------------- |
| file1 | `colr` sRGB (16); `xml ` before and after `jp2h` | 3×8-bit, RCT | box walk, enumerated colour, unknown-box skip | bit-exact |
| file2 | `colr` sYCC (18); `cdef` mapping comps 0,1,2 → colours 3,2,1 | 3×8-bit, no MCT | cdef reorder (stored Cr,Cb,Y), sYCC → RGB | PAE 127–197 |
| file3 | `colr` sYCC (18) | 3×8-bit, comps 1–2 sub-sampled 2×2 | chroma replication, sYCC → RGB | n/a (needs upsampling) |
| file4 | `colr` greyscale (17) | 1×8-bit | greyscale | bit-exact (reference repeats the channel ×3) |
| file5 | brand `jpx ` with `jp2 ` in the compatibility list; `rreq`; `colr` restricted ICC (RGB, gamma 1.8, wide primaries); second `colr` enum 21 | 3×8-bit, RCT | JP2-compatible JPX brand, ICC → sRGB, ignore the second `colr` | PAE 126 |
| file6 | `colr` greyscale | 1×12-bit | 12-bit grey; reference is `>> 4` | bit-exact after `>> 4` |
| file7 | brand `jpx ` + `jp2 `; `rreq`; `colr` restricted ICC (16-bit RGB, 2048-entry curves); second `colr` enum 20 | 3×16-bit, RCT | 16-bit ICC → sRGB, then `>> 8` | PAE 96 |
| file8 | `colr` restricted ICC (monochrome, kTRC gamma 1.8); `xml ` after `jp2c` | 1×8-bit | grey ICC → sRGB tone curve; boxes after the codestream | n/a (1 vs 3 channels) |
| file9 | `pclr` 256×3 @ 8-bit; `cmap` three channels from component 0; `colr` sRGB | 1×8-bit index plane | palette expansion | n/a (1 vs 3 channels) |

Two consequences shape the plan:

- **The reference is display output, not coded samples.** Files 2, 3, 5, 7 and
  8 can only be graded against the ISO images after a colour conversion. The
  box, palette and cdef work grades files 1, 4, 6 and 9 on its own; the sRGB
  step (P3.6) is what unlocks the other five.
- **OpenJPEG's own bar is PAE ≤ 4, MSE ≤ 1 per channel** (its
  `tests/conformance/CMakeLists.txt`: "Tolerances given by Part 4 Table G.1,
  Peak is set to 4"), decoding with `-p 8S -force-rgb`. OpenJPEG 2.5.0 + lcms
  lands within PAE 1 on all nine and bit-exact on 1, 4, 6, 9. Those are the
  bounds the manifest will record, with `bit_exact` set where OpenJPEG is.

## Public API design

### Entry point: `decode` auto-detects the container

`decode(bytes)` and `decode_with(bytes, options)` accept either a raw codestream
or a JP2 file. Input that opens with `FF4F` (`SOC`) is a codestream; otherwise
input whose bytes 4..8 are `jP  ` (the signature box's type) is a JP2 file, so a
damaged signature box is a `FileFormat` error rather than a missing `SOC`.
Anything else goes to the codestream parser, which rejects it. This is
additive: every input that decoded before still decodes to the same `Image`,
and the one input class that was `Error::Unsupported` ("JP2 file format
wrapper") now decodes. openreadout sniffs the same two magics before calling
us, so the auto-detect replaces their unwrapping (their test fixture needs a
`jp2h` box, see P3.1).

A file whose `ftyp` lacks `jp2 ` in brand or compatibility list (a JPX, JPM,
or MJ2 that is not JP2-compatible) is `Error::Unsupported`, not a parse error:
it is valid JPEG 2000 we do not read. files 5 and 7 are `jpx `-branded *and*
`jp2 `-compatible, so the compatibility list is what decides.

### `Image` grows one optional field

`Image` is `#[non_exhaustive]`, so a new field is not a breaking change, and
`Image::new` keeps its signature (the field defaults to `None`):

```rust
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub components: Vec<Component>,
    /// What the JP2 wrapper said about the samples. `None` for a raw codestream.
    pub jp2: Option<Jp2Metadata>,
}
```

`components` keeps its meaning: one grid per *output channel*. For a raw
codestream, and for a JP2 without a palette or `cdef`, that is one per SIZ
component in SIZ order, exactly as today. Palette expansion and `cdef` reorder
change what the channels are (below), and `jp2.channels` says what each one is.

### `Jp2Metadata`

One struct, `#[non_exhaustive]`, plain data a caller can match on:

```rust
pub struct Jp2Metadata {
    /// The first usable colour specification box, which is the one a JP2 reader uses.
    pub colour: ColourSpace,
    /// One entry per `components` entry, after palette and cdef application.
    pub channels: Vec<ChannelDefinition>,
    /// Whether the samples went through the palette (false when the caller
    /// asked for the raw index plane, or there is no palette).
    pub palette_applied: bool,
    pub resolution: Resolution,          // capture and/or default display, each optional
    pub xml: Vec<Vec<u8>>,               // every `xml ` box, in file order, bytes verbatim
    pub uuid: Vec<UuidBox>,              // every `uuid` box: 16-byte id + payload
}

pub enum ColourSpace {
    Srgb,                                // EnumCS 16
    Greyscale,                           // EnumCS 17
    Sycc,                                // EnumCS 18
    /// METH 2: a restricted ICC profile (monochrome or three-component
    /// matrix/TRC input profile), bytes verbatim. Not applied unless asked.
    RestrictedIcc(Vec<u8>),
    /// A Part 2 enumerated value (12 CMYK, 20 e-sRGB, 21 ROMM-RGB, 24 e-sYCC …),
    /// reported, not interpreted.
    Enumerated(u32),
    /// No usable `colr` box. `ihdr.UnkC` is not consulted (OpenJPEG ignores
    /// it, and Kakadu writes `UnkC = 1` beside a valid `colr`).
    Unknown,
}

pub struct ChannelDefinition { pub kind: ChannelType, pub association: Association }
pub enum ChannelType { Colour, Opacity, PremultipliedOpacity, Unspecified, Other(u16) }
pub enum Association { WholeImage, Colour(u16), None, Other(u16) }

pub struct Resolution { pub capture: Option<Res>, pub display: Option<Res> }
pub struct Res { pub vertical: Ratio, pub horizontal: Ratio }   // N/D × 10^E grid points per metre
```

Decisions baked in, each with its reason:

- **ICC is passthrough by default, conversion opt-in.** A viewer (openreadout)
  or a PDF engine (pdfcer) wants the profile bytes to hand to its own colour
  management; forcing a conversion would destroy information they need. The
  conversion exists (P3.6) because the Part 4 references require it, and it is
  gated by a `DecodeOptions` field.
- **Palette is applied by default.** A JP2 reader presents the palette's
  channels; the index plane is an encoding detail. OpenJPEG applies it unless
  told `OPJ_DPARAMETERS_IGNORE_PCLR_CMAP_CDEF_FLAG`; we mirror that with
  `DecodeOptions::raw_channels`.
- **`cdef` is applied by default**, with OpenJPEG's placement rule: colour
  channels are swapped into their `Asoc` order (so component 0 is colour 1, etc.)
  and non-colour channels stay where the file put them. For the common RGBA
  layout this is the identity; for file2 it reverses the three channels. The
  definitions ride along in `jp2.channels`, so a caller can always tell.
- **Only the first `colr` counts** (I.5.3.3: a conforming reader ignores the
  rest). `METH > 2` makes the whole box ignorable; if nothing usable is left,
  `ColourSpace::Unknown`.
- **`ihdr.BPC` and `bpcc` are read and cross-checked but SIZ is the truth** for
  depth and sign, as in OpenJPEG. Width/height and component count must agree
  with SIZ (below).
- **No colour conversion changes `bit_depth`.** sRGB output of a 16-bit ICC file
  is 16-bit sRGB; the harness does the `>> 8` the ISO references need.

### `DecodeOptions` grows two fields

```rust
pub struct DecodeOptions {
    pub resolution_reduction: u8,
    /// Skip palette expansion and cdef reorder: `components` are the SIZ
    /// components as coded. Default false.
    pub raw_channels: bool,
    /// What colour space the colour channels come out in. Default `AsCoded`.
    pub colour: ColourOutput,
}
pub enum ColourOutput {
    /// The samples as the codestream coded them (after palette and cdef).
    AsCoded,
    /// sRGB: sYCC is converted (sub-sampled chroma replicated to full
    /// resolution first), a restricted ICC profile is applied, sRGB and
    /// greyscale pass through. Anything else (`Enumerated`, `Unknown`) is
    /// `Error::Unsupported`.
    Srgb,
}
```

`raw_channels` + `AsCoded` is bit-for-bit the Phase 2 decode of the `jp2c`
payload; a test pins that.

### Error variant for the box layer

A new `Error::FileFormat(String)`: the JP2 box structure or a box field is
malformed (truncated box, missing or misplaced mandatory box, illegal field
value, box/codestream disagreement). `Error` is `#[non_exhaustive]`, so this
is additive. The existing variants keep their codestream meaning
(`Codestream` = structural damage inside the codestream, `Marker` = illegal
marker field), and `Unsupported` / `Limit` keep theirs. One variant for the
whole box layer rather than two: a caller distinguishes "the container is
broken" from "the codestream is broken"; finer than that has no consumer.

### Header-only reads: `read_header`

```rust
pub fn read_header(bytes: &[u8]) -> Result<Header>;

#[non_exhaustive]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub components: Vec<ComponentInfo>,  // bit_depth, signed, dx, dy
    pub jp2: Option<Jp2Metadata>,        // what `decode` would put on `Image.jp2`
}
```

It accepts the same two inputs as `decode`, reads the boxes and the main
header, and stops before tile data. It returns the same error `decode` would
for the same header fault. It lands in P3.5 (decision 5).

### Release

Phase 3 ships as **0.4.0**. All changes are additive on `#[non_exhaustive]`
types, but a new container decoding where it used to be refused is a
behaviour change worth a minor bump.

## Build order and why

The pipeline gains one stage in front and one optional stage behind:

```text
bytes ─▶ jp2 boxes ─▶ codestream ─▶ tier-2 ─▶ tier-1 ─▶ dequant ─▶ IDWT ─▶ MCT ─▶ Image
          (Annex I)                                                            └▶ palette/cdef ─▶ [sRGB]
```

Nothing between `codestream` and `MCT` changes. The new work is a `src/jp2/`
module (box reader, header boxes, palette, channel definitions, metadata), a
`src/colour.rs` (or `src/jp2/srgb.rs`) for the opt-in conversion, and the
glue in `lib.rs`. The build rule from Phases 1 and 2 holds: build in the order
each addition can be proven against an oracle before the next sits on top.

```text
P3.0 harness ─ P3.1 boxes + ihdr + jp2c + auto-detect ─┬─ P3.2 colr/bpcc metadata ──────────────┐
                                                        ├─ P3.3 pclr/cmap palette ─────────────────┤
                                                        ├─ P3.4 cdef channel definitions ──────────┼─ P3.6 sRGB output (needs P3.2, P3.4) ─┐
                                                        └─ P3.5 res/xml/uuid, JPX-compat skipping ─┤                                      ├─ P3.8 integrate ─ Part 4 JP2 gate
                                                           P3.7 fuzz target + reject matrix (starts after P3.1, runs throughout) ────────────┘
```

Three tracks after P3.1: the **metadata track** (P3.2, P3.5), the **channel
track** (P3.3, P3.4), and the **robustness track** (P3.7). They touch disjoint
code and can run in parallel in worktrees. P3.6 waits on P3.2 (it needs the
colour space and ICC bytes) and P3.4 (file2 needs cdef applied before sYCC).

## Milestones

### P3.0 — JP2 conformance harness and corpus

**Goal:** the Part 4 JP2 grading scaffold exists before the features it grades,
the way P2.0 stood up the Part 1 harness.

**Work:**
- Vendor `file1.jp2` .. `file9.jp2` under `tests/fixtures/conformance/jp2/`
  (4.4 MB) with the same `COPYRIGHT` and the same `exclude` in `Cargo.toml`.
  Extend `regenerate.py` to fetch them from the pinned commit and to convert
  each `jp2_N.tif` reference into 8-bit `.pgx` planes the existing PGX reader
  consumes: three planes for colour files, one for the greyscale files 4, 6, 8
  (their three TIFF channels are identical; measured). About 7.4 MB of PGX.
- A manifest section per entry: boxes present, colour method, codestream
  features, graded channel count, `bit_exact` (files 1, 4, 6, 9), bounds PAE 4 /
  MSE 1 otherwise, and the **output depth scaling** the reference needs
  (`shift_right`: 4 for file6, 8 for file7; the ISO reference is OpenJPEG's
  `-p 8S`, which is a plain right shift, verified bit-exact on file6).
- `tests/conformance_jp2.rs`: decode each file with
  `DecodeOptions::default()` (entries marked `needs_srgb` report *needs sRGB
  output* until P3.6 switches them to the sRGB option), shift, grade with
  `support::part4`'s comparator and ratchet. `IN_CLASS` starts empty; all nine
  report *not yet decoded* because `decode` still rejects the wrapper.

**Oracle:** the Part 4 JP2 references are the authority; this milestone is the
machinery.

**Done:** the harness runs, every reference parses, all nine entries report
*not yet decoded*, `cargo deny` and the packaged crate are unaffected
(`cargo publish --dry-run` shows no conformance file).

### P3.1 — Box reader, `ihdr`, `jp2c`, and auto-detect (`src/jp2/`, `src/lib.rs`)

**Goal:** a minimal JP2 (signature, `ftyp`, `jp2h` with `ihdr` + `colr`,
`jp2c`) decodes through `decode`.

**Work:**
- Box reader over a byte slice: `LBox`/`TBox`, `XLBox` when `LBox == 1`,
  `LBox == 0` means "to the end of the file" and is legal only for the last
  box. Every box length is checked against its parent before use; no
  allocation is sized from a box length.
- Top level: signature box first (exact 12 bytes), `ftyp` second, `jp2h`
  before `jp2c`, unknown boxes skipped. `jp2 ` must appear as brand or in the
  compatibility list, else `Unsupported`. The first `jp2c` is the codestream;
  later ones are ignored (I.5.4).
- `jp2h`: `ihdr` must be present and first (14 bytes); `HEIGHT`/`WIDTH` must
  equal the SIZ image area and `NC` must equal `Csiz`, else `FileFormat`. The
  other `jp2h` boxes are recognised and parked for P3.2–P3.5.
- Until P3.3 and P3.4 land, a `jp2h` holding `pclr`, `cmap` or `cdef` is
  `Error::Unsupported` naming the box, so `main` never decodes a JP2 it would
  present wrongly (decision 3). Each of those milestones lifts its own
  guard.
- `decode` sniffs the two magics and dispatches. The `JP2_SIGNATURE` reject in
  `codestream::parse` goes away; the reject-matrix row moves to the new
  `FileFormat`/`Unsupported` rows.
- `Image.jp2 = Some(Jp2Metadata { colour: Unknown, channels: one Colour entry
  per component, .. })` as the skeleton the later milestones fill in.

**Oracle:** the Part 4 references for files 1, 4 and 6 (no conversion needed;
file6 through the harness shift). For real-world structure, every `.jp2` in
the GDAL sample set (P3.0's `real-world` fixtures, below) must
decode to exactly what `decode` gives for its extracted `jp2c` payload, or be
`Unsupported` while it carries `pclr`/`cmap`/`cdef`.

**Done:** `file1`, `file4`, `file6` in class; the real-world set decodes equal
to its raw payloads (or is `Unsupported` per the guard); a JPX without `jp2 ` compatibility is `Unsupported`;
openreadout's `jp2_boxes_are_unwrapped` shape (signature + ftyp + jp2c, no
`jp2h`) is `FileFormat`. OpenJPEG 2.5.0 rejects that shape too ("bad placed
jpeg codestream"), and I.5.3 requires `jp2h`, so openreadout's test fixture
needs a `jp2h` with an `ihdr` to decode through `decode`.

### P3.2 — Colour specification and `bpcc` (`src/jp2/`)

**Goal:** `Jp2Metadata.colour` is right for every `colr` form, and ICC bytes
reach the caller untouched.

**Work:** parse `colr` (`METH`, `PREC`, `APPROX`, then `EnumCS` or the ICC
bytes); first box wins, `METH` other than 1 or 2 ignored, `UnkC` not consulted;
check every `bpcc`'s length is `NC` (OpenJPEG does so whatever `BPC` is); record nothing
from it beyond a consistency check against SIZ (SIZ is the truth).

**Oracle:** the box bytes themselves. Assert file8's profile is the 414 bytes
in the box, file5's 546, file7's 13332; files 1 and 9 are `Srgb`, 2 and 3
`Sycc`, 4, 6 and 8 `Greyscale`/`RestrictedIcc`; GDAL's `jpx_disguised_as_jp2`
shape (`METH 3`) comes out `Unknown`. No new conformance entry turns green
here; this milestone is metadata.

**Done:** every fixture's `colour` matches its box dump; `bpcc` length faults
are `FileFormat`.

### P3.3 — Palette expansion: `pclr` + `cmap` (`src/jp2/palette.rs`)

**Goal:** a palettised JP2 decodes to the palette's channels.

**Work:** parse `pclr` (`NE` 1..=1024, `NPC` 1..=255, `Bi` depth/sign per
column, entries at `ceil((Bi+1)/8)` bytes each) and `cmap` (`CMP`, `MTYP`,
`PCOL` per output channel). Validate: `cmap` without `pclr` is `FileFormat`;
`pclr` without `cmap` means no palette (I.5.3.4, both or none; OpenJPEG does
the same); `CMP >= Csiz`, `MTYP > 1`, `PCOL >= NPC`, or `MTYP == 0 &&
PCOL != 0` are `FileFormat`. Apply: for each output channel, `MTYP 0` copies
component `CMP`; `MTYP 1` looks up component `CMP`'s sample in column `PCOL`,
clamping the index to `0..NE` as OpenJPEG does. Output channel depth and sign
come from `Bi`. OpenJPEG's "pcol[i] must equal i" is an implementation
limitation, not a rule of I.5.3.5; we implement the general mapping.

**Oracle:** `file9` (bit-exact). Synthetic fixtures from
`scripts/gen-jp2-fixtures.py` wrapping an existing lossless codestream in
hand-built boxes: a 12-bit palette (two-byte entries), a `cmap` mixing direct
and palette channels, out-of-range indices (clamp), and a `PCOL` permutation
OpenJPEG refuses (graded by table lookup over OpenJPEG's raw index-plane
decode, since the oracle rejects the file). The rest grade against
`opj_decompress` on the `.jp2`.

**Done:** `file9` in class; the synthetic palette fixtures bit-exact;
`raw_channels: true` returns the index plane.

### P3.4 — Channel definitions: `cdef` (`src/jp2/channels.rs`)

**Goal:** colour channels come out in colour order and alpha is labelled.

**Work:** parse `cdef` (`N`, then `Cn`, `Typ`, `Asoc` triples). Validate
against the channel count *after* palette expansion: `Cn` out of range,
`Asoc - 1` out of range, or an incomplete list (a channel with no definition;
OpenJPEG rejects per its issue 397) are `FileFormat`. Apply OpenJPEG's
placement: swap colour channels (`Typ 0`) into `Asoc` order, leave the rest
in place, fill `jp2.channels`. Without a `cdef`, every channel is
`Colour`/`Colour(i+1)`.

**Oracle:** `opj_decompress` on synthetic fixtures: RGBA (opj_compress
writes `cdef` for a PNG with alpha), the file2 reversed layout rebuilt over a
lossless codestream, a `Typ 65535` "unspecified" layout (ECW SDK writes this;
GDAL's `rgb16_ecwsdk.jp2`), and a 1-bit alpha with `bpcc` (GDAL's
`3_13bit_and_1bit.jp2`, 13-bit colour + 1-bit alpha). `file2` itself grades
only after P3.6.

**Done:** every `cdef` fixture matches the oracle's channel order and depth,
and `jp2.channels` matches the box dump.

### P3.5 — Resolution, metadata passthrough, JPX-compatible skipping (`src/jp2/`)

**Goal:** the remaining Annex I boxes are read or skipped on purpose, and a
JP2-compatible JPX decodes.

**Work:** `res ` superbox with `resc`/`resd` (four `u16` and two `i8` each)
into `Resolution`; `xml ` and `uuid` boxes collected verbatim in file order,
including boxes after `jp2c` (file8); `uinf`, `jp2i`, and every JPX box
(`rreq`, `jpch`, `jplh`, `asoc`, `lbl `, `ftbl`) skipped without descending.
Guard: metadata copies are bounded by the input length, and only `jp2h` and
`res ` are descended into, so there is no nesting to bomb.

**Oracle:** box dumps of the real-world set (`resc 10000/254 e0` in GDAL's
`byte_tlm_plt.jp2`, `resd 12901/32768 e4` in `rgbwcmyk01_YeGeo_kakadu.jp2`,
GeoJP2 `uuid` payloads, GML `asoc` trees) and the Part 4 files' `xml ` boxes.

**Done:** metadata matches the dumps; files 5 and 7 (`jpx ` brand, `rreq`,
`jp2 `-compatible) decode; `is_jpx`-style 64-bit `XLBox` on `jp2c` decodes.

### P3.6 — sRGB output: sYCC and restricted ICC (`src/jp2/srgb.rs`)

**Goal:** `ColourOutput::Srgb` reproduces the Part 4 JP2 reader references.

**Work:**
- **sYCC → RGB** (IEC 61966-2-1 Amd 1): offset `Cb`/`Cr` by `2^(depth-1)`,
  `R = Y + 1.402 Cr`, `G = Y − 0.344136 Cb − 0.714136 Cr`, `B = Y + 1.772 Cb`,
  clamp to the depth. Sub-sampled chroma is replicated to the luma grid first
  (OpenJPEG's `sycc420_to_rgb`/`sycc422_to_rgb` rule; the odd-origin edge
  case gets a unit test). Output channels are full resolution.
- **Restricted ICC → sRGB.** Annex I restricts `METH 2` to two ICC v2 input
  profile forms, and both are closed-form: *monochrome* (`kTRC` only) and
  *three-component matrix/TRC* (`rXYZ gXYZ bXYZ` + `rTRC gTRC bTRC`, D50 PCS).
  Parse just those tags (a `curv` of 0 entries is identity, 1 entry is a
  `u8.8` gamma, n entries a table; `para` for v4 parametric curves); linearise;
  matrix to XYZ(D50); multiply by the inverse of the D50-adapted sRGB matrix
  (the one lcms builds, so no separate chromatic adaptation step); encode with
  the sRGB curve; round. For monochrome, `kTRC` then the sRGB curve gives one
  grey channel. Any other profile class or tag set is `Unsupported` (it is not a
  restricted profile, so not a JP2).
- Everything else: `Srgb` and `Greyscale` pass through; `Enumerated(_)` and
  `Unknown` are `Unsupported` under `Srgb`.

**Oracle:** the Part 4 references for files 2, 3, 5, 7, 8 (PAE 4 / MSE 1). The
secondary oracle is `opj_decompress` 2.5.0 with lcms2 (what the apt package
links), which lands within PAE 1 of the references on all five. If we land
within the ISO bound but off OpenJPEG, correctness.md's rule applies: the
conformance image wins, and the ICC arithmetic is documented against ICC.1.

**Done:** all nine Part 4 JP2 entries in class.

### P3.7 — Fuzz target and the reject matrix (`fuzz/`, `src/jp2/tests.rs`)

**Goal:** the box layer is fuzzed from day one and every malformed shape has a
pinned error.

**Work:** a `jp2_boxes` fuzz target that wraps a pinned valid codestream in
fuzzed boxes (the `tile_body` pattern: the fuzzer reaches the box reader
without synthesising a codestream), a `jp2.dict` of box types and the
signature, a seed corpus of the nine Part 4 files plus the synthetic
fixtures; the `decode` target's corpus gains the small real-world files. The
reject matrix in `src/jp2/tests.rs` pins each row of the table below.

**Oracle:** no panic, bounded allocation, every reject a typed `Error`; the
60-second CI smoke stays green.

**Done:** a documented `cargo fuzz` run over `jp2_boxes` and `decode` finds
nothing; the reject matrix passes.

### P3.8 — Integration and the phase gate

**Goal:** everything wired, documented, released.

**Work:** `lib.rs` docs and README stop saying "raw codestream only";
`scope.md` Phase 3 rows ticked; `correctness.md` gains the JP2 corpus and the
"reference is display output" note; `CHANGELOG.md` for 0.4.0; the pre-release
review in `RELEASING.md`.

**Gate (the Phase 3 exit):** all nine ISO/IEC 15444-4 JP2 reader files decode
within PAE 4 / MSE 1 against their references (bit-exact where the manifest
says so), tracked by `IN_CLASS` in `tests/conformance_jp2.rs`; the real-world
set matches `opj_decompress` with the right metadata; the Part 1
corpus and every Phase 1–2 fixture are unchanged; fuzzing clean; the quality
gates green.

## Fixture and corpus plan

| Set | Where | Size | License | Role |
| --- | ----- | ---- | ------- | ---- |
| ISO/IEC 15444-4 JP2 reader files `file1..9.jp2` + references as PGX | `tests/fixtures/conformance/jp2/` | 4.4 MB + ~7.4 MB | the conformance grant already in `tests/fixtures/conformance/COPYRIGHT`; excluded from the crate | the phase gate |
| Synthetic JP2 wrappers from `scripts/gen-jp2-fixtures.py` | `tests/fixtures/jp2_*.jp2` + `.expected.json` | tens of KB | ours (wraps existing Apache-2.0 / public-domain codestreams) | isolate each box: palette variants, cdef variants, bpcc, res, xml/uuid, XLBox, LBox 0, second colr, JPX brand + compat, unknown boxes |
| Real-world encoder zoo from GDAL `autotest/gdrivers/data/jpeg2000/` | `tests/fixtures/jp2_real/` | < 100 KB total | rights unconfirmed (GDAL is "in general" MIT; test data not covered; accepted, see Decisions); nine small files, per-file GDAL path, commit SHA and encoder recorded | structure from Kakadu, ECW SDK, OpenJPEG, GDAL: `cdef` alpha, `Typ 65535`, `bpcc` 13+1-bit, `resc`/`resd`, GeoJP2 `uuid`, GML `asoc`, `UnkC = 1`, signed 16-bit |

Candidates from GDAL (all under 30 KB): `byte.jp2`, `byte_tlm_plt.jp2` (`resc`),
`small_200ppcm.jp2` (`resd`), `rgbwcmyk01_YeGeo_kakadu.jp2` (Kakadu, `resd`),
`rgb16_ecwsdk.jp2` (ECW, `cdef` all-unspecified), `stefan_full_rgba.jp2`
(`cdef` alpha), `3_13bit_and_1bit.jp2` (`bpcc`, 13-bit + 1-bit alpha),
`int16.jp2` (signed), `byte_with_xmp.jp2` (`asoc`/`lbl `/`xml `, two `uuid`).
Their oracle is `opj_decompress` on the `.jp2`, recorded as `.expected.json`
with a new optional `jp2` section (colour, channels, resolution) that
`tests/support/mod.rs`'s `Expected` gains; `deny_unknown_fields` stays.

Not vendored: `openpreserve/jpylyzer-test-files` (per-file licensing is mixed
and several say "license unknown"; useful for a local smoke run only, notably
`palettedImage.jp2` and the Kakadu/Aware/ECW files) and openjpeg-data's
`nonregression/*.jp2` (no license statement; many are crash reproducers that
are good fuzz seeds locally but should not be committed).

The synthetic generator works like `gen-qcc-fixture.py`: take a committed
lossless codestream, build the boxes by hand in Python, run `opj_decompress`
for the oracle, and record the exact command in `provenance`. Every fixture
is lossless so the grade is bit-exact; the only tolerance in the phase is the
Part 4 PAE 4 / MSE 1 on the five conversion files.

## Fuzz and robustness plan

The input is untrusted and the box layer is the first thing it touches. The
rows below are the reject matrix; where OpenJPEG is more lenient the row says
so and why we differ (or do not).

| Input | OpenJPEG 2.5.0 | Ours | Variant |
| ----- | -------------- | ---- | ------- |
| Neither magic | error | reject | `Codestream` ("does not start with the SOC marker") |
| Signature box wrong length or payload | error | reject | `FileFormat` |
| `ftyp` missing or not second | error | reject | `FileFormat` |
| No `jp2 ` in brand or compatibility list | decodes anyway | reject | `Unsupported` (a JPX/JPM we do not read; stricter, because the spec says such a reader may not understand the file and we would be guessing) |
| Box length `< 8` (or `< 16` with `XLBox`) | error | reject | `FileFormat` |
| Box runs past its parent or the file | error | reject | `FileFormat` (truncation) |
| `jp2c` runs past the end of the file | decodes (the codestream is read to its end) | clamped to the file, decodes | none (match oracle); the codestream parser still rejects a codestream that is cut short |
| `jp2c` with `LBox` 2..7 or `XLBox < 16` | decodes (the length is not used) | reject | `FileFormat`; stricter, not yet decided |
| `jp2c` with `LBox >= 8` but shorter than its codestream | decodes (the length is not used; output identical to the raw decode) | reject (the codestream is cut at the box end) | `Codestream` ("Psot overruns the codestream", "truncated marker segment"); stricter, not yet decided |
| `LBox = 0` on a box that is not last | error unless `jp2c` | same | `FileFormat`; legal on the last box, including `jp2c` |
| `XLBox >= 2^32` | error ("cannot handle"), except on `jp2c`, which decodes | accept if it fits the slice; past the end of the file, truncation, or clamped for `jp2c` | `FileFormat` for a box other than `jp2c` past the end of the file; slice-based, nothing is allocated from the length |
| `jp2h` missing | error | reject | `FileFormat` |
| Second `jp2h` before `jp2c` | merged (its `ihdr` ignored, other children applied) | merged the same way | none (match oracle); its `pclr`/`cmap` are applied and its `cdef` meets the guard; each `jp2h` must hold an `ihdr`, as OpenJPEG requires |
| `jp2c` before `jp2h` | error | reject | `FileFormat` |
| `ihdr` missing or not 14 bytes | error | reject | `FileFormat` |
| `ihdr` not first in `jp2h` | decodes | decodes | none (match oracle; I.5.3.1 says first, but nothing depends on the order) |
| `ihdr` W/H `!=` SIZ image area | error | reject | `FileFormat` |
| `ihdr.NC != Csiz` | not checked | reject | `FileFormat`; stricter: palette and `cdef` index against `NC`, so a lie here steers the channel mapping. Relax if a real file trips it. |
| `ihdr.BPC` or `bpcc` disagree with SIZ depth | not checked | accept, SIZ wins | none (match oracle) |
| Second `ihdr` | ignored with warning | ignored | none (match oracle; spec says one) |
| Second `colr` | ignored | ignored | none (I.5.3.3) |
| `colr METH` other than 1 or 2 | box ignored, and it does not count as the first `colr` | the same | none (Table I.9); `METH 0` is skipped the same way |
| `colr` under 3 bytes | error, on every `colr` before the codestream including ignored later ones | reject | `FileFormat`; the same shapes after `jp2c`: see the row on boxes after `jp2c` |
| `colr METH 1` under 7 bytes, or `METH 2` with no profile | error (checked only on the first usable `colr`) | reject | `FileFormat` |
| `colr METH 1` with bytes past `EnumCS`, or a short `METH 2` profile | accepted | accepted | none; `RestrictedIcc` holds whatever follows the 3 header bytes |
| `bpcc` not `NC` bytes | error, on every `bpcc` before the codestream, whatever `ihdr.BPC` is; a `bpcc` before `ihdr` sees `NC` as 0 | reject | `FileFormat`; the same shapes after `jp2c`: see the row on boxes after `jp2c` |
| `ihdr.UnkC = 1` with a `colr` | `UnkC` read, never used | `colr` value reported | none (match oracle; I.5.3.1) |
| No `colr` at all | decodes, colour unknown | decodes, `Unknown` | none (samples are unaffected) |
| `colr METH 2` with too few bytes for a profile header | reads what is there | reject | applies only to the later sRGB conversion: `FileFormat` under `Srgb` only; the colour space and bytes are passed through as they are |
| `pclr NE = 0` or `> 1024`, `NPC = 0`, truncated entries | error | reject | `FileFormat` |
| `cmap` without `pclr`, or before it in file order | error ("Need to read a PCLR box before the CMAP box") | reject | `FileFormat` |
| `cmap` shorter than `4 * NPC` bytes | error | reject | `FileFormat` |
| `cmap` longer than `4 * NPC` bytes | reads `NPC` entries, ignores the rest | same | none (match oracle): the channel count is `NPC` |
| `pclr` without `cmap` | palette dropped | palette dropped | none (I.5.3.4) |
| `cmap CMP >= Csiz`, `MTYP > 1`, `PCOL >= NPC`, `MTYP 0 && PCOL != 0` | error | reject | `FileFormat` |
| `cmap` palette column mapped twice, or palette channel `i` with column `i` unmapped (a direct channel marks column 0) | error | reject | `FileFormat` |
| `cmap` with `MTYP 1 && PCOL != i` | error (implementation limit) | accept | none; spec allows it |
| One component, `cmap` leaves a column unused | rewritten to the identity mapping ("Trying to correct") | same | none (match oracle) |
| Direct channel `i` | labelled with column `i`'s `Bi` | same | none (match oracle) |
| Palette column deeper than the `i32` container (unsigned 32, any > 32) | decodes (> 32 bits misread at 4 bytes per entry) | reject when applied | `Unsupported`, as for such a codestream component |
| Signed palette column | entries not sign-extended | same | none (match oracle; owner question open) |
| Palette index out of range | clamp | clamp | none (match oracle) |
| Second `pclr` / `cmap` / `cdef` | error | reject | `FileFormat` |
| `cdef N = 0`, truncated, `Cn` or `Asoc-1` out of range, incomplete list | error | reject | `FileFormat` |
| `cdef Typ` not 0/1/2/65535 | passed through | `Other(n)` | none |
| Unknown box anywhere | skipped | skipped | none |
| `colr`, `bpcc`, `pclr`, `cmap`, `cdef` at the top level after `jp2h` | applied ("read anyway, we already have jp2h") | recorded as `jp2h` children | none for `pclr`/`cmap` (applied); `Unsupported` for `cdef` until applied, the same guard as inside `jp2h` |
| The same boxes at the top level before `jp2h` | ignored | ignored | none (match oracle) |
| Boxes after `jp2c` | walked after decoding: `LBox` 2..7 or `XLBox < 16` is an error; anything it cannot follow (fewer than 8 bytes, `LBox` 0, a box past the end) ends the walk with a warning; a second `jp2c` ends it; known header boxes are read and can fail | the same length faults rejected, the rest accepted unread | `FileFormat` for the two length faults; a malformed header box after `jp2c` (a second signature, an empty `jp2h`, a short `colr`, a wrong-length `bpcc`, a short `METH 1` or empty `METH 2` with no usable `colr` before it, a `cmap` without `pclr`) fails OpenJPEG but decodes here |
| Second `jp2c` | never read | ignored | none (I.5.4) |
| Deeply nested superboxes | n/a | only `jp2h` and `res ` are entered | no recursion, so no depth bomb |
| Metadata boxes totalling more than the file | impossible by construction | copies bounded by input length | none |

Allocation discipline: no buffer is sized from a box field. Palette tables are
bounded by `NE <= 1024` and `NPC <= 255` from the standard. Palette expansion
allocates `NPC` planes of the index component's size, which is already inside
the Phase 2 sample budget per plane; the total is checked against the same
`Limit` budget before allocation. The codestream stage keeps all its existing
guards, since `jp2c` is handed to it as a slice.

Fuzzing: the `jp2_boxes` target (P3.7) plus the existing `decode` target with
the new seeds; the 60-second CI smoke covers both. A clean documented run is
part of the gate.

## Risks

- **Colour conversion is new ground for this crate.** The restricted ICC
  forms are closed-form and small, but the reference was produced by lcms, and
  rounding or curve interpolation differences could land outside PAE 4 on the
  16-bit file7 (2048-entry curves) once scaled to 8 bits. Mitigation: the
  bound is loose (OpenJPEG+lcms is within 1), and the step is opt-in so a miss
  does not hold the default decode path hostage. If it cannot be earned, the
  gate is stated honestly as 4 of 9 plus the OpenJPEG raw-payload oracle on the
  rest, and P3.6 moves to a follow-up.
- **Reference TIFFs have to be converted.** The PGX references are produced
  by `regenerate.py` from the ISO TIFFs; a conversion bug would be a silent
  mis-grade. Mitigation: the conversion script re-grades OpenJPEG's own
  `-p 8S -force-rgb` output against the converted PGX and asserts the PAE/MSE
  this document records.
- **Corpus size.** About 13 MB more under `tests/fixtures/conformance/`
  (already 56 MB). Acceptable once; if it bothers, the PGX could be stored
  gzip'd with a dev-only inflate, but that is not planned.
- **Behaviour change on `decode`.** A JP2 that used to be `Unsupported` now
  decodes, and its channel count may differ from `Csiz` (palette, `cdef`). A
  consumer that assumed `components.len() == Csiz` after unwrapping the
  codestream themselves is unaffected (they still pass the payload), but one
  that starts passing the `.jp2` gets more channels than before. Documented in
  the changelog and on `Image.jp2`.
- **Stricter-than-oracle rows** (`NC != Csiz`, non-`jp2 `-compatible brand,
  a `jp2c` whose `LBox` is 2..7 or `XLBox` below 16) may refuse real files
  OpenJPEG opens. Each is one `if`; the rule from correctness.md applies: a
  demonstrated real file relaxes the check. Where Annex I is bent in ways
  OpenJPEG tolerates (`ihdr` not first, a second `jp2h`, a `jp2c` past the end
  of the file, a header box misplaced after `jp2h`), the box layer follows
  OpenJPEG (owner decision, 2026-11-01).
- **GDAL sample provenance.** GDAL's license covers its code "in general"
  and says nothing about test data, and some samples are third-party encoder
  output. Committing them anyway is decision 2; the fixture README states the
  uncertainty and each file's origin.

## Out of scope for Phase 3

Rejected cleanly, never attempted: JPX-only features (fragment tables `ftbl`
and fragmented codestreams, compositing layers, `rreq` requirements, multiple
codestreams, any `colr METH 3/4`, Part 2 enumerated colour spaces as anything
but a reported number); applying non-restricted ICC profiles (LUT-based, output
or display classes); CMYK / e-sYCC / CIELab conversion; JPM, MJ2; writing JP2.
HTJ2K stays Phase 4, the encoder Phase 5, Part 2 Phase 6. See
[roadmap.md](roadmap.md) and [scope.md](scope.md).
