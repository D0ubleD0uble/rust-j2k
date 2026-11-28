//! Header reads: [`read_header`], which the crate root's
//! [`read_header_with`](crate::read_header_with) wraps, and what the
//! incremental decoder keeps between its reads of a header that is still
//! growing ([`HeaderCache`]). It also holds [`read_jp2_prefix`], which the
//! one-shot decode reads a JP2 file's header with too.

use std::collections::BTreeMap;
use std::ops::ControlFlow;

use super::decode::{check_format_known, check_options, check_raw_color, check_reduction, pick};
use super::meter;
use crate::{
    ColorOutput, ComponentInfo, DecodeOptions, Error, Header, Jp2Metadata, Result, TileGrid,
    codestream, header, jp2, samples, tier2, window,
};

/// The [`Header`] that [`read_header_with`](crate::read_header_with) returns
/// for `bytes` under `options`.
pub(crate) fn read_header(bytes: &[u8], options: &DecodeOptions) -> Result<Header> {
    read_header_kept(bytes, options, None)
}

/// [`read_header`], going on from what `kept` holds of an earlier call on
/// bytes that `bytes` extend, and keeping in it what a later call on more
/// bytes can go on from (see [`HeaderCache`]). It gives what
/// [`read_header`] gives, but that in a JP2 file the message of an
/// [`Error::Incomplete`] may name where the input cuts the main header, not
/// where the end of a shorter `jp2c` box does.
pub(crate) fn read_header_kept(
    bytes: &[u8],
    options: &DecodeOptions,
    kept: Option<&mut HeaderCache>,
) -> Result<Header> {
    read_header_from(bytes, options, kept, false)
}

/// [`read_header_kept`] for a stream that may still grow, with `strict` off.
/// The decode parses every tile-part before it counts a tile, so an error
/// of the tile-parts may yet give way to a fault in a later one. In a
/// codestream, a tile refusal, by the reduction or a tile cap, is held, and
/// the header given, while the tile-parts have not closed with an `EOC`
/// ([`codestream::Codestream::closed`]). In a JP2 file, an error of its
/// boxes that more bytes can replace is held ([`read_jp2_prefix`]). Boxes
/// can follow the codestream, and the decode checks them first, so every
/// error of the tile-parts is held while a box after the codestream can
/// still be read ([`jp2::Jp2File::after_settled`]). Once none can, the
/// tile-parts are held as a codestream's are, or, where the codestream is
/// the same whatever follows ([`jp2::Jp2File::codestream_fixed`]), only a
/// tile refusal more bytes lift is. Where a box after the codestream can
/// still be read but passes whatever follows
/// ([`jp2::Jp2File::after_passes_on`]), an error of the count of the tiles
/// is held only while the sample limit can grow. Past that, it gives what
/// [`read_header`] gives.
pub(crate) fn read_header_streamed(
    bytes: &[u8],
    options: &DecodeOptions,
    kept: Option<&mut HeaderCache>,
) -> Result<Header> {
    read_header_from(bytes, options, kept, true)
}

/// [`read_header_kept`], holding the errors of the tile-parts where `hold`
/// is set (see [`read_header_streamed`]).
fn read_header_from(
    bytes: &[u8],
    options: &DecodeOptions,
    kept: Option<&mut HeaderCache>,
    hold: bool,
) -> Result<Header> {
    // What is kept is not keyed on `strict`: only reads with it off keep,
    // and only those hold.
    debug_assert!(kept.is_none() || !options.strict);
    debug_assert!(!hold || !options.strict);
    check_options(options)?;
    let Some(kept) = kept else {
        return read_header_cut(bytes, options, None, None, hold);
    };
    // What an earlier call kept of a header cut short, if it holds for
    // these bytes, and kept again only while the header is cut short.
    let mut cut = kept
        .cut
        .take()
        .filter(|cut| cut.holds_for(bytes))
        .map_or_else(KeptCut::default, |cut| *cut);
    let read = read_header_cut(bytes, options, Some(&mut *kept), Some(&mut cut), hold);
    if matches!(read, Err(Error::Incomplete(_))) && cut.keeps() {
        cut.key(bytes);
        kept.cut = Some(Box::new(cut));
    }
    read
}

/// [`read_header_from`], going on from `cut` where an earlier call found
/// the main header cut short.
fn read_header_cut(
    bytes: &[u8],
    options: &DecodeOptions,
    kept: Option<&mut HeaderCache>,
    mut cut: Option<&mut KeptCut>,
    hold: bool,
) -> Result<Header> {
    let reduction = options.resolution_reduction;
    if !bytes.starts_with(&[0xFF, 0x4F]) && jp2::looks_like_jp2(bytes) {
        // Where an earlier call found the input ending before the main
        // header is whole, a call goes on from there, and gives the same
        // while it still ends there.
        if let Some(cut) = cut.as_deref_mut() {
            if let Some(ended) = cut.boxes_still_cut(bytes) {
                return Err(ended);
            }
            if let Some(ended) = cut.main_still_cut(bytes) {
                return Err(ended);
            }
        }
        let prefix = match read_jp2_prefix(bytes, options, hold, cut.as_deref_mut()) {
            Ok(prefix) => prefix,
            // A fault in a box before the codestream that more bytes can
            // change: the main header is not in, and may never be.
            Err(fault) if fault.open => {
                return Err(Error::incomplete(format!(
                    "the input ends inside a box of length 0 before the codestream, at \
                     fault so far: {}",
                    fault.error
                )));
            }
            Err(fault) => {
                // The input ends before the `jp2c` box: the next call walks
                // the boxes from where this one stops.
                if let Some(cut) = cut {
                    if matches!(fault.error, Error::Incomplete(_)) && cut.jp2c.is_none() {
                        cut.boxes = jp2::BoxesCut::start(bytes);
                    }
                }
                return Err(fault.error);
            }
        };
        // Where the codestream starts in the input, which keys what is kept.
        let start = prefix
            .codestream
            .as_ptr()
            .addr()
            .wrapping_sub(bytes.as_ptr().addr());
        // An error held back from the boxes comes before those of the
        // tile-parts. It is held whatever it is, a refusal more bytes lift
        // included, as a fault of a box still to come after the codestream
        // comes first.
        if let Some(held) = &prefix.held {
            if !hold {
                return Err(held.clone());
            }
        } else if !hold || prefix.settled || prefix.passes_on {
            // Once no box after the codestream can be read, the codestream
            // grows as a raw one does, and its tile-parts are held as a raw
            // one's are, or, where it is the same whatever follows, only a
            // tile refusal more bytes lift is
            // ([`jp2::Jp2File::codestream_fixed`]). Where a box there can
            // still be read but passes whatever follows, more bytes change
            // only the sample limit, so only an error they can lift is
            // held ([`Hold::Lifting`]). Otherwise a fault of such a box
            // comes first, so every error of the tile-parts is held, and
            // they are not walked.
            check_geometry(
                prefix.codestream,
                &prefix.parsed,
                &prefix.selection,
                reduction,
                samples::cap(&options.sample_limit, bytes.len()),
                options.strict,
                match (hold, prefix.fixed, prefix.settled) {
                    (false, ..) => Hold::Never,
                    (true, true, _) => Hold::Liftable,
                    (true, false, true) => Hold::UntilClosed,
                    (true, false, false) => Hold::Lifting,
                },
                kept.map(|kept| (kept, start)),
            )?;
        }
        let area = &prefix.selection.area;
        let (width, height) = area.image_extent_at(reduction);
        let mut components = if prefix.selection.subset {
            pick(
                ComponentInfo::of_siz(area, reduction),
                &prefix.selection.order,
            )
        } else {
            prefix.plans.outputs(area, reduction)?
        };
        if let Some(srgb) = &prefix.srgb {
            srgb.describe_outputs(&mut components);
        }
        return Ok(Header {
            width,
            height,
            components,
            max_reduction: max_reduction(&prefix.parsed.header),
            quality_layers: prefix.parsed.header.cod.layers,
            image_area: header::image_area(&prefix.parsed.header.siz),
            tiles: TileGrid::of_siz(&prefix.parsed.header.siz),
            jp2: Some(prefix.metadata),
        });
    }
    check_format_known(bytes)?;
    let parsed = match cut {
        Some(cut) => cut.parse_main(bytes, 0)?,
        None => codestream::parse_header(bytes)?,
    };
    let siz = &parsed.header.siz;
    // The selection's refusals come after the checks before it, as the decode
    // has them. The sample limit counts the tiles the selection reads, so it
    // is held only to a selection that is valid: a refused one is reported as
    // such, as no limit raised would help it.
    let selection = window::select(siz, options);
    if let Ok(selection) = &selection {
        samples::check_read(
            siz,
            selection,
            reduction,
            samples::cap(&options.sample_limit, bytes.len()),
        )?;
    }
    check_raw_color(options, siz.components.len())?;
    check_reduction(&parsed.header, reduction, None)?;
    let selection = selection?;
    check_geometry(
        bytes,
        &parsed,
        &selection,
        reduction,
        samples::cap(&options.sample_limit, bytes.len()),
        options.strict,
        if hold { Hold::UntilClosed } else { Hold::Never },
        kept.map(|kept| (kept, 0)),
    )?;
    let (width, height) = selection.area.image_extent_at(reduction);
    Ok(Header {
        width,
        height,
        components: pick(
            ComponentInfo::of_siz(&selection.area, reduction),
            &selection.order,
        ),
        max_reduction: max_reduction(&parsed.header),
        quality_layers: parsed.header.cod.layers,
        image_area: header::image_area(siz),
        tiles: TileGrid::of_siz(siz),
        jp2: None,
    })
}

/// The steps [`check_geometry`] takes counting tiles under the main header,
/// a step for each resolution of each tile-component, and again under the
/// tiles' own headers, where reading a header costs steps too. Past either
/// budget it leaves the rest of the tiles to the decode, which counts each
/// tile as it reads it. A tile its size admits costs nothing, so only a
/// header that declares many tiles of many components and levels gets this
/// far. Both budgets together took 63 ms for 1000 tiles of 4096 components,
/// each with a tile-part `COD`.
const GEOMETRY_STEPS: u64 = 1 << 22;

