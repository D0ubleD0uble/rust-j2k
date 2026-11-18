//! What a decode with `strict` off reports of each tile: the packets
//! missing from it, the bytes its walk left over, and the smallest
//! reduction at which it is complete.

use crate::{ReportEntry, Result, codestream, dwt, tier2};

/// The part of a tile a decode returns, for [`report_part`].
pub(super) struct Scope<'s, 'a, F> {
    /// The tile's coded data.
    pub(super) coded: &'s tier2::CodedData<'a>,
    /// The components reconstructed.
    pub(super) wanted: &'s [bool],
    /// Whether the decode returns less than the image.
    pub(super) windowed: bool,
    /// The footprint of the window on a component at a reduction: `None`
    /// when the window needs all of its code-blocks. Asked only when
    /// `windowed`.
    pub(super) footprint_at: F,
}

/// [`report_tile`] for a decode that returns part of `tile`: only the
/// components `scope` reconstructs count, and of them only the code-blocks
/// the window needs. A missing packet whose code-blocks the samples do not
/// need is not missing from them.
///
/// The smallest complete reduction is the smallest `r` at which, and at
/// every larger one the tile allows, no needed code-block lacks a packet.
/// The window needs other code-blocks at each reduction, so each is
/// checked, over the code-blocks with a packet missing and the components
/// that hold them: at most one pass over them per resolution. A tile whose
/// data ended has paid the packet work bound for each of its components'
/// resolutions, which bounds that.
pub(super) fn report_part<F>(
    header: &codestream::MainHeader,
    tile: &codestream::Tile<'_>,
    reach: &tier2::Reach,
    scope: &Scope<'_, '_, F>,
    reduction: u8,
    cap: Option<u16>,
    entries: &mut Vec<ReportEntry>,
) -> Result<Option<u8>>
where
    F: Fn(usize, u8) -> Result<Option<dwt::Footprint>>,
{
    let layers = u32::from(cap.map_or(header.cod.layers, |cap| cap.min(header.cod.layers)));
    let components = &scope.coded.components;
    let kept: Vec<usize> = (0..components.len())
        .filter(|&c| scope.wanted.get(c).copied().unwrap_or(false))
        .collect();
    // The code-blocks of the components reconstructed with a packet missing
    // from a layer the decode keeps.
    let mut missing = Vec::new();
    for &c in &kept {
        for (res, resolution) in components[c].resolutions.iter().enumerate() {
            for band in &resolution.subbands {
                for block in &band.blocks {
                    if let Some(from) = block.missing_from.filter(|&l| l < layers) {
                        missing.push((c, res, band.kind, block, from));
                    }
                }
            }
        }
    }
    let mut lacking = vec![false; components.len()];
    for &(c, ..) in &missing {
        lacking[c] = true;
    }
    let resolutions = |c: usize| components[c].resolutions.len();
    // The most a reduction can drop of every component reconstructed.
    let most = kept
        .iter()
        .map(|&c| resolutions(c).saturating_sub(1))
        .min()
        .unwrap_or(0);
    // The first layer missing from a code-block the samples need at
    // reduction `r`.
    let first_missing = |r: u8| -> Result<Option<u32>> {
        // Only the components with a code-block missing need a footprint.
        let mut footprints = vec![None; components.len()];
        if scope.windowed {
            for &c in &kept {
                if lacking[c] {
                    footprints[c] = (scope.footprint_at)(c, r)?;
                }
            }
        }
        let mut first: Option<u32> = None;
        for &(c, res, kind, block, from) in &missing {
            let needed = res < resolutions(c).saturating_sub(usize::from(r))
                && footprints[c]
                    .as_ref()
                    .is_none_or(|footprint: &dwt::Footprint| footprint.needs(res, kind, block));
            if needed {
                first = Some(first.map_or(from, |f| f.min(from)));
            }
        }
        Ok(first)
    };
    let (whole, tile_complete) = if missing.is_empty() {
        (layers, Some(0))
    } else {
        let mut tile_complete = None;
        for r in (0..=most).rev() {
            let Ok(r) = u8::try_from(r) else { continue };
            if first_missing(r)?.is_some() {
                break;
            }
            tile_complete = Some(r);
        }
        (first_missing(reduction)?.unwrap_or(layers), tile_complete)
    };
    let leftover = reach.leftover.is_some() || reach.leftover_packed.is_some();
    let tile_complete = tile_complete.filter(|_| !leftover);
    push_tile_entries(tile, reach, whole, layers, tile_complete, entries);
    Ok(tile_complete)
}

/// Add what a non-strict walk of `tile` found to `entries`, and give the
/// smallest reduction at which the tile has every packet. Under a layer cap
/// of `cap`, only the packets of the layers it keeps count: one past it that
/// was not read is not missing.
pub(super) fn report_tile(
    header: &codestream::MainHeader,
    tile: &codestream::Tile<'_>,
    reach: &tier2::Reach,
    reduction: u8,
    cap: Option<u16>,
    entries: &mut Vec<ReportEntry>,
) -> Option<u8> {
    let layers = u32::from(cap.map_or(header.cod.layers, |cap| cap.min(header.cod.layers)));
    let missing = |comp: usize, res: usize| reach.missing_from[comp][res].filter(|&l| l < layers);
    let resolutions: Vec<usize> = reach.missing_from.iter().map(Vec::len).collect();
    // The smallest reduction at which every resolution kept is whole.
    let most = resolutions.iter().min().map_or(0, |&n| n.saturating_sub(1));
    let leftover = reach.leftover.is_some() || reach.leftover_packed.is_some();
    let tile_complete = (0..=most)
        .find(|&r| {
            resolutions
                .iter()
                .enumerate()
                .all(|(comp, &n)| (0..n.saturating_sub(r)).all(|res| missing(comp, res).is_none()))
        })
        .and_then(|r| u8::try_from(r).ok())
        .filter(|_| !leftover);
    // The leading layers whole at the resolutions this decode keeps.
    let mut whole = layers;
    for (comp, &n) in resolutions.iter().enumerate() {
        for res in 0..n.saturating_sub(usize::from(reduction)) {
            if let Some(from) = missing(comp, res) {
                whole = whole.min(from);
            }
        }
    }
    push_tile_entries(tile, reach, whole, layers, tile_complete, entries);
    tile_complete
}

/// Add to `entries` the packets missing from `tile`, when only the leading
/// `whole` of its `layers` are whole, and the bytes its walk left over.
fn push_tile_entries(
    tile: &codestream::Tile<'_>,
    reach: &tier2::Reach,
    whole: u32,
    layers: u32,
    tile_complete: Option<u8>,
    entries: &mut Vec<ReportEntry>,
) {
    if whole < layers {
        entries.push(ReportEntry::MissingPackets {
            tile: tile.index,
            complete_layers: whole as u16,
            complete_reduction: tile_complete,
        });
    }
    if let Some((at, bytes)) = reach.leftover {
        entries.push(ReportEntry::LeftoverBytes {
            tile: tile.index,
            tile_part: tile.part_at(at),
            bytes: bytes as u64,
            in_packed_headers: false,
        });
    }
    if let Some(bytes) = reach.leftover_packed {
        entries.push(ReportEntry::LeftoverBytes {
            tile: tile.index,
            tile_part: tile.part_at(tile.data.len()),
            bytes: bytes as u64,
            in_packed_headers: true,
        });
    }
}
