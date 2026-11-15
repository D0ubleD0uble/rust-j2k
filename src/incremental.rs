//! A decoder fed a file as it arrives, which gives a refined image as
//! more of it comes in (see [`IncrementalDecoder`]).

use std::cell::Cell;
use std::fmt;

use crate::tier2::Meter;
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
/// `refine` does not decode every time it is called. It decodes once the
/// bytes fed have at least doubled since its last attempt, and returns
/// `Ok(None)` the rest of the time, so a caller can call it after every
/// read, however small. A file of `n` bytes is decoded about `log2(n)`
/// times, on prefixes that double in length, so the work is less than
/// twice that of one decode of the whole file.
///
/// The work bounds of [`decode`](crate::decode) hold across all the refines
/// together: the packet visits and the code-block work every `refine` has
/// spent are held to the bound one decode of the bytes fed has, raised as
/// the options say. A refine the rest of that bound cannot cover returns
/// `Ok(None)`, so a file whose decode costs about the same however few of
/// its bytes are there may give no image before `finish`, which is a decode
/// of its own, under its own bound. Each
/// decode holds no more memory than [`decode_with`] holds for the same
/// bytes, and the decoder keeps the bytes fed, and nothing else, between
/// calls.
///
/// ```
/// use rust_j2k::{DecodeOptions, IncrementalDecoder};
///
/// let file = include_bytes!(concat!(
///     env!("CARGO_MANIFEST_DIR"),
///     "/tests/fixtures/layers/lrcp.j2k"
/// ));
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
/// // The input has ended: decode all of it, strictly.
/// let image = decoder.finish()?;
/// assert!(image.report.is_complete());
/// # Ok::<(), rust_j2k::Error>(())
/// ```
///
/// An error from `refine` is the error [`decode_with`] gives with `strict`
/// off on the same bytes, such as a marker segment with an invalid field.
/// More bytes do not undo it, so a caller stops there. Two kinds of error
/// are not final while bytes may still come, so `refine` returns `Ok(None)`
/// for them and leaves them to `finish`: a main header cut short
/// ([`Error::Incomplete`]), and a limit ([`Error::Limit`]), since the work
/// bounds grow with the bytes fed.
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
    /// The work the refines have spent.
    ledger: Ledger,
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
            ledger: Ledger::default(),
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

    /// Decode the bytes fed so far, if they have at least doubled since the
    /// last time this decoded or tried to.
    ///
    /// The image is the one [`decode_with`] gives with
    /// [`strict`](DecodeOptions::strict) off on all the bytes fed, and its
    /// [`report`](Image::report) lists what they do not hold yet.
    /// `Ok(None)` means there is no new image yet: too few bytes have come
    /// in since the last decode, the main header is not whole, or the decode
    /// reached a limit, which may be the rest of the work bound the earlier
    /// refines left. Any other error is the one `decode_with` gives on
    /// those bytes, and more bytes will not clear it.
    pub fn refine(&mut self) -> Result<Option<Image>> {
        let fed = self.bytes.len();
        if fed == 0 || fed < self.tried.saturating_mul(2) {
            return Ok(None);
        }
        self.tried = fed;
        match decode_metered(&self.bytes, &self.lenient, Some(&self.ledger)) {
            Ok(image) => {
                self.decoded = fed;
                Ok(Some(image))
            }
            // More bytes can complete the header, and raise the work
            // bounds past what the earlier refines spent.
            Err(Error::Incomplete(_) | Error::Limit(_)) => Ok(None),
            Err(error) => Err(error),
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
    pub fn finish(self) -> Result<Image> {
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
                    Error::FileFormat(why)
                } else {
                    Error::Codestream(why)
                })
            }
            decoded => decoded,
        }
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

/// The packet visits and code-block work the decodes of an
/// [`IncrementalDecoder`] have spent, which the next decode's bounds are
/// cut by, so that all of them together stay within the bounds one decode
/// of the same bytes has.
#[derive(Debug, Default)]
pub(crate) struct Ledger {
    /// The packet visits spent.
    packets: Cell<u64>,
    /// The code-block work spent.
    blocks: Cell<u64>,
}

impl Ledger {
    /// The packet visits a decode whose own bound is `bound` may spend.
    pub(crate) fn packets_left(&self, bound: u64) -> u64 {
        self.left(bound, self.packets.get())
    }

    /// The code-block work a decode whose own bound is `bound` may spend.
    pub(crate) fn blocks_left(&self, bound: u64) -> u64 {
        self.left(bound, self.blocks.get())
    }