/// Hold the tiles `selection` reads to the precincts and code-blocks `cap`
/// allows a tile (see [`tier2::check_tile`]), so that
/// [`read_header_with`](crate::read_header_with) refuses a tile over a cap
/// under the main header that the decode refuses too.
///
/// Each tile is counted under the main header first, which reads nothing
/// past the first `SOT`. A tile whose size shows it within both caps is not
/// counted: it holds at most a code-block for each sample of its components
/// on the reference grid, and at most a precinct for each sample of each
/// resolution, of which the main header gives the count. Only the tiles over
/// a cap under the main header have their tile-part headers read
/// ([`Rescue`]), to count them as the decode does. The tile-parts are
/// walked once in all, as far as each tile counted needs.
///
/// The decode counts every tile it reads under the tile's own header, in
/// index order, so a tile-part `COD` or `COC` can take a lower tile over a
/// cap, and the decode refuses that tile first, as it does a tile whose
/// tile-part `COD` or `COC` gives fewer levels than the reduction takes. So
/// before a tile over a cap under the main header is counted, each lower
/// tile read since the last one counted whose own header could refuse it is
/// counted under that header too, in index order: under a reduction every
/// such tile, and otherwise each a tile-part header could take over a cap
/// (its size does not rule that out at the most resolutions a header can
/// give). Of those, a tile whose first tile-part has no `COD` or `COC` is
/// coded as the main header has it, so [`Rescue::count`] skips it unread.
/// The first tile refused is then the lowest, as in the decode. With
/// `strict` on the tiles read up to the one over a cap are looked up first,
/// as a tile read with no tile-part has the decode refuse the file before
/// it counts any. Such
/// a tile with no tile over a cap after it is left to the decode, so a file
/// with no tile over a cap under the main header is still read no further
/// than the first `SOT`. A refusal or a stop at a budget gives the decode's
/// error for the tile-parts where its parse would find one, with `strict`
/// on or off (`codestream::tile_parts_fault`, a walk that keeps no tile
/// record).
///
/// With [`Hold::UntilClosed`], a refusal with no fault in the tile-parts is
/// held while the walk of the tile-parts has not closed (see
/// [`read_header_streamed`]), one more bytes lift included. With
/// [`Hold::Liftable`], only a refusal more bytes lift is held. With
/// [`Hold::Lifting`], an error is held only where a larger cap can take
/// it away.
///
/// `kept` is what an earlier call on the start of the same input left, with
/// where the codestream `bytes` starts in that input (see [`HeaderCache`]).
#[allow(clippy::too_many_arguments)]
fn check_geometry(
    bytes: &[u8],
    parsed: &codestream::ParsedHeader,
    selection: &window::Selection,
    reduction: u8,
    cap: samples::Cap,
    strict: bool,
    hold: Hold,
    kept: Option<(&mut HeaderCache, usize)>,
) -> Result<()> {
    let (mut cache, start) = kept.map_or((None, 0), |(kept, start)| (Some(kept), start));
    // Hashed only for a call that keeps, as a read from the start has no use
    // for it.
    let main = cache.is_some().then(|| MainKey::of(bytes, parsed));
    let mut faults = Faults {
        keeps: cache.is_some(),
        closed: false,
        walk: cache
            .as_deref_mut()
            .zip(main)
            .and_then(|(cache, main)| cache.take_fault(start, main)),
    };
    let counted = count_geometry(
        bytes,
        parsed,
        selection,
        reduction,
        cap,
        strict,
        cache
            .as_deref_mut()
            .zip(main)
            .map(|(cache, main)| (cache, start, main)),
        &mut faults,
    );
    // The decode parses every tile-part before it counts any tile, whatever
    // the selection, so where the count refuses a tile or stops at a budget,
    // that parse's fault comes first.
    let checked = if hold == Hold::Lifting && cap.grows() {
        // Only a larger cap can take a stop away here, so the error of this
        // count is given at once only where the count under the largest cap
        // more bytes can bring, at the samples the target can address,
        // meets the same stop ([`same_stop`]): no byte lifts it. Otherwise
        // a longer input takes it away or gives another, and it is held.
        match settle(counted, &mut faults, bytes, parsed, strict) {
            None => Ok(()),
            Some(now) => {
                let largest = largest_stop(
                    bytes,
                    parsed,
                    selection,
                    reduction,
                    cap,
                    strict,
                    cache
                        .as_deref_mut()
                        .zip(main)
                        .map(|(cache, main)| (cache, start, main)),
                    &mut faults,
                );
                match largest {
                    Some(largest) if same_stop(&now, &largest) => Err(now),
                    _ => Ok(()),
                }
            }
        }
    } else {
        let mut fault = || faults.find(bytes, parsed, strict);
        match counted {
            Ok(true) => Ok(()),
            Ok(false) => fault().map_or(Ok(()), Err),
            Err(error @ (Error::Limit(_) | Error::InvalidOptions(_))) => match fault() {
                Some(fault) => Err(fault),
                // A later tile-part may yet hold a fault, which comes first,
                // whether or not more bytes would lift the refusal.
                None if hold == Hold::UntilClosed && !faults.closed => Ok(()),
                None if hold == Hold::Liftable
                    && matches!(&error, Error::Limit(limit) if meter::lifts_with_bytes(limit)) =>
                {
                    Ok(())
                }
                None => Err(error),
            },
            Err(error) => Err(error),
        }
    };
    if let (Some(cache), Some(main), Some(walk)) = (cache, main, faults.walk) {
        cache.fault = Some(Box::new(KeptFault { start, main, walk }));
    }
    checked
}

/// The error [`check_geometry`] gives for `counted`, the count's result,
/// where it holds none: the decode's fault for the tile-parts where the
/// count refuses a tile or stops at a budget, else the refusal. `None`
/// where it gives none.
fn settle(
    counted: Result<bool>,
    faults: &mut Faults,
    bytes: &[u8],
    parsed: &codestream::ParsedHeader,
    strict: bool,
) -> Option<Error> {
    match counted {
        Ok(true) => None,
        Ok(false) => faults.find(bytes, parsed, strict),
        Err(error @ (Error::Limit(_) | Error::InvalidOptions(_))) => {
            Some(faults.find(bytes, parsed, strict).unwrap_or(error))
        }
        Err(error) => Some(error),
    }
}

/// What [`settle`] gives for the count under the largest cap more bytes can
/// bring `cap` to: one at the samples the target can address. Under
/// [`Hold::Lifting`] the codestream is the same whatever follows, so a call
/// that keeps counts it once for the codestream, main header, selected area
/// and reduction, and keeps what it gives ([`KeptLargest`]). The count keeps
/// nothing else, and runs under the same budgets as the first.
#[allow(clippy::too_many_arguments)]
fn largest_stop(
    bytes: &[u8],
    parsed: &codestream::ParsedHeader,
    selection: &window::Selection,
    reduction: u8,
    cap: samples::Cap,
    strict: bool,
    kept: Option<(&mut HeaderCache, usize, MainKey)>,
    faults: &mut Faults,
) -> Option<Error> {
    let (cache, key) = match kept {
        Some((cache, start, main)) => (
            Some(cache),
            Some(KeptLargest {
                start,
                len: bytes.len(),
                main,
                area: selection.area.image_rect(),
                reduction,
                stop: None,
            }),
        ),
        None => (None, None),
    };
    if let (Some(cache), Some(key)) = (&cache, &key) {
        if let Some(largest) = cache.largest.as_deref().filter(|kept| kept.is_for(key)) {
            return largest.stop.clone();
        }
    }
    #[cfg(test)]
    LARGEST_COUNTS.with(|counts| counts.set(counts.get() + 1));
    let largest = samples::Cap {
        allowed: samples::ADDRESSABLE_SAMPLES,
        ..cap
    };
    let counted = count_geometry(
        bytes, parsed, selection, reduction, largest, strict, None, faults,
    );
    let stop = settle(counted, faults, bytes, parsed, strict);
    if let (Some(cache), Some(key)) = (cache, key) {
        cache.largest = Some(Box::new(KeptLargest {
            stop: stop.clone(),
            ..key
        }));
    }
    stop
}

#[cfg(test)]
thread_local! {
    /// The counts [`largest_stop`] has run on this thread.
    static LARGEST_COUNTS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Whether `now`, the error of the count under the cap of the bytes so
/// far, is the stop the count under the largest cap meets, `largest`: the
/// same error, or a refusal of a limit of the same kind for the same tile,
/// which differs only in what it says of the cap. Then no byte lifts it.
fn same_stop(now: &Error, largest: &Error) -> bool {
    match (now, largest) {
        (Error::Limit(now), Error::Limit(largest)) => {
            now.kind() == largest.kind()
                && now.refused_at().map(|at| at.tile()) == largest.refused_at().map(|at| at.tile())
        }
        _ => format!("{now:?}") == format!("{largest:?}"),
    }
}

/// Whether [`check_geometry`] holds a tile refusal back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    /// It gives every error: a call on the whole input.
    Never,
    /// It holds a tile refusal until the tile-parts close.
    UntilClosed,
    /// It holds only a tile refusal more bytes lift, a cap that grows with
    /// the input ([`meter::lifts_with_bytes`]), as `refine` waits on
    /// it: the tile-parts never close, and no byte changes them
    /// ([`jp2::Jp2File::codestream_fixed`]).
    Liftable,
    /// It holds every error of the count of the tiles, and the fault of
    /// the tile-parts each meets, while the cap can grow
    /// ([`samples::Cap::grows`]), and gives them once it cannot: under a
    /// ceiling the caller set
    /// ([`SampleLimit::AtMost`](crate::SampleLimit::AtMost)), or at the
    /// samples the target can address, as under `Unlimited`. The codestream
    /// is the same whatever follows, and a box after it can still be read
    /// but passes whatever follows ([`jp2::Jp2File::after_passes_on`]), so
    /// more bytes change only the cap. A larger one can take a stop of the
    /// count away: a refusal, a stop at a budget, or the refusal of a lower
    /// tile counted only because a tile over the cap follows it. So a stop
    /// is given at once only where the count under a cap at the samples
    /// the target can address meets the same one ([`same_stop`]), as for a
    /// tile past the address space, and held otherwise.
    Lifting,
}

/// The walk of the tile-parts for the decode's error for them
/// (`codestream::tile_parts_fault`): from the start, or, for a call that
/// keeps, from where the last such call left it on the start of the same
/// input (`codestream::tile_parts_fault_kept`).
#[derive(Default)]
struct Faults {
    /// Whether the call keeps.
    keeps: bool,
    /// Whether the last [`find`](Self::find) found no fault and the walk
    /// closed ([`codestream::Codestream::closed`]).
    closed: bool,
    walk: Option<codestream::KeptWalk>,
}

impl Faults {
    /// The error of the decode's parse of the tile-parts of `bytes`, with
    /// `strict` as given, or `None` if it finds none.
    fn find(
        &mut self,
        bytes: &[u8],
        parsed: &codestream::ParsedHeader,
        strict: bool,
    ) -> Option<Error> {
        let walked = if self.keeps {
            codestream::tile_parts_walk_kept(bytes, parsed, strict, &mut self.walk)
        } else {
            codestream::tile_parts_walk(bytes, parsed, strict)
        };
        self.closed = matches!(walked, Ok(true));
        walked.err()
    }
}

