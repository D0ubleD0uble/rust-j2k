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
  | `HeaderIncomplete { .. }` | `decode_with` gives `Error::Incomplete`: the main header is cut short. |
  | `SampleLimit { limit, .. }` | The default or a `Raised` sample limit refused the image, or a tile of more code-blocks or precincts than it allows a tile, with a `retry_with`. A refusal with none (under `AtMost`, or past the address space) is an error, not a wait, except a tile's before the input closes (`Held`). In either case, a later tile's cap refusal met while a refine counts the code-block work ends the count as `BoundSpent` instead. |
  | `WorkLimit { limit, .. }` | The bytes fed cost more than the packet or code-block work bound allows; a decode of them on its own is refused too, with the same detail. |
  | `BoundSpent { limit, .. }` | The earlier refines spent the code-block bound the refines share, so the refine stopped decoding at a tile, the cut, whose work fits the whole bound. Decode the bytes fed now or start a new decoder. That decode can be refused for work or fail: where the whole bound refuses a later tile, the detail's `refused_at` gives it and its work, and that decode is refused there or fails first in a tile the refine did not decode. |
  | `MissingTiles { limit, .. }` | The charge for missing tiles passed the packet bound. |
  | `RepeatLimit { .. }` | The bound on the walks of tiles not kept is spent (see [Work and memory](#work-and-memory)). |
  | `Held { error }` | The decode of the bytes fed fails with `error`, but more bytes can still put an earlier error, or an image, in its place: a fault in a later tile-part, or in a JP2 box still to come or still arriving. `error` is a tile error met once every tile-part has parsed (a reduction past the tile's own levels, a tile cap under `AtMost`, a damaged packet header, too many bit-planes), in a JP2 file also a fault of the tile-parts, or an error of the boxes that more bytes can replace (see [Errors in a JP2 file's boxes](#errors-in-a-jp2-files-boxes)). In a codestream, the next refine that decodes once the `EOC` is in gives the final answer. In a JP2 file it does once that `EOC` is in and no box after the codestream can still be read; otherwise `finish` does, under the decoder's options, so with `strict` on it can give a stricter error than `error`. An image can still follow it, for example where a later tile-part's `POC` changes which packets are read. See [When an error in a tile is final](#when-an-error-in-a-tile-is-final). |

  More bytes can complete the header, the bounds grow with the bytes, and
  the missing tiles shrink, so each of these is a wait, and such a limit is
  reported as an error only by `finish`. The `LimitDetail` a wait carries is
  the refusal's, with its `retry_with` hint. For the sample limit, the
  charge for missing tiles and the packet bound it is the refusal
  `decode_with` gives on the bytes fed, and so it is for a `WorkLimit` of
  the code-block bound, which never stands in for another error.
  `BoundSpent` is a refine that passed what the earlier refines left of the
  code-block bound at a tile whose work fits the whole bound; its
  `retry_with` is `None`. A decode of the bytes fed can still be refused for
  work or give another error (see [Work and memory](#work-and-memory)).
  `RepeatLimit` carries no limit: the bound it names is not one
  `DecodeOptions` sets on its own, though a higher `packet_work_limit`
  raises it. The sample limit of a refine is that of a one-shot decode of
  its bytes: 2^29 samples, or 8 samples per byte when that is more. So a
  file of more than 2^29 samples gives no image until the bytes fed reach an
  eighth of its samples: about 64 MiB just past 2^29 samples, and more for a
  larger image. Each refine before that waits on `Waiting::SampleLimit`. The
  cadence doubles the bytes between tries, so the first image can come at up
  to twice that: a 296 MB file of more than 2^29 samples gave its first image at
  128 MiB fed (measured, reported in #474). The sample count is taken at the
  reduction, so a viewer that wants a preview sooner can set a resolution
  reduction first. The tile caps on code-blocks and precincts are not counted
  at the reduction, so a reduction does not shorten a wait on them. A sample
  limit of `SampleLimit::AtMost(n)` does not grow with the bytes, so a refusal
  of the image under it is not a wait: `refine` returns it as an error,
  `header` returns it too, and it sticks however many bytes come after, until
  a change of options. A server that streams files in can refuse one as soon
  as its main header has come in. A tile's refusal under it waits until the
  input closes (see [When an error in a tile is
  final](#when-an-error-in-a-tile-is-final)), and so does the later-tile case
  in the table above. On a 32-bit target the sample limit is the
  address-space cap of 2^29 - 1 samples, which no limit lifts, so `refine`
  never waits on `Waiting::SampleLimit` there (see [32-bit
  targets](stability.md#32-bit-targets)). Any other error, including a
  limit that does not depend on the bytes (an image or a tile too large for
  the target to address, too many `POC` volumes, too many bit-planes), is
  the one `decode_with` gives on those bytes. A refusal by a bound only the
  refines have, `BoundSpent` or `RepeatLimit`, can stand in for an error
  that block decoding, the wavelet or assembly would meet in a tile the
  refine did not decode: the refine waits where `decode_with` fails, and
  `finish` reports that error (see [Work and memory](#work-and-memory)).
  Once `refine` returns an error, every later `refine` returns the same
  error without decoding, until a change of options (see [Changing the
  options](#changing-the-options)).
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

## When an error in a tile is final

With `strict` off, `decode_with` parses every tile-part before it decodes
any tile, so a fault in any tile-part comes before an error in a tile: a
reduction past the tile's levels, a tile cap, a damaged packet header, too
many bit-planes. A prefix that ends before a faulty tile-part gives the
error in the tile, and a longer one the fault: a later tile-part of the
tile that restates its `COD` with other values, or a `COC` for a component
the image does not have. So such an error is final only once the input
closes. A codestream closes when the chain of `SOT`s steps by nonzero
`Psot`s to an `EOC` in the bytes fed; past that, more bytes change no
tile-part. A last tile-part whose `Psot` is 0 runs to the end of the
input, which may still grow, so such a codestream never closes.

In a JP2 file the decode checks the boxes after the codestream before it
parses the tile-parts, so a damaged box that follows the codestream comes
before every error of the tiles, a fault of the tile-parts included. So
each such error waits while a box after the codestream can still be read.
None can once OpenJPEG's walk there meets a `jp2c` or an `XLBox` of 2^32
or more, and none ever is where the last tile-part has `Psot` 0, which runs
to the end of the file, or where the tile-part chain is broken on the bytes
fed, so that OpenJPEG's walk cannot reach the boxes after it. A box of length 0
after the codestream is still read while under 2^32 bytes follow it, so it does
not end the wait. Once no box there can be read, the tile-parts are held as a
codestream's are: a fault of the tile-parts is final, and an error in a tile
waits until they close.

Until the input closes, `refine` waits on the error as `Waiting::Held`,
which carries the error `decode_with` gives on the bytes fed, and `header`
holds it back and gives the header. Once a codestream closes, the next
refine that decodes returns the error `decode_with` gives then, the error
in the tile or the fault, and `header` gives what `read_header_with` gives.
Where the input never closes, `refine` waits and `finish` reports the error.
More bytes can replace the error with one that comes earlier in the
input, and an image can still follow, for example where a later
tile-part's `POC` changes which packets the decode reads, so that the
packet that failed is no longer read. A
caller that stops on a `Held` gives up on such a file. A caller that
stopped on such an error from
`refine` now waits for it: a tile cap refusal under `AtMost`, for one, comes
at the `EOC` in a codestream, and in a JP2 file at the `EOC` once no box
after the codestream can be read, or else from `finish`.

The rule covers every error the decode meets once it has parsed the
tile-parts. A refusal under the default or a `Raised` limit waits anyway,
as `Waiting::SampleLimit`, and `header` holds a tile's such refusal back
until the input closes too, as a later fault replaces it as well; once a
codestream closes, `header` gives it, and more bytes may still lift it. An
error of the image (its samples, or a reduction past the main header's
levels) or of the main header depends on no tile-part, so it stays final
at once, except a refusal of the sRGB step in a JP2 file, which waits
while a box after the codestream can still be read. In a codestream, so
does a fault of a tile-part. In a JP2 file every error from the
tile-parts on, a fault of a tile-part included, waits as `Waiting::Held`
while a box after the codestream can still be read, as a damaged one
comes before it, and is then held as in a codestream.
`finish`, `decode_with` and `read_header_with` are not changed, nor is
anything with `strict` on, as `refine` and `header` always read with it
off.

A refine that waits decodes no more than the one-shot decode of its bytes
does: that decode stops at the refused tile before it builds the tile's
geometry. `tests/tile_refusal_held_alloc.rs` feeds a tile of 10^6
precincts, over the cap of `AtMost(2^24)`, with no `EOC`: the calls that
hold the refusal peak at about 67 KB of heap, where a decode under a
ceiling that admits the tile builds about 264 MB (2026-11-22).
`tests/tile_error_final.rs` feeds files of each shape, as codestreams and
as JP2 files with and without a damaged box after the codestream, in every
two pieces and a byte at a time, and checks that `refine` and `header` each
go from the main header cut short to images or waits to one error, the
one-shot call's.

### Errors in a JP2 file's boxes

Some errors of a JP2 file's boxes can also be replaced by more bytes, and
wait the same way:

- A fault in a box the bytes fed end inside. After the codestream,
  `decode_with` refuses a box OpenJPEG reads that runs past the end of the
  input ("box `colr` after the codestream is 15 bytes but only 8 remain"),
  and more bytes complete it. A box of length 0 runs to the end of the
  input, so its contents grow with it: a `colr` of length 0 after the
  codestream is too short on a prefix and whole on the file, and a `jp2h`
  or `ftyp` of length 0 before the codestream can be at fault on a prefix
  and whole on a longer one, where the file has no `jp2c`, or whole on a
  prefix and at fault on a longer one. A box other than a `jp2c` after the
  signature, cut inside its `XLBox`, is judged on its type ("the `ftyp` box
  must follow the signature"), and the `XLBox` still to come can be at
  fault, which comes first. A `jp2c` there is refused on its type at once,
  as its length is never at fault.
- A refusal of the sRGB step. `decode_with` checks the boxes after the
  codestream before it plans that step, so a box at fault there, still to
  come, replaces it. That holds for a refusal more bytes lift too: `header`
  gives the header for it, and `refine` waits on it as
  `Waiting::SampleLimit`.

Some of these are final all the same, and are given at once:

- A fault of a whole child of a `jp2h` of length 0 before the codestream,
  such as an `ihdr` that declares a height of 0, or a child whose length
  is shorter than its header. The children are read in order, so the
  ones after it come too late.
- A refusal of the sRGB step where OpenJPEG reads no box after the
  codestream, whatever follows: its last tile-part has `Psot` 0, so it
  runs to the end of the file, or the walk after it stops at a `jp2c` or
  at an `XLBox` of 2^32 or more.

An unsupported brand in an `ftyp` of length 0 is not final: more bytes
add compatibility codes, and `jp2 ` among them makes the file readable,
while a byte or three leaves the box out of whole codes. Nor is any fault
of a box of length 0 after the codestream, even one judged on its type
(a second `pclr`): OpenJPEG skips such a box unread once 2^32 bytes or
more follow it, and the file then decodes.

`refine` waits on these as `Waiting::Held`, with the error
`decode_with` gives on the bytes fed, and `finish` reports what the whole
file gives under the decoder's options: with `strict` on, that can be a
stricter error. `header` gives the header, with the channels as coded where the
sRGB step is refused, or `None` where the box of length 0 is before the
codestream, as there is no main header yet. A fault in a box the bytes fed
hold whole stays final at once, before or after the codestream: the boxes
before it are whole too, and more bytes change none of them. OpenJPEG
reports the same errors on the same prefixes (`opj_decompress`, with or
without `-allow-partial`): "Invalid box size 15 for box 'colr'. Need 7
bytes, 0 bytes remaining" for the cut `colr`, "Bad COLR header box (bad
size)" for the short one, and "JP2H box missing" or "Expected a SOC
marker" where a box of length 0 swallows the rest of the file, and then
"Error with FTYP signature Box size" once an `ftyp` of length 0 is at
fault, or "Cannot handle box of undefined sizes" once an `XLBox` of 0 is
in. It does not refuse sRGB output; that
refusal is this crate's. `tests/jp2_box_error_final.rs` feeds each shape,
synthetic and from `jp2_real/byte_tlm_plt.jp2`, and checks every prefix
through `header`, `refine` and `finish` with `strict` on and off.

OpenJPEG takes the first error in the order of the codestream. Under
`-r 1` it refuses the first tile-part's `COD` whose levels the reduction
discards ("Error decoding component 0 ... Modify the cp_reduce
parameter"), on every prefix from that tile-part on, and does not read on to
the later `COD` (opj_decompress 2.5.0, `-allow-partial`, measured
2026-11-22). Without the reduction it accepts the later `COD` that differs,
which this decoder refuses. So OpenJPEG's error is final at once, and its
choice of error differs from `decode_with`'s here.

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

`header` keeps what its last call found for the tiles over a cap under the
main header, in `pipeline::header::HeaderCache`: the walk of the tile-parts,
where it stopped, the first tile-part offset of each tile it reached, and
the counts of each tile's own header once that tile-part is whole. A later
call goes on from there, so the calls after each feed read each `SOT` about
once in all. Once the main header is whole, each call also parses it
again, so a call costs the main header (`PPM` data included) plus the
tile-parts fed since. While the bytes fed end before the main header is
whole, a call goes on from where the last one stopped instead, in
`pipeline::header::KeptCut`: after the last whole box before a JP2 file's
`jp2c`, and after the main header's last whole marker segment, with what
the segments before it set (`PPM` data as offsets, not a copy). The calls
then read those boxes and segments about once in all. Two cases are read from
the start on each call: a box of length 0 before the codestream, which runs to
the end of the bytes fed, and a `jp2c` box that ends inside its own main header
where the bytes after the box read as marker segments that the header's parse
refuses, or that end the header (`KeptCut::main_still_cut` then drops its
parse), until the bytes fed let the walk of their segment and tile-part lengths
reach an end or a fault, which a large `Psot` can put gigabytes away. Where the
codestream is then the box's payload whatever follows
(`jp2::Jp2File::after_settled`), the error of that read is kept
(`KeptCut::ended`) and given by every later call, and the parse is let go. The
header of a tile-part not yet fed in full is read
again on each call, as is that of a last tile-part whose `Psot` is 0, which is
never whole. The tiles it learned of are charged the
read's budget again, as a read from the start charges them, so each call gives
what `read_header_with` gives with `strict` off on the same bytes. It keeps
about 140 bytes for each tile counted under its own header and a byte for each
other tile, and a key of the main header (two integers), not a copy. A file
whose tiles' sizes show them within the caps keeps nothing; any other keeps a
few hundred bytes, plus the per-tile counts. What it keeps is for one selected
area and reduction, and a change of options drops it.

## Changing the options

`set_resolution_reduction`, `set_max_quality_layers` and `set_region`
change the options of the later refines and of `finish`. Each image a
refine returns after a change is the one-shot strict-off decode of its
bytes under the new options. `header` reads the header of the bytes fed
under the current options (`Ok(None)` until the main header is whole,
except that a JP2 file whose `pclr`, `cmap` or `cdef` boxes are malformed is
refused once its `jp2c` box header is in), and `bytes` borrows the bytes fed.

- A walk reads its tile's packets under one layer cap, which decides the
  coding passes it keeps. So when the cap has changed since the walks were
  built, the next refine that decodes lets every walk go before it walks a
  tile, and builds each again. A change back before that refine, or a set
  to the value the decoder has, keeps them.
- The walks do not depend on the reduction. A walk builds its tile's
  geometry at full resolution whatever the reduction, and the reduction
  decides only the code-blocks of the resolutions it drops, which each
  decode charges to the packet bound. So a change of the reduction alone
  keeps every walk, and each walk charges the code-blocks the new
  reduction drops, counted from the geometry it holds. A walk whose
  geometry failed holds none to count from, and is let go. The public API does
  not reach this: a decoder keeps a walk only after `check_tile` has built the
  same geometry and passed the caps. A larger reduction has a smaller image, so
  the walks kept may no longer fit in the room (see [Work and
  memory](#work-and-memory)). Then the next refine lets go of the walks it reads
  that no longer fit, and walks those tiles from the start until there is room.
- The walks do not depend on the region. A tile the new region reads goes
  on from where its walk stopped, and the walks of the tiles it does not
  read stay for a later change back. A smaller region has a smaller image,
  so the walks kept may be past the room of its decode (see
  [Work and memory](#work-and-memory)): then the refine lets go of the
  walks of the tiles it does not read, from the first, until the rest fit.
  Of the committed fixtures, whose tile walks are large beside their small
  images, a change to one tile lets go of some walks on most multi-tile
  files.
- A walk let go for any of these reasons is built again on the next walk
  of its tile, and charged as a walk of a tile not kept. So changing the
  options back and forth spends the same bound as walking a tile again, and
  cannot buy free walks
  (`options_changed_back_and_forth_stay_within_the_bound_on_walks_read_again`).
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
It counts the walk visits to check that a change of region or of reduction
reads no packet twice (`a_change_of_reduction_keeps_the_walks`), and that a
change of layer cap is charged what a one-shot walk of the tiles spends.
`tests/incremental.rs` changes the reduction between refines of one-tile
files and checks that no walk is charged as read again, and that at the
packet bound a refine after a change is refused where the one-shot decode
is, and gives the image where it does
(`a_change_of_reduction_at_the_packet_bound_is_refused_as_the_one_shot_decode`).

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
(raised as the options say), so all the refines together spend at most that
one bound. Where the tile that rest refuses passes the whole bound too, the
refine returns `Waiting::WorkLimit` with the refusal a decode of its bytes
on its own gives: it decoded every tile before that one, as that decode
does. Otherwise it goes on counting the code-block work of the later tiles,
without decoding them, and returns `Waiting::BoundSpent`. Where the count
reaches the end, its own work fits the whole bound, and only what the
earlier refines spent refused it. Where the whole bound refuses a later
tile, the limit's `refused_at` gives that tile and its work, and a decode of
the bytes on their own is refused there with the same `refused_at`, unless
it fails first in a tile the refine counted. An error in a later tile's
header or packets, or a packet-work refusal of its walk, also ends the
count, and the refine returns
`Waiting::BoundSpent` rather than that error. A one-shot decode decodes each
tile whole before the next, so the code-blocks the refine skipped, in the
tile it was refused at and after it, can hold the error that decode meets
first (see the tests in `tests/incremental.rs`). The refine does not decode
them to find out: each such refine could spend up to one more bound on that.
Either wait is for more bytes, which raise the bound. `finish` is one more
decode, under its own bound, and reports a work limit as one decode of the
whole file does.

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
refusal names, as `DecodeOptions::raised_for` does, and feeds the file to a
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
until the bytes fed make room for it. After a change of the options, these
bounds hold from the first refine that decodes under the new options. A
refine refused before that, partway through the tiles, can leave the walks of
later tiles above the new room, but not above the larger of 1 MiB and 32
bytes per byte fed; the next refine that decodes lets them go. A change of
the layer cap lets go of every walk, and the largest walk with them, so the
memory room is then that of a decoder made with the new options. That
holds for memory only. The repeat bound stays spent, and the walks built
again are charged to it as repeats (see above). A change of the
reduction or the window keeps the largest walk, which still bounds the walks
kept. Of the
committed fixtures fed 1 KB at a time, this keeps fewer walks than the image
bound alone only for files made to test the work bounds, and for a
6000 x 4000 RGB file of 148 KB until 64 KB of it are in. On top of these bounds
the decoder keeps a table of 24 bytes for each tile index up to the highest it
has seen (`size_of::<Slot>()` in `src/tier2/resume.rs`). That is about 1.5 MiB
for the 65535 tiles a file can have, and up to twice that in capacity as the
table grows: an 82-byte file of 1 x 1 tiles whose one tile-part is tile 65534
holds 1.57 MB between calls. Each refine holds what `decode_with` holds for the
same bytes on top of the walks kept, so the decoder holds at most one decode's
peak plus the walks kept (twice the image's samples plus twice the walk of the
largest tile walked so far; a change of the region or the reduction keeps that
walk), plus up to twice the bytes fed. `tests/incremental_alloc.rs` counts the
bytes held on files of many one-sample tiles of 512 components, in LRCP order
and under 32 PCRL `POC` volumes, fed in pieces, and
`tests/incremental_kept_alloc.rs` on a file of 1228 bytes that declares a 2048 x
2048 image of 4 x 4 code-blocks. The decoder is `Send` and `Sync`. The ledger's
counters live only for the length of a decode.

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
