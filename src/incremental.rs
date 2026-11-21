//! A decoder fed a file as it arrives, which gives a refined image as
//! more of it comes in (see [`IncrementalDecoder`]).

use std::cell::{Cell, RefCell};
use std::fmt;
use std::sync::{Mutex, PoisonError, TryLockError};

use crate::codestream::{MainHeader, Tile};
use crate::pipeline::decode::{HeaderCache, check_options, decode_metered, read_header_kept};
use crate::tier2::resume::{Repeats, Walks};
use crate::tier2::{CodedData, Meter, Reach, Refused};
use crate::{
    DecodeOptions, Error, Header, Image, LimitDetail, LimitKind, Raise, Region, Result,
    decode_with, jp2,
};

/// A decoder for a file that arrives a piece at a time, from a network or a
/// slow disk.
///
/// [`feed`](Self::feed) appends bytes as they come in, and
/// [`refine`](Self::refine) decodes what has come in so far. Each image
/// `refine` returns is the image [`decode_with`] gives with
/// [`strict`](DecodeOptions::strict) off on the bytes fed so far, with its
/// [`report`](Image::report) saying what those bytes do not hold yet: the
/// quality layers and resolutions that are complete, and the tiles still
/// missing. [`bytes_decoded`](Self::bytes_decoded) is how many bytes that
/// image is the decode of. Once the input ends, [`finish`](Self::finish)
/// decodes all of it under the decoder's options.
///
/// `refine` does not have to decode every time it is called, so a caller
/// can call it after every read, however small. It decodes at least each
/// time the bytes fed have doubled since it last decoded or tried to, and
/// may decode more often; when it does not decode, it returns
/// [`Waiting::MoreBytes`]. The first refine called with at least one byte
/// fed tries; one called with none fed waits and is not a try. A try that
/// a work bound refuses counts, so the next refine waits for twice its
/// bytes.
///
/// The decoder keeps each tile's packet walk (what it has read of the
/// tile's packet headers so far) between refines, so a refine reads only
/// the packets that have come in since the last one. Over all
/// the refines, the packet headers are read about once, as one decode reads
/// them; a packet the data cuts is read again by each refine ([the packets
/// a refine reads
/// again](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/incremental-decoding.md)).
/// Each refine is held to the packet bound of a decode of its bytes on its
/// own: it is charged the packet visits that decode would spend, and a
/// refine that decode is refused for returns [`Waiting::WorkLimit`].
///
/// The code-block work is done again by each refine that decodes, so the
/// code-block work every `refine` has spent is held to the bound one decode
/// of the bytes fed has, raised as the options say. A refine the rest of
/// that bound cannot cover returns [`Waiting::WorkLimit`] where its own
/// work passes the whole bound at the tile it is refused at, and
/// [`Waiting::BoundSpent`] where what the earlier refines spent put it past
/// at an earlier tile, whether or not the whole bound refuses a later one.
/// So a file whose code-blocks cost a lot may give no image before
/// `finish`, which is a decode of its own, under its own bound. Each refine that returns an
/// image also builds the whole image, as one decode does: it allocates the
/// samples and runs the inverse wavelet and color transforms over them,
/// outside the work bounds. Under the doubling cadence a file of `n` bytes
/// gives at most about `log2(n)` images, so a large image fed in small
/// pieces costs up to that many reconstructions.
///
/// Between calls the decoder keeps the bytes fed and the tiles' packet
/// walks. The walks kept hold at most twice what one decode holds at its
/// peak: twice the image's samples, plus twice the packet walk of the
/// largest tile. They also hold at most the larger of 1 MiB and 32 bytes
/// per byte fed, so a short file that declares a large image holds little
/// between calls: a file of 1228 bytes that declares a 16384 x 16384 image
/// holds no walk. A tile whose walk would pass either bound is walked from
/// the start by each refine instead, until more bytes make room for it.
/// Those walks, and a walk started again when a later tile-part changes
/// a tile's order of packets, spend at most five times the packet bound of
/// one decode of the bytes fed over all the refines together: once they
/// pass four, a refine that would walk such a tile again returns
/// [`Waiting::RepeatLimit`], and `finish` decodes the file.
/// On top of these bounds the decoder keeps a table of 24 bytes for each
/// tile index up to the highest it has seen, which comes to about 1.5 MiB
/// for the 65535 tiles a file can have (up to twice that in capacity as
/// the table grows). What [`header`](Self::header) keeps between its calls
/// comes to about 140 bytes for each tile counted under its own header and
/// a byte for each other tile, plus a key of the main header, its length and
/// a hash. Each refine also holds what [`decode_with`] holds for the same
/// bytes, so the decoder holds at most about three times one
/// decode's peak memory, plus the bytes fed. After the options change,
/// these bounds hold from the first refine that decodes under the new
/// options. A refine refused before that, partway through the tiles, can
/// leave the walks of later tiles above the new room, but not above the
/// larger of 1 MiB and 32 bytes per byte fed.
///
/// The reduction, the layer cap and the region can change between refines,
/// with [`set_resolution_reduction`](Self::set_resolution_reduction),
/// [`set_max_quality_layers`](Self::set_max_quality_layers) and
/// [`set_region`](Self::set_region), for a viewer that zooms or pans while
/// the file arrives. Each later image is then the strict-off decode of its
/// bytes under the new options. A change of region keeps the packet walks,
/// unless the walks kept are past the room of the new region's decode: then
/// the next refine that decodes lets go of the walks of tiles the region
/// does not read until the rest fit (see [`set_region`](Self::set_region)).
/// A change of the reduction keeps them too. A larger reduction has a
/// smaller image, so the walks kept may no longer fit: the next refine lets
/// go of the walks it reads that no longer fit, and walks those tiles from
/// the start until there is room. A change of the
/// layer cap lets them go, and the walks built again are charged to the
/// same bound as the walks of tiles not kept, so changing the options back
/// and forth does not buy free walks.
/// [`header`](Self::header) reads the header of the bytes fed so far, and
/// [`bytes`](Self::bytes) borrows them.
///
/// ```
/// use rust_j2k::{DecodeOptions, IncrementalDecoder, Refined};
///
/// # let file = include_bytes!(concat!(
/// #     env!("CARGO_MANIFEST_DIR"),
/// #     "/tests/fixtures/layers/lrcp.j2k"
/// # )).to_vec();
/// # /*
/// let file = std::fs::read("image.j2k")?;
/// # */
/// let mut decoder = IncrementalDecoder::new(DecodeOptions::default())?;
/// // The file arrives 500 bytes at a time.
/// for piece in file.chunks(500) {
///     decoder.feed(piece);
///     if let Refined::Image(image) = decoder.refine()? {
///         // Show the image of what has arrived so far. Its report says
///         // what is still missing.
///         println!(
///             "{} bytes: {}x{}, complete: {}",
///             decoder.bytes_decoded(),
///             image.width,
///             image.height,
///             image.report.is_complete()
///         );
///     }
/// }
/// // The input has ended: decode all of it, under the decoder's options
/// // (strict here).
/// let image = decoder.finish()?;
/// assert!(image.report.is_complete());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// An error from `refine` is the error [`decode_with`] gives with `strict` off
/// on the same bytes, such as a marker segment with an invalid field, or an
/// [`Error::Limit`] that does not depend on how many bytes there are: an image
/// too large for the target to address, too many `POC` volumes or too many
/// bit-planes, or a refusal by a sample limit of
/// [`SampleLimit::AtMost`](crate::SampleLimit::AtMost). More bytes do not
/// undo these, so once `refine` returns an error, every later
/// `refine` returns the same error without decoding, until a change of
/// options. Some refusals are not final
/// while bytes may still come, so `refine` waits on them, with the reason as a
/// [`Waiting`], and only `finish` reports them: a main header cut short
/// ([`Error::Incomplete`]), a refusal by the default or a
/// [`Raised`](crate::SampleLimit::Raised) sample limit (of the image, or of the
/// precincts and code-blocks a tile may hold), the packet or
/// code-block work bound or the charge for missing tiles, which more bytes
/// raise, and a refusal by the bound on the walks of tiles not kept, which the
/// refines spend together. A caller
/// who raises the packet work limit to the factor a refusal names, as
/// [`DecodeOptions::with_retry`] does, and feeds the file to a new decoder,
/// accepts up to about ten times the packet work the file needs over its
/// refines: a factor up to twice what it needs, times the five bounds of the
/// walks of tiles not kept.
///
/// The decoder is [`Send`] and [`Sync`].
pub struct IncrementalDecoder {
    /// The options `finish` decodes under.
    options: DecodeOptions,
    /// The same options with `strict` off, which `refine` decodes under.
    lenient: DecodeOptions,
    /// The bytes fed so far.
    bytes: Vec<u8>,
    /// How many bytes had been fed at the last refine that decoded, or
    /// tried to: the next one waits for twice as many.
    tried: usize,
    /// How many bytes the last image `refine` returned is the decode of.
    decoded: usize,
    /// The work the refines have spent, and the packet walks they keep.
    spent: Spent,
    /// Whether a work bound refused the last refine. Only the unit tests
    /// read it; `refine` names the reason in its `Waiting`.
    #[cfg(test)]
    refused: bool,
    /// Whether the bound on the walks of tiles not kept refused it.
    refused_by_repeats: bool,
    /// The packet bound of that refine, when that bound refused it.
    repeat_bound: u64,
    /// The error a refine returned, which every later call returns.
    failed: Option<Error>,
    /// What [`header`](Self::header) keeps between its calls. A lock, so
    /// that the decoder stays [`Sync`] and `header` takes `&self`.
    header_kept: Mutex<HeaderCache>,
}

