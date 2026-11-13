//! The advice an [`Error::Limit`](crate::Error::Limit) for a work bound
//! carries: the smallest [`WorkLimit::Raised`](crate::WorkLimit::Raised)
//! factor the refusal already shows is needed.
//!
//! A bound scales with the file, so `Raised(n)` allows `n` times the default
//! bound. The figure is plain arithmetic on what the refusal knows; nothing
//! is walked or decoded to find it, so a refused file costs no more to
//! refuse. It is a lower bound: later tiles may need more.

/// The smallest `Raised` factor for `needed` against a `default` bound.
fn factor(needed: u64, default: u64) -> u32 {
    let n = needed.div_ceil(default.max(1)).max(1);
    u32::try_from(n).unwrap_or(u32::MAX)
}

const TRUSTED: &str = "or use `WorkLimit::Unlimited` for trusted input";

/// The message for a packet walk that ran out of `allowed` visits, where
/// `default` is the default bound and `why` the plain message. All that is
/// known is that the walk needs more than it was allowed.
pub(crate) fn packet_walk(why: &str, allowed: u64, default: u64) -> String {
    let n = factor(allowed.saturating_add(1), default);
    format!(
        "{why}; it needs at least `WorkLimit::Raised({n})` (later tiles may need more), {TRUSTED}"
    )
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
