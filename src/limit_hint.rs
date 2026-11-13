//! The advice an [`Error::Limit`](crate::Error::Limit) for a work bound
//! carries: a [`WorkLimit::Raised`](crate::WorkLimit::Raised) factor worked
//! out from what the refusal already shows.
//!
//! A bound scales with the file, so `Raised(n)` allows `n` times the default
//! bound. The figure is plain arithmetic on what the refusal knows; nothing
//! is walked or decoded to find it, so a refused file costs no more to
//! refuse. Only the code-block bound and a POC sequence's reach give the
//! factor the tile needs. A packet walk that runs out of visits shows only
//! that it needs more than it was allowed, so its message names the next
//! factor up. Later tiles are never counted.

use crate::tier2;

/// The smallest `Raised` factor for `needed` against a `default` bound.
fn factor(needed: u64, default: u64) -> u32 {
    let n = needed.div_ceil(default.max(1)).max(1);
    u32::try_from(n).unwrap_or(u32::MAX)
}

const TRUSTED: &str = "or use `WorkLimit::Unlimited` for trusted input";

/// The message for a packet walk that ran out of `allowed` visits, where
/// `default` is the default bound and `why` the plain message. All that is
/// known is that the walk needs more than it was allowed, so this names the
/// next factor up, which may not be enough.
pub(crate) fn packet_walk(why: &str, allowed: u64, default: u64) -> String {
    let n = factor(allowed.saturating_add(1), default);
    format!(
        "{why}; the next factor up is `WorkLimit::Raised({n})`, but the file may need more, \
         {TRUSTED}"
    )
}

/// The message for a POC sequence that reaches more packets than the walk may
/// visit, after earlier tiles spent `spent` visits. `why` is the refusal,
/// whose count follows [`tier2::POC_REACH`].
pub(crate) fn poc_volumes(why: &str, spent: u64, default: u64) -> String {
    let reached = why
        .strip_prefix(tier2::POC_REACH)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|digits| digits.parse::<u64>().ok());
    match reached {
        Some(reached) => {
            let n = factor(spent.saturating_add(reached), default);
            format!(
                "{why}; the file needs at least `WorkLimit::Raised({n})` (later tiles may need \
                 more), {TRUSTED}"
            )
        }
        None => format!("{why}; {TRUSTED}"),
    }
}

/// The message for a tile whose code-blocks need `work` sample-passes when
/// `left` of the bound were left, after earlier tiles spent `spent`.
pub(crate) fn code_blocks(index: usize, work: u64, left: u64, spent: u64, default: u64) -> String {
    let n = factor(spent.saturating_add(work), default);
    format!(
        "code-block decoding exceeds its work bound: tile {index} needs {work} \
         sample-passes, {left} are left (see `DecodeOptions::code_block_work_limit`); \
         the file needs at least `WorkLimit::Raised({n})` (later tiles may need more), {TRUSTED}"
    )
}
