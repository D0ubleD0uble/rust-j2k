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
  It also returns `Ok(None)` when the packet or code-block work bound, the
  charge for missing tiles, or the bound on the walks of tiles not kept
  (see [Work and memory](#work-and-memory)) refuses the decode: those
  bounds grow with the bytes and the missing tiles shrink, so a work limit
  is reported only by `finish`. Any other error, including a limit that does not depend on the
  bytes (the sample limit, too many precincts, code-blocks or `POC` volumes,
  too many bit-planes), is the one `decode_with` gives on those bytes. Once
  `refine` returns an error, every later `refine` returns the same error
  without decoding.
- A refine tries to decode when the bytes fed are at least twice those of
  the last refine that decoded or tried to. Before any try, the first refine
  called with at least one byte fed tries; with none fed, `refine` returns
  `Ok(None)` and does not count as a try. A refine that tries gives an image
  when the one-shot decode of the bytes fed gives one, except when the
  earlier refines have spent the code-block bound, or the bound on the
  walks of tiles not kept (see below). A try that
  gave no image still counts: after a main header cut short, or a refusal
  by a work bound or the charge for missing tiles, the next refine waits for
  twice the bytes of that try, even if the one-shot decode of fewer bytes
  gives an image by then
  (`a_refused_refine_puts_off_the_next_until_the_bytes_double`). Doubling
  from the last try is what bounds the work of all the refines together.
- `finish` is `decode_with` on all the bytes under the decoder's options,
  `strict` included, except that `Error::Incomplete` becomes a truncation
  error: `Error::FileFormat` when a JP2 file ends inside its boxes before
  the codestream, `Error::Codestream` when the input ends inside the main
  header. With `strict` on, a damaged segment the prefixes let pass, such as
  a PTERM segment with bytes left over in a block that could still take
  passes, is refused here. `finish` decodes the bytes also after a `refine`
  error, so with `strict` on it may report an earlier fault than the one
  `refine` found, such as the data ending before it. `finish` borrows the
  decoder and can be called again, with the same result. `into_bytes`
  consumes the decoder and returns the bytes fed, so a caller whose `finish`
  refused a work limit can decode them again with the limit raised.

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
decoded or tried to, and may decode more often. At present it tries only at
those doublings, so a file of `n` bytes gives at most about `log2(n)` images,
on prefixes that double. The schedule does not look at the codestream, so a
refine does not wait for a layer or a resolution to complete; the report of
each image says how far the data reached.

## What the decoder keeps

Each tile's packet walk is kept between refines, in `tier2::resume`. A refine
goes on from where the last one stopped, and reads only the packets that
have come in since. The walk keeps:

- the tile's geometry, and the state of its code-blocks and tag trees, which
  carry from one layer's packets to the next;
- its place in the progression order, as a cursor that steps through the
  same order the one-shot walk takes (`progression::Order`); a packet a
  `POC` volume emits again is told by the count of its precinct's layers
  read;
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
data, a visit for each packet after it, and the skips past a `POC` bitmap. The
charges come from the counts the kept walks hold, and a refine is refused,
as `Ok(None)`, where that decode is refused for work, and fails where it
fails. So the packet bound is the one-shot bound of the bytes fed, not cut
by what earlier refines spent. A `POC` sequence's volumes emit each
precinct's layers from the first up, so a kept walk tells a packet emitted
again by the precinct's count of layers read; its charges are those of
the bitmap or counts that decode holds. The work the kept
walks do is the visits they spend for the first time: a byte-at-a-time
feeder that refines after every byte spends, over all its refines, the
visits of one walk of the whole file
(`a_byte_at_a_time_feeder_stays_within_one_decodes_bounds`). That count
is of distinct visits. The walks can step through each point of the order
twice, once to read and once to count, a packet the data cuts is parsed
again on each refine, and a tile whose walk is not kept (see below) is
walked from the start on each refine, as a one-shot decode walks it. So a
refine is not cheaper than a decode of the same bytes, and on a file of many
components under `POC` volumes it costs more: one refine of a whole file of
200 tiles of 16384 components under 32 `POC` volumes takes 10.4 s, where one
decode takes 6.9 s (release build, 2026-11-16, PR #409's shapes).

The code-block work is done again by each refine that decodes. A ledger
keeps the code-block work every refine has spent, and each refine's
code-block bound is what is left of the bound one decode of its bytes has
(raised as the options say). A refine that bound refuses returns `Ok(None)`
and waits for more bytes, which raise the bound. `finish` is one more
decode, under its own bound, and reports a work limit as one decode of the
whole file does.

A tile whose walk is not kept (see below) is walked from the start by each
refine, so its walk is done again as the code-block work is. The ledger
keeps the packet visits these walks have spent over all the refines. Once
that is past four times the packet bound of one decode of the bytes fed
(`REPEAT_BOUNDS` in `src/tier2/resume.rs`), a refine that comes to such a
tile is refused for work before it walks it, and returns `Ok(None)`. The
refine whose walk takes the count past that bound is refused after the
walk. So these walks spend at most five packet bounds over all the refines:
four, and the one walk that goes past them, which one decode's bound holds.
The bound follows the packet work limit: raised by `WorkLimit::Raised`, none
under `WorkLimit::Unlimited`. Of the committed fixtures, fed in quarters, in
pieces of random sizes and one byte at a time, at no reduction and at a
reduction of 1, normal files spend at most 0.008 of a bound on these walks,
and files made to reach the work bounds at most 3 bounds, so none of them
gives another result. On a file of 525 KB of 256 components and 65535
layers, made for this
([#416](https://github.com/D0ubleD0uble/rust-j2k/issues/416)), fed one byte
at a time with a refine after each, the refines took 8.7 times one decode
before the bound and take 4.7 times with it
(`the_walks_of_a_tile_not_kept_stay_within_a_few_bounds` checks a smaller
copy of it).

Between calls the decoder keeps the bytes fed (in a `Vec`, so up to twice
their length in capacity) and the tiles' packet walks: their geometry, the
state of their code-blocks and the `POC` gaps, and, until a walk has read
all of its tile's packets, its progression order, tag trees, counts and
buffers. One decode holds at its peak the samples of its image and the
packet walk of the tile it is reading. The walks kept hold at most twice
that: twice the image's samples (4 bytes each) plus twice the walk of the
largest tile walked so far. They also hold at most the larger of 1 MiB and
32 bytes per byte fed, so that the memory held between calls follows the
input as well as the image the file declares. Each walk's size counts every
heap allocation it holds, capacity included. A tile whose walk would pass
either bound is not kept, and each refine walks its packets from the start,
until the bytes fed make room for it. Of the committed fixtures fed 1 KB at
a time, this keeps fewer walks than the image bound alone only for files
made to test the work bounds, and for a 6000 x 4000 RGB file of 148 KB until
64 KB of it are in. On top of these bounds the decoder keeps a table of 24
bytes for each tile index up to the highest it has seen (`size_of::<Slot>()`
in `src/tier2/resume.rs`). That is about 1.5 MiB for the 65535 tiles a file
can have, and up to twice that in capacity as the table grows: an 82-byte
file of 1 x 1 tiles whose one tile-part is tile 65534 holds 1.57 MB between
calls. Each refine holds what `decode_with` holds for the same bytes on top
of the walks kept, so the decoder holds at most about three times one
decode's peak, plus the bytes fed. `tests/incremental_alloc.rs` counts the
bytes held on files of many one-sample tiles of 512 components, in LRCP
order and under 32 PCRL `POC` volumes, fed in pieces, and
`tests/incremental_kept_alloc.rs` on a file of 1228 bytes that declares a
2048 x 2048 image of 4 x 4 code-blocks. The decoder is `Send` and `Sync`.
The ledger's counters live only for the length of a decode.

A file of 1228 bytes that declares a 16384 x 16384 image, in 64 tiles of
4 x 4 code-blocks with every packet empty, holds no walk between calls: each
tile's walk is about 10 MiB, past the 1 MiB the bytes give. A refine of it
holds what one decode holds, 1076 MiB at its peak, and nothing is kept after
it but the bytes. Each of its refines walks every tile from the start, and
those walks are charged to the bound on the walks of tiles not kept.

Three costs are outside the bounds, as they are outside one decode's work
bounds. Each refine that returns an image builds the whole image,
as one decode does: it decodes every code-block again, allocates the samples
and runs the inverse wavelet and color transforms over them, whatever share
of the bytes is in. That is one image reconstruction per image, so at most
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