    /// What is left of `bound` after `spent`. No bound
    /// ([`WorkLimit::Unlimited`](crate::WorkLimit::Unlimited)) stays none.
    fn left(&self, bound: u64, spent: u64) -> u64 {
        if bound == u64::MAX {
            return bound;
        }
        bound.saturating_sub(spent)
    }

    /// Add `work` to the code-block work spent.
    pub(crate) fn spend_blocks(&self, work: u64) {
        self.blocks.set(self.blocks.get().saturating_add(work));
    }

    /// A guard that adds what `meter` has spent to the packet visits spent
    /// when it is dropped, however the decode ends.
    pub(crate) fn record<'a>(&'a self, meter: &'a Meter) -> Record<'a> {
        Record {
            ledger: self,
            meter,
        }
    }
}

/// See [`Ledger::record`].
pub(crate) struct Record<'a> {
    ledger: &'a Ledger,
    meter: &'a Meter,
}

impl Drop for Record<'_> {
    fn drop(&mut self) {
        let spent = &self.ledger.packets;
        spent.set(spent.get().saturating_add(self.meter.spent()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{codestream, tier1, tier2};

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    /// The packet-visit and code-block work bounds one decode of `bytes`
    /// has under the default options.
    fn one_shot_bounds(bytes: &[u8]) -> (u64, u64) {
        let parsed = codestream::parse_header(bytes).unwrap();
        let cs = codestream::parse_tiles_with(bytes, parsed, false).unwrap();
        let tile_bytes = crate::tile_bytes(&cs);
        (
            tier2::decode_visits(tile_bytes),
            tier1::decode_work(tile_bytes),
        )
    }

    /// A feeder that hands over one byte at a time and refines after each
    /// spends, over all its refines, no more packet visits or code-block
    /// work than one decode of the whole file is allowed, and decodes the
    /// file about `log2(n)` times, not `n`.
    #[test]
    fn a_byte_at_a_time_feeder_stays_within_one_decodes_bounds() {
        for name in [
            "layers/lrcp.j2k",
            "layers/rpcl.j2k",
            "layers/bypass_pterm.j2k",
            "jpeg2000_regular_latlon.j2k",
        ] {
            let bytes = fixture(name);
            let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
            let mut images = 0;
            crate::tests::BLOCK_WORK.with(|spent| spent.set(0));
            for byte in &bytes {
                decoder.feed(std::slice::from_ref(byte));
                if decoder.refine().unwrap().is_some() {
                    images += 1;
                }
            }
            let block_work = crate::tests::BLOCK_WORK.with(Cell::get);
            let (packet_bound, block_bound) = one_shot_bounds(&bytes);
            let packets = decoder.ledger.packets.get();
            let blocks = decoder.ledger.blocks.get();
            assert!(packets > 0 && blocks > 0, "{name}: nothing was decoded");
            assert_eq!(blocks, block_work, "{name}: the ledger missed block work");
            assert!(
                packets <= packet_bound,
                "{name}: {packets} > {packet_bound}"
            );
            assert!(blocks <= block_bound, "{name}: {blocks} > {block_bound}");

            // Against the work one decode of the whole file does: the
            // doubling keeps the refines under twice that.
            crate::tests::BLOCK_WORK.with(|spent| spent.set(0));
            let lenient = DecodeOptions::default().with_strict(false);
            let whole = decode_with(&bytes, &lenient).unwrap();
            let once = crate::tests::BLOCK_WORK.with(Cell::get);
            assert!(blocks < 2 * once, "{name}: {blocks} >= 2 * {once}");

            let log2 = usize::BITS - bytes.len().leading_zeros();
            assert!(
                images > 0 && images <= log2 as usize,
                "{name}: {images} images"
            );
            // The last refine was at the largest power of two fed.
            assert_eq!(decoder.bytes_decoded(), 1 << (log2 - 1), "{name}");
            let (whole_samples, finished) = (whole.components, decoder.finish().unwrap());
            assert_eq!(finished, whole_strict(&bytes), "{name}");
            assert_eq!(finished.components, whole_samples, "{name}");
        }
    }

    fn whole_strict(bytes: &[u8]) -> Image {
        decode_with(bytes, &DecodeOptions::default()).unwrap()
    }

    /// A refine that the rest of the bound cannot cover is put off, not
    /// refused, and `finish` still decodes the file under a bound of its
    /// own.
    #[test]
    fn a_refine_past_the_rest_of_the_bound_waits() {
        let bytes = fixture("layers/lrcp.j2k");
        let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
        decoder.ledger.blocks.set(u64::MAX - 1);
        decoder.feed(&bytes);
        assert_eq!(decoder.refine().unwrap(), None);
        assert_eq!(decoder.bytes_decoded(), 0);
        assert_eq!(decoder.finish().unwrap(), whole_strict(&bytes));
    }
}
