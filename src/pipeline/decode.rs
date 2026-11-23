//! Decode orchestration: from the input bytes to an [`Image`] or its
//! [`Header`].
//!
//! The crate root's [`decode_with`](crate::decode_with) and
//! [`read_header_with`](crate::read_header_with) are thin wrappers over
//! [`decode_metered`] and [`read_header`]. The incremental decoder calls
//! [`decode_metered`] with its [`Ledger`](incremental::Ledger), and
//! [`check_options`] when it is built.

use std::collections::BTreeMap;
use std::ops::ControlFlow;

use super::report::{Scope, report_part, report_tile};
use crate::incremental::Refusal;
use crate::{
    ColorOutput, ComponentInfo, DecodeOptions, DecodeReport, Error, Header, Image, Jp2Metadata,
    ReportEntry, Result, SampleLimit, TileGrid, WorkLimit, codestream, dwt, header, image,
    incremental, jp2, limit_hint, mct, samples, tier1, tier2, window,
};

/// The [`Header`] that [`read_header_with`](crate::read_header_with) returns
/// for `bytes` under `options`.
pub(crate) fn read_header(bytes: &[u8], options: &DecodeOptions) -> Result<Header> {
    read_header_kept(bytes, options, None)
}

/// [`read_header`], going on from what `kept` holds of an earlier call on
/// bytes that `bytes` extend, and keeping in it what a later call on more
/// bytes can go on from (see [`HeaderCache`]). It gives what
/// [`read_header`] gives.
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
/// tile-parts are held as a codestream's are. Past that, it gives what
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
    let reduction = options.resolution_reduction;
    if !bytes.starts_with(&[0xFF, 0x4F]) && jp2::looks_like_jp2(bytes) {
        let prefix = match read_jp2_prefix(bytes, options, hold) {
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
            Err(fault) => return Err(fault.error),
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
        } else if !hold || prefix.settled {
            // Once no box after the codestream can be read, the codestream
            // grows as a raw one does, and its tile-parts are held as a raw
            // one's are, or not at all where it is the same whatever follows
            // ([`jp2::Jp2File::codestream_fixed`]). Before that, a fault of
            // such a box comes first, so every error of the tile-parts is
            // held, and they are not walked.
            check_geometry(
                prefix.codestream,
                &prefix.parsed,
                &prefix.selection,
                reduction,
                samples::cap(&options.sample_limit, bytes.len()),
                options.strict,
                if hold && !prefix.fixed {
                    Hold::UntilClosed
                } else {
                    Hold::Never
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
    let parsed = codestream::parse_header(bytes)?;
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
/// on or off ([`codestream::tile_parts_fault`], a walk that keeps no tile
/// record).
///
/// With [`Hold::UntilClosed`], a refusal with no fault in the tile-parts is
/// held while the walk of the tile-parts has not closed (see
/// [`read_header_streamed`]), one more bytes lift included.
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
    let mut fault = || faults.find(bytes, parsed, strict);
    let checked = match counted {
        Ok(true) => Ok(()),
        Ok(false) => fault().map_or(Ok(()), Err),
        Err(error @ (Error::Limit(_) | Error::InvalidOptions(_))) => match fault() {
            Some(fault) => Err(fault),
            // A later tile-part may yet hold a fault, which comes first,
            // whether or not more bytes would lift the refusal.
            None if hold == Hold::UntilClosed && !faults.closed => Ok(()),
            None => Err(error),
        },
        Err(error) => Err(error),
    };
    if let (Some(cache), Some(main), Some(walk)) = (cache, main, faults.walk) {
        cache.fault = Some(Box::new(KeptFault { start, main, walk }));
    }
    checked
}

/// Whether [`check_geometry`] holds a tile refusal back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    /// It gives every error: a call on the whole input.
    Never,
    /// It holds a tile refusal until the tile-parts close.
    UntilClosed,
}

/// The walk of the tile-parts for the decode's error for them
/// ([`codestream::tile_parts_fault`]): from the start, or, for a call that
/// keeps, from where the last such call left it on the start of the same
/// input ([`codestream::tile_parts_fault_kept`]).
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
/// error for the tile-parts ([`Faults`]). The bytes fed only grow, so what a call
/// found in them stays so, and a call gives what a read from the start
/// gives. It is kept for one codestream, main header, selection and
/// reduction; a call that reads another, as after a change of region,
/// starts again.
#[derive(Default)]
pub(crate) struct HeaderCache {
    /// Boxed, so a decoder with nothing kept holds a pointer.
    kept: Option<Box<Kept>>,
    /// The walk for the decode's error for the tile-parts, which does not
    /// depend on the selection or the reduction, so it is kept apart.
    fault: Option<Box<KeptFault>>,
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
/// the bytes fed only grow, so a main header read whole stays the same, and
/// because a change of options drops the cache. The hash is a cheap defense
/// against a read of another input on the same cache, and probabilistic. Any
/// future way to truncate or reset the bytes fed must clear the cache.
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
    /// `strict` as the read has it ([`codestream::tile_parts_fault`]), as
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
    /// of that parse ([`codestream::tile_parts_fault`]), which reads the
    /// main header in place, the tile-part headers and not the tile data,
    /// and keeps no tile record. `None` only if the parse finds no fault,
    /// which the walk finding no tile-part rules out.
    fn walk_fault(&mut self) -> Option<Error> {
        self.faults.find(self.bytes, self.parsed, true)
    }
}

/// The entries of `all` that `order` names, in its order. Each index is in
/// range and named once ([`window::select`] checked).
fn pick<T: Clone>(all: Vec<T>, order: &[usize]) -> Vec<T> {
    if order.len() == all.len() && order.iter().enumerate().all(|(i, &c)| i == c) {
        return all;
    }
    order.iter().filter_map(|&c| all.get(c).cloned()).collect()
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

/// [`decode_with`](crate::decode_with), with the code-block work also held
/// to what `ledger` has left of its bound, and the packet walks with
/// `strict` off kept on it, when there is one (see [`incremental::Ledger`]).
pub(crate) fn decode_metered(
    bytes: &[u8],
    options: &DecodeOptions,
    ledger: Option<&incremental::Ledger>,
) -> Result<Image> {
    check_options(options)?;
    // A codestream opens with SOC; anything else that carries the signature
    // box's type is read as a JP2 file, so a damaged signature box is reported
    // as a box fault. The rest falls to the codestream parser, which reports
    // the missing SOC.
    if !bytes.starts_with(&[0xFF, 0x4F]) && jp2::looks_like_jp2(bytes) {
        return decode_jp2(bytes, options, ledger);
    }
    check_format_known(bytes)?;
    // The header checks come before the tile walk, as `read_header_with` has it.
    let parsed = codestream::parse_header(bytes)?;
    let selection = window::select(&parsed.header.siz, options);
    if let Ok(selection) = &selection {
        samples::check_read(
            &parsed.header.siz,
            selection,
            options.resolution_reduction,
            samples::cap(&options.sample_limit, bytes.len()),
        )?;
    }
    check_raw_color(options, parsed.header.siz.components.len())?;
    check_reduction(&parsed.header, options.resolution_reduction, None)?;
    let selection = selection?;
    let cs = codestream::parse_tiles_with(bytes, parsed, options.strict)?;
    let cap = samples::cap(&options.sample_limit, bytes.len());
    let closed = cs.closed;
    held_unless(
        closed,
        ledger,
        decode_codestream(cs, options, &selection, cap, ledger),
    )
}

/// `decoded`, a step of the decode past the parse of every tile-part, noted
/// on `ledger` when it is an error and the tile-parts have not `closed`: a
/// later tile-part may hold a fault the decode gives first, so a refine
/// waits on the error ([`Waiting::Held`](crate::Waiting::Held)).
/// A JP2 file's error of the boxes that more bytes can replace is noted the
/// same way, as never `closed`, and so is an error of its tile-parts while
/// a box after the codestream can still be read.
fn held_unless<T>(
    closed: bool,
    ledger: Option<&incremental::Ledger>,
    decoded: Result<T>,
) -> Result<T> {
    if let (Err(_), Some(ledger)) = (&decoded, ledger) {
        ledger.hold(closed);
    }
    decoded
}

/// Refuse with [`Error::Incomplete`] input that is not read as a JP2 file
/// and already differs from `SOC`, but is too short to say it is not a JP2
/// file. Input that carries the signature box's type in bytes 4 to 8 is read
/// as a JP2 file whatever its first four bytes ([`jp2::looks_like_jp2`]), so
/// input under 8 bytes whose bytes from 4 on start that type may yet be one.
/// Were it taken for a codestream with no `SOC`, more bytes could turn that
/// error into a fault of the signature box. Input that `SOC` still starts
/// (empty, or `FF`) is left to the codestream parser, which waits on it too.
fn check_format_known(bytes: &[u8]) -> Result<()> {
    let kind = &jp2::SIGNATURE[4..8];
    let not_soc = bytes
        .iter()
        .zip([0xFF, 0x4F])
        .any(|(&byte, soc)| byte != soc);
    let may_be_jp2 =
        not_soc && bytes.len() < 8 && kind.starts_with(bytes.get(4..).unwrap_or_default());
    if may_be_jp2 {
        return Err(Error::incomplete(
            "the input ends before its first 8 bytes, which tell a codestream from a JP2 file",
        ));
    }
    Ok(())
}

/// Options that contradict each other, whatever the input.
pub(crate) fn check_options(options: &DecodeOptions) -> Result<()> {
    if options.color == ColorOutput::Srgb && options.raw_channels {
        return Err(Error::invalid_options(
            "`raw_channels` returns the channels as coded, so it cannot be combined with \
             `ColorOutput::Srgb`",
        ));
    }
    for (raised_zero, name, limit) in [
        (
            options.packet_work_limit == WorkLimit::Raised(0),
            "packet_work_limit",
            "WorkLimit",
        ),
        (
            options.code_block_work_limit == WorkLimit::Raised(0),
            "code_block_work_limit",
            "WorkLimit",
        ),
        (
            options.sample_limit == SampleLimit::Raised(0),
            "sample_limit",
            "SampleLimit",
        ),
    ] {
        if raised_zero {
            return Err(Error::invalid_options(format!(
                "`DecodeOptions::{name}` is `{limit}::Raised(0)`, but the factor must be at \
                 least 1"
            )));
        }
    }
    if options.sample_limit == SampleLimit::AtMost(0) {
        return Err(Error::invalid_options(
            "`DecodeOptions::sample_limit` is `SampleLimit::AtMost(0)`, but an image holds at \
             least one sample",
        ));
    }
    if options.max_quality_layers == Some(0) {
        return Err(Error::invalid_options(
            "`DecodeOptions::max_quality_layers` is `Some(0)`, but a decode keeps at least \
             one layer",
        ));
    }
    window::check_options(options)
}

/// A reduction must leave every component a resolution to decode, checked
/// against each component's own decomposition count: a COC can give one
/// component a shallower pyramid than the rest (p0_08 does exactly that).
/// `decode_with` and `read_header_with` make this check against the main header
/// (COD and COC) before any tile is read, with `tile` of `None`. A tile-part COD
/// or COC with fewer levels is only found while decoding, where the same test
/// runs per tile with that tile's index.
pub(crate) fn check_reduction(
    header: &codestream::MainHeader,
    reduction: u8,
    tile: Option<usize>,
) -> Result<()> {
    for (component, params) in header.components.iter().enumerate() {
        let resolutions = u32::from(params.coding.decomposition_levels) + 1;
        if u32::from(reduction) >= resolutions {
            let place = tile.map_or_else(String::new, |index| format!(" in tile {index}"));
            return Err(Error::invalid_options(format!(
                "resolution reduction {reduction} discards all {resolutions} resolutions \
                 of component {component}{place}"
            )));
        }
    }
    Ok(())
}

/// A raw codestream declares no color space. Like OpenJPEG, one or two
/// components are read as gray (and gray plus opacity) and pass through; three
/// or more have nothing to convert from, so sRGB output refuses them.
fn check_raw_color(options: &DecodeOptions, components: usize) -> Result<()> {
    if options.color == ColorOutput::Srgb && components > 2 {
        return Err(Error::unsupported(format!(
            "sRGB output from a raw codestream of {components} components, which declares \
             no color space"
        )));
    }
    Ok(())
}

/// A JP2 file read up to its tile data: the boxes, the codestream's main
/// header, the channel plans and the metadata. Everything here is checked
/// before any tile is read, so [`read_header`](crate::read_header) and
/// [`decode`](crate::decode) share it and report a header fault the same way.
struct Jp2Prefix<'a> {
    codestream: &'a [u8],
    parsed: codestream::ParsedHeader,
    plans: jp2::ChannelPlans,
    metadata: Jp2Metadata,
    /// The sRGB step, when the caller asked for it.
    srgb: Option<jp2::SrgbPlan>,
    /// The area and components the decode returns.
    selection: window::Selection,
    /// For a stream that may still grow, the first error of the read that a
    /// later box can replace ([`read_jp2_prefix`]), held back. It is `None`
    /// for a read of the whole input, which returns that error instead.
    held: Option<Error>,
    /// Whether no box after the codestream can still be read, whatever
    /// bytes follow ([`jp2::Jp2File::after_settled`]).
    settled: bool,
    /// Whether the codestream the decode reads is the same whatever bytes
    /// follow, with no box after it read ([`jp2::Jp2File::codestream_fixed`]),
    /// so that the tile-parts' errors are final though they have not closed.
    fixed: bool,
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
///   2^32 or more.
///
/// The faults before these, and those of whole boxes, are final, as more
/// bytes change neither the boxes they are in nor the ones before them.
/// The metadata fails only on an internal inconsistency, so it is not held.
fn read_jp2_prefix<'a>(
    bytes: &'a [u8],
    options: &DecodeOptions,
    streamed: bool,
) -> std::result::Result<Jp2Prefix<'a>, jp2::BoxFault> {
    let file =
        jp2::parse_boxes(bytes, &options.packet_work_limit).map_err(|fault| jp2::BoxFault {
            open: streamed && fault.open,
            ..fault
        })?;
    jp2::check_box_layout(&file)?;
    let parsed = codestream::parse_header(file.codestream)?;
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
        fixed: file.codestream_fixed,
    })
}