/// The count of [`check_geometry`], before a refusal or a stop at a budget
/// meets the decode's parse of the tile-parts. `Ok(false)` means the count
/// stopped at a budget, leaving the tiles past it to the decode. `faults`
/// is the walk for the decode's error for the tile-parts, which a tile
/// counted can need.
#[allow(clippy::too_many_arguments)]
fn count_geometry(
    bytes: &[u8],
    parsed: &codestream::ParsedHeader,
    selection: &window::Selection,
    reduction: u8,
    cap: samples::Cap,
    strict: bool,
    kept: Option<(&mut HeaderCache, usize, MainKey)>,
    faults: &mut Faults,
) -> Result<bool> {
    let header = &parsed.header;
    let siz = &header.siz;
    let components = siz.components.len() as u64;
    let main_resolutions = resolutions(header);
    // The most resolutions a tile-part header can give a tile: 32 levels in
    // each component.
    let most_resolutions = components.saturating_mul(33);
    // What an earlier call kept, if it was kept for this codestream, main
    // header, selected area and reduction.
    let area = selection.area.image_rect();
    let (mut kept, start, main) = match kept {
        Some((kept, start, main)) => (Some(kept), start, Some(main)),
        None => (None, 0, None),
    };
    let mut earlier = kept
        .as_deref_mut()
        .zip(main)
        .and_then(|(kept, main)| kept.take(start, bytes.len(), main, area, reduction));
    // The tiles read depend on the area alone, so those kept are reused.
    let reads = match &mut earlier {
        Some(earlier) => std::mem::take(&mut earlier.reads),
        None => selection.tiles(siz),
    };
    // For a tile read, whether its size shows it within both caps under the
    // main header, and whether its own header could refuse it: take it over
    // a cap (its size does not show it within them under any header) or,
    // under a reduction, have too few levels for it.
    let fits = |index: u32| {
        if reads.get(index as usize) != Some(&true) {
            return None;
        }
        let rect = siz.tile_rect(index)?;
        let (w, h) = (u64::from(rect.x1 - rect.x0), u64::from(rect.y1 - rect.y0));
        // A component's tile-component is at most w x h, and its resolution
        // at level s at most (w / 2^s + 1) x (h / 2^s + 1), so its
        // resolutions together hold at most 2wh + 2(w + h) samples and one
        // for each resolution.
        let blocks = components.saturating_mul(w.saturating_mul(h));
        let samples = blocks
            .saturating_mul(2)
            .saturating_add(components.saturating_mul(2 * (w + h)));
        let under = |resolutions: u64| {
            blocks <= cap.tile_blocks()
                && samples.saturating_add(resolutions) <= cap.tile_precincts()
        };
        Some((
            under(main_resolutions),
            reduction > 0 || !under(most_resolutions),
        ))
    };
    let mut counts = match &mut earlier {
        Some(earlier) => Some(std::mem::take(&mut earlier.counts)),
        None => kept.is_some().then(BTreeMap::new),
    };
    let mut rescue = Rescue {
        bytes,
        parsed,
        reads: &reads,
        reduction,
        cap,
        strict,
        walk: earlier.as_mut().and_then(|earlier| earlier.walk.take()),
        begun: false,
        known: match &mut earlier {
            Some(earlier) => Some(std::mem::take(&mut earlier.known)),
            None => kept.is_some().then(BTreeMap::new),
        },
        steps: 0,
        fault_free: false,
        faults: std::mem::take(faults),
    };
    let mut count = || {
        let mut steps = 0u64;
        // The lowest tile read since the last one counted whose own header
        // could refuse it.
        let mut pending: Option<u32> = None;
        for index in 0..reads.len() as u32 {
            let Some((main_fits, may_refuse)) = fits(index) else {
                continue;
            };
            if !may_refuse {
                continue;
            }
            let over = if main_fits {
                None
            } else {
                steps = steps.saturating_add(main_resolutions);
                if steps > GEOMETRY_STEPS {
                    return Ok(false);
                }
                over_under_main(header, index, cap, counts.as_mut())
            };
            let Some(over) = over else {
                pending.get_or_insert(index);
                continue;
            };
            // The decode counts the lower tiles under their own headers
            // first, and with `strict` on it refuses a file with a tile read
            // missing before it counts any tile, so the tiles read up to the
            // one over a cap are looked up first: that tile, which walks the
            // tile-parts as far as it, then the lower ones the walk has
            // passed.
            if let Some(lowest) = pending.take() {
                rescue.begin(lowest as usize);
                if strict
                    && (lowest..=index)
                        .rev()
                        .any(|tile| fits(tile).is_some() && rescue.find(tile).is_none())
                {
                    return Err(rescue
                        .walk_fault()
                        .unwrap_or_else(|| over.error(index, cap)));
                }
                for lower in lowest..index {
                    let counted = fits(lower).is_some_and(|(_, may_refuse)| may_refuse);
                    if counted && !rescue.count(lower, None)? {
                        return Ok(false);
                    }
                }
            }
            if !rescue.count(index, Some(over))? {
                return Ok(false);
            }
        }
        Ok(true)
    };
    let counted = count();
    *faults = std::mem::take(&mut rescue.faults);
    if let (Some(kept), Some(main)) = (kept, main) {
        let (walk, known) = (rescue.walk, rescue.known);
        let (counts, known) = (counts.unwrap_or_default(), known.unwrap_or_default());
        // Nothing is kept for a file with no walk and no count: the next
        // call has nothing to go on from. (`known` is filled only by a walk
        // or a count, so its clause is a guard.)
        if walk.is_some() || !counts.is_empty() || !known.is_empty() {
            kept.keep(Box::new(Kept {
                start,
                len: bytes.len(),
                main,
                area,
                reads,
                reduction,
                walk,
                counts,
                known,
            }));
        }
    }
    counted
}

/// Whether tile `index` is over a cap under the main header, as
/// [`tier2::check_tile`] has it, with the [`tier2::tile_counts`] of the
/// tiles already counted taken from `counts`, and this tile's added to it.
fn over_under_main(
    header: &codestream::MainHeader,
    index: u32,
    cap: samples::Cap,
    counts: Option<&mut BTreeMap<u32, (u64, u64)>>,
) -> Option<Over> {
    let refusal = |error: Error| matches!(error, Error::Limit(_)).then_some(Over::Refused(error));
    let Some(counts) = counts else {
        return tier2::check_tile(header, index, cap)
            .err()
            .and_then(refusal);
    };
    let known = match counts.get(&index) {
        Some(&known) => known,
        None => match tier2::tile_counts(header, index) {
            Ok(known) => *counts.entry(index).or_insert(known),
            Err(error) => return refusal(error),
        },
    };
    (!tier2::within_caps(known, cap)).then_some(Over::Counts(known))
}

/// A tile over a cap under the main header: the refusal, or the tile's
/// counts there, from which [`error`](Self::error) makes it only when it is
/// needed, as a call on the bytes fed so far counts the same tiles again.
enum Over {
    Refused(Error),
    Counts((u64, u64)),
}

impl Over {
    /// The refusal of tile `index` under `cap`, the error
    /// [`tier2::check_tile`] gives.
    fn error(self, index: u32, cap: samples::Cap) -> Error {
        match self {
            Over::Refused(error) => error,
            Over::Counts(counts) => tier2::check_counts(index, counts, cap)
                .err()
                .unwrap_or_else(|| Error::inconsistent(format!("tile {index} is within the caps"))),
        }
    }
}

/// What [`IncrementalDecoder::header`](crate::IncrementalDecoder::header)
/// keeps between its calls, so that each goes on from where the last one
/// stopped instead of reading the bytes fed again from the start: the walk
/// of the tile-parts, with where it found the first tile-part of each tile
/// the selection reads, the precincts and code-blocks of each tile counted
/// under the main header, and what the count read of each tile-part that
/// is whole in the input ([`Own`]), and, apart, the walk for the decode's
/// error for the tile-parts ([`Faults`]), and where a header cut short
/// stopped ([`KeptCut`]). The bytes fed only grow, so what a call found in
/// them stays so, and a call gives what a read from the start gives. That
/// holds for every field here: any future way to truncate, reset or rewrite
/// the bytes fed must clear the whole cache. [`MainKey`] checks the main
/// header against a hash, and [`KeptCut`] checks the prefix it was kept for
/// in a debug build, but neither makes a cache safe to reuse over other
/// bytes. It is kept for one codestream, main header, selection and
/// reduction; a call that reads another, as after a change of region,
/// starts again.
#[derive(Default)]
pub(crate) struct HeaderCache {
    /// Boxed, so a decoder with nothing kept holds a pointer.
    kept: Option<Box<Kept>>,
    /// The walk for the decode's error for the tile-parts, which does not
    /// depend on the selection or the reduction, so it is kept apart.
    fault: Option<Box<KeptFault>>,
    /// What the count under the largest cap gave, under [`Hold::Lifting`]
    /// ([`largest_stop`]).
    largest: Option<Box<KeptLargest>>,
    /// Where the last call stopped, if it found the main header cut short.
    /// It depends on no option, so a change of options keeps it.
    cut: Option<Box<KeptCut>>,
}

/// What a header read that found the input ending before the main header
/// is whole keeps, so that the next read goes on from there instead of from
/// the start: where the walk of a JP2 file's boxes before the codestream
/// stopped ([`jp2::BoxesCut`]), or where the parse of the main header
/// stopped ([`codestream::MainCut`]). A read on more bytes then reads each
/// box and marker segment once over the reads, and gives what a read from
/// the start gives. It is kept only while the reads find the main header
/// cut short: the first read that gives anything else drops it. Where the
/// `jp2c` box ends inside its own main header and no byte after it can
/// change the codestream, it keeps that error instead ([`Self::ended`]),
/// and until the bytes fed show whether that is so, the error with where
/// the walk that finds where the codestream ends stopped
/// ([`Self::unended`]).
#[derive(Default)]
pub(crate) struct KeptCut {
    /// The bytes of the input when it was kept.
    len: usize,
    /// In a JP2 file, where the walk of the boxes before the codestream
    /// stopped.
    boxes: Option<jp2::BoxesCut>,
    /// In a JP2 file, where the codestream starts, once a read has read the
    /// boxes before it and they pass. More bytes change neither.
    jp2c: Option<usize>,
    /// Where the parse of the main header stopped, for the codestream that
    /// starts at `.0` in the input.
    main: Option<(usize, Box<codestream::MainCut>)>,
    /// In a JP2 file whose codestream no appended byte changes
    /// ([`jp2::Jp2File::after_settled`]), and whose main header that
    /// codestream, the `jp2c` box's payload, cuts short, the error every
    /// read of more bytes gives: the boxes before the codestream, the box
    /// and its payload are all in and do not change.
    ended: Option<Error>,
    /// In a JP2 file whose `jp2c` box ends inside its own main header and
    /// within the input, where the walk that finds where the codestream ends
    /// stopped ([`jp2::Jp2File::end_walk`]), and the error the read gave.
    /// While that walk, gone on with on more bytes, finds no end or fault,
    /// the codestream is the box's payload, and every read gives that error
    /// (see [`Self::main_still_cut`]).
    unended: Option<(jp2::EndWalk, Error)>,
    /// A hash of a sample of the input when it was kept ([`sample_hash`]),
    /// to check in a debug build that a no shorter input still starts with
    /// it.
    #[cfg(debug_assertions)]
    prefix: u64,
}

/// How many bytes [`sample_hash`] takes from each end of its input.
#[cfg(debug_assertions)]
const SAMPLE: usize = 4096;

#[cfg(test)]
thread_local! {
    /// The bytes [`sample_hash`] has hashed on this thread.
    static HASHED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A hash of the length of `bytes` and its first and last [`SAMPLE`] bytes
/// (all of it if it is no longer than twice that), for the debug check of
/// [`KeptCut`]. It costs the same for any length, so a call is no dearer as
/// the input grows. It is a sample, so a probabilistic tripwire: a change
/// between the two ends goes unseen.
#[cfg(debug_assertions)]
fn sample_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let (head, tail) = if bytes.len() <= 2 * SAMPLE {
        (bytes, &bytes[..0])
    } else {
        (&bytes[..SAMPLE], &bytes[bytes.len() - SAMPLE..])
    };
    #[cfg(test)]
    HASHED.with(|hashed| hashed.set(hashed.get() + (head.len() + tail.len()) as u64));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.len().hash(&mut hasher);
    head.hash(&mut hasher);
    tail.hash(&mut hasher);
    hasher.finish()
}

impl KeptCut {
    /// Key what is kept on `bytes`, the input it was kept for.
    fn key(&mut self, bytes: &[u8]) {
        self.len = bytes.len();
        #[cfg(debug_assertions)]
        {
            self.prefix = sample_hash(bytes);
        }
    }

    /// Whether what is kept holds for `bytes`: they are no shorter than the
    /// input it was kept for. That is enough only because the bytes fed
    /// only grow (see [`HeaderCache`]): a no shorter input extends the one
    /// it was kept for. A debug build checks a sample of it ([`sample_hash`])
    /// and panics if that changed.
    fn holds_for(&self, bytes: &[u8]) -> bool {
        let holds = bytes.len() >= self.len;
        #[cfg(debug_assertions)]
        debug_assert!(
            !holds || sample_hash(&bytes[..self.len]) == self.prefix,
            "the bytes fed changed under the kept cut: they must only grow"
        );
        holds
    }

    /// Whether there is anything to go on from.
    fn keeps(&self) -> bool {
        self.boxes.is_some()
            || self.main.is_some()
            || self.ended.is_some()
            || self.unended.is_some()
    }

    /// The error a read of `bytes` gives where the walk of the boxes before
    /// the codestream still ends before the `jp2c` box, going on from where
    /// it stopped. `None` where there is no such walk or the read may give
    /// anything else, which drops the walk.
    fn boxes_still_cut(&mut self, bytes: &[u8]) -> Option<Error> {
        let ended = self.boxes.as_mut()?.resume(bytes);
        if ended.is_none() {
            self.boxes = None;
        }
        ended
    }

