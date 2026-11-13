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

use crate::tier2::WorkRefusal;

/// The smallest `Raised` factor for `needed` against a `default` bound.
fn factor(needed: u64, default: u64) -> u32 {
    let n = needed.div_ceil(default.max(1)).max(1);
    u32::try_from(n).unwrap_or(u32::MAX)
}

const TRUSTED: &str = "or use `WorkLimit::Unlimited` for trusted input";

/// The message for a tile whose packet walk was refused for `work`, after
/// earlier tiles spent `spent` visits of the `allowed` the decode started
/// with. `default` is the default bound.
///
/// A walk that ran out of visits shows only that it needs more than it was
/// allowed, so it names the next factor up, which may not be enough. A POC
/// sequence's reach is a count of packets, so it names the factor its reach
/// needs.
pub(crate) fn packet(work: WorkRefusal, spent: u64, allowed: u64, default: u64) -> String {
    let why = work.why();
    match work {
        WorkRefusal::PacketWalk => {
            let n = factor(allowed.saturating_add(1), default);
            format!(
                "{why}; the next factor up is `WorkLimit::Raised({n})`, but the file may need \
                 more, {TRUSTED}"
            )
        }
        WorkRefusal::PocReach { reached } => {
            let n = factor(spent.saturating_add(reached), default);
            format!(
                "{why}; the file needs at least `WorkLimit::Raised({n})` (later tiles may need \
                 more), {TRUSTED}"
            )
        }
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

    /// The POC factor counts the visits earlier tiles spent.
    #[test]
    fn poc_volumes_add_the_visits_already_spent() {
        let reach = WorkRefusal::PocReach { reached: 100 };
        assert_eq!(named(&packet(reach, 100, 150, 50)), 4);
        assert_eq!(named(&packet(reach, 0, 50, 50)), 2);
    }

    /// The packet walk names the factor above what it was allowed.
    #[test]
    fn the_packet_walk_names_the_next_factor_up() {
        assert_eq!(named(&packet(WorkRefusal::PacketWalk, 0, 50, 50)), 2);
        assert_eq!(named(&packet(WorkRefusal::PacketWalk, 0, 100, 50)), 3);
    }
}