impl IncrementalDecoder {
    /// A decoder with no bytes fed yet, which decodes under `options`.
    ///
    /// Options that are invalid whatever the input, which
    /// [`decode_with`] refuses before it reads a byte, are
    /// [`Error::InvalidOptions`] here. Options that do not fit the file,
    /// such as a reduction past its levels, are found by the first decode
    /// that reads its header.
    pub fn new(options: DecodeOptions) -> Result<Self> {
        check_options(&options)?;
        let lenient = options.clone().with_strict(false);
        Ok(IncrementalDecoder {
            options,
            lenient,
            bytes: Vec::new(),
            tried: 0,
            decoded: 0,
            spent: Spent::default(),
            #[cfg(test)]
            refused: false,
            refused_by_repeats: false,
            repeat_bound: 0,
            failed: None,
            header_kept: Mutex::default(),
        })
    }

    /// Append `bytes` to the input. This only copies them; the decoding is
    /// done by [`refine`](Self::refine) and [`finish`](Self::finish).
    pub fn feed(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// How many bytes have been fed.
    pub fn bytes_fed(&self) -> usize {
        self.bytes.len()
    }

    /// How many bytes the last image [`refine`](Self::refine) returned is
    /// the decode of, or 0 before it has returned one.
    pub fn bytes_decoded(&self) -> usize {
        self.decoded
    }

    /// The bytes fed so far.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The decoder's options, as the caller set them: those it was made with,
    /// as the setters have changed them. [`strict`](DecodeOptions::strict) is
    /// the caller's value, as [`finish`](Self::finish) uses it, not the
    /// strict-off copy [`refine`](Self::refine) decodes under.
    ///
    /// To show an image right after a change of options, without waiting for
    /// the next refine, decode the bytes fed so far with these options and
    /// `strict` off:
    ///
    /// ```
    /// use rust_j2k::{DecodeOptions, IncrementalDecoder, decode_with};
    ///
    /// # let file = include_bytes!(concat!(
    /// #     env!("CARGO_MANIFEST_DIR"),
    /// #     "/tests/fixtures/layers/lrcp.j2k"
    /// # )).to_vec();
    /// let mut decoder = IncrementalDecoder::new(DecodeOptions::default())?;
    /// // Most of the file has arrived, and the viewer zooms out.
    /// decoder.feed(&file[..file.len() - 5]);
    /// decoder.set_resolution_reduction(1)?;
    /// let options = decoder.options().clone().with_strict(false);
    /// let image = decode_with(decoder.bytes(), &options)?;
    /// assert_eq!((image.width, image.height), (32, 32));
    /// // With `strict` on, the cut file is refused.
    /// assert!(decode_with(decoder.bytes(), decoder.options()).is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// This is a decode of its own, under the bounds of these options. It
    /// spends nothing from the bounds the refines share and does not change
    /// when [`refine`](Self::refine) next decodes. The decoder keeps nothing
    /// from it, so a viewer that wants the image again keeps it itself.
    ///
    /// A decode of the bytes fed with these options is not held to the
    /// refine cadence or to the bounds the refines share, so it gives what
    /// [`decode_with`] gives on those bytes. Where a refine waits on
    /// [`MoreBytes`](Waiting::MoreBytes), [`BoundSpent`](Waiting::BoundSpent)
    /// or [`RepeatLimit`](Waiting::RepeatLimit), that can be an image, a work
    /// limit or another error. Where
    /// it waits on [`HeaderIncomplete`](Waiting::HeaderIncomplete),
    /// [`SampleLimit`](Waiting::SampleLimit),
    /// [`WorkLimit`](Waiting::WorkLimit) or
    /// [`MissingTiles`](Waiting::MissingTiles), it is the same refusal, as an
    /// error.
    ///
    /// With `strict` left on, it refuses a stream that is cut short, as
    /// [`finish`](Self::finish) does. The kind differs where the bytes end
    /// before the main header is whole: `decode_with` gives
    /// [`Error::Incomplete`], which `finish` reports as [`Error::FileFormat`]
    /// inside a JP2 file's boxes and as [`Error::Codestream`] inside the main
    /// header.
    pub fn options(&self) -> &DecodeOptions {
        &self.options
    }

    /// The header of the bytes fed so far, as
    /// [`read_header_with`](crate::read_header_with) reads it under the
    /// decoder's options with `strict` off: it describes the image
    /// [`refine`](Self::refine) returns. `Ok(None)` means the bytes fed end
    /// before the first `SOT` marker, or, in a JP2 file, inside the boxes
    /// before it, so the main header is not complete yet. An error is the
    /// one `read_header_with` gives on those bytes with `strict` off, so it
    /// refuses a region or reduction the file cannot take
    /// ([`Error::InvalidOptions`]), such as [`Region::Tile`]`(500)` on a
    /// one-tile file. A decoder with `strict` on can get a header here for a
    /// file that `read_header_with` under the same options refuses for what
    /// `strict` checks, such as a tile with no tile-part.
    /// [`read_header`](crate::read_header)`(decoder.bytes())` reads the
    /// header without the decoder's options. The default sample limit grows
    /// with the bytes fed, so a sample-limit [`Error::Limit`] here, of kind
    /// [`Samples`](LimitKind::Samples),
    /// [`TileCodeBlocks`](LimitKind::TileCodeBlocks) or
    /// [`TilePrecincts`](LimitKind::TilePrecincts), can turn into a header
    /// once more bytes come in; [`refine`](Self::refine) waits
    /// on it instead ([`Waiting::SampleLimit`]). Under a ceiling of
    /// [`SampleLimit::AtMost`](crate::SampleLimit::AtMost) it does not grow,
    /// so the refusal here is final, and `refine` returns it as an error
    /// too, except that a later tile's
    /// cap refusal met while a refine counts the code-block work ends the
    /// count as [`Waiting::BoundSpent`] (see [`Waiting`]). A server that
    /// streams files in can refuse a sample count as soon as its main header
    /// has come in.
    ///
    /// Each call parses the main header again (its `PPM` data included), so
    /// a call costs the main header plus the tile-parts fed since. For the
    /// tiles over a cap under it, the decoder keeps what the last call
    /// found: the walk of the tile-parts and the counts of the tiles' own
    /// headers. A call goes on from there, so the calls after each feed walk
    /// the tile-parts about once in all, and each call costs up to the
    /// read's budget besides. The header of a tile-part not yet fed in full, or
    /// of a last tile-part whose `Psot` is 0, is read again on each call. A
    /// call that refuses a tile, stops at a budget, or meets a tile whose
    /// own tile-part header is at fault also looks for the
    /// error the decode would give for the tile-parts first, as
    /// `read_header_with` does. That walk is kept too: a call walks the
    /// tile-parts fed since the last, checking each tile's header as it
    /// reads it. On a window of 1530 tiles of a 235 MB file of 16.7
    /// million tile-parts, calls after each 256 KiB fed take 66 ms in all;
    /// with its tile 0 refused and read alone, calls after each 1 MiB fed
    /// take 170 ms. With tile 1's own header at fault as well they take
    /// 1.3 ms: the walk stops at that header and is kept there, so a call
    /// reads only the tile-part at fault.
    /// What is kept, about 140 bytes for each tile counted under its own
    /// header and a byte for each other tile, plus a key of the main
    /// header (not a copy of it), is held until the options change, which
    /// drops it. The walk for the decode's error, once it has run, is kept
    /// over a change of options, as it depends on none of them, with its
    /// heap as `read_header_with` documents it (up to 14 bytes a tile of
    /// the grid, and the coding of each tile that later tile-parts restate
    /// with headers that differ). Nothing is
    /// kept for a file whose tiles' sizes show them within the caps;
    /// otherwise a few hundred bytes, plus the counts above. A call made
    /// while another thread is in one reads from the start.
    pub fn header(&self) -> Result<Option<Header>> {
        // A call on another thread holding the lock reads from the start. A
        // call that panicked may have left the walk half done, so it is
        // dropped.
        let mut kept = match self.header_kept.try_lock() {
            Ok(kept) => Some(kept),
            Err(TryLockError::Poisoned(poisoned)) => {
                let mut kept = poisoned.into_inner();
                *kept = HeaderCache::default();
                self.header_kept.clear_poison();
                Some(kept)
            }
            Err(TryLockError::WouldBlock) => None,
        };
        match read_header_kept(&self.bytes, &self.lenient, kept.as_deref_mut()) {
            Ok(header) => Ok(Some(header)),
            Err(Error::Incomplete(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Decode at a reduction of `levels` from now on: the
    /// [`resolution_reduction`](DecodeOptions::resolution_reduction) of the
    /// later refines and of [`finish`](Self::finish).
    ///
    /// The packet walks do not depend on the reduction, which decides only
    /// the code-blocks a decode charges for the resolutions it drops, so
    /// the walks kept are reused: each tile goes on from where its walk
    /// stopped, and is charged the code-blocks of the new reduction. A
    /// larger reduction has a smaller image, so the walks kept may no longer
    /// fit in the room for them (see [`IncrementalDecoder`]). Then the next
    /// refine that decodes lets go of the walks it reads that no longer fit,
    /// and walks those tiles from the start until there is room; those walks
    /// are charged as
    /// [`set_max_quality_layers`](Self::set_max_quality_layers) describes.
    /// Setting the
    /// reduction the decoder already has changes nothing. A change does not
    /// make the next refine decode sooner: it still waits for the bytes fed
    /// to double since the last try. For an image now, see
    /// [`options`](Self::options). It clears an error an earlier refine
    /// returned, as the new options may not give it.
    ///
    /// Options invalid whatever the input are [`Error::InvalidOptions`], as
    /// in [`new`](Self::new), and leave the decoder as it was.
    pub fn set_resolution_reduction(&mut self, levels: u8) -> Result<()> {
        if self.options.resolution_reduction == levels {
            return Ok(());
        }
        self.set(self.options.clone().with_resolution_reduction(levels))
    }

    /// Decode the first `layers` quality layers from now on: the
    /// [`max_quality_layers`](DecodeOptions::max_quality_layers) of the
    /// later refines and of [`finish`](Self::finish). `Some(0)` is
    /// [`Error::InvalidOptions`], and leaves the decoder as it was.
    ///
    /// The packet walks kept were read under the old layer cap, which
    /// decides the coding passes they keep, so the next refine that decodes
    /// lets them go and walks each tile again from the start. Those walks
    /// are charged as walks of tiles not kept (see [`IncrementalDecoder`]),
    /// so changing the options back and forth cannot buy more than a few
    /// packet bounds of work. Setting the layer cap the decoder already has
    /// changes nothing. As with
    /// [`set_resolution_reduction`](Self::set_resolution_reduction), the
    /// change does not make the next refine decode sooner, and it clears an
    /// error an earlier refine returned.
    pub fn set_max_quality_layers(&mut self, layers: Option<u16>) -> Result<()> {
        if self.options.max_quality_layers == layers {
            return Ok(());
        }
        self.set(self.options.clone().with_max_quality_layers(layers))
    }

    /// Decode `region` from now on: the [`region`](DecodeOptions::region)
    /// of the later refines and of [`finish`](Self::finish). An empty window
    /// is [`Error::InvalidOptions`], and leaves the decoder as it was.
    ///
    /// The packet walks do not depend on the region, so the walks kept are
    /// reused: a tile the new region reads goes on from where its walk
    /// stopped, and the walks of the tiles it does not read stay for a
    /// later change back. When a smaller region leaves the walks kept past
    /// the memory they may hold (see [`IncrementalDecoder`]), the next
    /// refine that decodes lets go of the walks of the tiles it does not
    /// read until the rest fit, or none is left to let go; a later walk of
    /// those tiles is charged as
    /// [`set_max_quality_layers`](Self::set_max_quality_layers)
    /// describes. As there, the change does not make the next refine decode
    /// sooner, and it clears an error an earlier refine returned.
    pub fn set_region(&mut self, region: Region) -> Result<()> {
        if self.options.region == region {
            return Ok(());
        }
        self.set(self.options.clone().with_region(region))
    }

    /// Decode under `options` from now on, which differ from the decoder's
    /// in the reduction, the layer cap or the region.
    fn set(&mut self, options: DecodeOptions) -> Result<()> {
        check_options(&options)?;
        self.lenient = options.clone().with_strict(false);
        self.options = options;
        self.failed = None;
        // What the header read kept is for the old selection, apart from
        // its walk for the decode's error for the tile-parts.
        self.header_kept
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .clear_selection();
        Ok(())
    }

    /// Decode the bytes fed so far. This decodes at least each time they
    /// have doubled since it last decoded or tried to, and may decode more
    /// often.
    ///
    /// [`Refined::Image`] is the image [`decode_with`] gives with
    /// [`strict`](DecodeOptions::strict) off on all the bytes fed, and its
    /// [`report`](Image::report) lists what they do not hold yet.
    /// [`Refined::Waiting`] means there is no new image yet, and says why
    /// (see [`Waiting`]): too few bytes have come in since the last try, the
    /// main header is not whole, or the sample limit, a work bound, the
    /// charge for missing tiles or the bound on the walks done again refused
    /// the decode. [`Refined::image`] gives the image or `None`, for a
    /// caller who does not need the reason.
    ///
    /// Any error is the one `decode_with` gives on those bytes, and more
    /// bytes will not clear it: each later call returns it again, until a
    /// change of options. A refusal by a bound only the refines have can
    /// stand in for an error that block decoding, the wavelet or assembly
    /// would meet in a tile the refine did not decode: the refine waits
    /// where `decode_with` fails, and `finish` reports that error. Those
    /// bounds are what the earlier refines left of the code-block bound
    /// ([`BoundSpent`](Waiting::BoundSpent)) and the bound on the walks done
    /// again ([`RepeatLimit`](Waiting::RepeatLimit)). A
    /// [`WorkLimit`](Waiting::WorkLimit) never stands in for an error.
    pub fn refine(&mut self) -> Result<Refined> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let fed = self.bytes.len();
        let until = self.tried.saturating_mul(2).max(1);
        if fed < until {
            return Ok(Refined::Waiting(Waiting::MoreBytes { until }));
        }
        self.tried = fed;
        // The ledger lives for the decode alone, so the decoder holds no
        // `Cell` between calls. Only the walks move into it: if the decode
        // panics, the code-block work spent stays, and the walks start
        // again.
        let spent = Spent {
            walks: std::mem::take(&mut self.spent.walks),
            ..self.spent
        };
        let ledger = Ledger::over(spent, fed as u64);
        let decoded = decode_metered(&self.bytes, &self.lenient, Some(&ledger));
        let refusal = ledger.work_refused.get();
        #[cfg(test)]
        {
            self.refused = refusal.is_some();
        }
        self.refused_by_repeats = ledger.repeats.refused.get();
        self.repeat_bound = ledger.repeats.bound.get();
        self.spent = ledger.into_spent();
        let waiting = match decoded {
            Ok(image) => {
                self.decoded = fed;
                return Ok(Refined::Image(image));
            }
            // More bytes can complete the header, and raise the work
            // bounds.
            Err(Error::Incomplete(_)) => Waiting::HeaderIncomplete {},
            // The bound on the walks of tiles not kept refuses as the packet
            // walk does, but its error names the packet bound, which is not
            // what refused.
            Err(Error::Limit(_)) if self.refused_by_repeats => Waiting::RepeatLimit {},
            Err(Error::Limit(limit)) => match refusal {
                Some(Refusal::MissingTiles) => Waiting::MissingTiles { limit },
                Some(Refusal::Work) => Waiting::WorkLimit { limit },
                Some(Refusal::BoundSpent) => Waiting::BoundSpent { limit },
                // The sample limit, and the code-blocks and precincts a tile
                // may hold, grow with the bytes too, but only a refusal that
                // can be lifted waits. The kind says whether an option
                // raises it (not past the address space, which is its own
                // kind), and a retry says whether the limit leaves room: it
                // is `None` under `AtMost`, a ceiling the caller set, which
                // more bytes do not raise. A refusal with no retry sticks.
                None if matches!(
                    limit.kind(),
                    LimitKind::Samples | LimitKind::TileCodeBlocks | LimitKind::TilePrecincts
                ) && matches!(
                    limit.retry_with(),
                    Some(Raise::Raised(_) | Raise::Unlimited)
                ) =>
                {
                    Waiting::SampleLimit { limit }
                }
                None => return Err(self.fail(Error::Limit(limit))),
            },
            Err(error) => return Err(self.fail(error)),
        };
        Ok(Refined::Waiting(waiting))
    }

    /// Keep `error` for every later refine, and return it.
    fn fail(&mut self, error: Error) -> Error {
        self.failed = Some(error.clone());
        error
    }

    /// The input has ended: decode all of it under the decoder's options,
    /// as [`decode_with`] does.
    ///
    /// With [`strict`](DecodeOptions::strict) on, the default, a file cut
    /// short is refused as `decode_with` refuses it. With `strict` off, it
    /// gives the image the bytes hold, with a report of what is missing. In
    /// both cases, a file that ends before its main header is whole, which
    /// [`refine`](Self::refine) waits on, is truncated now:
    /// [`Error::FileFormat`] when it ends inside the boxes of a JP2 file
    /// before its codestream, and [`Error::Codestream`] when it ends inside
    /// the codestream's main header. A JP2 file whose `pclr`, `cmap` or
    /// `cdef` boxes are malformed is refused for that with
    /// [`Error::FileFormat`] once its `jp2c` box header is in, by `refine`,
    /// [`header`](Self::header) and `finish` alike, even when the main header
    /// is cut.
    ///
    /// This is a decode of its own, also after a `refine` has returned an
    /// error: under strict options it may refuse the bytes for an earlier
    /// fault, such as the data ending, than the one `refine` found.
    ///
    /// `finish` borrows the decoder and keeps nothing from the call, so it
    /// can be called again and gives the same result. When it refuses a
    /// work limit, [`into_bytes`](Self::into_bytes) gives the bytes back to
    /// decode with the limit raised.
    pub fn finish(&self) -> Result<Image> {
        match decode_with(&self.bytes, &self.options) {
            Err(Error::Incomplete(why)) => {
                let in_boxes = !self.bytes.starts_with(&[0xFF, 0x4F])
                    && jp2::looks_like_jp2(&self.bytes)
                    && matches!(
                        jp2::parse(&self.bytes, &self.options.packet_work_limit),
                        Err(Error::Incomplete(_))
                    );
                let why = format!("the input is truncated: {why}");
                Err(if in_boxes {
                    Error::file_format(why)
                } else {
                    Error::codestream(why)
                })
            }
            decoded => decoded,
        }
    }

    /// A decoder whose earlier refines spent `blocks` of the code-block
    /// work, as the unit tests set it up (see [`crate::fuzz`]).
    pub(crate) fn with_code_block_work_spent(options: DecodeOptions, blocks: u64) -> Result<Self> {
        let mut decoder = Self::new(options)?;
        decoder.spent.blocks = blocks;
        Ok(decoder)
    }

    /// The code-block work the refines have spent, and the code-block
    /// bound of the last decode a refine tried, when that decode read the
    /// main header and has a bound (see [`crate::fuzz`]).
    pub(crate) fn code_block_work(&self) -> (u64, Option<u64>) {
        (self.spent.blocks, self.spent.bound)
    }

    /// The packet visits the walks of tiles not kept have spent over the
    /// refines; the packet bound of the last refine that tried, when the
    /// bound on those walks refused it; and how many tiles have a walk too
    /// large to keep (see [`crate::fuzz`]).
    pub(crate) fn repeat_work(&self) -> (u64, Option<u64>, usize) {
        let bound = self.refused_by_repeats.then_some(self.repeat_bound);
        (self.spent.repeats, bound, self.spent.walks.kept().1)
    }

    /// Consumes the decoder and returns the bytes fed.
    ///
    /// After [`finish`](Self::finish) refuses a work limit, decode these
    /// with [`decode_with`], or feed them to a new decoder, under options
    /// with the limit raised.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl fmt::Debug for IncrementalDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IncrementalDecoder")
            .field("options", &self.options)
            .field("bytes_fed", &self.bytes.len())
            .field("bytes_decoded", &self.decoded)
            .finish_non_exhaustive()
    }
}

// The decoder can be shared and sent between threads, and a panic in one
// of its calls leaves nothing half-borrowed.
const _: () = {
    const fn holds<T: Send + Sync + std::panic::UnwindSafe + std::panic::RefUnwindSafe>() {}
    holds::<IncrementalDecoder>();
};

/// What [`IncrementalDecoder::refine`] gives when it does not fail: an
/// image of the bytes fed so far, or why it has none yet.
///
/// [`image`](Self::image) drops the reason:
/// `decoder.refine()?.image()` is the image or `None`.
///
/// ```
/// use rust_j2k::{DecodeOptions, IncrementalDecoder, Refined, Waiting};
///
/// # let file = include_bytes!(concat!(
/// #     env!("CARGO_MANIFEST_DIR"),
/// #     "/tests/fixtures/layers/lrcp.j2k"
/// # )).to_vec();
/// let mut decoder = IncrementalDecoder::new(DecodeOptions::default())?;
/// decoder.feed(&file[..20]);
/// // 20 bytes end inside the SIZ marker segment.
/// assert!(matches!(
///     decoder.refine()?,
///     Refined::Waiting(Waiting::HeaderIncomplete { .. })
/// ));
/// decoder.feed(&file[20..30]);
/// // The refine tried at 20 bytes, so the next try is at 40 at the latest.
/// assert!(matches!(
///     decoder.refine()?,
///     Refined::Waiting(Waiting::MoreBytes { until: 40, .. })
/// ));
/// decoder.feed(&file[30..]);
/// match decoder.refine()? {
///     Refined::Image(image) => println!("{}x{}", image.width, image.height),
///     Refined::Waiting(why) => println!("no image yet: {why:?}"),
///     _ => {}
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
// The image is what most refines that decode return, unboxed as
// `Option<Image>` was: a box would cost an allocation per image and a
// dereference at every match, to shrink a value that is moved once.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Refined {
    /// The image [`decode_with`] gives with
    /// [`strict`](DecodeOptions::strict) off on the bytes fed so far, with
    /// its [`report`](Image::report) of what they do not hold yet.
    Image(Image),
    /// No new image, and why.
    Waiting(Waiting),
}

impl Refined {
    /// The image, or `None` when the refine waits, for a caller who does
    /// not need the reason.
    pub fn image(self) -> Option<Image> {
        match self {
            Refined::Image(image) => Some(image),
            Refined::Waiting(_) => None,
        }
    }
}

/// Why [`IncrementalDecoder::refine`] gave no image.
///
/// Each of these may clear as more bytes come in, so none is an error.
/// Once the input has ended, [`finish`](IncrementalDecoder::finish) decodes
/// all of it and reports a limit that still refuses it. To decode the bytes
/// fed now, decode them on their own, as
/// [`options`](IncrementalDecoder::options) shows:
/// `decode_with(decoder.bytes(), &decoder.options().clone().with_strict(false))`.
///
/// The enum and its struct-like variants are `#[non_exhaustive]`, so a
/// `match` names their fields with `..` and has a `_` arm:
///
/// ```
/// use rust_j2k::{DecodeOptions, IncrementalDecoder, Refined, Waiting};
///
/// # let file = include_bytes!(concat!(
/// #     env!("CARGO_MANIFEST_DIR"),
/// #     "/tests/fixtures/layers/lrcp.j2k"
/// # )).to_vec();
/// let mut decoder = IncrementalDecoder::new(DecodeOptions::default())?;
/// for piece in file[..30].chunks(10) {
///     decoder.feed(piece);
///     let Refined::Waiting(why) = decoder.refine()? else {
///         continue;
///     };
///     match why {
///         // `until` is a total of bytes fed, not the bytes still to come.
///         Waiting::MoreBytes { until, .. } => {
///             let more = until - decoder.bytes_fed();
///             println!("the next try is at {until} bytes, {more} from now");
///         }
///         Waiting::HeaderIncomplete { .. } => println!("the main header is not in yet"),
///         Waiting::RepeatLimit { .. } => println!("decode the bytes fed now"),
///         Waiting::BoundSpent { .. } => println!("decode the bytes fed now"),
///         Waiting::WorkLimit { limit, .. } => println!("{limit}"),
///         _ => println!("{why:?}"),
///     }
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Waiting {
    /// The refine did not decode: no bytes have been fed, or the bytes fed
    /// have not doubled since the last refine that decoded or tried to.
    #[non_exhaustive]
    MoreBytes {
        /// The total of bytes fed at which a refine decodes again, at the
        /// latest: twice those fed at the last try, or 1 before any. It
        /// counts from the start of the input, not from the bytes fed now.
        until: usize,
    },
    /// The main header is not whole yet: the bytes fed end before the first
    /// `SOT` marker, or, in a JP2 file, inside the boxes before it.
    /// [`header`](IncrementalDecoder::header) returns `Ok(None)` for the
    /// same bytes.
    #[non_exhaustive]
    HeaderIncomplete {},
    /// The sample limit ([`DecodeOptions::sample_limit`]) refused the image,
    /// or a tile of more code-blocks or precincts than it allows a tile
    /// ([`LimitKind::TileCodeBlocks`], [`LimitKind::TilePrecincts`]).
    /// The default limit and a [`Raised`](crate::SampleLimit::Raised) one
    /// grow with the bytes fed, so more bytes may admit it. The
    /// detail is the one [`decode_with`] gives for the bytes fed, with its
    /// [`retry_with`](LimitDetail::retry_with). A retry can still meet a
    /// fixed limit, such as the address space, on the way. The detail's
    /// [`refused_at`](LimitDetail::refused_at) describes the bytes fed so
    /// far: a later tile-part can change which tile is refused.
    ///
    /// Only a refusal that more bytes can lift waits: one with a
    /// [`retry_with`](LimitDetail::retry_with). A ceiling of
    /// [`AtMost`](crate::SampleLimit::AtMost) does not grow, and a refusal past
    /// what the target can address ([`LimitKind::AddressSpace`]) is lifted
    /// by no limit, so neither is a wait: `refine` returns it as an error,
    /// and every later `refine` returns it again, whatever is fed, until a
    /// change of options. The exception is a later tile's cap refusal met
    /// while a refine counts the code-block work: that ends the count as
    /// [`BoundSpent`](Self::BoundSpent).
    #[non_exhaustive]
    SampleLimit {
        /// The refusal, as an error would carry it.
        limit: LimitDetail,
    },
    /// The bytes fed cost more work than the packet work bound
    /// ([`LimitKind::PacketWork`]) or the code-block work bound
    /// ([`LimitKind::CodeBlockWork`]) allows: a decode of them on its own is
    /// refused too. Both bounds grow with the bytes fed, so more bytes may
    /// admit them.
    ///
    /// The detail is the one [`decode_with`] gives for the bytes fed, with
    /// its [`retry_with`](LimitDetail::retry_with).
    /// [`DecodeOptions::with_retry`] applies it, for a new decoder or a
    /// decode of [`into_bytes`](IncrementalDecoder::into_bytes).
    ///
    /// A code-block `WorkLimit` comes only where the refine decoded every
    /// tile before the one the whole bound refuses. A refine that earlier
    /// refines cut short does not decode the tiles past the cut, and where
    /// the whole bound refuses a later tile it waits on
    /// [`BoundSpent`](Self::BoundSpent) instead.
    #[non_exhaustive]
    WorkLimit {
        /// The refusal, as an error would carry it.
        limit: LimitDetail,
    },
    /// The earlier refines have spent the code-block work bound
    /// ([`LimitKind::CodeBlockWork`]). The refines share that bound (see
    /// [`IncrementalDecoder`]), and each decodes the code-blocks again, so
    /// what they spent is cut from the bound of the next. The refine passed
    /// what was left of the bound at a tile, the cut, whose work fits the
    /// whole bound after the tiles before it, so it was refused for what was
    /// spent before it. It did not decode the tiles from the cut on.
    ///
    /// Decode the bytes fed now, but that decode can still be refused or
    /// fail. The refine counts the work of the tiles after the cut without
    /// decoding their code-blocks. Where the count ends with the file, the
    /// work fits the whole bound, and that decode can still give another
    /// error, a packet-work limit included. Where the whole bound refuses a
    /// later tile, the detail's [`refused_at`](LimitDetail::refused_at)
    /// gives that tile and its work, and that decode is refused there, with
    /// the same `refused_at`, or fails first in a tile the refine did not
    /// decode. `decode_with` decodes each tile whole before the
    /// next, so the code-blocks the refine skipped, at the cut and after it,
    /// can hold the error `decode_with` meets first. An error in a later
    /// tile's header or packets, or a packet-work refusal of its walk, ends
    /// the count too, and the refine waits here rather than return it. To
    /// count, the refine walks the packets of the tiles after the cut, and
    /// the walks of tiles not kept spend the bound on the walks done again
    /// that the refines share; if that bound refuses a walk, the refine
    /// waits on [`RepeatLimit`](Self::RepeatLimit), not here.
    ///
    /// Raising the code-block limit is not the way out where the count ends
    /// with the file: the bytes fed pass the code-block bound they have.
    /// Decode them now,
    /// `decode_with(decoder.bytes(), &decoder.options().clone().with_strict(false))`
    /// (see [`options`](IncrementalDecoder::options)), or start a new
    /// decoder. A viewer that streams the file is better served by the
    /// decode of the bytes fed: a new decoder starts without the packet
    /// walks this one keeps. Later refines of this decoder may wait on this
    /// too, though more bytes raise the bound, and `finish` decodes under a
    /// bound of its own. The detail's
    /// [`retry_with`](LimitDetail::retry_with) is `None`.
    #[non_exhaustive]
    BoundSpent {
        /// The refusal, as an error would carry it.
        limit: LimitDetail,
    },
    /// The charge for missing tiles passed the packet work bound: each
    /// tile-component of a tile that has not come in yet costs a step,
    /// before any tile is decoded. Tiles that come in lower the charge, and
    /// more bytes raise the bound. The detail is the refusal's, of kind
    /// [`LimitKind::PacketWork`], as a decode of the bytes fed on its own
    /// gives it.
    #[non_exhaustive]
    MissingTiles {
        /// The refusal, as an error would carry it.
        limit: LimitDetail,
    },
    /// The bound on the packet walks the refines do again is spent: the
    /// walks of tiles whose walk is too large to keep, and of walks started
    /// again (see [`IncrementalDecoder`]). All the refines spend it
    /// together, up to four times the packet bound of a decode of the bytes
    /// fed, so a refine that would walk such a tile again waits. More bytes
    /// raise it only as they raise the packet bound, so on a small file it
    /// can stay spent for the rest of the stream. Between due refines every
    /// call returns [`MoreBytes`](Self::MoreBytes), so this shows only on a
    /// refine that is due. A decode of the bytes fed now (see
    /// [`options`](IncrementalDecoder::options)) is not held to this bound
    /// and may give an image. A decoder made with a higher
    /// [`packet_work_limit`](DecodeOptions::packet_work_limit) has a higher
    /// bound.
    #[non_exhaustive]
    RepeatLimit {},
}

/// Checks, as doctests, that the struct variants of [`Waiting`] that carry a
/// [`LimitDetail`], and `HeaderIncomplete`, are `#[non_exhaustive]`.
///
/// A `match` outside the crate cannot name `Waiting::WorkLimit`'s fields
/// without `..`:
///
/// ```compile_fail,E0638
/// fn limit(why: rust_j2k::Waiting) -> Option<rust_j2k::LimitDetail> {
///     match why {
///         rust_j2k::Waiting::WorkLimit { limit } => Some(limit),
///         _ => None,
///     }
/// }
/// ```
///
/// A `match` outside the crate cannot name `Waiting::BoundSpent`'s fields
/// without `..`:
///
/// ```compile_fail,E0638
/// fn limit(why: rust_j2k::Waiting) -> Option<rust_j2k::LimitDetail> {
///     match why {
///         rust_j2k::Waiting::BoundSpent { limit } => Some(limit),
///         _ => None,
///     }
/// }
/// ```
///
/// A `match` outside the crate cannot name `Waiting::SampleLimit`'s fields
/// without `..`:
///
/// ```compile_fail,E0638
/// fn limit(why: rust_j2k::Waiting) -> Option<rust_j2k::LimitDetail> {
///     match why {
///         rust_j2k::Waiting::SampleLimit { limit } => Some(limit),
///         _ => None,
///     }
/// }
/// ```
///
/// A `match` outside the crate cannot name `Waiting::MissingTiles`'s fields
/// without `..`:
///
/// ```compile_fail,E0638
/// fn limit(why: rust_j2k::Waiting) -> Option<rust_j2k::LimitDetail> {
///     match why {
///         rust_j2k::Waiting::MissingTiles { limit } => Some(limit),
///         _ => None,
///     }
/// }
/// ```
///
/// A `match` outside the crate cannot name `Waiting::HeaderIncomplete`
/// without `..`:
///
/// ```compile_fail,E0638
/// fn header(why: rust_j2k::Waiting) -> bool {
///     matches!(why, rust_j2k::Waiting::HeaderIncomplete {})
/// }
/// ```
#[cfg(doctest)]
pub struct WaitingVariantsAreNonExhaustive;

/// Which work bound refused a decode under a [`Ledger`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The packet walk, the walks of tiles not kept (which
    /// [`Repeats`] tells apart), or the code-block work.
    Work,
    /// The charge for the missing tiles' tile-components.
    MissingTiles,
    /// What the earlier decodes left of the code-block bound, where the
    /// work of this decode fits the whole bound.
    BoundSpent,
}

/// What the decodes of an [`IncrementalDecoder`] keep between them (see
/// [`Ledger`]).
#[derive(Default)]
struct Spent {
    walks: Walks,
    /// The code-block work spent.
    blocks: u64,
    /// The packet visits the walks of tiles not kept have spent.
    repeats: u64,
    /// The code-block bound of the last decode, when it had one.
    bound: Option<u64>,
}

/// What the decodes of an [`IncrementalDecoder`] keep between them, so
/// that all of them together stay within the work bounds one decode of the
/// same bytes has. It is built from [`Spent`] for the length of one decode.
///
/// Each tile's packet walk is kept (see [`Walks`]): a decode goes on from
/// where the last one stopped, and reads each packet once, however many
/// decodes there are. It is charged what the walk of a decode of its bytes
/// on its own would spend, and refused where that walk is refused, so the
/// packet bound is not cut. The code-block work is done again by each
/// decode, so the code-block bound of each is cut by what the earlier ones
/// spent. So are the walks of tiles too large to keep, which each decode
/// does from the start: the visits they spend over all the decodes are
/// held to a few packet bounds (see [`Walks::walk`]).
pub(crate) struct Ledger {
    /// The tiles' packet walks.
    walks: RefCell<Walks>,
    /// The code-block work spent.
    blocks: Cell<u64>,
    /// The packet visits the walks of tiles not kept have spent (see
    /// [`Walks::walk`]).
    repeats: Repeats,
    /// The code-block bound of this decode, once it is known.
    bound: Cell<Option<u64>>,
    /// Which work bound refused the decode, if one did.
    work_refused: Cell<Option<Refusal>>,
    /// The bytes the decode is of, which the walks kept are held to (see
    /// [`Walks`]).
    fed: u64,
}

impl fmt::Debug for Ledger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ledger")
            .field("blocks", &self.blocks)
            .field("repeats", &self.repeats)
            .field("bound", &self.bound)
            .field("work_refused", &self.work_refused)
            .field("fed", &self.fed)
            .finish_non_exhaustive()
    }
}

