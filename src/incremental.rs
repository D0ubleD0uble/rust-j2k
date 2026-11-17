//! A decoder fed a file as it arrives, which gives a refined image as
//! more of it comes in (see [`IncrementalDecoder`]).

use std::cell::{Cell, RefCell};
use std::fmt;

use crate::codestream::{MainHeader, Tile};
use crate::tier2::resume::Walks;
use crate::tier2::{CodedData, Meter, Reach, Refused};
use crate::{DecodeOptions, Error, Image, Result, check_options, decode_metered, decode_with, jp2};

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
/// again](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/incremental-decoding.md)).
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
/// largest tile. A tile whose walk would pass that is walked from the start
/// by each refine instead. Each refine also holds what [`decode_with`]
/// holds for the same bytes, so the decoder holds at most about three
/// times one decode's peak memory, plus the bytes fed.
///
/// That memory follows the image size the file declares, not the bytes fed:
/// a file of 1228 bytes that declares a 16384 x 16384 image holds about
/// 640 MB between calls, until the decoder is dropped. A server that keeps
/// many slow streams open should check the size that
/// [`read_header`](crate::read_header) reports before it feeds the rest of
/// a file, or drop the decoders it is not refining.
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
/// An error from `refine` is the error [`decode_with`] gives with `strict`
/// off on the same bytes, such as a marker segment with an invalid field,
/// or an [`Error::Limit`] that does not depend on how many bytes there are:
/// an image past the sample limit, too many precincts or code-blocks, too
/// many `POC` volumes, or too many bit-planes. More bytes do not undo
/// these, so once `refine` returns an error, every later `refine` returns
/// the same error without decoding. Two refusals are not final while bytes
/// may still come, so `refine` returns `Ok(None)` for them and only
/// `finish` reports them: a main header cut short ([`Error::Incomplete`]),
/// and a refusal by the packet or code-block work bound, which more bytes
/// raise.
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

    /// Decode the bytes fed so far. This decodes at least each time they
    /// have doubled since it last decoded or tried to, and may decode more
    /// often.
    ///
    /// The image is the one [`decode_with`] gives with
    /// [`strict`](DecodeOptions::strict) off on all the bytes fed, and its
    /// [`report`](Image::report) lists what they do not hold yet.
    /// `Ok(None)` means there is no new image yet: too few bytes have come
    /// in since the last decode, the main header is not whole, or a work
    /// bound refused the decode. Any error is the one `decode_with` gives
    /// on those bytes, and more bytes will not clear it: each later call
    /// returns it again.
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
        let walks = std::mem::take(&mut self.spent.walks);
        let ledger = Ledger::over(walks, self.spent.blocks);
        let decoded = decode_metered(&self.bytes, &self.lenient, Some(&ledger));
        self.refused = ledger.work_refused.get();
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
/// spent.
pub(crate) struct Ledger {
    /// The tiles' packet walks.
    walks: RefCell<Walks>,
    /// The code-block work spent.
    blocks: Cell<u64>,
    /// Whether a work bound refused the last decode.
    work_refused: Cell<bool>,
}

impl fmt::Debug for Ledger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ledger")
            .field("blocks", &self.blocks)
            .field("work_refused", &self.work_refused)
            .finish_non_exhaustive()
    }
}

impl Ledger {
    /// A ledger over the walks and the code-block work of the earlier
    /// decodes.
    fn over(walks: Walks, blocks: u64) -> Self {
        Ledger {
            walks: RefCell::new(walks),
            blocks: Cell::new(blocks),
            work_refused: Cell::new(false),
        }
    }

    /// What the decodes have spent, this one included.
    fn into_spent(self) -> Spent {
        Spent {
            walks: self.walks.into_inner(),
            blocks: self.blocks.get(),
        }
    }

    /// The code-block work a decode whose own bound is `bound` may spend:
    /// what is left of it after the earlier decodes. No bound
    /// ([`WorkLimit::Unlimited`](crate::WorkLimit::Unlimited)) stays none.
    pub(crate) fn blocks_left(&self, bound: u64) -> u64 {
        if bound == u64::MAX {
            return bound;
        }
        bound.saturating_sub(self.blocks.get())
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
        self.walks
            .borrow_mut()
            .walk(header, tile, meter, poc_bitmap, reduction, layers, image)
    }

    /// Record that a work bound refused a decode: the packet walk, the
    /// code-block work or the charge for missing tiles. The bounds grow
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
