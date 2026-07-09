# Phase 2 roadmap — general Part 1 decoder

> **Status: in progress.** Phase 1 shipped the GRIB2 subset in v0.1.0. P2.0, the
> conformance harness, has landed; the decoding milestones have not started. The
> milestones below are the build order, each with the oracle that gates it.
> Milestone numbers track the Phase 2 milestone in GitHub.

The overall [roadmap.md](roadmap.md) sequences the whole project; this file zooms
into Phase 2 alone. It breaks the line item "general Part 1 decoder" into ordered
milestones, each tied to the module it widens, the oracle that proves it, and the
condition that lets the next milestone start.

The feature rows this phase covers are the Phase 2 entries in [scope.md](scope.md).
The gate moves up a level of authority: where Phase 1 graded against a reference
decoder, Phase 2 gates on the **ISO/IEC 15444-4 conformance suite** and its
compliance classes (bounded per-pixel and mean-squared error, not bit-exact for
lossy). That hierarchy is set out in [correctness.md](correctness.md); this file
says in what order to earn the greens.

## What Phase 2 decodes

Phase 1 decodes one tile, one component, one quality layer, LRCP, no precincts,
no ROI: the plain end of Part 1. Phase 2 lifts every one of those restrictions so
the decoder accepts **any conformant Part 1 codestream**, while staying inside the
bare codestream (Annex A). The JP2 file format is still Phase 3.

What comes online:

- multiple **tiles** and tile-parts; multiple **components** with per-component
  subsampling;
- the multiple **component transform**: reversible RCT and irreversible ICT
  (color reconstruction);
- all five **progression orders** plus POC, multiple **quality layers**, and
  **precinct** partition;
- **region of interest** (maxshift) and every **code-block coding style** (bypass,
  reset, restart, vertically causal, predictable termination, segmentation
  symbols);
- the **error-resilience** markers (SOP/EPH), the **packed-header / length**
  markers (PPM/PPT/PLM/PLT/TLM), and the informational **component
  registration** marker (CRG).

The subset boundary moves outward, but the contract is unchanged: a feature this
phase does not yet own is rejected with `Error::Unsupported`, never half-decoded,
and malformed input is a typed `Error`, never a panic.

## Build order and why

The pipeline is unchanged (`codestream → tier2 → tier1 → quant → dwt → image`);
Phase 2 widens each stage rather than adding stages. The build rule from Phase 1
holds: build in the order each addition can be proven against the conformance
oracle before the next is wired on top, so no untested breadth sits on untested
breadth.

The work falls into four tracks that mostly run in parallel and meet at
integration:

- **Structural track** (P2.1 → P2.5): the codestream-and-Tier-2 breadth. Multiple
  components, then per-component overrides, then multiple tiles, then precincts,
  then the remaining progressions and layers. Each step changes how packets are
  enumerated and how coefficients are placed, so it is largely sequential.
- **Tier-1 track** (P2.6): the code-block coding styles. They touch only the block
  decoder and are golden-vector testable in isolation, so this runs alongside the
  structural track, the way the reconstruction track ran beside the entropy track
  in Phase 1.
- **Reconstruction track** (P2.8, P2.9): the multiple component transform and
  ROI. MCT needs multiple components (P2.1); ROI is per-component dequant
  scaling and needs only the harness, so the two are independent of each
  other. Both are checkable on synthetic coefficient input before the
  structural track is finished.