impl Ledger {
    /// A ledger over the walks and the work of the earlier decodes, for a
    /// decode of `fed` bytes.
    fn over(spent: Spent, fed: u64) -> Self {
        Ledger {
            walks: RefCell::new(spent.walks),
            blocks: Cell::new(spent.blocks),
            repeats: Repeats::new(spent.repeats),
            bound: Cell::new(None),
            work_refused: Cell::new(None),
            fed,
        }
    }

    /// What the decodes have spent, this one included.
    fn into_spent(self) -> Spent {
        Spent {
            walks: self.walks.into_inner(),
            blocks: self.blocks.get(),
            repeats: self.repeats.visits.get(),
            bound: self.bound.get(),
        }
    }

    /// The code-block work a decode whose own bound is `bound` may spend:
    /// what is left of it after the earlier decodes. No bound
    /// ([`WorkLimit::Unlimited`](crate::WorkLimit::Unlimited)) stays none.
    /// The ledger keeps `bound` as this decode's.
    pub(crate) fn blocks_left(&self, bound: u64) -> u64 {
        self.bound.set(Some(bound));
        if bound == u64::MAX {
            return bound;
        }
        bound.saturating_sub(self.blocks.get())
    }

    /// Make the walks kept fit this decode, which reads the tiles `reads`
    /// flags under `reduction` and `layers`, and whose image is of `image`
    /// bytes (see [`Walks::prepare`]).
    pub(crate) fn prepare(&self, reads: &[bool], reduction: u8, layers: Option<u16>, image: u64) {
        self.walks
            .borrow_mut()
            .prepare(reads, reduction, layers, image, self.fed);
    }

    /// Tile `tile`'s packets, as a walk with `strict` off of the bytes it
    /// holds gives them (see [`Walks::walk`]), going on from the walk the
    /// earlier decodes left.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn walk<'t>(
        &self,
        header: &MainHeader,
        tile: &'t Tile<'t>,
        meter: &Meter,
        poc_bitmap: u64,
        reduction: u8,
        layers: Option<u16>,
        image: u64,
    ) -> std::result::Result<(CodedData<'t>, Reach), Refused> {
        self.walks.borrow_mut().walk(
            header,
            tile,
            meter,
            poc_bitmap,
            reduction,
            layers,
            image,
            self.fed,
            &self.repeats,
        )
    }

    /// Record that a work bound refused a decode, for `why`: the packet
    /// walk, the walks of tiles not kept, the code-block work or the charge
    /// for missing tiles. The bounds grow with the bytes and the missing
    /// tiles shrink, so more bytes may clear it.
    pub(crate) fn refuse(&self, why: Refusal) {
        self.work_refused.set(Some(why));
    }

    /// Add `work` to the code-block work spent.
    pub(crate) fn spend_blocks(&self, work: u64) {
        self.blocks.set(self.blocks.get().saturating_add(work));
    }
}

#[cfg(test)]
mod tests;
