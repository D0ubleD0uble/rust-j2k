//! The advice an [`Error::Limit`](crate::Error::Limit) for a work bound or
//! the sample limit carries: a [`WorkLimit::Raised`](crate::WorkLimit::Raised)
//! factor worked out from what the refusal already shows.
//!
//! A bound scales with the file, so `Raised(n)` allows `n` times the default
//! bound. The figure is plain arithmetic on what the refusal knows; nothing
//! is walked or decoded to find it, so a refused file costs no more to
//! refuse. The sample limit's factor is exact for the check it names, as the
//! samples it counts are known from the header; palette expansion or sRGB
//! output is checked after the image area and may need a higher one. Of the
//! work bounds, only the code-block bound, a POC
//! sequence's reach and the code-blocks of the resolutions a reduction drops
//! give the factor the tile needs. A packet walk that runs out of steps shows
//! only that it needs more than it was allowed, so its message names the next
//! factor up. Later tiles are never counted. The messages call the packet work
//! bound's unit a step, as the public docs do; the code calls it a visit.

use crate::tier2::WorkRefusal;

/// The smallest `Raised` factor for `needed` against a `default` bound.
fn factor(needed: u64, default: u64) -> u32 {
    let n = needed.div_ceil(default.max(1)).max(1);
    u32::try_from(n).unwrap_or(u32::MAX)
}

const TRUSTED: &str = "or with `WorkLimit::Unlimited` for trusted input";

/// The message for tile `tile`, whose packet walk was refused for `work`
/// after `spent` of the `allowed` steps (visits) the decode started with were
/// spent. `default` is the default bound.
///
/// A walk that ran out of steps shows only that it needs more than it was
/// allowed, so it names the next factor up, which may not be enough. A POC
/// sequence's reach is a count of packets, so it names the factor its reach
/// needs.
pub(crate) fn packet(
    work: WorkRefusal,
    tile: usize,
    spent: u64,
    allowed: u64,
    default: u64,
) -> String {
    const OPTION: &str = "(see `DecodeOptions::packet_work_limit`)";
    let left = allowed.saturating_sub(spent);
    match work {
        WorkRefusal::PacketWalk => {
            let n = factor(allowed.saturating_add(1), default);
            format!(
                "reading the packet headers of tile {tile} goes past the packet work bound of \
                 {allowed} steps {OPTION}; retry with at least `WorkLimit::Raised({n})` (the \
                 file may need more), {TRUSTED}"
            )
        }
        WorkRefusal::PocReach { reached } => {
            let n = factor(spent.saturating_add(reached), default);
            format!(
                "the `POC` volumes of tile {tile} reach {reached} packets, a step each, more than the {left} \
                 steps left of the packet work bound of {allowed} {OPTION}; retry with at \
                 least `WorkLimit::Raised({n})` (later tiles may need more), {TRUSTED}"
            )
        }
        WorkRefusal::DroppedBlocks { blocks } => {
            let n = factor(spent.saturating_add(blocks), default);
            format!(
                "the resolutions the reduction drops hold {blocks} code-blocks in one \
                 component of tile {tile}, a step each, more than the {left} steps left of the \
                 packet work bound of {allowed} {OPTION}; retry with at least \
                 `WorkLimit::Raised({n})` (later tiles may need more), {TRUSTED}"
            )
        }
    }
}

/// The message for a decode with `strict` off whose missing tiles have
/// `cost` tile-components, a step each, more than the `allowed` steps.
/// `default` is the default bound.
pub(crate) fn missing_tiles(cost: u64, allowed: u64, default: u64) -> String {
    let n = factor(cost, default);
    format!(
        "the missing tiles cost {cost} steps, one for each tile-component, more than the \
         packet work bound of {allowed} steps (see `DecodeOptions::packet_work_limit`); retry \
         with at least `WorkLimit::Raised({n})`, {TRUSTED}"
    )
}

/// The advice for an image of `total` samples against a sample limit whose
/// default is `default`: the smallest factor that admits it, or, past
/// `u32::MAX` times the default, where no `Raised` factor does, only
/// `Unlimited`.
pub(crate) fn samples(total: u64, default: u64) -> String {
    match u32::try_from(total.div_ceil(default.max(1))) {
        Ok(n) => format!("retry with at least `WorkLimit::Raised({n})`, {TRUSTED}"),
        Err(_) => "only `WorkLimit::Unlimited` admits it, for trusted input".into(),
    }
}

/// The message for a tile whose code-blocks need `work` sample-passes when
/// `left` of the bound were left, after earlier tiles spent `spent`.
pub(crate) fn code_blocks(index: usize, work: u64, left: u64, spent: u64, default: u64) -> String {
    let n = factor(spent.saturating_add(work), default);
    format!(
        "tile {index} needs {work} sample-passes to decode its code-blocks, more than the \
         {left} left of the code-block work bound (see \
         `DecodeOptions::code_block_work_limit`); retry with at least \
         `WorkLimit::Raised({n})` (later tiles may need more), {TRUSTED}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(message: &str) -> u32 {
        let (_, rest) = message.split_once("`WorkLimit::Raised(").unwrap();
        rest.split_once(')').unwrap().0.parse().unwrap()
    }

    /// The code-block factor counts what earlier tiles spent.
    #[test]
    fn code_blocks_add_the_work_already_spent() {
        // 100 spent, 10 more needed, 50 a factor: 110 / 50 rounds up to 3.
        assert_eq!(named(&code_blocks(1, 10, 5, 100, 50)), 3);
        assert_eq!(named(&code_blocks(1, 100, 0, 100, 50)), 4);
        assert_eq!(named(&code_blocks(0, 100, 0, 0, 50)), 2);
    }

    /// The POC factor counts the steps earlier tiles spent.
    #[test]
    fn poc_volumes_add_the_visits_already_spent() {
        let reach = WorkRefusal::PocReach { reached: 100 };
        assert_eq!(named(&packet(reach, 0, 100, 150, 50)), 4);
        assert_eq!(named(&packet(reach, 0, 0, 50, 50)), 2);
    }

    /// The code-blocks a reduction drops count the steps already spent.
    #[test]
    fn dropped_blocks_add_the_visits_already_spent() {
        let dropped = WorkRefusal::DroppedBlocks { blocks: 100 };
        assert_eq!(named(&packet(dropped, 0, 100, 150, 50)), 4);
    }

    /// The packet walk names the factor above what it was allowed.
    #[test]
    fn the_packet_walk_names_the_next_factor_up() {
        assert_eq!(named(&packet(WorkRefusal::PacketWalk, 0, 0, 50, 50)), 2);
        assert_eq!(named(&packet(WorkRefusal::PacketWalk, 0, 0, 100, 50)), 3);
    }
}
