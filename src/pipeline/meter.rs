//! What an incremental decoder holds its decodes to together: the work
//! bounds they share (see [`Ledger`]) and the refusals those give.
//!
//! The decode pipeline takes a [`Ledger`] when it is given one, and the
//! incremental decoder builds one for each decode, so both use this module
//! and neither depends on the other.

use std::cell::{Cell, RefCell};
use std::fmt;

use crate::codestream::{MainHeader, Tile};
use crate::tier2::resume::{Repeats, Walks};
use crate::tier2::{CodedData, Meter, Reach, Refused};
use crate::{LimitDetail, LimitKind, Raise};

/// Whether more bytes may lift the refusal `limit`, so that a refine waits
/// on it: the sample limit, and the code-blocks and precincts it allows a
/// tile, grow with the bytes. The kind says whether an option raises it (not
/// past the address space, which is its own kind), and a retry says whether
/// the limit leaves room: it is `None` under `AtMost`, a ceiling the caller
/// set, which more bytes do not raise.
pub(crate) fn lifts_with_bytes(limit: &LimitDetail) -> bool {
    matches!(
        limit.kind(),
        LimitKind::Samples | LimitKind::TileCodeBlocks | LimitKind::TilePrecincts
    ) && matches!(
        limit.retry_with(),
        Some(Raise::Raised(_) | Raise::Unlimited)
    )
}

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

/// What the decodes of an [`IncrementalDecoder`](crate::IncrementalDecoder)
/// keep between them (see [`Ledger`]).
#[derive(Default)]
pub(crate) struct Spent {
    pub(crate) walks: Walks,
    /// The code-block work spent.
    pub(crate) blocks: u64,
    /// The packet visits the walks of tiles not kept have spent.
    pub(crate) repeats: u64,
    /// The code-block bound of the last decode, when it had one. Only the
    /// tests and the `__fuzz` hooks read it.
    #[cfg_attr(not(any(test, feature = "__fuzz")), allow(dead_code))]
    pub(crate) bound: Option<u64>,
}

/// What the decodes of an [`IncrementalDecoder`](crate::IncrementalDecoder)
/// keep between them, so that all of them together stay within the work
/// bounds one decode of the same bytes has. It is built from [`Spent`] for the length of one decode.
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
    pub(crate) repeats: Repeats,
    /// The code-block bound of this decode, once it is known.
    bound: Cell<Option<u64>>,
    /// Which work bound refused the decode, if one did.
    pub(crate) work_refused: Cell<Option<Refusal>>,
    /// Whether the decode refused a tile, by the reduction or a tile cap,
    /// on a codestream whose tile-parts have not closed
    /// ([`Codestream::closed`](crate::codestream::Codestream::closed)).
    pub(crate) held: Cell<bool>,
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
            .field("held", &self.held)
            .field("fed", &self.fed)
            .finish_non_exhaustive()
    }
}

impl Ledger {
    /// A ledger over the walks and the work of the earlier decodes, for a
    /// decode of `fed` bytes.
    pub(crate) fn over(spent: Spent, fed: u64) -> Self {
        Ledger {
            walks: RefCell::new(spent.walks),
            blocks: Cell::new(spent.blocks),
            repeats: Repeats::new(spent.repeats),
            bound: Cell::new(None),
            work_refused: Cell::new(None),
            held: Cell::new(false),
            fed,
        }
    }

    /// What the decodes have spent, this one included.
    pub(crate) fn into_spent(self) -> Spent {
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

    /// Record that the decode failed past the parse of every tile-part, on
    /// an input whose tile-parts have `closed` or not. Where they have not, a
    /// later tile-part may hold a fault the decode gives first, so a refine
    /// waits on the error ([`Waiting::Held`](crate::Waiting::Held)).
    pub(crate) fn hold(&self, closed: bool) {
        self.held.set(!closed);
    }

    /// Add `work` to the code-block work spent.
    pub(crate) fn spend_blocks(&self, work: u64) {
        self.blocks.set(self.blocks.get().saturating_add(work));
    }
}
