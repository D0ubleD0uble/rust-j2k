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
- `refine` returns `Ok(Refined::Image(image))` or, when it has no new image,
  `Ok(Refined::Waiting(why))`. `Refined::image()` gives the image or `None`
  for a caller who does not need the reason. `Waiting` says why:

  | `Waiting` | Why the refine has no image |
  | --- | --- |
  | `MoreBytes { until, .. }` | It did not decode: the bytes fed have not doubled since the last try, or none are fed. `until` is a total of bytes fed, not the bytes still to come: a refine decodes once that many are fed, if not before. |
  | `HeaderIncomplete` | `decode_with` gives `Error::Incomplete`: the main header is cut short. |
  | `SampleLimit(limit)` | The default or a `Raised` sample limit refused the image. |
  | `WorkLimit(limit)` | The bytes fed cost more than the packet or code-block work bound allows; a decode of them on its own is refused too. |
  | `BoundSpent(limit)` | The earlier refines spent the code-block bound the refines share. This refine's own work fits the whole bound, so decode the bytes fed now or start a new decoder; raising the limit is not the way out. |
  | `MissingTiles(limit)` | The charge for missing tiles passed the packet bound. |
  | `RepeatLimit { .. }` | The bound on the walks of tiles not kept is spent (see [Work and memory](#work-and-memory)). |

  More bytes can complete the header, the bounds grow with the bytes, and
  the missing tiles shrink, so each of these is a wait, and such a limit is
  reported as an error only by `finish`. The `LimitDetail` a wait carries is
  the refusal's, with its `retry_with` hint. For the sample limit, the
  charge for missing tiles and the packet bound it is the refusal
  `decode_with` gives on the bytes fed, and so it is for a `WorkLimit` of
  the code-block bound. `BoundSpent` is a refine whose own work, counted
  over every tile, fits the whole code-block bound but passed what the
  earlier refines left of it; its `retry_with` is `None`, since the file
  fits the limit it has. `RepeatLimit` carries no limit: the bound it names
  is not one `DecodeOptions` sets on its own, though a higher
  `packet_work_limit` raises it. The sample limit of a refine is that of a
  one-shot decode of its bytes: 2^29 samples, or 8 samples per byte when
  that is more. A sample limit of `WorkLimit::AtMost(n)` does not grow with
  the bytes, so a refusal under it is not a wait: `refine` returns it as an
  error, `header` returns it too, and it sticks however many bytes come
  after, until a change of options. A server that streams files in can
  refuse one as soon as its main header has come in. Any other error,
  including a limit that does not depend on the bytes (an image too large
  for the target to address, too many precincts, code-blocks or `POC`
  volumes, too many bit-planes), is the one `decode_with` gives on those
  bytes. A refusal by a bound the refines
  share can stand in for an error that block decoding, the wavelet or
  assembly would meet later in the file: the refine waits where
  `decode_with` fails, and `finish` reports that error. A refine refused
  only for what earlier refines spent still reads the packets of the later
  tiles, so an error there comes out as the error `decode_with` gives.
  Once `refine` returns an error, every later `refine` returns
  the same error without decoding, until a change of options (see
  [Changing the options](#changing-the-options)).
- A refine tries to decode when the bytes fed are at least twice those of
  the last refine that decoded or tried to. Before any try, the first refine
  called with at least one byte fed tries; with none fed, `refine` waits
  (`MoreBytes { until: 1, .. }`) and does not count as a try. A refine that
  tries gives an image when the one-shot decode of the bytes fed gives
  one, except when the earlier refines have spent the code-block bound
  (`BoundSpent`), or the bound on the walks of tiles not kept (see
  below). A try that gave no image still counts: after a main header cut
  short, or a refusal by a work bound or the charge for missing tiles, the
  next refine waits for twice the bytes of that try, even if the one-shot
  decode of fewer bytes gives an image by then
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

## Changing the options

`set_resolution_reduction`, `set_max_quality_layers` and `set_region`
change the options of the later refines and of `finish`. Each image a
refine returns after a change is the one-shot strict-off decode of its
bytes under the new options. `header` reads the header of the bytes fed
under the current options (`Ok(None)` until the main header is whole), and
`bytes` borrows the bytes fed.

- A walk reads its tile's packets under one reduction and one layer cap:
  they decide the geometry it builds and the coding passes it keeps. So
  when either has changed since the walks were built, the next refine that
  decodes lets every walk go before it walks a tile, and builds each again.
  A change back before that refine, or a set to the value the decoder has,
  keeps them.
- The walks do not depend on the region. A tile the new region reads goes
  on from where its walk stopped, and the walks of the tiles it does not
  read stay for a later change back. A smaller region has a smaller image,
  so the walks kept may be past the room of its decode (see
  [Work and memory](#work-and-memory)): then the refine lets go of the
  walks of the tiles it does not read, from the first, until the rest fit.
  Of the committed fixtures, whose tile walks are large beside their small
  images, a change to one tile lets go of some walks on most multi-tile
  files.
- A walk let go for either reason is built again on the next walk of its
  tile, and charged as a walk of a tile not kept. So changing the options
  back and forth spends the same bound as walking a tile again, and cannot
  buy free walks (`options_changed_back_and_forth_stay_within_the_bound_on_walks_read_again`).
  The visits spent and the code-block work spent carry across a change.
- A change does not reset the cadence: the next refine still waits for
  twice the bytes of the last try. Resetting it would give a caller who
  changes the options on every call a decode on every call, outside the
  bounds the refines share. A caller who wants an image straight after a
  change asks for that decode and pays for it:
  `decode_with(decoder.bytes(), &decoder.options().clone().with_strict(false))`.
  `options()` returns the options as the caller set them, `strict`
  included. The decode runs under the bounds of those options. It spends
  nothing from the decoder's bounds and does not change when the next
  refine decodes. A decode of the bytes fed with these options is not held
  to the refine cadence or to the bounds the refines share, so it gives
  what `decode_with` gives on those bytes. Where a refine waits on
  `MoreBytes`, `BoundSpent` or `RepeatLimit`, that can be an image. Where
  it waits on `HeaderIncomplete`, `SampleLimit`, `WorkLimit` or
  `MissingTiles`, it is the same refusal, as an error. `finish` decodes
  under the caller's `strict`, so on a stream that is stalled or cut short
  it returns an error, not an image. A caller who has fed the whole file gets
  the image under the new options from `finish`.
- A change clears an error an earlier refine returned, since the new
  options may not give it (a reduction past the file's levels). A set to
  the value the decoder has keeps it.

`src/incremental/tests.rs` changes the options before every refine of
files with several tiles and tile-parts, `TLM` and `PLT`, a `POC` volume in
a later tile-part and a MIXED HT file, and checks each refine against the
one-shot decode
(`options_changed_between_refines_refine_to_the_one_shot_decode_of_their_prefix`).
It counts the walk visits to check that a change of region reads no packet
twice, and that a change of reduction or layer cap is charged what a
one-shot walk of the tiles spends.

## Work and memory

Each refine is held to the packet bound of a decode of its bytes on its own.
The code-block bound holds across all the refines together.

Each refine is charged the packet visits the walk of a decode of its bytes on
its own spends: the geometry of the resolutions a reduction drops, the
progression's loops, the visits each packet read spends, the end of the
data, a visit for each packet after it, and the skips past a `POC`
bitmap. The charges come from the counts the kept walks hold, and a refine
is refused, as `Waiting::WorkLimit`, where that decode is refused for work,
and fails where it fails. So the packet bound is the one-shot bound of
the bytes fed, not cut by what earlier refines spent. A `POC` sequence's
volumes emit each precinct's layers from the first up, so a kept walk
tells a packet emitted again by the precinct's count of layers read; its
charges are those of the bitmap or counts that decode holds. The work the kept
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
(raised as the options say). A refine that rest refuses goes on counting
the code-block work of the later tiles, without decoding them. Where its
own work passes the whole bound, it returns `Waiting::WorkLimit` with the
refusal a decode of its bytes on its own gives. Where its own work fits
the whole bound, it returns `Waiting::BoundSpent`: only what the earlier
refines spent refused it. Either waits for more bytes, which raise the
bound. `finish` is one more decode, under its own bound, and reports a
work limit as one decode of the whole file does.

A tile whose walk is not kept (see below) is walked from the start by each
refine, so its walk is done again as the code-block work is. So is a tile
whose walk starts again (above), or is built once more bytes make room for
it, on the first refine of the new walk. Only the first walk of a tile
kept from its start is free. The ledger keeps the packet visits these
walks have spent over all the refines. Once that is past four times the
packet bound of one decode of the bytes fed (`REPEAT_BOUNDS` in
`src/tier2/resume.rs`), a refine that comes to such a walk is refused for
work before it walks, and returns `Waiting::RepeatLimit`, not the packet
bound's `WorkLimit`. The refine whose walk takes the count past that
bound is refused after the walk. So these walks spend at most five packet
bounds over all the refines: four, and the one walk
that goes past them, which one decode's bound holds. The bound follows the
packet work limit: raised by `WorkLimit::Raised`, none under
`WorkLimit::Unlimited`. So a caller who raises the limit to the factor a
refusal names, as `DecodeOptions::with_retry` does, and feeds the file to a
new decoder, accepts up to about ten times the packet work the file needs
over its refines: a factor up to twice what it needs, times five bounds. A
walk let go when the options change (see
[Changing the options](#changing-the-options)) is charged here too. Of the
committed fixtures, fed in quarters, in pieces of random sizes and one
byte at a time, at no reduction and at a reduction of 1, normal files
spend at most 0.008 of a bound on these walks, and files made to reach
the work bounds at most 3 bounds, so none of them gives another result.
Two files made for this
([#416](https://github.com/D0ubleD0uble/rust-j2k/issues/416)) show the
cost it holds. One of 525 KB, 256 components and 65535 layers, fed one
byte at a time with a refine after each, took 8.4 times one decode before
the bound and takes 4.6 times with it
(`the_walks_of_a_tile_not_kept_stay_within_a_few_bounds` checks a smaller
copy of it). One of 525 KB, one tile and 65535 layers, whose 13 later
tile-parts each add a `POC` volume, fed 64 bytes at a time, took 7.3 times
and takes 3.5 times
(`a_walk_started_again_is_charged_as_a_walk_not_kept`).

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
bounds. Each refine that returns an image builds the whole image, as one
decode does: it decodes every code-block again, allocates the samples and
runs the inverse wavelet and color transforms over them, whatever share of
the bytes is in. That is one image reconstruction per image, so at most
about `log2(n)` of them under the doubling cadence. Each refine also
parses the main header and walks the tile-part headers again; these are
linear in the headers, and take about 30 µs for the 16 KB of `TLM` entries
of `tile_header/tlm_32_16k.j2k`. And for a JP2 file whose last tile-part
has `Psot = 0`, each refine makes the walk that finds where its packets end
(see [The `read_header` walk](resource-bounds.md#the-read_header-walk)).

## What a later decoder could add

- A tier-1 cache that decodes again only the code-blocks a feed changed,
  which would let a refine come after every packet at little cost.
- A main-header and tile-part walk kept across feeds, so the `TLM`, `PLM`
  and `PPM` segments and the tile-part index are parsed once.

Each refine would still have to give the one-shot strict-off image of its
prefix, so the tests apply unchanged.
