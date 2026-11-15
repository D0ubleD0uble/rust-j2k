# Incremental decoding

`IncrementalDecoder` decodes a file while it arrives. The caller feeds bytes
with `feed`, asks for an image of what has come in with `refine`, and calls
`finish` once the input ends. This note says what each call guarantees, how
the work is bounded, and what a later, more stateful version would change.

## What the calls guarantee

- Every image `refine` returns is the image `decode_with` gives with
  `strict` off on the bytes fed so far (`bytes_decoded()` of them), report
  included. The report says which quality layers and resolutions are
  complete and which tiles are missing.
- `refine` returns `Ok(None)` while the main header is cut short, where
  `decode_with` gives `Error::Incomplete`. Any other error is the one
  `decode_with` gives on those bytes.
- `finish` is `decode_with` on all the bytes under the decoder's options,
  `strict` included, except that `Error::Incomplete` becomes a truncation
  error: `Error::FileFormat` when a JP2 file ends inside its boxes before
  the codestream, `Error::Codestream` when the input ends inside the main
  header. With `strict` on, a damaged segment the prefixes let pass, such as
  a PTERM segment with bytes left over in a block that could still take
  passes, is refused here.

The correctness oracle is the crate's own one-shot strict-off decode of the
same prefix. `tests/incremental.rs` checks it on every fixture under
`tests/fixtures/`, fed in pieces of random sizes and cut at many points,
including the MIXED HT files and the PTERM, POC and multi-tile files. To
keep its time down, it feeds only the first 96 KB of a file, decodes a large
image at a resolution reduction, and feeds the files made to reach the work
bounds in two halves without cutting them. That
one-shot decode is not the same as OpenJPEG's `-allow-partial` on every
prefix: on `pterm_cut.j2k` and `bypass_pterm.j2k` with no layer cap, 1954 of
2588 prefixes differ, while all of them match at `-l 1`
(`tests/fixtures/layers/prefixes.sha256`).

## When `refine` decodes

`refine` decodes the whole buffer each time it decodes, and it decodes only
once the bytes fed have at least doubled since its last attempt. A file of
`n` bytes is decoded about `log2(n)` times, on prefixes that double, so the
total work is under twice one decode of the whole file. The schedule does not
look at the codestream, so a refine does not wait for a layer or a resolution
to complete; the report of each image says how far the data reached.

## Work and memory

The work bounds of `decode` hold across all the refines together. A ledger
keeps the packet visits and code-block work every refine has spent, and each
refine's bounds are what is left of the bounds one decode of its bytes has
(raised as the options say). A refine that runs out returns `Ok(None)` and
waits for more bytes, which raise the bound. `finish` is one more decode,
under its own bound. A file whose packet walk costs about the same however
few of its bytes are there, such as a positional sweep over a large grid, can
spend most of the bound on its first refine; its later refines then wait, and
only `finish` decodes it. The unit test
`a_byte_at_a_time_feeder_stays_within_one_decodes_bounds` feeds files one
byte at a time, refines after each byte, and checks both totals against one
decode's bounds.

Each decode holds what `decode_with` holds for the same bytes. Between calls
the decoder keeps only the bytes fed (in a `Vec`, so up to twice their length
in capacity) and a few counters; the images go to the caller.

Two costs are outside the ledger, as they are outside one decode's work
bounds: parsing the headers and walking the tile-part headers, both linear in
what they read, so also under twice one decode in total; and, for a JP2 file
whose last tile-part has `Psot = 0`, the walk that finds where its packets end
(see [The `read_header` walk](resource-bounds.md#the-read_header-walk)).

## What a stateful decoder would add

Decoding each prefix again keeps the result equal to the one-shot decode by
construction, at the cost of redoing work. A decoder that keeps parse state
between feeds would decode only what each feed adds, and could refine after
every packet. It needs:

- A resumable main-header and tile-part walk, so the `TLM`, `PLM` and `PPM`
  segments and the tile-part index are parsed once.
- A resumable packet walk per tile: the progression position as a cursor, the
  per-block state (inclusion, `Lblock`, zero bit-planes, passes, segments and
  the MIXED `coder` each block picked), and the per-packet undo log, kept
  across feeds instead of rebuilt. Coded segments would hold offsets into the
  growing tile data rather than borrowed slices.
- The tier-2 `unread` mark split in two: packets a `POC` sequence never
  reaches, which no later byte changes, and packets past the end of the data
  so far, which the next feed may bring.
- A tier-1 cache that decodes again only the code-blocks a feed changed.

Each refine would still have to give the one-shot strict-off image of its
prefix, so this suite applies unchanged.
