# Incremental decoding

`IncrementalDecoder` decodes a file while it arrives. The caller feeds bytes
with `feed`, asks for an image of what has come in with `refine`, and calls
`finish` once the input ends. This note says what each call guarantees, how
the work is bounded, what the decoder keeps between calls, and what it still
does again on each refine.

## What the calls guarantee

- Every image `refine` returns is the image `decode_with` gives with
  `strict` off on the bytes fed so far (`bytes_decoded()` of them), report
  included. The report says which quality layers and resolutions are
  complete and which tiles are missing.
- `refine` returns `Ok(None)` where `decode_with` gives `Error::Incomplete`
  (the main header is cut short), since more bytes can complete the header.
  It also returns `Ok(None)` when the packet or code-block work bound, or the
  charge for missing tiles, refuses the decode: those bounds grow with the
  bytes and the missing tiles shrink, so a work limit is reported only by
  `finish`. Any other error, including a limit that does not depend on the
  bytes (the sample limit, too many precincts, code-blocks or `POC` volumes,
  too many bit-planes), is the one `decode_with` gives on those bytes. Once
  `refine` returns an error, every later `refine` returns the same error
  without decoding.
- A refine gives an image when one is due: once the bytes fed are twice those
  of the last image, or, before the first, twice those the header needs, and
  the one-shot decode of the bytes fed gives an image. The exception is a
  refine for which the earlier refines have spent the work bound (see
  below).
- `finish` is `decode_with` on all the bytes under the decoder's options,
  `strict` included, except that `Error::Incomplete` becomes a truncation
  error: `Error::FileFormat` when a JP2 file ends inside its boxes before
  the codestream, `Error::Codestream` when the input ends inside the main
  header. With `strict` on, a damaged segment the prefixes let pass, such as
  a PTERM segment with bytes left over in a block that could still take
  passes, is refused here. `finish` decodes the bytes also after a `refine`
  error, so with `strict` on it may report an earlier fault than the one
  `refine` found, such as the data ending before it.

The correctness oracle is the crate's own one-shot strict-off decode of the
same prefix. `tests/incremental.rs` checks it on every fixture under
`tests/fixtures/`, fed in pieces of random sizes and cut at many points,
including the MIXED HT files and the PTERM, POC and multi-tile files, and
checks that each refine that is due gives an image. To keep its time down, it
feeds only the first 96 KB of a file, decodes a large image at a resolution
reduction, and feeds the files made to reach the work bounds in two halves
without cutting them. That
one-shot decode is not the same as OpenJPEG's `-allow-partial` on every
prefix: on `pterm_cut.j2k` and `bypass_pterm.j2k` with no layer cap, 1954 of
2588 prefixes differ, while all of them match at `-l 1`
(`tests/fixtures/layers/prefixes.sha256`).

The unit tests in `src/incremental/tests.rs` check the kept walks against the
same oracle: they refine after every byte of files with `SOP` and `EPH`,
`PPM` and `PPT`, `PLT` and `TLM`, and `POC` volumes in the main and
tile-part headers, and after pieces of several sizes of multi-tile files and
the MIXED HT files, under a reduction and a layer cap. The fuzz target
`incremental_decode` feeds its input in pieces a hash of it picks and checks
each refine that decodes against the same oracle.

## When `refine` decodes

`refine` decodes at least each time the bytes fed have doubled since it last
decoded, and may decode more often. At present it decodes only at those
doublings, so a file of `n` bytes gives about `log2(n)` images, on prefixes
that double. The schedule does not
look at the codestream, so a refine does not wait for a layer or a resolution
to complete; the report of each image says how far the data reached.

## What the decoder keeps

Each tile's packet walk is kept between refines, in `tier2::resume`. A refine
goes on from where the last one stopped, and reads only the packets that
have come in since. The walk keeps:

- the tile's geometry, and the state of its code-blocks and tag trees, which
  carry from one layer's packets to the next;
- its place in the progression order, as a cursor that steps through the
  same order the one-shot walk takes (`progression::Order`), and the set of
  packets the `POC` volumes have already emitted;
- where it is in the packet headers and the packet bodies, and the packet
  number an `SOP` marker must carry;
- how many packets of each precinct it has read.

Each coded segment holds the offsets of its bytes in the tile data, not
slices of it, so the state outlives the bytes it was read from and goes on
over a longer copy of them. The image of a refine is built from the state by
resolving the offsets against the tile data of that refine. The tier-1
scratch is made per decode, as in a one-shot decode; nothing tier-1 is kept.

