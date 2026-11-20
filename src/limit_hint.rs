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
//! work bounds, only the code-block bound, a POC sequence's reach and the
//! code-blocks of the resolutions a reduction drops give the factor the tile
//! needs; the missing-tiles charge gives one the whole file needs at least.
//! A packet walk that runs out of steps shows only that it needs more than it
//! was allowed. Later tiles are never counted.
//! So the work bounds name the larger of what the tiles seen need and twice
//! the factor the bound ran under: a hint that named only the need would
//! climb a factor per tile, each retry costing a decode, and this one takes
//! about log2 of the factor the file needs. The messages call the packet work
//! bound's unit a step, as the public docs do; the code calls it a visit.

use crate::tier2::WorkRefusal;
use crate::{Error, LimitKind, WorkLimit};

/// The smallest `Raised` factor for `needed` against a `default` bound.
fn factor(needed: u64, default: u64) -> u32 {
    let n = needed.div_ceil(default.max(1)).max(1);
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The `Raised` factor a refusal names when it shows `needed` of a bound
/// that allowed `allowed`: what it needs, and at least twice the factor it
/// ran under. A refusal shows only the tiles seen so far, so a ladder of
/// retries that stepped up to each tile's need would take a step per tile,
/// each costing a decode; doubling ends it in about log2 of the factor the
/// file needs. It stops at `u32::MAX`; a bound already under that factor has
/// no higher `Raised` one to name, so it gives `None`.
fn doubled(needed: u64, allowed: u64, default: u64) -> Option<u32> {
    let under = u64::from(factor(allowed, default));
    let n = u64::from(factor(needed, default)).max(under.saturating_mul(2));
    match u32::try_from(n) {
        Ok(n) => Some(n),
        Err(_) if under < u64::from(u32::MAX) => Some(u32::MAX),
        Err(_) => None,
    }
}

/// The [`Error::Limit`] of `kind` for `head`, naming the factor [`doubled`]
/// gives (with `tail` after it), or `Unlimited` when none does.
fn escalated(kind: LimitKind, raised: Option<u32>, head: &str, tail: &str) -> Error {
    match raised {
        Some(n) => Error::limit_retry(
            kind,
            WorkLimit::Raised(n),
            format!("{head}; retry with `WorkLimit::Raised({n})` ({tail}), {TRUSTED}"),
        ),
        None => Error::limit_retry(
            kind,
            WorkLimit::Unlimited,
            format!(
                "{head}, and no `Raised` factor is higher; only `WorkLimit::Unlimited` admits \
                 it, for trusted input"
            ),
        ),
    }
}

const TRUSTED: &str = "or with `WorkLimit::Unlimited` for trusted input";

/// The [`Error::Limit`] for tile `tile`, whose packet walk was refused for `work`
/// after `spent` of the `allowed` steps (visits) the decode started with were
/// spent. `default` is the default bound.
///
/// A walk that ran out of steps shows only that it needs more than it was
/// allowed, so it names twice the factor it ran under (see [`doubled`]),
/// which may not be enough. A POC sequence's reach is a count of packets, so
/// it names the factor its reach needs.
pub(crate) fn packet(
    work: WorkRefusal,
    tile: usize,
    spent: u64,
    allowed: u64,
    default: u64,
) -> Error {
    const OPTION: &str = "(see `DecodeOptions::packet_work_limit`)";
    let left = allowed.saturating_sub(spent);
    match work {
        WorkRefusal::PacketWalk => escalated(
            LimitKind::PacketWork,
            doubled(allowed.saturating_add(1), allowed, default),
            &format!(
                "reading the packet headers of tile {tile} goes past the packet work bound of \
                 {allowed} steps {OPTION}"
            ),
            "the file may need more",
        ),
        WorkRefusal::PocReach { reached } => escalated(
            LimitKind::PacketWork,
            doubled(spent.saturating_add(reached), allowed, default),
            &format!(
                "the `POC` volumes of tile {tile} reach {reached} packets, a step each, more \
                 than the {left} steps left of the packet work bound of {allowed} {OPTION}"
            ),
            "later tiles may need more",
        ),
        WorkRefusal::DroppedBlocks { blocks } => escalated(
            LimitKind::PacketWork,
            doubled(spent.saturating_add(blocks), allowed, default),
            &format!(
                "the resolutions the reduction drops hold {blocks} code-blocks in one \
                 component of tile {tile}, a step each, more than the {left} steps left of the \
                 packet work bound of {allowed} {OPTION}"
            ),
            "later tiles may need more",
        ),
    }
}

/// The [`Error::Limit`] for a decode with `strict` off whose missing tiles have
/// `cost` tile-components, a step each, more than the `allowed` steps.
/// `default` is the default bound.
///
/// The charge is exact for the tiles missing, but the tiles read after it
/// spend steps too, so it names the factor [`doubled`] gives, as a packet walk
/// does.
pub(crate) fn missing_tiles(cost: u64, allowed: u64, default: u64) -> Error {
    escalated(
        LimitKind::PacketWork,
        doubled(cost, allowed, default),
        &format!(
            "the missing tiles cost {cost} steps, one for each tile-component, more than the \
             packet work bound of {allowed} steps (see `DecodeOptions::packet_work_limit`)"
        ),
        "the tiles read may need more",
    )
}

/// The advice for an image of `total` samples against a sample limit whose
/// default is `default`: the smallest factor that admits it, or, past
/// `u32::MAX` times the default, where no `Raised` factor does, only
/// `Unlimited`. The [`WorkLimit`] it names, and the words that name it.
pub(crate) fn samples(total: u64, default: u64) -> (WorkLimit, String) {
    match u32::try_from(total.div_ceil(default.max(1))) {
        Ok(n) => (
            WorkLimit::Raised(n),
            format!("retry with at least `WorkLimit::Raised({n})`, {TRUSTED}"),
        ),
        Err(_) => (
            WorkLimit::Unlimited,
            "only `WorkLimit::Unlimited` admits it, for trusted input".into(),
        ),
    }
}

/// The [`Error::Limit`] of `kind` for a tile whose code-blocks or precincts
/// need a sample limit of `needed` samples, under one that allowed `allowed`
/// (see [`samples::over_tile`](crate::samples::over_tile)). The tiles after
/// it are not counted, so it names the factor [`doubled`] gives.
pub(crate) fn tile_geometry(
    kind: LimitKind,
    head: &str,
    needed: u64,
    allowed: u64,
    default: u64,
) -> Error {
    escalated(
        kind,
        doubled(needed, allowed, default),
        head,
        "later tiles may need more",
    )
}

/// The [`Error::Limit`] for a tile whose code-blocks need `work` sample-passes when
/// `left` of the bound were left, after earlier tiles spent `spent`. It
/// carries the tile and its work as [`refused_at`](crate::LimitDetail::refused_at).
pub(crate) fn code_blocks(index: u32, work: u64, left: u64, spent: u64, default: u64) -> Error {
    let refusal = escalated(
        LimitKind::CodeBlockWork,
        doubled(
            spent.saturating_add(work),
            spent.saturating_add(left),
            default,
        ),
        &format!(
            "tile {index} needs {work} sample-passes to decode its code-blocks, more than the \
             {left} left of the code-block work bound (see \
             `DecodeOptions::code_block_work_limit`)"
        ),
        "later tiles may need more",
    );
    refusal.refused_at(index, work)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The factor `error` names, read from its data, after checking that
    /// its message names the same one.
    fn named(error: &Error) -> u32 {
        let Error::Limit(limit) = error else {
            panic!("{error:?}");
        };
        let Some(WorkLimit::Raised(n)) = limit.retry_with() else {
            panic!("{error:?}");
        };
        assert!(
            limit
                .message()
                .contains(&format!("`WorkLimit::Raised({n})`")),
            "{error:?}"
        );
        n
    }

    /// The code-block factor counts what earlier tiles spent, and is at least
    /// twice the factor the bound was at.
    #[test]
    fn code_blocks_add_the_work_already_spent() {
        // 100 spent, 10 more needed, 50 a factor: 110 / 50 rounds up to 3,
        // but the bound was at 105 / 50 = 3, doubled 6.
        assert_eq!(named(&code_blocks(1, 10, 5, 100, 50)), 6);
        // 200 needed against a bound at 2: the need, 4, is the doubling.
        assert_eq!(named(&code_blocks(1, 100, 0, 100, 50)), 4);
        // 500 needed against a bound at 2: the need, 10, beats the doubling.
        assert_eq!(named(&code_blocks(1, 400, 0, 100, 50)), 10);
        assert_eq!(named(&code_blocks(0, 100, 0, 0, 50)), 2);
    }

    /// The code-block refusal carries the tile and the work its message
    /// names.
    #[test]
    fn code_blocks_carry_the_tile_and_the_work() {
        let Error::Limit(limit) = code_blocks(7, 400, 0, 100, 50) else {
            panic!();
        };
        let at = limit.refused_at().expect("the tile and its work");
        assert_eq!((at.tile(), at.work()), (7, 400));
        assert!(
            limit
                .message()
                .starts_with("tile 7 needs 400 sample-passes ")
        );
    }

    /// The POC factor counts the steps earlier tiles spent, and is at least
    /// twice the factor the bound was at.
    #[test]
    fn poc_volumes_add_the_visits_already_spent() {
        let reach = WorkRefusal::PocReach { reached: 100 };
        // 200 needed (4) against a bound at 3: doubled, 6.
        assert_eq!(named(&packet(reach, 0, 100, 150, 50)), 6);
        assert_eq!(named(&packet(reach, 0, 0, 50, 50)), 2);
        let far = WorkRefusal::PocReach { reached: 1000 };
        assert_eq!(named(&packet(far, 0, 100, 150, 50)), 22);
    }

    /// The code-blocks a reduction drops count the steps already spent, and
    /// the factor is at least twice the one the bound was at.
    #[test]
    fn dropped_blocks_add_the_visits_already_spent() {
        let dropped = WorkRefusal::DroppedBlocks { blocks: 100 };
        assert_eq!(named(&packet(dropped, 0, 100, 150, 50)), 6);
    }

    /// A ladder over many tiles that each reach one bound takes about log2
    /// of the tile count in steps, not a step for each tile. 64 tiles each
    /// reach one bound of 50 steps; each retry names the factor of the first
    /// tile it cannot walk, and a hint that named only that tile's need took
    /// 63 steps.
    #[test]
    fn a_ladder_over_many_tiles_takes_log_steps() {
        const TILES: u64 = 64;
        const BOUND: u64 = 50;
        let mut factor = 1;
        let mut steps = 0;
        // Tile `k` is refused when the `k` tiles before it spent `k` bounds
        // and its own reach goes past the factor's.
        while let Some(k) = (0..TILES).find(|&k| (k + 1) * BOUND > factor * BOUND) {
            let reach = WorkRefusal::PocReach { reached: BOUND };
            let hint = named(&packet(reach, k as usize, k * BOUND, factor * BOUND, BOUND));
            assert!(u64::from(hint) > factor);
            factor = u64::from(hint);
            steps += 1;
        }
        assert_eq!(steps, 6);
    }

    /// A hint that doubles does not say "at least": it may exceed the need.
    #[test]
    fn a_doubled_hint_does_not_say_at_least() {
        let reach = WorkRefusal::PocReach { reached: 100 };
        for error in [
            packet(reach, 0, 100, 150, 50),
            packet(WorkRefusal::PacketWalk, 0, 0, 50, 50),
            code_blocks(1, 10, 5, 100, 50),
        ] {
            let Error::Limit(limit) = error else { panic!() };
            assert!(!limit.message().contains("at least"), "{limit}");
        }
    }

    /// The missing-tiles charge names the larger of its need and twice the
    /// factor the bound ran under, and does not say "at least".
    #[test]
    fn the_missing_tiles_hint_doubles_the_factor() {
        // The issue's case: 65535 one-sample tiles of 1280 components cost
        // 83884800 steps; at a default of 20 million, a need of 5, under
        // `Raised(4)`: doubled, 8.
        let hint = |cost, allowed| missing_tiles(cost, allowed, 20_000_000);
        let error = hint(83_884_800, 80_000_000);
        assert_eq!(named(&error), 8);
        let Error::Limit(limit) = &error else {
            panic!()
        };
        assert!(!limit.message().contains("at least"), "{limit}");
        // A need past the doubling is named as it is.
        assert_eq!(named(&hint(300_000_000, 40_000_000)), 15);
        // At the default bound: twice it.
        assert_eq!(named(&hint(20_000_001, 20_000_000)), 2);
    }

    /// The packet walk names twice the factor it ran under.
    #[test]
    fn the_packet_walk_doubles_the_factor() {
        let walk = |allowed| named(&packet(WorkRefusal::PacketWalk, 0, 0, allowed, 50));
        assert_eq!(walk(50), 2);
        assert_eq!(walk(100), 4);
        assert_eq!(walk(150), 6);
        assert_eq!(walk(50 * 31), 62);
        // A bound that is not a whole factor (the default of a file that
        // grew) rounds up first: 120 / 50 is 3, doubled 6.
        assert_eq!(walk(120), 6);
    }

    /// Doubling stops at `u32::MAX`, and past it only `Unlimited` is higher.
    #[test]
    fn the_packet_walk_saturates() {
        let max = u64::from(u32::MAX);
        let walk = |allowed| match packet(WorkRefusal::PacketWalk, 0, 0, allowed, 1) {
            Error::Limit(limit) => limit.retry_with(),
            other => panic!("{other:?}"),
        };
        assert_eq!(walk(max / 2 + 1), Some(WorkLimit::Raised(u32::MAX)));
        assert_eq!(walk(max - 1), Some(WorkLimit::Raised(u32::MAX)));
        assert_eq!(walk(max), Some(WorkLimit::Unlimited));
        assert_eq!(walk(u64::MAX), Some(WorkLimit::Unlimited));
    }
}
