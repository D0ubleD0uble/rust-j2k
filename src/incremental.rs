//! A decoder fed a file as it arrives, which gives a refined image as
//! more of it comes in (see [`IncrementalDecoder`]).

use std::cell::{Cell, RefCell};
use std::fmt;

use crate::codestream::{MainHeader, Tile};
use crate::pipeline::decode::{check_options, decode_metered};
use crate::tier2::resume::{Repeats, Walks};
use crate::tier2::{CodedData, Meter, Reach, Refused};
use crate::{
    DecodeOptions, Error, Header, Image, LimitKind, Region, Result, decode_with, jp2,
    read_header_with,
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
/// may decode more often; when it does not decode, it returns `Ok(None)`.
/// The first refine called with at least one byte fed tries; one called
/// with none fed returns `Ok(None)` and is not a try. A try that a work
/// bound refuses counts, so the next refine waits for twice its bytes.
///
/// The decoder keeps each tile's packet walk (what it has read of the
/// tile's packet headers so far) between refines, so a refine reads only
/// the packets that have come in since the last one. Over all
/// the refines, the packet headers are read about once, as one decode reads
/// them; a packet the data cuts is read again by each refine ([the packets
/// a refine reads
/// again](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.5.0/docs/incremental-decoding.md)).
/// Each refine is held to the packet bound of a decode of its bytes on its
/// own: it is charged the packet visits that decode would spend, and a
/// refine that decode is refused for returns `Ok(None)`.
///
/// The code-block work is done again by each refine that decodes, so the
/// code-block work every `refine` has spent is held to the bound one decode
/// of the bytes fed has, raised as the options say. A refine the rest of
/// that bound cannot cover returns `Ok(None)`, so a file whose code-blocks
/// cost a lot may give no image before `finish`, which is a decode of its
/// own, under its own bound. Each refine that returns an image also builds
/// the whole image, as one decode does: it allocates the samples and runs
/// the inverse wavelet and color transforms over them, outside the work
/// bounds. Under the doubling cadence a file of `n` bytes gives at most
/// about `log2(n)` images, so a large image fed in small pieces costs up to
/// that many reconstructions.
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
/// `Ok(None)`, and `finish` decodes the file.
/// On top of these bounds the decoder keeps a table of 24 bytes for each
/// tile index up to the highest it has seen, which comes to about 1.5 MiB
/// for the 65535 tiles a file can have (up to twice that in capacity as
/// the table grows). Each refine also holds what [`decode_with`] holds for
/// the same bytes, so the decoder holds at most about three times one
/// decode's peak memory, plus the bytes fed.
///
/// The reduction, the layer cap and the region can change between refines,
/// with [`set_resolution_reduction`](Self::set_resolution_reduction),
/// [`set_max_quality_layers`](Self::set_max_quality_layers) and
/// [`set_region`](Self::set_region), for a viewer that zooms or pans while
/// the file arrives. Each later image is then the strict-off decode of its
/// bytes under the new options. A change of region keeps the packet walks.
/// A change of the reduction or the layer cap lets them go, and the walks
/// built again are charged to the same bound as the walks of tiles not
/// kept, so changing the options back and forth does not buy free walks.
/// [`header`](Self::header) reads the header of the bytes fed so far, and
/// [`bytes`](Self::bytes) borrows them.
///
/// ```
/// use rust_j2k::{DecodeOptions, IncrementalDecoder};
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
///     if let Some(image) = decoder.refine()? {
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
/// too large for the target to address, too many precincts or code-blocks, too
/// many `POC` volumes, or too many bit-planes. More bytes do not undo these, so
/// once `refine` returns an error, every later `refine` returns the same error
/// without decoding, until a change of options. Some refusals are not final
/// while bytes may still come, so `refine` returns `Ok(None)` for them and only
/// `finish` reports them: a main header cut short ([`Error::Incomplete`]), a
/// refusal by the sample limit, the packet or code-block work bound or the
/// charge for missing tiles, which more bytes raise, and a refusal by the bound
/// on the walks of tiles not kept, which the refines spend together. A caller
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
    /// Whether a work bound refused the last refine.
    refused: bool,
    /// Whether the bound on the walks of tiles not kept refused it.
    refused_by_repeats: bool,
    /// The error a refine returned, which every later call returns.
    failed: Option<Error>,
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
            refused: false,
            refused_by_repeats: false,
            failed: None,
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

    /// The header of the bytes fed so far, as [`read_header_with`] reads it
    /// under the decoder's options: it describes the image
    /// [`refine`](Self::refine) returns. `Ok(None)` means the bytes fed end
    /// before the first `SOT` marker, or, in a JP2 file, inside the boxes
    /// before it, so the main header is not complete yet. An error is the
    /// one `read_header_with` gives on those bytes, such as a reduction the
    /// main header's levels cannot take ([`Error::InvalidOptions`]). The
    /// sample limit grows with the bytes fed, so a sample-limit
    /// [`Error::Limit`] here can turn into a header once more bytes come in;
    /// [`refine`](Self::refine) returns `Ok(None)` for it instead.
    ///
    /// Call it after a feed rather than after each byte: each call parses
    /// the main header again.
    pub fn header(&self) -> Result<Option<Header>> {
        match read_header_with(&self.bytes, &self.lenient) {
            Ok(header) => Ok(Some(header)),
            Err(Error::Incomplete(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Decode at a reduction of `levels` from now on: the
    /// [`resolution_reduction`](DecodeOptions::resolution_reduction) of the
    /// later refines and of [`finish`](Self::finish).
    ///
    /// The packet walks kept were read under the old reduction, so the
    /// next refine that decodes lets them go and walks each tile again from
    /// the start. Those walks are charged as walks of tiles not kept (see
    /// [`IncrementalDecoder`]), so changing the options back and forth
    /// cannot buy more than a few packet bounds of work. Setting the
    /// reduction the decoder already has changes nothing. A change does not
    /// make the next refine decode sooner: it still waits for the bytes fed
    /// to double since the last try. It clears an error an earlier refine
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
    /// The packet walks kept were read under the old layer cap, so the next
    /// refine that decodes lets them go, as
    /// [`set_resolution_reduction`](Self::set_resolution_reduction) does,
    /// and is charged the same for walking the tiles again.
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
    /// [`set_resolution_reduction`](Self::set_resolution_reduction)
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
        Ok(())
    }

    /// Decode the bytes fed so far. This decodes at least each time they
    /// have doubled since it last decoded or tried to, and may decode more
    /// often.
    ///
    /// The image is the one [`decode_with`] gives with
    /// [`strict`](DecodeOptions::strict) off on all the bytes fed, and its
    /// [`report`](Image::report) lists what they do not hold yet.
    /// `Ok(None)` means there is no new image yet: too few bytes have come
    /// in since the last decode, the main header is not whole, or the
    /// sample limit or a work bound refused the decode (each grows with the
    /// bytes). Any error is the one `decode_with` gives
    /// on those bytes, and more bytes will not clear it: each later call
    /// returns it again. A refusal by a bound all the refines spend
    /// together can stand in for an error later in the file: the refine
    /// returns `Ok(None)` where `decode_with` fails, and `finish` reports
    /// that error.
    pub fn refine(&mut self) -> Result<Option<Image>> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let fed = self.bytes.len();
        if fed == 0 || fed < self.tried.saturating_mul(2) {
            return Ok(None);
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
        self.refused = ledger.work_refused.get();
        self.refused_by_repeats = ledger.repeats.refused.get();
        self.spent = ledger.into_spent();
        match decoded {
            Ok(image) => {
                self.decoded = fed;
                Ok(Some(image))
            }
            // More bytes can complete the header, and raise the work
            // bounds.
            Err(Error::Incomplete(_)) => Ok(None),
            Err(Error::Limit(_)) if self.refused => Ok(None),
            // The sample limit grows with the bytes too.
            Err(Error::Limit(limit)) if limit.kind() == LimitKind::Samples => Ok(None),
            Err(error) => {
                self.failed = Some(error.clone());
                Err(error)
            }
        }
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
    /// the codestream's main header.
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

    /// Whether the bound on the walks of tiles not kept, which the refines
    /// spend together, refused the last refine that tried (see
    /// [`crate::fuzz`]).
    pub(crate) fn refused_by_repeats(&self) -> bool {
        self.refused_by_repeats
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
    /// Whether a work bound refused the last decode.
    work_refused: Cell<bool>,
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
            work_refused: Cell::new(false),
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

    /// Record that a work bound refused a decode: the packet walk, the
    /// walks of tiles not kept, the code-block work or the charge for
    /// missing tiles. The bounds grow
    /// with the bytes and the missing tiles shrink, so more bytes may
    /// clear it.
    pub(crate) fn refuse(&self) {
        self.work_refused.set(true);
    }

    /// Add `work` to the code-block work spent.
    pub(crate) fn spend_blocks(&self, work: u64) {
        self.blocks.set(self.blocks.get().saturating_add(work));
    }
}

#[cfg(test)]
mod tests;