- **Marker plumbing** (P2.7): the packed-header, length, resilience, and
  registration markers. Mostly parsing breadth the other tracks can lean on
  but do not block on (CRG's per-component record is the one piece that waits
  on P2.1's component geometry).

```text
P2.0 harness ─┬─ P2.1 multi-component ─ P2.2 COC/QCC ─ P2.3 tiles ─ P2.4 precincts ─ P2.5 progressions/POC/layers ─┐
              ├─ P2.6 code-block coding styles (Tier-1) ───────────────────────────────────────────────────────────┤
              ├─ P2.7 PPM/PPT/PLM/PLT/TLM + SOP/EPH + CRG markers ─────────────────────────────────────────────────┼─ P2.10 integrate ─ ISO 15444-4 gate
              ├─ P2.8 MCT: RCT/ICT (needs P2.1) ───────────────────────────────────────────────────────────────────┤
              └─ P2.9 ROI maxshift ────────────────────────────────────────────────────────────────────────────────┘
```

## Milestones

### P2.0 — Conformance harness (ISO/IEC 15444-4)

**Goal:** the Part 4 grading scaffold exists before the features it grades, the
way P1.0 stood up the oracle harness before the decoder.

**Work:** the corpus itself is already vendored (`tests/fixtures/conformance/`):
`manifest.json` records each entry's provenance, compliance class, per-component
PAE/MSE bounds, `bit_exact` flag, and feature tags (markers, coding-style bits,
progression, geometry). Build the harness that consumes it: decode each entry
and grade by **compliance class** — bounded per-pixel maximum error and bounded
mean-squared error against the reference, not bit-exact for lossy. Grade
exactness off each entry's `bit_exact` flag, never off its wavelet: p0_09 is
9/7 yet graded bit-exact. This milestone also settles the multi-component
`Image` representation — the class-1 references are one `.pgx` per component,
so the harness fixes the output shape the rest of Phase 2 threads toward (a
breaking API change; Phase 2 releases as 0.3).

**Oracle:** the Part 4 suite is itself the authority here; this milestone is the
machinery that consumes it.

**Done** (landed): `tests/conformance_part4.rs` grades all 23 entries at class 1.
Twenty-two report *not yet decoded* — the features they need are still rejected
as `Error::Unsupported` — and `p0_09` decodes and matches its reference exactly.
`Image` now carries the reference-grid image area and a `Component` per
codestream component. Unblocks the gate for every milestone below.

### P2.1 — Multiple components and subsampling (`src/codestream/`, `src/image.rs`)

**Goal:** decode `Csiz > 1` codestreams with independent per-component geometry,
the foundation the transform and progression work builds on.

**Work:** read per-component depth, sign, and subsampling (`Ssiz`, `XRsiz`,
`YRsiz`) from SIZ; carry a component dimension through Tier-2 packet enumeration,
Tier-1, dequant, and the inverse DWT; assemble the components into the output.
Components stay independent at this milestone (no inter-component transform yet).

**Oracle:** `opj_dump` for the parsed per-component geometry; `opj_decompress`
raw per-component output for the reconstruction, plus the matching Part 4
multi-component codestreams.

**Done** (landed): every declared component reconstructs on its own grid, with
its own depth, sign, and sub-sampling. Graded against synthetic OpenJPEG-encoded
fixtures, bit-exact: the Part 4 corpus cannot grade this milestone on its own,
because every one of its multi-component entries also needs a progression, a
layer count, a tile grid, an image origin, or the colour transform that is not
decoded yet. `p0_14` is closest and still needs RCT. Unblocks COC/QCC, MCT, and
the progression orders that iterate over components.

### P2.2 — Per-component coding/quant overrides (COC/QCC) (`src/codestream/`)

**Goal:** honor per-component overrides layered over the COD/QCD main defaults.

**Work:** parse COC (Annex A.6.2) and QCC (A.6.5); resolve the effective coding
style and quantization for each component as override-else-default; thread the
resolved per-component parameters into Tier-1 and dequant.

**Oracle:** `opj_dump` of the resolved main and tile headers; a Part 4 codestream
that sets a component-specific style or step size.

**Done:** overrides resolve correctly and the affected components decode in class.

### P2.3 — Multiple tiles and tile-parts (`src/codestream/`, `src/image.rs`)

**Goal:** decode a tiled canvas with one or more tile-parts per tile.

**Work:** read the tile grid (`XTsiz`/`YTsiz`/`XTOsiz`/`YTOsiz`) from SIZ; parse
multiple SOT tile-parts, including the tile-part index and count (`TPsot`/`TNsot`,
Annex A.4.2) and per-tile-part COD/QCD overrides; decode each tile in its own
coordinate frame and stitch the tiles into the canvas with correct offsets.

**Oracle:** `opj_dump` tile-part structure; a multi-tile Part 4 codestream
compared against its reference decode.

**Done:** multi-tile, multi-tile-part codestreams reconstruct the full canvas in
class. The Phase 1 single-tile guard in SIZ validation comes off here.

### P2.4 — Precinct partition (`src/tier2/`)

**Goal:** decode non-maximal precincts, the packet-grouping subdivision Phase 1
skipped by assuming one precinct per subband.

**Work:** read precinct sizes from COD (Annex B.6); partition each
resolution/subband into precincts; group code-blocks by precinct; run one
tag-tree per precinct per subband in the packet header parse. The incremental
tag-tree reader already exists; this generalizes how many of them there are and
which code-blocks each covers.

**Oracle:** a precinct-partitioned Part 4 codestream; `opj_dump` precinct
geometry.

**Done:** precinct-partitioned codestreams split into the correct packets and
decode in class.

### P2.5 — Progression orders, POC, and multiple quality layers (`src/tier2/`)

**Goal:** enumerate packets in any progression order, follow progression-order
changes, and accumulate multiple quality layers.

**Work:** the four remaining progression orders (RLCP, RPCL, PCRL, CPRL) over the
layer/resolution/component/precinct nesting (Annex B.12); the POC marker
(A.6.6) for progression-order changes mid-codestream; multiple-layer
accumulation, where each code-block's contributions add up across layers
(B.10). With P2.4 done, the packet iterator now ranges over all five axes.

**Oracle:** Part 4 codestreams exercising each progression order, a POC change,
and multiple layers, each against its reference decode — but note no corpus entry
is unblocked by layers alone, and `p0_01` cannot distinguish one progression
order from another (see [correctness.md](correctness.md) §A passing entry is not
proof the feature works). Both need synthetic OpenJPEG fixtures.

**Done:** multiple quality layers have landed (`scripts/gen-multilayer-fixtures.py`
grades them bit-exact). The progression orders and POC remain; they now have the
multi-layer fixture they need to be told apart from LRCP.

### P2.6 — Code-block coding styles (`src/tier1/`)

**Goal:** decode every code-block style flag, not just the Phase 1 default.

**Work:** read the SPcod/SPcoc style byte and decode the optional modes:
selective arithmetic-coding bypass (lazy, Annex D.5), reset of context
probabilities on each pass, termination on each coding pass (restart),
vertically causal context, predictable termination, and segmentation symbols.
Each changes MQ termination, context handling, or pass boundaries inside the
block decoder; the surrounding pipeline is untouched.

**Oracle:** per-style golden code-block vectors (sliced from codestreams encoded
with each style), plus Part 4 codestreams that set the corresponding flags. This
track is golden-vector testable in isolation, so it runs alongside P2.1–P2.5.

**Done:** each coding style decodes its golden vectors and the matching
conformance codestreams in class.

### P2.7 — Packed-header, length, resilience, and registration markers (`src/codestream/`, `src/tier2/`)

**Goal:** parse the markers that move or annotate packet data so codestreams that
carry them decode, whether or not the hints are used.

**Work:** PPM/PPT (packed packet headers, Annex A.7.4/A.7.5) — read packet
headers from the marker segment instead of inline; PLM/PLT and TLM (packet- and
tile-part-length markers, A.7.1–A.7.3) — parse and optionally use the lengths;
SOP/EPH (start-of-packet, end-of-packet-header, A.8.1/A.8.2) — recognize and
resynchronize on the packet delimiters; CRG (component registration, A.9.1) —
parse and record the sub-pixel registration offsets (informational, no
resampling; the per-component record is the one piece that waits on P2.1's
component geometry). Decoding must match with the lengths used and with them
ignored.

**Oracle:** Part 4 codestreams carrying each marker — the manifest's
`markers_main`/`markers_tile` fields say which entry carries what — except PLM,
which no conformance entry carries (asserted by
`tests/conformance_corpus.rs`): grade PLM against a synthetic fixture instead.
Assert the decode matches the reference both using and ignoring the length
hints.

**Done:** codestreams with packed-header, length, resilience, and registration
markers decode in class; PLM parses against its synthetic fixture.

### P2.8 — Multiple component transform: RCT and ICT (`src/image.rs` or `src/mct.rs`)

**Goal:** reconstruct color by inverting the inter-component transform when COD
signals it.

**Work:** inverse reversible color transform (RCT, integer, Annex G.2) and
inverse irreversible color transform (ICT, float YCbCr→RGB, G.3), applied to the
first three components when COD signals the transform. Note the wavelet picks
which one: 5/3 means RCT, 9/7 means ICT — there is no separate flag. Depends on
P2.1 (multiple components).

**Oracle:** three-component color Part 4 codestreams against their reference
decodes; RCT bit-exact (reversible), ICT within the compliance-class bounds.

**Done:** RCT has landed (`src/mct.rs`); `p0_14` decodes bit-exact, taking the
gate to 2/23. ICT remains: it needs the inverse DWT to hand back the 9/7 path's
floats rather than rounding to `i32` first, since rounding before the transform
loses the precision the compliance-class bounds assume.

### P2.9 — Region of interest, maxshift (`src/quant.rs`, `src/codestream/`)

**Goal:** invert the maxshift ROI scaling so ROI-coded codestreams decode.

**Work:** parse the RGN marker (Annex A.6.3); undo the maxshift up-scaling of ROI
coefficients (Annex H) before dequantization, so foreground and background
coefficients return to a common scale. Slots into the existing dequant stage;
depends only on the harness (P2.0), not on MCT — maxshift is per-component
scaling.

**Oracle:** Part 4 codestreams with an ROI against their reference decodes.

**Done:** maxshift ROI codestreams decode in class.

### P2.10 — Integration and the phase gate

**Goal:** wire every track together and pass the Part 4 conformance gate.

**Work:** integrate the structural, Tier-1, reconstruction, and marker tracks in
`decode()`; resolve the seams the combinations reveal (per-tile per-component
overrides interacting with precincts and progressions, MCT after multi-tile
assembly, ROI under each coding style); keep the subset-boundary rejects sharp for
whatever remains Phase 3+.

**Gate (this is the Phase 2 exit):** the decoder passes the ISO/IEC 15444-4 Part 1
conformance codestreams within their compliance-class per-pixel maximum-error and
mean-squared-error bounds; fuzzing is clean — the 60-second CI smoke stays green
and a documented one-hour local `cargo fuzz` run over `decode`, seeded with the
conformance corpus, finds nothing (no panics, allocations bounded by the declared
image area, every reject a typed `Error`); and the quality
gates are green: `cargo fmt --all -- --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo deny check`.

## Sequencing notes

- **Critical path** is the structural track P2.1 → P2.5, because each step changes
  how packets are enumerated and coefficients are placed, and the later steps read
  the geometry the earlier ones establish. Start P2.6 (code-block styles), P2.9
  (ROI), and P2.8 (MCT, once P2.1 lands) in parallel to keep the critical path
  busy.
- **Highest-risk milestones** are P2.5 (the five-axis packet iterator with POC and
  layers) and P2.6 (the optional MQ modes, especially bypass and predictable
  termination). Both have small isolable vectors — a hand-built packet sequence,
  a per-style golden block — so pin those down before the surrounding code.
- **Reject path stays sharp.** Every milestone that widens the subset also moves
  the `Error::Unsupported` boundary outward in lockstep, so a Phase 3 feature
  (JP2 boxes) is still rejected cleanly the moment it appears, never half-parsed.

## Out of scope for Phase 2

Everything below is Phase 3+ and must be rejected cleanly, not attempted: the JP2
file format and its boxes (Phase 3), HTJ2K / the FBCOT block decoder (Phase 4),
the Part 1 encoder (Phase 5), and the Part 2 extensions (Phase 6). See
[roadmap.md](roadmap.md) for the phase sequence and [scope.md](scope.md) for the
full feature mapping.
