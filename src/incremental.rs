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
/// `refine` does not have to decode every time it is called, so a caller
/// can call it after every read, however small. It decodes at least each
/// time the bytes fed have doubled since it last decoded, and may decode
/// more often; when it does not decode, it returns `Ok(None)`.
///
/// The work bounds of [`decode`](crate::decode) hold across all the refines
/// together: the packet visits and the code-block work every `refine` has
/// spent are held to the bound one decode of the bytes fed has, raised as
/// the options say. A refine the rest of that bound cannot cover returns
/// `Ok(None)`, so a file whose decode costs about the same however few of
/// its bytes are there may give no image before `finish`, which is a decode
/// of its own, under its own bound. Each refine that returns an image also
/// builds the whole image, as one decode does: it allocates the samples and
/// runs the inverse wavelet and color transforms over them, outside the
/// work bounds. Under the doubling cadence a file of `n` bytes gives at
/// most about `log2(n)` images, so a large image fed in small pieces costs
/// up to that many reconstructions. Each decode holds no more memory than
/// [`decode_with`] holds for the same bytes, and the decoder keeps the
/// bytes fed, and nothing else, between calls.
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
/// More bytes do not undo it, so a caller stops there. That includes an
/// [`Error::Limit`], such as an image past the sample limit. Two refusals
/// are not final while bytes may still come, so `refine` returns `Ok(None)`
/// for them: a main header cut short ([`Error::Incomplete`]), and a work
/// bound the earlier refines' spending cut short, which more bytes raise.
/// `finish` gives either as an error.
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

    /// Decode the bytes fed so far. This decodes at least each time they
    /// have doubled since it last decoded or tried to, and may decode more
    /// often.
    ///
    /// The image is the one [`decode_with`] gives with
    /// [`strict`](DecodeOptions::strict) off on all the bytes fed, and its
    /// [`report`](Image::report) lists what they do not hold yet.
    /// `Ok(None)` means there is no new image yet: too few bytes have come
    /// in since the last decode, the main header is not whole, or the decode
    /// ran out of what the earlier refines left of a work bound. Any error
    /// is the one `decode_with` gives on those bytes, and more bytes will
    /// not clear it.
    pub fn refine(&mut self) -> Result<Option<Image>> {
        let fed = self.bytes.len();
        if fed == 0 || fed < self.tried.saturating_mul(2) {
            return Ok(None);
        }
        self.tried = fed;
        self.ledger.cut.set(false);
        match decode_metered(&self.bytes, &self.lenient, Some(&self.ledger)) {
            Ok(image) => {
                self.decoded = fed;
                Ok(Some(image))
            }
            // More bytes can complete the header, and raise the work
            // bounds past what the earlier refines spent.
            Err(Error::Incomplete(_)) => Ok(None),
            Err(Error::Limit(_)) if self.ledger.cut.get() => Ok(None),
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
    /// Whether the last decode was refused for work that its bound, had
    /// the earlier spending not cut it, may have allowed.
    cut: Cell<bool>,
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

    /// Record that a decode was refused for work, and whether the cut its
    /// bound took may be why.
    pub(crate) fn refuse(&self, by_cut: bool) {
        self.cut.set(by_cut);
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
    /// work than one decode of the whole file is allowed. This holds for
    /// any cadence, since the ledger cuts each refine's bounds by what the
    /// earlier ones spent. The last image comes within the last doubling of
    /// the input, as the cadence promises.
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
            assert!(images > 0, "{name}: no image");
            assert!(
                2 * decoder.bytes_decoded() > bytes.len(),
                "{name}: the last image is of {} of {} bytes",
                decoder.bytes_decoded(),
                bytes.len()
            );
            assert_eq!(decoder.finish().unwrap(), whole_strict(&bytes), "{name}");
        }
    }

    /// A file whose packet walk costs about the same however few of its
    /// bytes are there, fed in two halves, still spends no more packet
    /// visits over both refines than one decode of it is allowed: the
    /// second refine waits, and `finish` decodes it.
    #[test]
    fn a_costly_walk_fed_in_halves_stays_within_one_decodes_bound() {
        // Its walk takes about twice the default bound, at any length.
        let bytes = fixture("work_bound/sweep_rpcl_1024_64.j2k");
        let options = DecodeOptions::default().with_packet_work_limit(crate::WorkLimit::Raised(4));
        let packet_bound = 4 * one_shot_bounds(&bytes).0;
        let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
        let half = bytes.len() / 2;
        decoder.feed(&bytes[..half]);
        assert!(
            decoder.refine().unwrap().is_some(),
            "the first half decodes"
        );
        decoder.feed(&bytes[half..]);
        let _ = decoder.refine().unwrap();
        let packets = decoder.ledger.packets.get();
        // The first refine alone spends more than half the bound, so a
        // second at full cost would pass it.
        assert!(2 * packets > packet_bound, "{packets} of {packet_bound}");
        assert!(packets <= packet_bound, "{packets} > {packet_bound}");
        let finished = decoder.finish().unwrap();
        assert_eq!(finished, decode_with(&bytes, &options).unwrap());
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