    /// The error a read of the JP2 file `bytes` gives where the main header
    /// is still cut short, going on from where its parse stopped. `None`
    /// where there is no such parse or the main header may be whole, which
    /// drops the parse.
    ///
    /// The boxes before the codestream passed, and more bytes do not change
    /// them. The codestream the read parses starts where the parse kept
    /// started, and runs at most to the end of the input, so where the
    /// input still ends inside the main header, so does the codestream: a
    /// main header the input cuts short is cut short in any shorter input.
    ///
    /// Where the `jp2c` box ends inside its main header, the parse here runs
    /// on past the box, as the codestream does where the bytes after the box
    /// go on with the header. Once they do not, this gives `None`, and the
    /// read from the start finds the codestream fixed to the box's payload
    /// and keeps its error in [`Self::ended`], which every later call gives.
    ///
    /// Until then, where that read found the `jp2c` box whole and the walk
    /// that finds where the codestream ends cut short, it keeps where that
    /// walk stopped and its error in [`Self::unended`]. A later call goes on
    /// with the walk from there, over the bytes fed since, and gives that
    /// error while the walk still finds no end or fault: the codestream is
    /// then still the box's payload, which is whole and does not change
    /// ([`jp2::Jp2File::end_walk`]), and the boxes before it do not either,
    /// so a read from the start gives the same error. Nothing past the box
    /// is read but by that walk, so the error does not depend on what the
    /// bytes after the box hold, and the walk reads each of them once over
    /// the calls. Where it finds an end or a fault, it is let go and the
    /// call reads from the start.
    fn main_still_cut(&mut self, bytes: &[u8]) -> Option<Error> {
        if let Some(ended) = &self.ended {
            return Some(ended.clone());
        }
        let start = self.jp2c?;
        if let Some((walk, error)) = &mut self.unended {
            if walk.still_cut(bytes.get(start..)?) {
                return Some(error.clone());
            }
            self.unended = None;
        }
        let (at, main) = self.main.take()?;
        let tail = bytes.get(start..).filter(|_| at == start)?;
        match codestream::parse_header_resumed(tail, Some(main)) {
            Err((error @ Error::Incomplete(_), Some(main))) => {
                self.main = Some((start, main));
                Some(error)
            }
            _ => None,
        }
    }

    /// Parse the main header of `codestream`, which starts at `start` in the
    /// input, going on from where an earlier parse of the same codestream
    /// stopped, and keep where this one stops, if the input ends inside the
    /// header.
    fn parse_main(&mut self, codestream: &[u8], start: usize) -> Result<codestream::ParsedHeader> {
        let from = self
            .main
            .take()
            .filter(|(at, main)| *at == start && main.len() <= codestream.len())
            .map(|(_, main)| main);
        codestream::parse_header_resumed(codestream, from).map_err(|(error, main)| {
            self.main = main.map(|main| (start, main));
            error
        })
    }
}

/// [`HeaderCache::largest`]: what [`settle`] gave for the count under the
/// largest cap, and what it was kept for. Under [`Hold::Lifting`] the
/// codestream is whole in the input and no byte appended changes it, so
/// that count gives the same on any longer input for the same codestream,
/// main header, selected area and reduction. A header read keeps `strict`
/// off, and the sample limit is the decoder's, so neither is keyed.
struct KeptLargest {
    /// Where the codestream starts in the input.
    start: usize,
    /// The length of the codestream.
    len: usize,
    main: MainKey,
    area: codestream::markers::Rect,
    reduction: u8,
    stop: Option<Error>,
}

impl KeptLargest {
    /// Whether this was kept for what `key` is for.
    fn is_for(&self, key: &KeptLargest) -> bool {
        self.start == key.start
            && self.len == key.len
            && self.main == key.main
            && self.area == key.area
            && self.reduction == key.reduction
    }
}

/// [`HeaderCache::fault`], and what it was kept for.
struct KeptFault {
    /// Where the codestream starts in the input.
    start: usize,
    main: MainKey,
    /// It holds the bytes of the codestream it was kept for.
    walk: codestream::KeptWalk,
}

/// What a main header is keyed on: its length and a hash of its bytes, so
/// that the cache holds two integers for it, not a copy of the parsed header
/// (whose `PPM` chunks run to several times the input). It is sound because
/// the bytes fed only grow (see [`HeaderCache`]), so a main header read
/// whole stays the same, and because a change of options drops the cache.
/// The hash is a cheap defense against a read of another input on the same
/// cache, and probabilistic.
#[derive(Clone, Copy, PartialEq, Eq)]
struct MainKey {
    len: usize,
    hash: u64,
}

impl MainKey {
    /// The key of the main header `parsed` read from the start of `bytes`.
    fn of(bytes: &[u8], parsed: &codestream::ParsedHeader) -> Self {
        use std::hash::{Hash, Hasher};
        let len = parsed.main_header_len();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes[..len.min(bytes.len())].hash(&mut hasher);
        MainKey {
            len,
            hash: hasher.finish(),
        }
    }
}

/// [`HeaderCache`]'s contents, and what they were kept for.
struct Kept {
    /// Where the codestream starts in the input.
    start: usize,
    /// The bytes of the codestream the last call read.
    len: usize,
    main: MainKey,
    /// The area the selection reads, on the reference grid.
    area: codestream::markers::Rect,
    /// Which tiles it reads.
    reads: Vec<bool>,
    reduction: u8,
    walk: Option<Walk>,
    /// The [`tier2::tile_counts`] under the main header of the tiles
    /// counted under it.
    counts: BTreeMap<u32, (u64, u64)>,
    /// [`Rescue::known`].
    known: BTreeMap<u32, Own>,
}

impl HeaderCache {
    /// What was kept, if it was kept for a codestream at `start` of the
    /// input that `len` bytes of it now extend, with the main header
    /// keyed `main`, the area read `area` and the reduction `reduction`.
    /// Anything else kept is dropped.
    fn take(
        &mut self,
        start: usize,
        len: usize,
        main: MainKey,
        area: codestream::markers::Rect,
        reduction: u8,
    ) -> Option<Box<Kept>> {
        self.kept.take().filter(|kept| {
            kept.start == start
                && kept.len <= len
                && kept.reduction == reduction
                && kept.area == area
                && kept.main == main
        })
    }

    /// Keep `kept` for the next call.
    fn keep(&mut self, kept: Box<Kept>) {
        self.kept = Some(kept);
    }

    /// Let go of what was kept for the selection and the reduction. The
    /// walk for the decode's error for the tile-parts depends on neither,
    /// and the header read always reads with `strict` off, so it is kept.
    pub(crate) fn clear_selection(&mut self) {
        self.kept = None;
        self.largest = None;
    }

    /// The fault walk kept, if it was kept for a codestream at `start` of
    /// the input with the main header keyed `main`. It checks itself that
    /// the input has not shrunk. Anything else kept is dropped.
    fn take_fault(&mut self, start: usize, main: MainKey) -> Option<codestream::KeptWalk> {
        self.fault
            .take()
            .filter(|kept| kept.start == start && kept.main == main)
            .map(|kept| kept.walk)
    }
}

/// The resolutions of tile-components `header` gives a tile.
fn resolutions(header: &codestream::MainHeader) -> u64 {
    header
        .components
        .iter()
        .map(|params| u64::from(params.coding.decomposition_levels) + 1)
        .sum()
}

/// The count of the tiles over a cap under the main header from their own
/// tile-part headers in `bytes`, for [`check_geometry`].
struct Rescue<'a> {
    bytes: &'a [u8],
    parsed: &'a codestream::ParsedHeader,
    /// Which tiles the selection reads.
    reads: &'a [bool],
    reduction: u8,
    cap: samples::Cap,
    strict: bool,
    /// The walk of the tile-parts, from the first tile counted on, and
    /// where it found the first tile-part of each tile the selection reads:
    /// at index `tile - from`, for each tile from the first counted to the
    /// last read, 8 bytes a tile, or 0 where it has found none yet (a `SOT`
    /// is never at 0). `None` until a tile is counted, unless an earlier
    /// call on the start of the input kept one ([`HeaderCache`]).
    walk: Option<Walk>,
    /// Whether [`begin`](Self::begin) has been called.
    begun: bool,
    /// What [`count`](Self::count) read of the first tile-part of each tile
    /// it counted whose first tile-part is whole in the input, so that a
    /// later call on more of the input need not read it again. `None` where
    /// nothing is kept for a later call.
    known: Option<BTreeMap<u32, Own>>,
    /// The steps taken reading and counting the tiles' own headers.
    steps: u64,
    /// Whether [`own_fault`](Self::own_fault) has walked the tile-parts and
    /// found no fault, so that it walks them once at most.
    fault_free: bool,
    /// The walk for the decode's error for the tile-parts.
    faults: Faults,
}

/// What [`Rescue::count`] read of a tile's first tile-part, whole in the
/// input, which reads the same in any input that extends it.
#[derive(Clone, Copy)]
struct Own {
    /// [`may_set_coding`](codestream::FirstPart::may_set_coding).
    sets_coding: bool,
    /// What its header gave, or `None` if it was not read.
    header: Option<OwnHeader>,
}

/// [`Own::header`].
#[derive(Clone, Copy)]
enum OwnHeader {
    /// At fault ([`Rescue::own_fault`]).
    Fault,
    /// Read, and within the reduction (the one [`Kept`] is for): the
    /// resolutions it gives the tile, and its [`tier2::tile_counts`].
    Read {
        resolutions: u64,
        counts: (u64, u64),
    },
}

/// [`Rescue::walk`].
struct Walk {
    parts: codestream::FirstParts,
    from: usize,
    firsts: Vec<usize>,
}

impl<'a> Rescue<'a> {
    /// The walk, started for the tiles from `from` on if it has not started
    /// yet. A tile below where it started is never looked up.
    ///
    /// A walk an earlier call kept on the start of the input goes on, in the
    /// longer input, if it started at `from` or below: it has noted, or will
    /// note, every tile the selection reads from `from` on, where a walk
    /// from the start would. One that started above `from` is dropped.
    fn begin(&mut self, from: usize) -> &mut Walk {
        if !self.begun {
            self.begun = true;
            if self.walk.as_ref().is_some_and(|walk| walk.from > from) {
                self.walk = None;
            }
            if let Some(walk) = &mut self.walk {
                walk.parts.resume();
            }
        }
        let (parsed, reads) = (self.parsed, self.reads);
        self.walk.get_or_insert_with(|| {
            let span = reads
                .iter()
                .rposition(|&read| read)
                .map_or(0, |last| last + 1);
            Walk {
                parts: codestream::FirstParts::new(parsed),
                from,
                firsts: vec![0; span.saturating_sub(from)],
            }
        })
    }

    /// Tile `index`'s first tile-part, or `None` if the walk finds none, or
    /// none that reads as one, which the decode takes for a missing tile.
    fn find(&mut self, index: u32) -> Option<codestream::FirstPart<'a, 'a>> {
        let offset = self.first_part(index as usize)?;
        codestream::first_part_at(self.bytes, self.parsed, offset)
    }

    /// Where tile `index`'s first tile-part starts, or `None` if it has
    /// none. The walk goes on from where it stopped until it finds it,
    /// noting each tile the selection reads that it passes, so the
    /// tile-parts are walked once in all, and no further than the first
    /// tile-part of the last tile counted (to the end where a tile has
    /// none). The walk reads no tile-part header and takes no step of the
    /// budget: its cost is linear in the input.
    fn first_part(&mut self, index: usize) -> Option<usize> {
        let (bytes, reads) = (self.bytes, self.reads);
        let parsed = self.parsed;
        let walk = self.begin(index);
        let from = walk.from;
        let at = index.checked_sub(from)?;
        if *walk.firsts.get(at)? == 0 {
            let firsts = &mut walk.firsts;
            walk.parts.visit(bytes, parsed, |tile, part| {
                let tile = tile as usize;
                let wanted = tile
                    .checked_sub(from)
                    .filter(|_| reads.get(tile) == Some(&true));
                if let Some(first) = wanted.and_then(|seen| firsts.get_mut(seen)) {
                    if *first == 0 {
                        *first = part.offset();
                    }
                }
                if firsts.get(at) == Some(&0) {
                    ControlFlow::Continue(())
                } else {
                    ControlFlow::Break(())
                }
            });
        }
        walk.firsts.get(at).copied().filter(|&offset| offset != 0)
    }

