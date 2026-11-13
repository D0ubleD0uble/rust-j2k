//! The advice an [`Error::Limit`] for a work bound carries: the smallest
//! [`WorkLimit::Raised`](crate::WorkLimit::Raised) factor that admits the file.
//!
//! A bound scales with the file, so `Raised(n)` admits a file that needs at
//! most `n` times the default bound. The decode stops at the first tile that
//! goes past, so the total the file needs is found here, once, on the error
//! path.

use crate::codestream::Codestream;
use crate::{Error, tier1, tier2};

/// The most times the default packet bound a failed walk is repeated for.
const PROBE_FACTOR: u64 = 4;

/// The smallest `Raised` factor for `needed` against a `default` bound, as a
/// phrase for a message.
fn raised(needed: u64, default: u64) -> String {
    let factor = needed.div_ceil(default.max(1)).max(1);
    match u32::try_from(factor) {
        Ok(factor) => format!("`WorkLimit::Raised({factor})`"),
        Err(_) => "`WorkLimit::Unlimited`".into(),
    }
}

/// The message for a packet walk that ran out of visits in tile `index`
/// after the earlier tiles spent `spent` of `limit`, the visits the decode
/// started with. `default` is the default bound, `why` the plain message.
///
/// The walk is repeated from that tile on, under a larger limit, to count
/// what the whole file takes. That costs at most [`PROBE_FACTOR`] times the
/// default bound. If the repeat fails some other way the message is `why`
/// alone, and if the count passes that much, or `limit` is already that
/// much, the message says the file needs more than it.
pub(crate) fn packet_walk(
    cs: &Codestream<'_>,
    index: usize,
    (spent, limit): (u64, u64),
    default: u64,
    why: &str,
) -> String {
    let cap = default.saturating_mul(PROBE_FACTOR);
    // A caller who has already raised the bound past what is probed gets no
    // repeat: the file needs more than they allowed.
    if limit >= cap {
        return format!(
            "{why}; it needs more than {} times the default bound, so \
             `WorkLimit::Unlimited` is the way to decode it",
            limit / default.max(1)
        );
    }
    let mut left = cap.saturating_sub(spent);
    for (i, tile) in cs.tiles.iter().enumerate().skip(index) {
        let walked = cs
            .tile_header(i)
            .and_then(|header| tier2::decode_packets(&header, tile, &mut left, default));
        match walked {
            Ok(_) => {}
            Err(Error::Limit(m)) if m == tier2::WALK_LIMIT => {
                return format!(
                    "{why}; it needs more than {} times the default bound, so \
                     `WorkLimit::Unlimited` is the way to decode it",
                    cap / default.max(1)
                );
            }
            Err(_) => return why.to_owned(),
        }
    }
    format!(
        "{why}; {} is the smallest factor that admits the file",
        raised(cap - left, default)
    )
}

/// What the code-block bound reports about a tile that goes past it.
pub(crate) struct CodeBlocks {
    /// The tile that went past.
    pub(crate) index: usize,
    /// Its sample-passes.
    pub(crate) work: u64,
    /// The sample-passes the earlier tiles spent.
    pub(crate) spent: u64,
    /// The sample-passes left when the tile began.
    pub(crate) left: u64,
    /// The default bound.
    pub(crate) default: u64,
    /// The default packet bound, and the visits left after the tile's walk.
    pub(crate) visits: (u64, u64),
    /// The decode's resolution reduction.
    pub(crate) reduction: u8,
}

/// The message for a tile whose code-blocks go past the bound. The packets
/// of the tiles after it are walked, without decoding any code-block, to
/// add up what the whole file needs. If that walk fails, the factor is a
/// lower bound and the message says so.
pub(crate) fn code_blocks(cs: &Codestream<'_>, hit: &CodeBlocks) -> String {
    let (poc_bitmap, mut visits) = hit.visits;
    let mut total = hit.spent.saturating_add(hit.work);
    let mut exact = true;
    for (i, tile) in cs.tiles.iter().enumerate().skip(hit.index + 1) {
        let counted = cs.tile_header(i).and_then(|header| {
            let coded = tier2::decode_packets(&header, tile, &mut visits, poc_bitmap)?;
            Ok(tier1::work(&header, &coded, hit.reduction))
        });
        match counted {
            Ok(work) => total = total.saturating_add(work),
            Err(_) => {
                exact = false;
                break;
            }
        }
    }
    let factor = raised(total, hit.default);
    let verdict = if exact {
        format!("{factor} is the smallest factor that admits the file")
    } else {
        format!("it needs at least {factor}")
    };
    format!(
        "code-block decoding exceeds its work bound: tile {} needs {} \
         sample-passes, {} are left (see `DecodeOptions::code_block_work_limit`); \
         {verdict}",
        hit.index, hit.work, hit.left
    )
}
