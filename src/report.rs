//! What a decode with [`DecodeOptions::strict`](crate::DecodeOptions::strict)
//! off found missing or skipped: the [`DecodeReport`] on
//! [`Image::report`](crate::Image::report).

use core::fmt;
use core::ops::Range;

/// What a decode left out, and how far the data reached.
///
/// A strict decode (the default) returns an image only when nothing is
/// missing, so its report is always the complete one: no entries and a
/// [`complete_reduction`](Self::complete_reduction) of `Some(0)`. That is also
/// [`DecodeReport::default`]. With
/// [`strict`](crate::DecodeOptions::strict) off, the decode returns the image
/// it could make and says here what it left out.
///
/// ```
/// use rust_j2k::{DecodeOptions, decode_with};
///
/// let bytes = include_bytes!(concat!(
///     env!("CARGO_MANIFEST_DIR"),
///     "/tests/fixtures/jpeg2000_regular_latlon.j2k"
/// ));
/// // Only the first part of the codestream has arrived.
/// let prefix = &bytes[..bytes.len() * 3 / 4];
/// let options = DecodeOptions::default().with_strict(false);
/// let image = decode_with(prefix, &options).expect("the main header is there");
/// let report = &image.report;
/// if !report.is_complete() {
///     for entry in &report.entries {
///         println!("{entry}");
///     }
///     // The finest level the data already holds in full, if any.
///     if let Some(reduction) = report.complete_reduction {
///         let options = options.with_resolution_reduction(reduction);
///         let coarse = decode_with(prefix, &options).expect("decodes");
///         assert!(coarse.report.is_complete());
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodeReport {
    /// What was missing, skipped or irregular, in the order the decoder met
    /// them. Empty when the codestream was whole.
    ///
    /// The list is bounded by the input: at most five entries per tile-part
    /// the codestream carries, plus two. A run of missing tiles is one
    /// [`ReportEntry::MissingTiles`] entry, however many tiles it spans.
    pub entries: Vec<ReportEntry>,
    /// The smallest [`resolution_reduction`](crate::DecodeOptions::resolution_reduction)
    /// at which every tile has every packet: a decode at this reduction, or
    /// any larger one the file allows, gives the same samples as a decode of
    /// the whole codestream. It is counted over all of the data, whatever
    /// reduction this decode used, so one cheap decode at a coarse level says
    /// which finer level the data can already give.
    ///
    /// `None` when no reduction is complete: even the coarsest resolution has
    /// a packet missing, a tile is missing, or a tile has bytes left over (see
    /// [`ReportEntry::LeftoverBytes`]).
    pub complete_reduction: Option<u8>,
    /// The reduction the decode used, which [`Self::is_complete`] compares
    /// against.
    reduction: u8,
}

impl Default for DecodeReport {
    /// The report of a whole codestream: no entries, complete at reduction 0.
    fn default() -> Self {
        DecodeReport {
            entries: Vec::new(),
            complete_reduction: Some(0),
            reduction: 0,
        }
    }
}

impl DecodeReport {
    /// A report of `entries`, complete at `complete_reduction`, for a decode
    /// at `reduction`. Nothing missing at any reduction is the default report,
    /// whatever the reduction, so it compares equal to a strict decode's.
    pub(crate) fn new(
        entries: Vec<ReportEntry>,
        complete_reduction: Option<u8>,
        reduction: u8,
    ) -> Self {
        if entries.is_empty() && complete_reduction == Some(0) {
            return DecodeReport::default();
        }
        DecodeReport {
            entries,
            complete_reduction,
            reduction,
        }
    }

    /// Whether the samples are what the whole codestream gives at the
    /// reduction this decode used.
    ///
    /// That holds when [`complete_reduction`](Self::complete_reduction) is at
    /// most that reduction, no tile is missing and no tile has bytes left
    /// over. A missing `EOC` marker, a tile-part count that disagrees with the
    /// parts found ([`ReportEntry::TilePartCountMismatch`]), and packets
    /// missing only from the resolutions the reduction drops leave it true.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete_reduction
            .is_some_and(|complete| complete <= self.reduction)
            && !self.entries.iter().any(|entry| {
                matches!(
                    entry,
                    ReportEntry::MissingTiles { .. } | ReportEntry::LeftoverBytes { .. }
                )
            })
    }
}