    /// Count tile `index`, over a cap under the main header (`error`) or
    /// within them (`None`), as the decode counts it, and refuse it if the
    /// decode does: a tile with a tile-part under its own header (a
    /// tile-part `COD` or `COC` over the main header's), with the reduction
    /// checked first, as the decode checks it, and one with none with the
    /// decode's error for the tile-parts with `strict` on
    /// ([`walk_fault`](Self::walk_fault)) and not at all with it off (the
    /// decode leaves it out). A tile whose tile-part header is at fault
    /// gets the decode's error for the tile-parts
    /// ([`own_fault`](Self::own_fault)). A header that reads gives counts
    /// that hold: what `tile_counts` refuses, the parse refuses first. So a tile-part `COD`
    /// that brings a tile within the caps is admitted, and one that takes
    /// it over them is refused. A tile within them (`None`) whose first
    /// tile-part has no `COD` or `COC` is admitted unread and uncharged.
    ///
    /// The tiles are counted in index order, as the walk notes the
    /// tile-parts only from the first tile counted on. Only the header read
    /// here is held. `Ok(false)` means the steps passed [`GEOMETRY_STEPS`]
    /// and the rest is left to the decode.
    ///
    /// What [`known`](Self::known) holds of the tile stands in for reading
    /// its tile-part again, and is charged the same steps.
    fn count(&mut self, index: u32, over: Option<Over>) -> Result<bool> {
        let known = self.known.as_ref().and_then(|known| known.get(&index));
        if let Some(&Own {
            sets_coding,
            header,
        }) = known
        {
            if over.is_none() && !sets_coding {
                return Ok(true);
            }
            if let Some(header) = header {
                return self.count_known(index, header);
            }
        }
        // A tile with no tile-part: the decode refuses the file with `strict`
        // on, and leaves the tile out with it off.
        let part = match self.find(index) {
            Some(part) => part,
            None if self.strict => {
                let cap = self.cap;
                return match self
                    .walk_fault()
                    .or_else(|| over.map(|over| over.error(index, cap)))
                {
                    Some(fault) => Err(fault),
                    None => Ok(true),
                };
            }
            None => return Ok(true),
        };
        let whole = part.is_whole();
        let sets_coding = part.may_set_coding();
        // A tile not over a cap under the main header whose tile-part
        // carries no `COD` or `COC` is coded as the main header has it, which
        // passed the reduction and the caps, so it is neither read nor
        // charged.
        if over.is_none() && !sets_coding {
            self.learn(index, whole, sets_coding, None);
            return Ok(true);
        }
        if !self.charge(self.read_cost()) {
            return Ok(false);
        }
        let Ok(own) = part.header() else {
            self.learn(index, whole, sets_coding, Some(OwnHeader::Fault));
            // With `strict` off a fault the input may yet complete is a cut,
            // where the decode's parse ends.
            if !self.strict && part.header_fault_may_be_cut() {
                return Ok(true);
            }
            return self.own_fault();
        };
        let resolutions = resolutions(&own);
        if !self.charge(resolutions) {
            return Ok(false);
        }
        check_reduction(&own, self.reduction, Some(index as usize))?;
        let counts = tier2::tile_counts(&own, index)?;
        self.learn(
            index,
            whole,
            sets_coding,
            Some(OwnHeader::Read {
                resolutions,
                counts,
            }),
        );
        self.check_own(index, counts)
    }

    /// [`count`](Self::count) for a tile whose own header gave `header`.
    fn count_known(&mut self, index: u32, header: OwnHeader) -> Result<bool> {
        if !self.charge(self.read_cost()) {
            return Ok(false);
        }
        let OwnHeader::Read {
            resolutions,
            counts,
        } = header
        else {
            return self.own_fault();
        };
        if !self.charge(resolutions) {
            return Ok(false);
        }
        // It was within the reduction when it was read, and is whole.
        self.check_own(index, counts)
    }

    /// Hold tile `index` to the caps under its own header, which gave its
    /// tile `counts`, as [`tier2::check_tile`] holds it.
    fn check_own(&self, index: u32, counts: (u64, u64)) -> Result<bool> {
        match tier2::check_counts(index, counts, self.cap) {
            Err(error @ Error::Limit(_)) => Err(error),
            _ => Ok(true),
        }
    }

    /// The error for a tile whose own tile-part header is at fault: the
    /// decode's error for the tile-parts, with
    /// `strict` as the read has it (`codestream::tile_parts_fault`), as
    /// the decode parses every tile-part before it counts any tile. A
    /// tile-part header at fault is one that parse finds, if it finds no
    /// fault before it, except, with `strict` off, a fault the input may
    /// yet complete, which [`count`](Self::count) leaves to the decode
    /// unwalked. Where the parse finds none, the tile is left to the
    /// decode. The tile-parts are walked once at most a call, and a call of
    /// [`IncrementalDecoder::header`](crate::IncrementalDecoder::header)
    /// goes on from the walk the last one kept ([`Faults`]).
    fn own_fault(&mut self) -> Result<bool> {
        if !self.fault_free {
            match self.faults.find(self.bytes, self.parsed, self.strict) {
                Some(fault) => return Err(fault),
                None => self.fault_free = true,
            }
        }
        Ok(true)
    }

    /// The steps reading a tile's header costs. It copies the main header's
    /// coding parameters when the tile-part overrides them, so it is charged
    /// a step for each component and, as that copy costs about ten counting
    /// steps per component, ten more, before it is read.
    fn read_cost(&self) -> u64 {
        11 * self.parsed.header.components.len() as u64
    }

    /// Charge `steps`, and whether they are within [`GEOMETRY_STEPS`].
    fn charge(&mut self, steps: u64) -> bool {
        self.steps = self.steps.saturating_add(steps);
        self.steps <= GEOMETRY_STEPS
    }

    /// Note in [`known`](Self::known) what tile `index`'s first tile-part
    /// gave, if it is `whole` in the input.
    fn learn(&mut self, index: u32, whole: bool, sets_coding: bool, header: Option<OwnHeader>) {
        if let Some(known) = self.known.as_mut().filter(|_| whole) {
            known.insert(
                index,
                Own {
                    sets_coding,
                    header,
                },
            );
        }
    }

    /// The error the decode gives for the tile-parts with `strict` on, for a
    /// tile the walk found no tile-part for. The decode parses the tile-parts
    /// before it counts a tile, so it refuses the file for the first fault
    /// its parse finds (in any tile's tile-part headers, or the chain), or
    /// for the missing tile, and never gets to the count. This is the error
    /// of that parse (`codestream::tile_parts_fault`), which reads the
    /// main header in place, the tile-part headers and not the tile data,
    /// and keeps no tile record. `None` only if the parse finds no fault,
    /// which the walk finding no tile-part rules out.
    fn walk_fault(&mut self) -> Option<Error> {
        self.faults.find(self.bytes, self.parsed, true)
    }
}

/// The largest reduction [`check_reduction`] lets the main header take: the
/// fewest decomposition levels of any component.
fn max_reduction(header: &codestream::MainHeader) -> u8 {
    header
        .components
        .iter()
        .map(|params| params.coding.decomposition_levels)
        .min()
        .unwrap_or(0)
}

/// A JP2 file read up to its tile data: the boxes, the codestream's main
/// header, the channel plans and the metadata. Everything here is checked
/// before any tile is read, so [`read_header`](crate::read_header) and
/// [`decode`](crate::decode) share it and report a header fault the same way.
pub(super) struct Jp2Prefix<'a> {
    pub(super) codestream: &'a [u8],
    pub(super) parsed: codestream::ParsedHeader,
    pub(super) plans: jp2::ChannelPlans,
    pub(super) metadata: Jp2Metadata,
    /// The sRGB step, when the caller asked for it.
    pub(super) srgb: Option<jp2::SrgbPlan>,
    /// The area and components the decode returns.
    pub(super) selection: window::Selection,
    /// For a stream that may still grow, the first error of the read that a
    /// later box can replace ([`read_jp2_prefix`]), held back. It is `None`
    /// for a read of the whole input, which returns that error instead.
    pub(super) held: Option<Error>,
    /// Whether no box after the codestream can still be read, whatever
    /// bytes follow ([`jp2::Jp2File::after_settled`]).
    pub(super) settled: bool,
    /// Whether, though a box after the codestream can still be read, the
    /// boxes there that pass now pass whatever bytes follow, and the
    /// codestream is the same ([`jp2::Jp2File::after_passes_on`]). More
    /// bytes then change only the sample limit, so only an error a larger
    /// limit can take away is held ([`Hold::Lifting`]).
    pub(super) passes_on: bool,
    /// Whether the codestream the decode reads is the same whatever bytes
    /// follow, with no box after it read ([`jp2::Jp2File::codestream_fixed`]),
    /// so that the tile-parts' errors are final though they have not closed.
    pub(super) fixed: bool,
}