A packet the data cuts is read as far as the data goes, for the image of
that refine: its whole code-blocks are kept, as the one-shot decode keeps
them. After the image is built, the packet is put back: the code-block undo
log, a log of the tag-tree nodes it changed, and the stream positions undo
it, and the next refine reads it again from its start. A packet read whole is
kept, except with `SOP` markers when it starts with fewer than two bytes of
the data left. It may yet have an `SOP` before it once more data comes:
with headers inline, the one byte read as its header may be the first byte
of that marker, and with packed headers (`PPM` or `PPT`) the header is read
whole from elsewhere while the marker is still to come. Such a packet, and
every packet after it, is read for the image and then put back the same
way.

Two kinds of packet are not read. The packets a `POC` sequence never reaches
are found once, when the walk starts, and stay missing however much data
comes. The packets past the end of the data so far are counted by a second
walk of the order, which reads no data and runs once per tile: for each
precinct, how many packets the order emits. A precinct with fewer packets
read than the order emits is missing from its first packet not read, which
is what the one-shot walk notes, since the order emits a precinct's layers
in order. The report of each refine is built from the two.

A tile whose later tile-part adds `POC` volumes, or packed headers where it
had none, has another order of packets or reads them from elsewhere, so its
walk starts again.

## Work and memory

Each refine is held to the packet bound of a decode of its bytes on its own.
The code-block bound holds across all the refines together.

Each refine is charged the packet visits the walk of a decode of its bytes on
its own spends: the geometry of the resolutions a reduction drops, the
progression's loops, the visits each packet read spends, the end of the
data, a visit for each packet after it, and the skips from a `POC` set. The
charges come from the counts the kept walks hold, and a refine is refused,
as `Ok(None)`, where that decode is refused for work, and fails where it
fails. So the packet bound is the one-shot bound of the bytes fed, not cut
by what earlier refines spent. A `POC` sequence's volumes emit each
precinct's layers from the first up, so a kept walk tells a packet emitted
again by the precinct's count of layers read, with no packet set; its
charges are those of the set that decode holds. The work the kept walks do is the visits
they spend for the first time: a byte-at-a-time feeder that refines after
every byte spends, over all its refines, the visits of one walk of the whole
file (`a_byte_at_a_time_feeder_stays_within_one_decodes_bounds`). That count
is of distinct visits. The walks can step through each point of the order
twice, once to read and once to count, a packet the data cuts is parsed
again on each refine, and a tile whose walk is not kept (see below) is
walked from the start on each refine, as a one-shot decode walks it.

The code-block work is done again by each refine that decodes. A ledger
keeps the code-block work every refine has spent, and each refine's
code-block bound is what is left of the bound one decode of its bytes has
(raised as the options say). A refine that bound refuses returns `Ok(None)`
and waits for more bytes, which raise the bound. `finish` is one more
decode, under its own bound, and reports a work limit as one decode of the
whole file does.

Between calls the decoder keeps the bytes fed (in a `Vec`, so up to twice
their length in capacity) and the tiles' packet walks: their geometry, the
state of their code-blocks, and, until a walk has read all of its tile's
packets, its tag trees and counts. One decode holds at its peak the samples
of its image and the packet state of the tile it is reading. The walks kept
hold at most twice that: twice the image's samples (4 bytes each) plus
twice the packet state of the largest tile walked so far. A tile whose walk
would pass that is not kept, and each refine walks its packets from the
start. Each refine holds what `decode_with` holds for the same bytes on top
of the walks kept, so the decoder holds at most about three times one
decode's peak, plus the bytes fed. The decoder is `Send` and `Sync`. The
ledger's counters live only for the length of a decode.

Three costs are outside the bounds, as they are outside one decode's work
bounds. Each refine that returns an image builds the whole image, as one
decode does: it decodes every code-block again, allocates the samples and
runs the inverse wavelet and color transforms over them, whatever share of
the bytes is in. That is one image reconstruction per image, so at most
about `log2(n)` of them under the doubling cadence. Each refine also parses
the main header and walks the tile-part headers again; these are linear in
the headers, and take about 30 µs for the 16 KB of `TLM` entries of
`tile_header/tlm_32_16k.j2k`. And for a JP2 file whose last tile-part has
`Psot = 0`, each refine makes the walk that finds where its packets end (see
[The `read_header` walk](resource-bounds.md#the-read_header-walk)).

## What a later decoder could add

- A tier-1 cache that decodes again only the code-blocks a feed changed,
  which would let a refine come after every packet at little cost.
- A main-header and tile-part walk kept across feeds, so the `TLM`, `PLM`
  and `PPM` segments and the tile-part index are parsed once.

Each refine would still have to give the one-shot strict-off image of its
prefix, so the tests apply unchanged.