/// One thing a decode with [`strict`](crate::DecodeOptions::strict) off left
/// out or skipped, on [`DecodeReport::entries`].
///
/// The enum and each of its variants are `#[non_exhaustive]`: a later version
/// can add variants and fields, so match with a `_` arm and with
/// `{ field, .. }`, and [`MissingEoc`](Self::MissingEoc) as `MissingEoc { .. }`.
///
/// Tiles are numbered in raster order from 0, as the codestream's `Isot`
/// numbers them, and a tile's tile-parts from 0, as its `TPsot` does.
///
/// The [`Display`](fmt::Display) text is a diagnostic for a log, not a stable
/// format: do not parse it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReportEntry {
    /// No tile-part of these tiles was found. Their samples are 0 in the
    /// output, after the DC level shift, as `opj_decompress -allow-partial`
    /// writes them: 0 for an unsigned component, not its mid-gray, and 0 for
    /// a signed one. A JP2 palette then maps them as any other index 0.
    #[non_exhaustive]
    MissingTiles {
        /// The tiles, a run of consecutive tile indices.
        tiles: Range<u32>,
    },
    /// Packets of this tile that the decode keeps were never read: the data
    /// ended first, or the `POC` volumes leave them out. A missing packet
    /// adds nothing to its code-blocks, so they decode from the layers that
    /// were read, as a codestream truncated after those layers would.
    ///
    /// A packet cut short keeps each code-block whose data is all there, and
    /// the rest of its code-blocks keep only their earlier layers. A packet
    /// whose header is cut adds nothing.
    #[non_exhaustive]
    MissingPackets {
        /// The tile.
        tile: u32,
        /// How many of the leading quality layers are whole, counted over
        /// the resolutions this decode keeps.
        complete_layers: u16,
        /// The smallest reduction at which this tile has every packet, as
        /// [`DecodeReport::complete_reduction`] counts it, or `None`.
        complete_reduction: Option<u8>,
    },
    /// The input ends inside this tile-part: its `Psot` declares more bytes
    /// than remain. The bytes that are there are read.
    #[non_exhaustive]
    TruncatedTilePart {
        /// The tile.
        tile: u32,
        /// The tile-part, as its `TPsot` numbers it.
        tile_part: u8,
        /// The tile-part's length as its `Psot` declares it, from its `SOT`
        /// marker on.
        declared: u64,
        /// The bytes of it the input holds.
        present: u64,
    },
    /// The tile's `SOT` markers declare a number of tile-parts (`TNsot`, after
    /// OpenJPEG's `TPsot == TNsot` correction when that applies) other than
    /// the number found. The tile decodes from the parts found; any packets
    /// they lack are reported as [`MissingPackets`](Self::MissingPackets).
    #[non_exhaustive]
    TilePartCountMismatch {
        /// The tile.
        tile: u32,
        /// The count the markers declare.
        declared: u16,
        /// The tile-parts found.
        found: u16,
    },
    /// Bytes of this tile were not read: every packet the tile's progression
    /// holds was read, and bytes were left after the last one.
    ///
    /// The tile's samples may be wrong: leftover bytes can mean the packets
    /// were misread. Two encoder bugs leave them in valid tiles. OpenJPEG's
    /// encoder writes duplicate packets under overlapping progression changes,
    /// which decode right, and a phantom packet under `-TP R` or `-TP C` with
    /// empty resolutions, which does not. So the tile counts as incomplete at
    /// every reduction.
    #[non_exhaustive]
    LeftoverBytes {
        /// The tile.
        tile: u32,
        /// The tile-part the leftover bytes start in.
        tile_part: u8,
        /// How many bytes were left.
        bytes: u64,
        /// Whether the bytes are packet headers packed in `PPT` or `PPM`
        /// markers rather than tile data.
        in_packed_headers: bool,
    },
    /// The codestream ends without its `EOC` marker. On its own this changes
    /// no sample.
    #[non_exhaustive]
    MissingEoc,
}

impl fmt::Display for ReportEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReportEntry::MissingTiles { tiles } if tiles.len() == 1 => {
                write!(f, "tile {} is missing; its samples are 0", tiles.start)
            }
            ReportEntry::MissingTiles { tiles } => write!(
                f,
                "tiles {} to {} are missing; their samples are 0",
                tiles.start,
                tiles.end.saturating_sub(1)
            ),
            ReportEntry::MissingPackets {
                tile,
                complete_layers,
                complete_reduction,
            } => {
                write!(
                    f,
                    "tile {tile} has packets missing; {complete_layers} layer(s) are whole"
                )?;
                match complete_reduction {
                    Some(r) => write!(f, ", and every packet is there at reduction {r}"),
                    None => write!(f, ", and no reduction has every packet"),
                }
            }
            ReportEntry::TruncatedTilePart {
                tile,
                tile_part,
                declared,
                present,
            } => write!(
                f,
                "tile {tile} tile-part {tile_part} is cut short: {present} of its {declared} \
                 bytes are there"
            ),
            ReportEntry::TilePartCountMismatch {
                tile,
                declared,
                found,
            } => write!(
                f,
                "tile {tile} declares {declared} tile-part(s) and {found} were found"
            ),
            ReportEntry::LeftoverBytes {
                tile,
                tile_part,
                bytes,
                in_packed_headers,
            } => {
                let place = if *in_packed_headers {
                    "packed packet header"
                } else {
                    "tile data"
                };
                write!(
                    f,
                    "tile {tile} has {bytes} byte(s) of {place} left from tile-part \
                     {tile_part} on, not read; its samples may be wrong"
                )
            }
            ReportEntry::MissingEoc => write!(f, "the codestream ends without its EOC marker"),
        }
    }
}