/// Read a JP2 file up to its tile data, in OpenJPEG's order, all of it
/// before any tile is read:
/// 1. the box walk, then the layout of the palette and channel definition
///    boxes, which OpenJPEG checks as it reads `jp2h`;
/// 2. the codestream's main header, then the sample limit, which OpenJPEG
///    does not have, on the area the selection reads;
/// 3. `ihdr` against SIZ, then the reduction, which OpenJPEG refuses as it
///    reads SIZ and then `COD` or `COC`;
/// 4. the selection, which OpenJPEG refuses when the decoded area is set;
/// 5. the channel plans against the components, then the header boxes after
///    the codestream, which OpenJPEG checks once it has decoded, then the
///    metadata;
/// 6. the sRGB step, which OpenJPEG does not refuse.
///
/// Steps 2 to 4 are in the codestream path's order. The one difference is
/// sRGB output: a codestream declares no color space, so the codestream path
/// refuses sRGB output of three or more components before the reduction and
/// the selection, while here the sRGB step is planned last, from the color
/// space and the channels the plans give.
///
/// With `streamed`, `bytes` are a stream that may still grow, and an error
/// that more bytes can replace is held:
///
/// - A fault in a box the input ends inside, before the codestream or after
///   it ([`jp2::BoxFault::open`]), as more bytes can complete the box or
///   change what it holds. Before the codestream there is no prefix to give,
///   so the fault comes back open; after it, it is held in
///   [`Jp2Prefix::held`].
/// - A refusal of the sRGB step. It comes after the header boxes after the
///   codestream, and a box yet to come there can be at fault, which comes
///   first. It is final where OpenJPEG reads no box there whatever follows
///   ([`jp2::Jp2File::after_settled`]): a last tile-part with `Psot = 0`,
///   or a walk after the codestream stopped by a `jp2c` or an `XLBox` of
///   2^32 or more, or ending on a box of length 0 of a type it does not
///   check, such as `xml `.
///
/// The faults before these, and those of whole boxes, are final, as more
/// bytes change neither the boxes they are in nor the ones before them.
/// The metadata fails only on an internal inconsistency, so it is not held.
///
/// `cut` is what a header read keeps while the main header is cut short
/// ([`KeptCut`]): the main header's parse goes on from there.
pub(super) fn read_jp2_prefix<'a>(
    bytes: &'a [u8],
    options: &DecodeOptions,
    streamed: bool,
    cut: Option<&mut KeptCut>,
) -> std::result::Result<Jp2Prefix<'a>, jp2::BoxFault> {
    let file =
        jp2::parse_boxes(bytes, &options.packet_work_limit).map_err(|fault| jp2::BoxFault {
            open: streamed && fault.open,
            ..fault
        })?;
    jp2::check_box_layout(&file)?;
    let parsed = match cut {
        Some(cut) => {
            // The boxes before the codestream are read and pass, and more
            // bytes do not change them.
            let start = file
                .codestream
                .as_ptr()
                .addr()
                .wrapping_sub(bytes.as_ptr().addr());
            cut.jp2c = Some(start);
            let parsed = cut.parse_main(file.codestream, start);
            // The main header is cut short and no byte appended changes the
            // codestream, so every read of more bytes gives this error:
            // the boxes before it are whole and pass, and nothing it parses
            // changes. Where the walk after the codestream is settled
            // (`after_settled`), a cut main header means the codestream is
            // a whole `jp2c` box's payload, by the arms of
            // `locate_codestream`:
            // - an end found at `EOC`, whatever the walk after it met (a
            //   `jp2c`, an `XLBox` of 2^32 or more, or a box of length 0 of
            //   a type it does not check), a tile-part of `Psot = 0` run to
            //   the end of the input, or one cut two bytes past the box: the
            //   walk stepped through the main header to an `SOT` in the
            //   codestream, so the header is whole, not cut;
            // - a payload that ends in `FF D9` before a tile-part of
            //   `Psot = 0`, or a broken walk: the codestream is the payload.
            //   Where the box is whole, it is fixed. Where the input cuts the
            //   box, the payload is the rest of the input, the walk's own
            //   bytes, which reach an `SOT` or a fault, so the parse does not
            //   stop cut short.
            //
            // Where the walk that finds where the codestream ends is cut
            // short, and the box is whole, the codestream is the box's
            // payload until more bytes let that walk reach an end or a
            // fault, so a later read gives this error while it does not
            // ([`KeptCut::main_still_cut`]).
            if let Err(error @ Error::Incomplete(_)) = &parsed {
                if file.after_settled {
                    cut.main = None;
                    cut.ended = Some(error.clone());
                } else if let Some(walk) = file.end_walk {
                    cut.unended = Some((walk, error.clone()));
                }
            }
            parsed?
        }
        None => codestream::parse_header(file.codestream)?,
    };
    let siz = &parsed.header.siz;
    let cap = samples::cap(&options.sample_limit, bytes.len());
    let selection = window::select(siz, options);
    if let Ok(selection) = &selection {
        samples::check_read(siz, selection, options.resolution_reduction, cap)?;
    }
    jp2::check_against_siz(&file.header, siz)?;
    check_reduction(&parsed.header, options.resolution_reduction, None)?;
    let selection = selection?;
    // A component subset names codestream components, which come out as
    // coded: OpenJPEG bypasses the JP2 channel transforms under `-c`.
    let raw_channels = options.raw_channels || options.components.is_some();
    // A palette expands the area the decode returns.
    let plans = jp2::ChannelPlans::plan(
        &file.header,
        &selection.area,
        options.resolution_reduction,
        cap,
        raw_channels,
        jp2::is_jph(&file.file_type),
    )?;
    let mut held = match jp2::check_after_codestream(&file, raw_channels) {
        Err(fault) if streamed && fault.open => Some(fault.error),
        checked => {
            checked.map_err(|fault| fault.error)?;
            None
        }
    };
    let mut metadata = jp2::metadata(&file, &plans, siz)?;
    if selection.subset {
        metadata.channels = pick(metadata.channels, &selection.order);
    }
    let srgb = match options.color {
        ColorOutput::AsCoded => None,
        ColorOutput::Srgb => {
            let planned = jp2::plan_srgb(
                &metadata,
                &plans,
                &selection.area,
                options.resolution_reduction,
                cap,
            )
            .and_then(|plan| {
                // Chroma replication keys on the parity of the area's origin
                // on the reference grid (the full-resolution one, whatever
                // the reduction), so on part of the image it would not give
                // the crop of the whole image's conversion. An area equal to
                // the image (a window clipped to it, or the only tile) is the
                // whole image.
                if plan.replicates_chroma() && selection.area.image_rect() != siz.image_rect() {
                    return Err(Error::unsupported(
                        "sRGB output from sub-sampled sYCC chroma on an area smaller than the \
                         image, under `DecodeOptions::region`",
                    ));
                }
                Ok(plan)
            });
            match planned {
                Ok(plan) => {
                    // A converted image is sRGB (or gray) and carries no
                    // profile, as OpenJPEG reports it.
                    plan.describe(&mut metadata);
                    Some(plan)
                }
                // Only the first error is held: it is the one the decode
                // gives. With no box after the codestream left to read,
                // none can come first.
                Err(error) if streamed && (held.is_some() || !file.after_settled) => {
                    held.get_or_insert(error);
                    None
                }
                Err(error) => return Err(error.into()),
            }
        }
    };
    Ok(Jp2Prefix {
        codestream: file.codestream,
        parsed,
        plans,
        metadata,
        srgb,
        selection,
        held,
        settled: file.after_settled,
        passes_on: file.after_passes_on,
        fixed: file.codestream_fixed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Region, SampleLimit, Window};

    /// `tiles` tiles of 520 x 512 in a row, each in `parts` tile-parts: over
    /// the precinct cap under the main header (1x1 precincts, 266,240 a
    /// tile), and brought within by a tile-part `COD` of the maximal
    /// precincts in its first tile-part. Each tile's parts come together,
    /// in tile order, so a walk to a tile's first tile-part passes every
    /// tile-part before it. The tiles of `refused` have no tile-part `COD`,
    /// so they are over the cap, and the tiles of `missing` no tile-part.
    fn rescued_tiles(tiles: u32, parts: u8, refused: &[u32], missing: &[u32]) -> Vec<u8> {
        rescued_tiles_of_width(520, tiles, parts, refused, missing)
    }

    /// The same with tiles `width` x 512: 512 gives 2^18 precincts a tile,
    /// which is within the cap, where 520 is over it.
    fn rescued_tiles_of_width(
        width: u32,
        tiles: u32,
        parts: u8,
        refused: &[u32],
        missing: &[u32],
    ) -> Vec<u8> {
        use crate::synth;
        let mut cod = synth::cod_body(1, 0, 4, 0);
        cod[0] = 1;
        cod.push(0x00);
        let siz = synth::siz_body([width * tiles, 512, 0, 0, width, 512, 0, 0], &[[7, 1, 1]]);
        let mut bytes = [
            vec![0xFF, 0x4F],
            synth::seg(0xFF51, &siz),
            synth::seg(0xFF52, &cod),
            synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
        ]
        .concat();
        let own = synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0));
        for tile in (0..tiles).filter(|tile| !missing.contains(tile)) {
            for part in 0..parts {
                let extra: &[u8] = if part == 0 && !refused.contains(&tile) {
                    &own
                } else {
                    &[]
                };
                let data: &[u8] = if part == 0 { &[0] } else { &[] };
                bytes.extend(synth::tile_part(tile as u16, part, parts, extra, data));
            }
        }
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    }

    /// The `SOT` marker segments `read_header_with` reads on `bytes` under
    /// `options`, after checking that it admits them.
    fn sot_reads(bytes: &[u8], options: &DecodeOptions) -> u64 {
        let (reads, header) = sot_reads_of(bytes, options);
        header.expect("the header is admitted");
        reads
    }

    /// The same, and what the header read gives.
    fn sot_reads_of(bytes: &[u8], options: &DecodeOptions) -> (u64, Result<Header>) {
        codestream::tests::SOT_READS.with(|reads| reads.set(0));
        let header = read_header(bytes, options);
        (
            codestream::tests::SOT_READS.with(std::cell::Cell::get),
            header,
        )
    }

    /// The header read walks the tile-parts once to find the first
    /// tile-part of each tile over a cap under the main header, and then
    /// reads each of those by its offset: 64 tiles of 16 tile-parts, all
    /// over the cap and each brought within by its own `COD`, cost the `SOT`s
    /// of one walk, up to the last tile's first tile-part (1009 of 1024),
    /// and one `SOT` a tile. A walk for each batch of 1, 2, 4 and so on
    /// tiles would be seven walks, and one a tile 64.
    #[test]
    fn the_rescue_walks_the_tile_parts_once() {
        let bytes = rescued_tiles(64, 16, &[], &[]);
        for strict in [true, false] {
            let options = DecodeOptions::default().with_strict(strict);
            let reads = sot_reads(&bytes, &options);
            assert_eq!(reads, 63 * 16 + 1 + 64);
            // Within the bar of 1.5 walks.
            assert!(reads * 2 <= 3 * 64 * 16, "{reads}");
        }
        // Not where nothing is over a cap: a tile the size admits is not
        // counted, so nothing past the first `SOT` is read. Tiles of 512 x
        // 512 hold 2^18 precincts, the cap's floor on every target, so none
        // is over it under the default.
        let within = rescued_tiles_of_width(512, 64, 16, &[], &[]);
        assert_eq!(sot_reads(&within, &DecodeOptions::default()), 0);
        // The same where a larger limit lifts the cap past 520 x 512. On a
        // 32-bit target `Raised(4)` cannot lift the precinct cap past the
        // address space (see `ADDRESSABLE_SAMPLES`), so the 520-wide tiles
        // are still over it and the rescue walk runs as under the default.
        let raised = DecodeOptions::default().with_sample_limit(SampleLimit::Raised(4));
        #[cfg(target_pointer_width = "64")]
        let expected = 0;
        #[cfg(not(target_pointer_width = "64"))]
        let expected = 63 * 16 + 1 + 64;
        assert_eq!(sot_reads(&bytes, &raised), expected);
    }

    /// The walk stops at the first tile-part of the last tile over a cap
    /// that the selection reads: a window of tiles 2 and 3 of 64 walks the
    /// 48 tile-parts of tiles 0 to 2 and stops at tile 3's first, and reads
    /// the two tiles' by offset.
    #[test]
    fn the_rescue_walk_stops_at_the_last_tile_read() {
        let bytes = rescued_tiles(64, 16, &[], &[]);
        let window = Window::new(2 * 520, 0, 4 * 520, 512);
        let options = DecodeOptions::default().with_region(Region::Window(window));
        assert_eq!(sot_reads(&bytes, &options), 3 * 16 + 1 + 2);
        // A tile read alone: the walk passes the tile-parts before it.
        let options = DecodeOptions::default().with_region(Region::Tile(63));
        assert_eq!(sot_reads(&bytes, &options), 63 * 16 + 1 + 1);
    }

    /// The walk goes only as far as the tile it counts: where tile 1 of 64
    /// is refused, it walks the 16 tile-parts of tile 0 and tile 1's first,
    /// and reads tiles 0 and 1 by offset. The refusal then looks for the
    /// error of the decode's parse of the tile-parts, which the decode runs
    /// before it counts any tile ([`codestream::tile_parts_fault`]). That
    /// walk reads the `SOT` of every tile-part, and with `strict` on, once,
    /// when tile 0 has all 16 of its tile-parts, looks ahead for a 17th (the
    /// `TPsot == TNsot` correction), which reads each `SOT` after it up to
    /// the `EOC`. With `strict` off the look-ahead only changes the counts
    /// in an error, so it is not taken here. A tile with no tile-part takes
    /// the first walk to the end of the tile-parts.
    #[test]
    fn a_refusal_at_an_early_tile_walks_little() {
        // The `SOT`s the fault walk reads over `parts` tile-parts of 16 a
        // tile.
        let fault_walk = |parts: u64, strict: bool| parts + if strict { parts - 16 + 1 } else { 0 };
        let bytes = rescued_tiles(64, 16, &[1], &[]);
        for strict in [false, true] {
            let options = DecodeOptions::default().with_strict(strict);
            let (reads, header) = sot_reads_of(&bytes, &options);
            assert!(matches!(header, Err(Error::Limit(_))), "{header:?}");
            assert_eq!(reads, 16 + 1 + 2 + fault_walk(64 * 16, strict));
        }
        // Tile 1's tile-parts left out: with `strict` on the walk runs to
        // `EOC` to find it has none, and the file is refused with the
        // decode's error for the tile-parts.
        let missing = rescued_tiles(64, 16, &[], &[1]);
        let (reads, header) = sot_reads_of(&missing, &DecodeOptions::default());
        let decoded = crate::decode_with(&missing, &DecodeOptions::default());
        assert!(matches!(header, Err(Error::Codestream(_))), "{header:?}");
        assert_eq!(
            format!("{:?}", header.err()),
            format!("{:?}", decoded.err())
        );
        assert_eq!(reads, 63 * 16 + 1 + 1 + fault_walk(63 * 16, true));
        // With it off the tile is left out and the rest are read by offset.
        assert_eq!(
            sot_reads(&missing, &DecodeOptions::default().with_strict(false)),
            63 * 16 + 1 + 63
        );
    }

    /// `IncrementalDecoder::header` goes on from what its last call found,
    /// so calling it after each feed reads each `SOT` about once in all: 64
    /// tiles of 16 tile-parts, each over the cap under the main header and
    /// brought within by its own `COD`, fed 5 bytes at a time. A call reads
    /// the `SOT`s fed since the last one, the one where the walk stopped for
    /// want of bytes, and that of a first tile-part not yet whole. A read
    /// from the start on each call reads them all again.
    #[test]
    fn header_after_each_feed_reads_each_sot_about_once() {
        let bytes = rescued_tiles(64, 16, &[], &[]);
        let lenient = DecodeOptions::default().with_strict(false);
        let mut decoder =
            crate::IncrementalDecoder::new(DecodeOptions::default()).expect("valid options");
        let (mut kept, mut afresh, mut calls) = (0, 0, 0u64);
        for chunk in bytes.chunks(5) {
            decoder.feed(chunk);
            codestream::tests::SOT_READS.with(|reads| reads.set(0));
            let header = decoder.header();
            kept += codestream::tests::SOT_READS.with(std::cell::Cell::get);
            let (reads, alone) = sot_reads_of(decoder.bytes(), &lenient);
            afresh += reads;
            calls += 1;
            let alone = match alone {
                Ok(header) => Ok(Some(header)),
                Err(Error::Incomplete(_)) => Ok(None),
                Err(error) => Err(error),
            };
            assert_eq!(format!("{header:?}"), format!("{alone:?}"));
        }
        let parts = 64 * 16;
        eprintln!("{kept} reads in {calls} calls, {afresh} from the start");
        assert!(kept > 0);
        assert!(kept <= parts + 2 * calls, "{kept} reads in {calls} calls");
        assert!(kept * 100 < afresh, "{kept} reads, {afresh} from the start");
    }

    /// The same where each call refuses tile 1, or holds the refusal back
    /// until the `EOC`, and so walks the tile-parts
    /// fed for the decode's error for them first: the walk goes on from
    /// where the last call kept it, so the calls read each `SOT` about once
    /// in all. A walk from the start on each call read 3,083,441 here.
    #[test]
    fn header_after_each_feed_of_a_refused_tile_walks_about_once() {
        let bytes = rescued_tiles(64, 16, &[1], &[]);
        let lenient = DecodeOptions::default().with_strict(false);
        let mut decoder =
            crate::IncrementalDecoder::new(DecodeOptions::default()).expect("valid options");
        let (mut kept, mut calls, mut refusals) = (0, 0u64, 0);
        for chunk in bytes.chunks(5) {
            decoder.feed(chunk);
            codestream::tests::SOT_READS.with(|reads| reads.set(0));
            let header = decoder.header();
            kept += codestream::tests::SOT_READS.with(std::cell::Cell::get);
            calls += 1;
            let alone = match read_header(decoder.bytes(), &lenient) {
                Ok(header) => Ok(Some(header)),
                Err(Error::Incomplete(_)) => Ok(None),
                Err(error) => Err(error),
            };
            let held = held_while_open(&header, &alone, decoder.bytes().len() < bytes.len());
            refusals += u64::from(held || matches!(header, Err(Error::Limit(_))));
            if !held {
                assert_eq!(format!("{header:?}"), format!("{alone:?}"));
            }
        }
        let parts = 64 * 16;
        eprintln!("{kept} reads in {calls} calls, {refusals} refusals");
        assert!(refusals * 10 > calls * 9, "{refusals} of {calls}");
        assert!(kept <= parts + 2 * calls, "{kept} reads in {calls} calls");
    }

    /// The same where tile 0 is refused and tile 1's own header is at
    /// fault: a `COD` of one level under a main `QCD` of one step, which
    /// the decode's parse refuses for the tile-parts. The fault walk stops
    /// in tile 1's first tile-part and is kept there, so the calls still
    /// read each `SOT` about once in all.
    #[test]
    fn header_after_each_feed_of_a_tile_header_at_fault_walks_about_once() {
        use crate::synth;
        let mut cod = synth::cod_body(1, 0, 4, 0);
        cod[0] = 1;
        cod.push(0x00);
        let siz = synth::siz_body([520 * 64, 512, 0, 0, 520, 512, 0, 0], &[[7, 1, 1]]);
        let mut bytes = [
            vec![0xFF, 0x4F],
            synth::seg(0xFF51, &siz),
            synth::seg(0xFF52, &cod),
            synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
        ]
        .concat();
        let own = synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0));
        let at_fault = synth::seg(0xFF52, &synth::cod_body(1, 1, 4, 0));
        for tile in 0..64u16 {
            let header: &[u8] = match tile {
                0 => &[],
                1 => &at_fault,
                _ => &own,
            };
            bytes.extend(synth::tile_part(tile, 0, 16, header, &[0]));
            for part in 1..16 {
                bytes.extend(synth::tile_part(tile, part, 16, &[], &[]));
            }
        }
        bytes.extend([0xFF, 0xD9]);
        let lenient = DecodeOptions::default().with_strict(false);
        assert!(
            matches!(read_header(&bytes, &lenient), Err(Error::Marker(_))),
            "{:?}",
            read_header(&bytes, &lenient)
        );
        let mut decoder =
            crate::IncrementalDecoder::new(DecodeOptions::default()).expect("valid options");
        let (mut kept, mut calls, mut faults) = (0, 0u64, 0);
        for chunk in bytes.chunks(5) {
            decoder.feed(chunk);
            codestream::tests::SOT_READS.with(|reads| reads.set(0));
            let header = decoder.header();
            kept += codestream::tests::SOT_READS.with(std::cell::Cell::get);
            calls += 1;
            faults += u64::from(matches!(header, Err(Error::Marker(_))));
            let alone = match read_header(decoder.bytes(), &lenient) {
                Ok(header) => Ok(Some(header)),
                Err(Error::Incomplete(_)) => Ok(None),
                Err(error) => Err(error),
            };
            if !held_while_open(&header, &alone, decoder.bytes().len() < bytes.len()) {
                assert_eq!(format!("{header:?}"), format!("{alone:?}"));
            }
        }
        let parts = 64 * 16;
        eprintln!("{kept} reads in {calls} calls, {faults} at fault");
        assert!(faults * 10 > calls * 9, "{faults} of {calls}");
        assert!(kept <= parts + 2 * calls, "{kept} reads in {calls} calls");
    }

    /// Whether `header`, an incremental decoder's, holds back the tile
    /// refusal `alone`, the header read's, on bytes that have not reached the
    /// `EOC` (`open`). A later tile-part may hold a fault that comes first,
    /// so the header holds the refusal until the `EOC` is in, whether more
    /// bytes lift it (the default cap on a 64-bit target) or not (the
    /// address space on a 32-bit one).
    fn held_while_open(
        header: &Result<Option<Header>>,
        alone: &Result<Option<Header>>,
        open: bool,
    ) -> bool {
        let held = matches!((header, alone), (Ok(Some(_)), Err(Error::Limit(_))));
        assert!(!held || open, "held at the EOC: {alone:?}");
        held
    }

    /// A gray codestream of `tiles` tiles in a row, whose main header and
    /// tiles' first tile-parts are `main` and `own(tile)`, as marker
    /// segments, with an empty packet in each tile.
    fn tiles_of(grid: [u32; 8], main: &[Vec<u8>], own: impl Fn(u16) -> Vec<u8>) -> Vec<u8> {
        use crate::synth;
        let siz = synth::siz_body(grid, &[[7, 1, 1]]);
        let mut bytes = [vec![0xFF, 0x4F], synth::seg(0xFF51, &siz), main.concat()].concat();
        let tiles = (grid[0] - grid[6]).div_ceil(grid[4]) * (grid[1] - grid[7]).div_ceil(grid[5]);
        for tile in 0..tiles as u16 {
            bytes.extend(synth::tile_part(tile, 0, 1, &own(tile), &[0]));
        }
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    }

    /// The debug check sees a change in the first or last 4 KiB of a long
    /// kept input, and not one between them.
    #[cfg(debug_assertions)]
    #[test]
    #[cfg_attr(
        target_os = "wasi",
        ignore = "a panic aborts on wasm32-wasip1, so `catch_unwind` cannot catch it"
    )]
    fn a_kept_cut_sees_a_change_at_either_end_of_a_long_input() {
        let jp2 = |fill: u8, flip: Option<usize>| {
            let mut bytes = vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];
            bytes.extend_from_slice(&[0, 0, 0, 20, b'f', b't', b'y', b'p', b'j', b'p', b'2', b' ']);
            bytes.extend_from_slice(&[0, 0, 0, 0, b'j', b'p', b'2', b' ']);
            bytes.extend_from_slice(&[0, 0x10, 0, 0, b'x', b'm', b'l', b' ']);
            bytes.resize(1 << 16, fill);
            if let Some(at) = flip {
                bytes[at] ^= 1;
            }
            bytes
        };
        let options = DecodeOptions::default().with_strict(false);
        let kept_for = |bytes: &[u8]| {
            let mut kept = HeaderCache::default();
            let first = read_header_kept(bytes, &options, Some(&mut kept));
            assert!(matches!(first, Err(Error::Incomplete(_))), "{first:?}");
            assert!(kept.cut.is_some(), "the cut was not kept");
            kept
        };
        let base = jp2(7, None);
        let len = base.len();
        // Unchanged bytes, and a change in the middle that the sample does
        // not see, do not panic.
        let mut kept = kept_for(&base);
        let _ = read_header_kept(&jp2(7, Some(len / 2)), &options, Some(&mut kept));
        let mut kept = kept_for(&base);
        let _ = read_header_kept(&base, &options, Some(&mut kept));
        for at in [40, 4000, len - 4000, len - 1] {
            let result = std::panic::catch_unwind(|| {
                let mut kept = kept_for(&base);
                let _ = read_header_kept(&jp2(7, Some(at)), &options, Some(&mut kept));
            });
            let payload = result.expect_err(&format!("a change at {at} went unseen"));
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or_default();
            assert!(message.contains("must only grow"), "at {at}: {message}");
        }
    }

    /// The debug check hashes a bounded sample, so a call costs the same
    /// however much of a large box before the codestream is in: fed in
    /// chunks, the bytes hashed grow with the calls, not with the input.
    #[cfg(debug_assertions)]
    #[test]
    fn the_debug_check_of_a_kept_cut_hashes_a_bounded_sample() {
        let mut bytes = vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];
        bytes.extend_from_slice(&[0, 0, 0, 20, b'f', b't', b'y', b'p', b'j', b'p', b'2', b' ']);
        bytes.extend_from_slice(&[0, 0, 0, 0, b'j', b'p', b'2', b' ']);
        bytes.extend_from_slice(&[0, 0x80, 0, 0, b'x', b'm', b'l', b' ']);
        bytes.resize(8 << 20, 0);
        let options = DecodeOptions::default().with_strict(false);
        let mut kept = HeaderCache::default();
        HASHED.with(|hashed| hashed.set(0));
        let chunk = 64 << 10;
        let calls = bytes.len() / chunk;
        for call in 1..=calls {
            let read = read_header_kept(&bytes[..call * chunk], &options, Some(&mut kept));
            assert!(matches!(read, Err(Error::Incomplete(_))), "{read:?}");
            assert!(kept.cut.is_some(), "the cut was not kept at call {call}");
        }
        let hashed = HASHED.with(|hashed| hashed.replace(0));
        // Each call checks the cut it resumes and keys the one it keeps.
        let bound = (calls as u64) * 2 * (2 * SAMPLE as u64);
        assert!(hashed <= bound, "hashed {hashed} bytes in {calls} calls");
    }

    /// A cut kept for an input holds only for a no shorter input that
    /// starts with it: a debug build panics on one that does not.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "must only grow")]
    fn a_kept_cut_panics_on_other_bytes_of_the_same_length() {
        use crate::synth;
        let siz = synth::siz_body([64, 64, 0, 0, 64, 64, 0, 0], &[[7, 1, 1]]);
        let com = |byte: u8| synth::seg(0xFF64, &[0, 1, byte, byte]);
        let cut_short = |byte: u8| {
            let mut bytes = [vec![0xFF, 0x4F], synth::seg(0xFF51, &siz), com(byte)].concat();
            bytes.extend_from_slice(&[0xFF, 0x52, 0x00]);
            bytes
        };
        let options = DecodeOptions::default().with_strict(false);
        let (one, other) = (cut_short(b'a'), cut_short(b'b'));
        assert_eq!(one.len(), other.len());
        let mut kept = HeaderCache::default();
        let first = read_header_kept(&one, &options, Some(&mut kept));
        assert!(matches!(first, Err(Error::Incomplete(_))), "{first:?}");
        assert!(kept.cut.is_some(), "the cut was not kept");
        let _ = read_header_kept(&other, &options, Some(&mut kept));
    }

    /// What a header read keeps is for one area and reduction, and its walk
    /// for the tiles from where it started on: a read under other options
    /// on the same [`HeaderCache`] gives what [`read_header`] gives. The
    /// decoder drops what it keeps on any change of options, so only a read
    /// here can mix them.
    ///
    /// The first file has three tiles of 2048 x 1024 in 1x1 precincts, the
    /// first cut to 400 wide by the image offset: over the precinct cap
    /// under the default sample limit, but not under `Raised(4)`, while the
    /// other two, brought within by their own `COD`, are over it under
    /// both. Under `Raised(4)` the walk starts at tile 1; under the default
    /// it must start at tile 0, which it refuses, and so must a read of the
    /// whole image after a read of tile 1. In the second, of one level, tile
    /// 1's own `COD` has none, which a reduction of 1 refuses.
    #[test]
    fn what_is_kept_is_for_one_area_reduction_and_walk() {
        use crate::synth;
        let check = |bytes: &[u8], sequence: &[DecodeOptions]| {
            let mut kept = HeaderCache::default();
            for (call, options) in sequence.iter().enumerate() {
                let got = read_header_kept(bytes, options, Some(&mut kept));
                let want = read_header(bytes, options);
                assert_eq!(format!("{got:?}"), format!("{want:?}"), "call {call}");
            }
        };
        let lenient = DecodeOptions::default().with_strict(false);
        let mut cod = synth::cod_body(1, 0, 4, 0);
        cod[0] = 1;
        cod.push(0x00);
        let main = [
            synth::seg(0xFF52, &cod),
            synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
        ];
        let rescue = synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0));
        let grid = [3 * 2048, 1024, 1648, 0, 2048, 1024, 0, 0];
        let narrow_first = tiles_of(
            grid,
            &main,
            |tile| {
                if tile == 0 { vec![] } else { rescue.clone() }
            },
        );
        let refused = read_header(&narrow_first, &lenient);
        assert!(
            matches!(&refused, Err(Error::Limit(limit)) if limit.message().starts_with("tile 0 ")),
            "{refused:?}"
        );
        // On a 32-bit target the caps stop at their floor, so a raised
        // limit does not raise them.
        if cfg!(target_pointer_width = "64") {
            let raised = lenient.clone().with_sample_limit(SampleLimit::Raised(4));
            assert!(read_header(&narrow_first, &raised).is_ok());
            check(&narrow_first, &[raised, lenient.clone()]);
        }
        let tile = lenient.clone().with_region(Region::Tile(1));
        check(&narrow_first, &[tile, lenient.clone()]);

        let mut cod = synth::cod_body(1, 1, 4, 0);
        cod[0] = 1;
        cod.extend([0x00, 0x11]);
        let main = [
            synth::seg(0xFF52, &cod),
            synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8; 4])),
        ];
        let no_levels = [
            synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0)),
            synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
        ]
        .concat();
        let one_level = synth::seg(0xFF52, &synth::cod_body(1, 1, 4, 0));
        let grid = [2 * 1040, 512, 0, 0, 1040, 512, 0, 0];
        let own_no_levels = tiles_of(grid, &main, |tile| {
            if tile == 1 {
                no_levels.clone()
            } else {
                one_level.clone()
            }
        });
        let reduced = lenient.clone().with_resolution_reduction(1);
        let refused = read_header(&own_no_levels, &reduced);
        assert!(
            matches!(&refused, Err(Error::InvalidOptions(message)) if message.to_string().ends_with("in tile 1")),
            "{refused:?}"
        );
        assert!(read_header(&own_no_levels, &lenient).is_ok());
        check(&own_no_levels, &[lenient, reduced]);
    }

    /// What a header read keeps is for one input, which grows: a read of
    /// another on the same [`HeaderCache`] gives what [`read_header`] gives.
    /// Each file has two tiles of 1040 x 512 and one level, the main
    /// `COD`'s precincts `precincts`, and in tile 0's first tile-part a `COD`
    /// of maximal precincts or a `COM` of the same length. With 1x1
    /// precincts at the lowest resolution the tiles are over the precinct
    /// cap, so tile 0 is refused without its `COD`.
    ///
    /// The second file of each pair is no shorter than the first and lays
    /// its tile-parts out at the same offsets. Its codestream starts
    /// elsewhere (in a JP2 file) or its main header differs, so what the
    /// first read kept does not hold for it.
    #[test]
    fn what_is_kept_is_for_one_input() {
        use crate::synth;
        let file = |precincts: [u8; 2], rescued: bool| {
            let mut cod = synth::cod_body(1, 1, 4, 0);
            cod[0] = 1;
            cod.extend(precincts);
            let rescue = synth::seg(0xFF52, &synth::cod_body(1, 1, 4, 0));
            let mut comment = vec![0, 1];
            comment.resize(rescue.len() - 4, b'x');
            let comment = synth::seg(0xFF64, &comment);
            assert_eq!(comment.len(), rescue.len());
            let main = [
                synth::seg(0xFF52, &cod),
                synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8; 4])),
            ];
            let grid = [2 * 1040, 512, 0, 0, 1040, 512, 0, 0];
            tiles_of(grid, &main, |tile| match tile {
                0 if !rescued => comment.clone(),
                _ => rescue.clone(),
            })
        };
        let lenient = DecodeOptions::default().with_strict(false);
        let check = |first: &[u8], second: &[u8]| {
            assert!(read_header(first, &lenient).is_ok());
            let want = read_header(second, &lenient);
            assert!(
                matches!(&want, Err(Error::Limit(limit)) if limit.message().starts_with("tile 0 ")),
                "{want:?}"
            );
            let mut kept = HeaderCache::default();
            read_header_kept(first, &lenient, Some(&mut kept)).expect("a header");
            let got = read_header_kept(second, &lenient, Some(&mut kept));
            assert_eq!(format!("{got:?}"), format!("{want:?}"));
        };
        // The same main header, its codestream at another start.
        let over = [0x00, 0x11];
        let refused = file(over, false);
        let jp2 = synth::wrap_jp2_boxes(0x0C, &refused, &[]);
        check(&file(over, true), &jp2);
        // Another main header: one whose tiles are within the caps first.
        check(&file([0x11, 0x11], false), &refused);
        // The fault walk kept by a refusal is for one main header too: tile
        // 0's `COM` turned into an `MCC`, which the walk kept for the first
        // file has passed, under a main `QCD` of one guard bit, not two.
        let mut mcc = refused.clone();
        let at = mcc
            .windows(2)
            .rposition(|code| code == [0xFF, 0x64])
            .expect("the COM");
        mcc[at + 1] = 0x75;
        let qcd = mcc
            .windows(2)
            .position(|code| code == [0xFF, 0x5C])
            .expect("the QCD");
        assert_eq!(mcc[qcd + 4], 2 << 5);
        mcc[qcd + 4] = 1 << 5;
        let want = read_header(&mcc, &lenient);
        assert!(matches!(&want, Err(Error::Unsupported(_))), "{want:?}");
        let mut kept = HeaderCache::default();
        let first = read_header_kept(&refused, &lenient, Some(&mut kept));
        assert!(matches!(first, Err(Error::Limit(_))), "{first:?}");
        let got = read_header_kept(&mcc, &lenient, Some(&mut kept));
        assert_eq!(format!("{got:?}"), format!("{want:?}"));
    }

    /// Under `Hold::Lifting` the count under the largest cap is kept: a
    /// call on one byte more, or a second call, does not count again,
    /// while a change of the reduction does. The file is 1024 tiles of
    /// 16 x 16, each of 1024 components under 5 levels, whose count stops
    /// at its budget under the default cap and meets tile 28's tile-part
    /// fault, then a `colr` of length 0 that passes whatever follows. (On
    /// a 32-bit target the default cap is at the address space already.)
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn the_count_under_the_largest_cap_is_kept() {
        use crate::synth;
        let siz = synth::siz_body([512, 512, 0, 0, 16, 16, 0, 0], &vec![[7, 1, 1]; 1024]);
        let mut codestream = [vec![0xFF, 0x4F], synth::seg(0xFF51, &siz)].concat();
        codestream.extend(synth::seg(0xFF52, &synth::cod_body(1, 5, 4, 0)));
        codestream.extend(synth::seg(0xFF5C, &synth::qcd_none_body(2, &[9; 16])));
        codestream.extend(synth::tile_part(28, 2, 3, &[], &[]));
        codestream.extend([0xFF, 0xD9]);
        let header = [
            synth::ihdr_for(&codestream),
            synth::bx(b"colr", &synth::colr_enum(16)),
        ];
        let mut bytes = synth::jp2_file(&header, &[], &codestream, &[]);
        bytes.extend_from_slice(b"\0\0\0\0colr\x01\0\0\0\0\0\x11");
        let options = DecodeOptions::default().with_strict(false);
        let counts = || LARGEST_COUNTS.with(|counts| counts.replace(0));
        counts();
        let mut kept = HeaderCache::default();
        let mut longer = bytes.clone();
        for call in 0..3 {
            if call == 2 {
                longer.push(0);
            }
            let read = read_header_streamed(&longer, &options, Some(&mut kept));
            assert!(read.is_ok(), "call {call}: {read:?}");
        }
        assert_eq!(counts(), 1);
        // Without a cache each call counts again.
        assert!(read_header_streamed(&longer, &options, None).is_ok());
        assert_eq!(counts(), 1);
        // What was kept is for one reduction, and a change of the
        // selection drops it.
        let reduced = options.clone().with_resolution_reduction(1);
        for _ in 0..2 {
            let read = read_header_streamed(&longer, &reduced, Some(&mut kept));
            assert!(read.is_ok(), "{read:?}");
        }
        assert_eq!(counts(), 1);
        assert!(kept.largest.is_some());
        kept.clear_selection();
        assert!(kept.largest.is_none());
    }
}