/// Decode a JP2 file: read it up to its tile data, decode the codestream,
/// and apply the channel plans, then any sRGB step, to the samples.
fn decode_jp2(
    bytes: &[u8],
    options: &DecodeOptions,
    ledger: Option<&incremental::Ledger>,
) -> Result<Image> {
    // A refine reads a stream that may still grow, and holds an error that
    // more bytes can replace.
    let prefix = match read_jp2_prefix(bytes, options, ledger.is_some()) {
        Ok(prefix) => prefix,
        Err(fault) => return held_unless(!fault.open, ledger, Err(fault.error)),
    };
    if let Some(error) = prefix.held {
        return held_unless(false, ledger, Err(error));
    }
    // Boxes can follow the codestream, and their faults come before those of
    // the tile-parts, so while one can still be read every error here is
    // held. Once none can, the codestream grows as a raw one does
    // ([`jp2::Jp2File::after_settled`]), and its errors are held as a raw
    // one's are: a fault of the parse of the tile-parts is final, and an
    // error past it is held until they close, or, where the codestream is
    // the same whatever follows ([`jp2::Jp2File::codestream_fixed`]), not
    // at all.
    let cs = match codestream::parse_tiles_with(prefix.codestream, prefix.parsed, options.strict) {
        Ok(cs) => cs,
        Err(fault) => return held_unless(prefix.settled, ledger, Err(fault)),
    };
    let closed = prefix.fixed || (prefix.settled && cs.closed);
    let decoded = || {
        let cap = samples::cap(&options.sample_limit, bytes.len());
        let image = decode_codestream(cs, options, &prefix.selection, cap, ledger)?;
        // A subset is planned raw, so the plans would pass it through, but
        // they are planned for every component.
        let mut components = if prefix.selection.subset {
            image.components
        } else {
            prefix.plans.apply(image.components)?
        };
        if let Some(srgb) = &prefix.srgb {
            components = srgb.apply(components)?;
        }
        Ok(Image {
            components,
            jp2: Some(prefix.metadata),
            ..image
        })
    };
    held_unless(closed, ledger, decoded())
}

/// Decode a parsed codestream into the area and components `selection`
/// names, under the sample limit `cap`.
fn decode_codestream(
    cs: codestream::Codestream<'_>,
    options: &DecodeOptions,
    selection: &window::Selection,
    cap: samples::Cap,
    ledger: Option<&incremental::Ledger>,
) -> Result<Image> {
    let reduction = options.resolution_reduction;
    let layers = options.max_quality_layers;

    // Each tile is a whole decode of its own: its own header, its own packets,
    // its own wavelet pyramid in its own coordinate frame. Nothing crosses a
    // tile boundary — that is what tiling is for — so the tiles meet only at the
    // end, when each writes its rectangle of the canvas. A window keeps the
    // part of each tile inside it, and the tiles with no part inside it are
    // not decoded.
    let mut canvas = image::Canvas::over(&cs.header, &selection.area, reduction, &selection.order)?;
    let reads = selection.tiles(&cs.header.siz);
    // The walks an incremental decoder kept for other options are let go
    // here, before any tile is walked.
    if let Some(ledger) = ledger {
        ledger.prepare(&reads, reduction, layers, canvas.bytes());
    }
    // Whether the decode returns less than the image, so a tile may need
    // only part of its samples.
    let windowed = selection.area.image_rect() != cs.header.siz.image_rect();
    // One work bound for every tile's packet walk, and one for every tile's
    // code-blocks, in proportion to the tiles' data, so that many small
    // tiles cannot each spend a floor's worth. A POC sequence's bitmap stays
    // within the default bound however far the caller raises it.
    let bytes = tile_bytes(&cs);
    let poc_bitmap = tier2::decode_visits(bytes);
    let packet_bound = options.packet_work_limit.apply(poc_bitmap);
    // The walk is not cut by what earlier decodes of an incremental decoder
    // spent: those decodes' walks are kept, and this one goes on from them,
    // so that what they all spend is what one walk spends (see
    // [`incremental::Ledger`]).
    let meter = tier2::Meter::new(packet_bound).under(cap);
    // The code-block bound that `Default` sets and a refusal's factor is
    // worked out from. Under the fuzz targets' cap it is the cap, whatever
    // the tiles' data, and no limit, `Unlimited` included, sets more than
    // `fuzz::CODE_BLOCK_CAP_FACTOR` times it (see
    // `fuzz::with_code_block_work_cap`).
    let (block_default, block_ceiling) = match options.code_block_work_cap {
        None => (tier1::decode_work(bytes), None),
        Some(cap) => (
            cap,
            Some(cap.saturating_mul(crate::fuzz::CODE_BLOCK_CAP_FACTOR)),
        ),
    };
    // The whole code-block bound, which a decode of these bytes on its own
    // is held to.
    let mut full_blocks = 0;
    let mut block_work = match (&options.code_block_work_limit, block_ceiling) {
        (WorkLimit::Unlimited, None) => None,
        (limit, ceiling) => {
            full_blocks = limit.apply(block_default).min(ceiling.unwrap_or(u64::MAX));
            Some(ledger.map_or(full_blocks, |l| l.blocks_left(full_blocks)))
        }
    };
    let first_work = block_work.unwrap_or(0);
    // Under an incremental decoder, the code-block bound is what the
    // earlier decodes left of it. Once a tile passes that rest but the work
    // of this decode so far still fits the whole bound, the decode is
    // refused, but it goes on counting the work of the later tiles without
    // decoding them, to tell which bound refused it: the whole bound, which
    // a decode of these bytes on its own is held to, or only what the
    // earlier decodes spent of it. This holds the work of this decode.
    //
    // A tile the whole bound refuses ends the count too, and the decode is
    // again refused as `BoundSpent`, not as the whole bound's refusal: a
    // decode of these bytes on its own decodes the tiles counted before it
    // gets there, and can fail in one of them. Decoding them here to find
    // out would cost each such refine up to one more bound. Only where the
    // tile at the cut passes the whole bound itself is the decode refused
    // as the whole bound refuses a decode of these bytes on its own: every
    // tile before it was decoded.
    //
    // An error the count meets in a later tile ends the count, and the
    // decode is refused as `BoundSpent`, never with that error. A decode
    // with the whole bound does each tile whole, in order: its packets, then
    // its code-blocks, wavelet and assembly, before the next tile. So the
    // first error it meets can be in the code-blocks, wavelet or assembly of
    // the tile refused here, or of any tile counted after it, none of which
    // the count does; an error in a later tile's header or packets is then
    // not the one it gives. The refusals of a later tile's walk end the
    // count too, as an error does.
    //
    // The tests reach three of the steps that end the count: the reduction
    // check, `check_tile` (a later tile over a cap on its code-blocks or
    // precincts, which the parse does not check; see
    // `incremental/bound_spent_stands_in_for_a_tile_cap_refusal.seed`) and
    // the packet walk. The others cannot fail on a codestream that parsed.
    // `tile_header` fails only on a header the parse, which checks
    // every tile's header, has already refused, or on an `Error::inconsistent`
    // invariant (a missing tile). `footprint_of` and
    // `report_part` fail only on `Error::inconsistent` invariants (in
    // `footprint_of` and `dwt::Footprint::of`). They end the count
    // all the same.
    let mut counting: Option<u64> = None;
    // What a step of a tile that fails gives: its error, or, while counting,
    // the end of the count (see `counting`).
    macro_rules! or_end_count {
        ($result:expr) => {
            match $result {
                Ok(value) => value,
                Err(_) if counting.is_some() => break,
                Err(error) => return Err(error),
            }
        };
    }
    // Not strict: what the codestream walk found missing, then each tile's
    // packets, and the smallest reduction every tile is complete at so far.
    let mut entries = window::keep_entries(cs.entries.clone(), &reads);
    let mut complete = Some(0u8);
    // A missing tile reads no packet, but setting its samples to 0 visits
    // each of its tile-components, and a header of a few kilobytes can
    // declare 65535 tiles of 16384 components with none of them present. So
    // each tile-component of a missing tile costs a visit, charged here,
    // before any tile is decoded.
    let missing: u64 = entries
        .iter()
        .filter_map(|entry| match entry {
            ReportEntry::MissingTiles { tiles } => Some(u64::from(tiles.end - tiles.start)),
            _ => None,
        })
        .sum();
    let cost = missing.saturating_mul(cs.header.siz.components.len() as u64);
    if !meter.try_spend(cost) {
        if let Some(ledger) = ledger {
            ledger.refuse(Refusal::MissingTiles);
        }
        return Err(limit_hint::missing_tiles(cost, meter.first(), poc_bitmap));
    }
    for (position, tile) in cs.tiles.iter().enumerate() {
        // `tiles` holds every tile when strict, so `index` is also its
        // position; not strict, a missing tile is left out.
        let index = tile.index as usize;
        if !reads.get(index).copied().unwrap_or(false) {
            continue;
        }
        // The tile's own parameters: the main header with this tile's tile-part
        // header resolved over it. A tile with no markers of its own borrows the
        // main header; any other is a copy, sized by the component count, held
        // for this tile alone, as a codestream can declare tens of thousands.
        // That copy is paid for by the geometry the packet walk then builds for
        // every component, whether or not the tile's data runs out; a tile
        // with no data at all is not resolved. So the copies stay within
        // `tiles × components`, which SIZ's rule that no tile-component is
        // empty holds to the image's samples (under a window or a tile, the
        // samples of the tiles read), with `strict` off as with it on.
        let header = or_end_count!(cs.tile_header(position));

        // A tile COD can give one tile a shallower pyramid than the rest, so
        // the reduction is checked per tile, not once for the image.
        or_end_count!(check_reduction(&header, reduction, Some(index)));

        // The tile's precincts and code-blocks, counted before its geometry
        // is built, against what the sample limit allows a tile.
        or_end_count!(tier2::check_tile(&header, tile.index, cap));

        // Tier-2: parse packets into per-code-block coded segments, per
        // component. The walk covers every resolution and layer whatever the
        // reduction and layer cap: packet order is the codestream's framing,
        // so the dropped levels' and layers' packets must still be stepped
        // over to reach the kept ones.
        let walked = if options.strict {
            tier2::decode_packets_metered(&header, tile, &meter, poc_bitmap, reduction, layers)
                .map(|coded| (coded, None))
        } else if let Some(ledger) = ledger {
            ledger
                .walk(
                    &header,
                    tile,
                    &meter,
                    poc_bitmap,
                    reduction,
                    layers,
                    canvas.bytes(),
                )
                .map(|(coded, reach)| (coded, Some(reach)))
        } else {
            tier2::decode_packets_partial(&header, tile, &meter, poc_bitmap, reduction, layers)
                .map(|(coded, reach)| (coded, Some(reach)))
        };
        let (mut coded, reach) = match walked {
            Ok(walked) => walked,
            Err(_) if counting.is_some() => break,
            Err(tier2::Refused::Work(work)) => {
                if let Some(ledger) = ledger {
                    ledger.refuse(Refusal::Work);
                }
                // The meter holds what is left after the refusal, so this
                // counts this tile's charges before it as well as the
                // earlier tiles'.
                return Err(limit_hint::packet(
                    work,
                    index,
                    meter.spent(),
                    meter.first(),
                    poc_bitmap,
                ));
            }
            Err(tier2::Refused::Other(error)) => return Err(error),
        };
        // The components to reconstruct: those the decode returns, and with
        // them the rest of the color transform's three, which each need all
        // of them. It is a per-tile transform: the flag lives in COD, which a
        // tile-part header can override. On an image of fewer than three
        // components the flag is ignored, as OpenJPEG ignores it
        // (`mct::applies`).
        let mct = mct::applies(
            header.cod.multiple_component_transform,
            header.components.len(),
        );
        let mut wanted: Vec<bool> = (0..header.components.len())
            .map(|c| canvas.keeps(c))
            .collect();
        if mct && wanted[..3].contains(&true) {
            wanted[..3].fill(true);
        }

        // Under a window, each component reconstructed needs only the samples
        // whose wavelet support reaches the window: its footprint. `None`
        // where the window covers the whole tile-component.
        let footprints = or_end_count!(
            coded
                .components
                .iter()
                .enumerate()
                .map(|(component, coded)| {
                    if !windowed || !wanted[component] {
                        return Ok(None);
                    }
                    footprint_of(
                        &cs.header, &header, selection, tile, coded, component, reduction,
                    )
                })
                .collect::<Result<Vec<_>>>()
        );
        if let Some(reach) = reach {
            // The whole image, or a whole tile of it, is reported by packet;
            // a part of it by the code-blocks that part needs. Whether the
            // window covers the tile is judged at full resolution, so that
            // the report is the same at every reduction.
            let covers = |component: usize| {
                let rect = cs.header.siz.tile_component_rect(tile.index, component);
                let area = selection.area.component_rect(component);
                matches!((rect, area), (Some(rect), Some(area)) if area.intersect(&rect) == rect)
            };
            let part = wanted.contains(&false)
                || (windowed && (0..wanted.len()).any(|component| !covers(component)));
            let tile_complete = if part {
                let footprint_at = |component: usize, r: u8| {
                    if r == reduction {
                        return Ok(footprints[component].clone());
                    }
                    footprint_of(
                        &cs.header,
                        &header,
                        selection,
                        tile,
                        &coded.components[component],
                        component,
                        r,
                    )
                };
                let scope = Scope {
                    coded: &coded,
                    wanted: &wanted,
                    windowed,
                    footprint_at,
                };
                or_end_count!(report_part(
                    &header,
                    tile,
                    &reach,
                    &scope,
                    reduction,
                    layers,
                    &mut entries,
                ))
            } else {
                report_tile(&header, tile, &reach, reduction, layers, &mut entries)
            };
            complete = complete.zip(tile_complete).map(|(a, b)| a.max(b));
        }
        for (coded, footprint) in coded.components.iter_mut().zip(&footprints) {
            if let Some(footprint) = footprint {
                footprint.prune(coded);
            }
        }

        // Tier-1: the block coder (EBCOT or HT) decodes each code block into
        // subband coeffs. The dropped resolutions' code-blocks are skipped, never
        // decoded, and so are those of the components not reconstructed and
        // those outside a footprint. Their work is counted first, so a tile
        // past the bound is refused before any of it is done.
        if let Some(left) = &mut block_work {
            let mut work = tier1::work(&header, &coded, reduction, &wanted);
            // Under the fuzz targets' cap, the samples the tile reconstructs
            // are charged too, so that an image with little coded data per
            // sample is held to the same bound of time.
            if options.code_block_work_cap.is_some() {
                let samples = (0..wanted.len())
                    .filter(|&c| wanted[c])
                    .filter_map(|c| cs.header.siz.tile_component_rect(tile.index, c))
                    .map(|rect| rect.reduced(reduction).area())
                    .fold(0, u64::saturating_add);
                work =
                    work.saturating_add(samples.saturating_mul(crate::fuzz::CAP_WORK_PER_SAMPLE));
            }
            if let Some(own) = &mut counting {
                if work > full_blocks.saturating_sub(*own) {
                    // The whole bound refuses this tile, but a decode of
                    // these bytes on its own decodes the tiles counted
                    // first and can fail in one of them, so this is not
                    // the refusal it gives (see `counting`).
                    if let Some(ledger) = ledger {
                        ledger.refuse(Refusal::BoundSpent);
                    }
                    return Err(Error::limit_unhinted(
                        crate::LimitKind::CodeBlockWork,
                        format!(
                            "tile {index} needs {work} sample-passes to decode its code-blocks, \
                             past the code-block work bound of {full_blocks} after the {own} \
                             of the tiles before it; the earlier refines left {first_work} of \
                             the bound, so the tiles counted were not decoded, and a decode of \
                             the bytes fed on their own is refused here or fails in one of them; \
                             decode them on their own, or start a new decoder"
                        ),
                    )
                    .refused_at(
                        tile.index,
                        work,
                        crate::AmountUnit::SamplePasses,
                    ));
                }
                *own += work;
                continue;
            }
            #[cfg(test)]
            tests::BLOCK_WORK.with(|spent| spent.set(spent.get() + work));
            let Some(rest) = left.checked_sub(work) else {
                // The work of this decode before this tile.
                let own = first_work.saturating_sub(*left);
                if ledger.is_some() && work <= full_blocks.saturating_sub(own) {
                    counting = Some(own + work);
                    continue;
                }
                if let Some(ledger) = ledger {
                    ledger.refuse(Refusal::Work);
                }
                // The refusal a decode of these bytes on its own gives: its
                // bound is the whole one, of which this decode spent `own`.
                // Without a ledger the two are the same.
                return Err(limit_hint::code_blocks(
                    tile.index,
                    work,
                    full_blocks.saturating_sub(own),
                    own,
                    block_default,
                ));
            };
            *left = rest;
            if let Some(ledger) = ledger {
                ledger.spend_blocks(work);
            }
        }
        #[cfg(test)]
        tests::BLOCKS
            .with(|count| count.set(count.get() + tests::blocks(&coded, reduction, &wanted)));
        let coeffs = tier1::decode_code_blocks(&header, &coded, reduction, &wanted)?;
        // The segment records end with tier-1, so they do not add to the
        // inverse wavelet's peak.
        drop(coded);

        // Invert the DWT per resolution level into samples, over the whole
        // tile-component or over its footprint. Tier-1 has already
        // dequantized the 9/7 coefficients as it placed each block.
        let mut samples = coeffs
            .into_iter()
            .zip(&footprints)
            .enumerate()
            .map(|(component, (coeffs, footprint))| {
                coeffs
                    .map(|coeffs| match footprint {
                        Some(footprint) => dwt::inverse_window(coeffs, footprint),
                        None => dwt::inverse(&header, component, coeffs),
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>>>()?;

        // Undo the inter-component decorrelation, if any, before the DC level
        // shift (ISO/IEC 15444-1 G.1). Components past the third are
        // untouched.
        if mct && wanted[0] {
            let mut triple = samples[..3]
                .iter_mut()
                .map(Option::take)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    Error::inconsistent(
                        "the color transform's three components were not all reconstructed",
                    )
                })?;
            // The wavelet picks the transform (G.1): 5/3 reconstructs integers
            // and inverts with the reversible RCT, 9/7 reconstructs floats and
            // inverts with the irreversible ICT. All three color components
            // share the wavelet (checked at parse), so component 0's decides.
            match header.components[0].coding.transform {
                codestream::markers::Transform::Reversible53 => mct::inverse_rct(&mut triple)?,
                codestream::markers::Transform::Irreversible97 => mct::inverse_ict(&mut triple)?,
            }
            for (slot, plane) in samples.iter_mut().zip(triple) {
                *slot = Some(plane);
            }
        }

        for (component, samples) in samples.into_iter().enumerate() {
            let Some(samples) = samples else {
                continue;
            };
            let rect = cs
                .header
                .siz
                .tile_component_rect(tile.index, component)
                .ok_or_else(|| {
                    Error::inconsistent(format!(
                        "no tile {} or no component {component} in `SIZ`",
                        tile.index
                    ))
                })?;
            let rect = match footprints.get(component) {
                Some(Some(footprint)) => footprint.rect(),
                _ => rect.reduced(reduction),
            };
            canvas.place(component, rect, samples)?;
        }
    }

    if let (Some(own), Some(ledger)) = (counting, ledger) {
        ledger.refuse(Refusal::BoundSpent);
        return Err(Error::limit_unhinted(
            crate::LimitKind::CodeBlockWork,
            format!(
                "the code-blocks of the tiles counted need {own} sample-passes, within the \
                 code-block work bound of {full_blocks}, but the earlier refines left \
                 {first_work} of it; decode the bytes fed on their own, or start a new decoder"
            ),
        ));
    }
    let mut image = image::assemble_over(
        &selection.area,
        canvas.into_planes(),
        &selection.order,
        reduction,
    )?;
    if !options.strict {
        for entry in &entries {
            if let ReportEntry::MissingTiles { tiles } = entry {
                complete = None;
                for tile in tiles.clone() {
                    image::zero_tile(
                        &mut image,
                        &cs.header.siz,
                        &selection.area,
                        &selection.order,
                        tile,
                        reduction,
                    );
                }
            }
        }
        image.report = DecodeReport::new(entries, complete, reduction);
    }
    Ok(image)
}

/// The footprint of `selection`'s area on `component` of `tile`, whose
/// coded data is `coded`, at `reduction` (see [`dwt::Footprint::of`]).
/// `main` is the main header, `header` the tile's.
fn footprint_of(
    main: &codestream::MainHeader,
    header: &codestream::MainHeader,
    selection: &window::Selection,
    tile: &codestream::Tile<'_>,
    coded: &tier2::ComponentCoded<'_>,
    component: usize,
    reduction: u8,
) -> Result<Option<dwt::Footprint>> {
    let rect = main.siz.tile_component_rect(tile.index, component);
    let area = selection.area.component_rect(component);
    let transform = header.components.get(component).map(|c| c.coding.transform);
    let (Some(rect), Some(area), Some(transform)) = (rect, area, transform) else {
        return Err(Error::inconsistent(format!(
            "no tile {} or no component {component} in `SIZ`",
            tile.index
        )));
    };
    dwt::Footprint::of(
        coded,
        reduction,
        rect.reduced(reduction),
        area.reduced(reduction),
        transform,
    )
}

/// The bytes of tile data the codestream's packets are read from: every
/// tile's packet data and packed packet headers.
pub(crate) fn tile_bytes(cs: &codestream::Codestream<'_>) -> u64 {
    cs.tiles
        .iter()
        .map(|tile| (tile.data.len() + tile.packed_headers.len()) as u64)
        .fold(0, u64::saturating_add)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{Region, Window, decode_with};

    thread_local! {
        /// The sample-passes the decodes on this thread have counted against
        /// the code-block work bound.
        pub(crate) static BLOCK_WORK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
        /// The code-blocks with coding passes the decodes on this thread have
        /// handed to the block decoder.
        pub(crate) static BLOCKS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    /// The code-blocks with coding passes in the resolutions `reduction`
    /// keeps of the components `wanted` marks: those Tier-1 decodes.
    pub(crate) fn blocks(coded: &tier2::CodedData<'_>, reduction: u8, wanted: &[bool]) -> u64 {
        let mut count = 0;
        for (component, coded) in coded.components.iter().enumerate() {
            if !wanted[component] {
                continue;
            }
            let kept = coded
                .resolutions
                .len()
                .saturating_sub(usize::from(reduction));
            for resolution in coded.resolutions.iter().take(kept.max(1)) {
                for subband in &resolution.subbands {
                    count += subband.blocks.iter().filter(|b| b.num_passes > 0).count() as u64;
                }
            }
        }
        count
    }

    /// The sample-passes Tier-1 runs on `bytes` (one tile) under a layer
    /// cap of `layers`, and the packet visits the walk spends.
    fn capped_work(bytes: &[u8], layers: Option<u16>) -> (u64, u64) {
        let parsed = codestream::parse_header(bytes).unwrap();
        let cs = codestream::parse_tiles(bytes, parsed).unwrap();
        let header = cs.tile_header(0).unwrap();
        let meter = tier2::Meter::new(u64::MAX);
        let coded =
            tier2::decode_packets_metered(&header, &cs.tiles[0], &meter, u64::MAX, 0, layers)
                .unwrap();
        let wanted = vec![true; coded.components.len()];
        (tier1::work(&header, &coded, 0, &wanted), meter.spent())
    }

    /// Each layer the cap drops is Tier-1 work not done, and the packet walk
    /// spends the same visits whatever the cap, since it still reads every
    /// packet.
    #[test]
    fn a_layer_cap_cuts_block_work_and_not_packet_work() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/layers/lrcp.j2k"
        ))
        .unwrap();
        let (full, visits) = capped_work(&bytes, None);
        let mut previous = 0;
        for n in 1..=4 {
            let (work, spent) = capped_work(&bytes, Some(n));
            assert!(work > previous, "-l {n}: {work} after {previous}");
            assert_eq!(spent, visits, "-l {n}");
            previous = work;
        }
        assert_eq!(previous, full);
        assert_eq!(capped_work(&bytes, Some(u16::MAX)), (full, visits));
    }

    /// The reduction check names the component and the resolutions it would
    /// discard, and the tile only when it runs per tile.
    #[test]
    fn check_reduction_message_names_the_tile_only_when_given() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/roi_maxshift_lossless.j2k"
        ))
        .unwrap();
        let mut header = codestream::parse_header(&bytes).unwrap().header;
        // One component is shallower than the rest, as a COC or tile COD makes it.
        let last = header.components.len() - 1;
        header.components[last].coding.decomposition_levels = 1;
        assert!(check_reduction(&header, 1, None).is_ok());
        let message = |tile| match check_reduction(&header, 2, tile) {
            Err(Error::InvalidOptions(m)) => m.message().to_owned(),
            other => panic!("{other:?}"),
        };
        let tail = format!("resolution reduction 2 discards all 2 resolutions of component {last}");
        assert_eq!(message(None), tail);
        assert_eq!(message(Some(7)), format!("{tail} in tile 7"));
    }

    /// The sample-passes the block decoder counts on a decode of `name` (in
    /// `tests/fixtures/window/`) under `options`.
    fn block_work(name: &str, options: &DecodeOptions) -> u64 {
        let bytes = std::fs::read(format!(
            "{}/tests/fixtures/window/{name}.j2k",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        work_of(&bytes, options)
    }

    /// The sample-passes the block decoder counts on a decode of `bytes`
    /// under `options`.
    fn work_of(bytes: &[u8], options: &DecodeOptions) -> u64 {
        BLOCK_WORK.with(|spent| spent.set(0));
        BLOCKS.with(|count| count.set(0));
        decode_with(bytes, options).unwrap();
        BLOCK_WORK.with(std::cell::Cell::get)
    }

    /// A window inside one tile decodes only the code-blocks whose wavelet
    /// support reaches it, not every code-block of the tile.
    #[test]
    fn a_window_inside_a_tile_decodes_fewer_code_blocks_than_the_tile() {
        // `tests/fixtures/bench/` is left out of the published package, so
        // the test is skipped where the directory is not there; a file
        // missing from a directory that is there fails it.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bench");
        if !dir.is_dir() {
            eprintln!(
                "{} is not present (packaged crate?); skipping",
                dir.display()
            );
            return;
        }
        let path = dir.join("tiled_2048_gray.j2k");
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let all = DecodeOptions::default();
        // Tile 27 is 768..1024 on both axes: 256 x 256 in 64 x 64 code-blocks,
        // over four decomposition levels.
        let tile = work_of(&bytes, &all.clone().with_region(Region::Tile(27)));
        assert!(tile > 0);
        let blocks = || BLOCKS.with(std::cell::Cell::get);
        // The tile's code-blocks with coding passes: the LL band's, one in
        // each band of the three coarser levels, and six of the twelve of
        // the finest level's 128 x 128 bands.
        assert_eq!(blocks(), 16);
        // Windows inside one code-block of each finest band: at the tile's
        // top-left and bottom-right corners, and one sample in the
        // bottom-right one. Each needs that code-block of each finest band
        // and every coarser one; the bottom-right blocks of two finest bands
        // have no coding pass.
        for (window, count) in [
            (Window::new(768, 768, 800, 800), 13),
            (Window::new(992, 992, 1024, 1024), 11),
            (Window::new(900, 900, 901, 901), 11),
        ] {
            let part = work_of(&bytes, &all.clone().with_region(Region::Window(window)));
            assert!(part < tile, "{window:?}: {part} of {tile}");
            assert_eq!(blocks(), count, "{window:?}");
        }
        // A window across the middle of the tile, whose wavelet support
        // reaches all four code-blocks of each finest band.
        let middle = Window::new(880, 880, 912, 912);
        assert_eq!(
            work_of(&bytes, &all.clone().with_region(Region::Window(middle))),
            tile
        );
        assert_eq!(blocks(), 16);
        // The window that is the whole tile decodes all of it.
        let whole = Window::new(768, 768, 1024, 1024);
        assert_eq!(
            work_of(&bytes, &all.clone().with_region(Region::Window(whole))),
            tile
        );
        assert_eq!(blocks(), 16);
    }

    /// A window decodes only the tiles it reaches, and a component subset
    /// only its components' code-blocks, but every one of the color
    /// transform's three when it asks for one of them.
    #[test]
    fn a_window_or_a_subset_decodes_fewer_code_blocks() {
        let all = DecodeOptions::default();
        for name in ["tiled_53", "tiled_53_nomct"] {
            let whole = block_work(name, &all);
            // Tile 4, the middle one, is a ninth of the image.
            let tile = block_work(name, &all.clone().with_region(Region::Tile(4)));
            assert!(tile * 6 < whole, "{name}: {tile} of {whole}");
            assert_eq!(
                block_work(
                    name,
                    &all.clone()
                        .with_region(Region::Window(Window::new(27, 22, 51, 42)))
                ),
                tile,
                "{name}"
            );
            // Tiles 0, 1, 3 and 4.
            let four = block_work(
                name,
                &all.clone()
                    .with_region(Region::Window(Window::new(20, 10, 41, 31))),
            );
            assert!(four > tile && four * 2 < whole, "{name}: {four} of {whole}");
        }
        let one = all.clone().with_components(Some(vec![1]));
        let whole = block_work("tiled_53_nomct", &all);
        let single = block_work("tiled_53_nomct", &one);
        assert!(single * 2 < whole, "{single} of {whole}");
        assert_eq!(block_work("tiled_53", &one), block_work("tiled_53", &all));
    }

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
}
