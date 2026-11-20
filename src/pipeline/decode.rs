//! Decode orchestration: from the input bytes to an [`Image`] or its
//! [`Header`].
//!
//! The crate root's [`decode_with`](crate::decode_with) and
//! [`read_header_with`](crate::read_header_with) are thin wrappers over
//! [`decode_metered`] and [`read_header`]. The incremental decoder calls
//! [`decode_metered`] with its [`Ledger`](incremental::Ledger), and
//! [`check_options`] when it is built.

use std::ops::ControlFlow;

use super::report::{Scope, report_part, report_tile};
use crate::incremental::Refusal;
use crate::{
    ColorOutput, ComponentInfo, DecodeOptions, DecodeReport, Error, Header, Image, Jp2Metadata,
    ReportEntry, Result, TileGrid, WorkLimit, codestream, dwt, header, image, incremental, jp2,
    limit_hint, mct, samples, tier1, tier2, window,
};

/// The [`Header`] that [`read_header_with`](crate::read_header_with) returns
/// for `bytes` under `options`.
pub(crate) fn read_header(bytes: &[u8], options: &DecodeOptions) -> Result<Header> {
    check_options(options)?;
    let reduction = options.resolution_reduction;
    if !bytes.starts_with(&[0xFF, 0x4F]) && jp2::looks_like_jp2(bytes) {
        let prefix = read_jp2_prefix(bytes, options)?;
        check_geometry(
            prefix.codestream,
            &prefix.parsed,
            &prefix.selection,
            reduction,
            samples::cap(&options.sample_limit, bytes.len()),
            options.strict,
        )?;
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
/// than the first `SOT`.
fn check_geometry(
    bytes: &[u8],
    parsed: &codestream::ParsedHeader,
    selection: &window::Selection,
    reduction: u8,
    cap: samples::Cap,
    strict: bool,
) -> Result<()> {
    let header = &parsed.header;
    let siz = &header.siz;
    let components = siz.components.len() as u64;
    let main_resolutions = resolutions(header);
    // The most resolutions a tile-part header can give a tile: 32 levels in
    // each component.
    let most_resolutions = components.saturating_mul(33);
    let reads = selection.tiles(siz);
    // For a tile read, whether its size shows it within both caps under the
    // main header, and whether its own header could refuse it: take it over
    // a cap (its size does not show it within them under any header) or,
    // under a reduction, have too few levels for it.
    let fits = |index: u32| {
        let rect = siz
            .tile_rect(index)
            .filter(|_| reads.get(index as usize) == Some(&true))?;
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
    let mut steps = 0u64;
    let mut rescue = Rescue {
        bytes,
        parsed,
        reads: &reads,
        reduction,
        cap,
        strict,
        walk: None,
        steps: 0,
    };
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
                return Ok(());
            }
            tier2::check_tile(header, index, cap)
                .err()
                .filter(|error| matches!(error, Error::Limit(_)))
        };
        let Some(error) = over else {
            pending.get_or_insert(index);
            continue;
        };
        // The decode counts the lower tiles under their own headers first,
        // and with `strict` on it refuses a file with a tile read missing
        // before it counts any tile, so the tiles read up to the one over a
        // cap are looked up first: that tile, which walks the tile-parts as
        // far as it, then the lower ones the walk has passed.
        if let Some(lowest) = pending.take() {
            rescue.begin(lowest as usize);
            if strict
                && (lowest..=index)
                    .rev()
                    .any(|tile| fits(tile).is_some() && rescue.find(tile).is_none())
            {
                return Err(rescue.walk_fault().unwrap_or(error));
            }
            for lower in lowest..index {
                let counted = fits(lower).is_some_and(|(_, may_refuse)| may_refuse);
                if counted && !rescue.count(lower, None)? {
                    return Ok(());
                }
            }
        }
        if !rescue.count(index, Some(error))? {
            return Ok(());
        }
    }
    Ok(())
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
    /// is never at 0). `None` until a tile is counted.
    walk: Option<Walk>,
    /// The steps taken reading and counting the tiles' own headers.
    steps: u64,
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
    fn begin(&mut self, from: usize) -> &mut Walk {
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
    /// decode leaves it out). A tile whose tile-part header is at fault is
    /// the decode's to report. So a tile-part `COD` that brings a tile
    /// within the caps is admitted, and one that takes it over them is
    /// refused. A tile within them (`None`) whose first tile-part has no
    /// `COD` or `COC` is admitted unread and uncharged.
    ///
    /// The tiles are counted in index order, as the walk notes the
    /// tile-parts only from the first tile counted on. Only the header read
    /// here is held. `Ok(false)` means the steps passed [`GEOMETRY_STEPS`]
    /// and the rest is left to the decode.
    fn count(&mut self, index: u32, error: Option<Error>) -> Result<bool> {
        // A tile with no tile-part: the decode refuses the file with `strict`
        // on, and leaves the tile out with it off.
        let part = match self.find(index) {
            Some(part) => part,
            None if self.strict => {
                return match self.walk_fault().or(error) {
                    Some(fault) => Err(fault),
                    None => Ok(true),
                };
            }
            None => return Ok(true),
        };
        // A tile not over a cap under the main header whose tile-part
        // carries no `COD` or `COC` is coded as the main header has it, which
        // passed the reduction and the caps, so it is neither read nor
        // charged.
        if error.is_none() && !part.may_set_coding() {
            return Ok(true);
        }
        // Reading a tile's header copies the main header's coding parameters
        // when the tile-part overrides them, so it is charged a step for each
        // component and, as that copy costs about ten counting steps per
        // component, ten more, before it is read.
        let read_cost = 11 * self.parsed.header.components.len() as u64;
        self.steps = self.steps.saturating_add(read_cost);
        if self.steps > GEOMETRY_STEPS {
            return Ok(false);
        }
        // A tile-part header at fault is the decode's to report.
        if let Ok(own) = part.header() {
            self.steps = self.steps.saturating_add(resolutions(&own));
            if self.steps > GEOMETRY_STEPS {
                return Ok(false);
            }
            check_reduction(&own, self.reduction, Some(index as usize))?;
            if let Err(error @ Error::Limit(_)) = tier2::check_tile(&own, index, self.cap) {
                return Err(error);
            }
        }
        Ok(true)
    }

    /// The error the decode gives for the tile-parts with `strict` on, for a
    /// tile the walk found no tile-part for. The decode parses the tile-parts
    /// before it counts a tile, so it refuses the file for the first fault
    /// its parse finds (in any tile's tile-part headers, or the chain), or
    /// for the missing tile, and never gets to the count. This is that
    /// parse, reading the main header in place. The walk has already
    /// followed the chain to its end, so the parse follows it no further;
    /// it reads the tile-part headers, not the tile data, and keeps the
    /// decode's tile records while it runs. `None` only if the parse finds
    /// no fault, which the walk finding no tile-part rules out.
    fn walk_fault(&self) -> Option<Error> {
        codestream::tile_parts_fault(self.bytes, self.parsed)
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
    decode_codestream(cs, options, &selection, cap, ledger)
}

/// Options that contradict each other, whatever the input.
pub(crate) fn check_options(options: &DecodeOptions) -> Result<()> {
    if options.color == ColorOutput::Srgb && options.raw_channels {
        return Err(Error::invalid_options(
            "`raw_channels` returns the channels as coded, so it cannot be combined with \
             `ColorOutput::Srgb`",
        ));
    }
    for (name, limit) in [
        ("packet_work_limit", &options.packet_work_limit),
        ("code_block_work_limit", &options.code_block_work_limit),
        ("sample_limit", &options.sample_limit),
    ] {
        if *limit == WorkLimit::Raised(0) {
            return Err(Error::invalid_options(format!(
                "`DecodeOptions::{name}` is `WorkLimit::Raised(0)`, but the factor must be \
                 at least 1"
            )));
        }
    }
    for (name, limit) in [
        ("packet_work_limit", &options.packet_work_limit),
        ("code_block_work_limit", &options.code_block_work_limit),
    ] {
        if let WorkLimit::AtMost(n) = limit {
            return Err(Error::invalid_options(format!(
                "`DecodeOptions::{name}` is `WorkLimit::AtMost({n})`, but `AtMost` sets only \
                 `DecodeOptions::sample_limit`"
            )));
        }
    }
    if options.sample_limit == WorkLimit::AtMost(0) {
        return Err(Error::invalid_options(
            "`DecodeOptions::sample_limit` is `WorkLimit::AtMost(0)`, but an image holds at \
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
}

/// Read a JP2 file up to its tile data, in OpenJPEG's order: the box walk,
/// the codestream's main header, `ihdr` against SIZ, then the channel plans.
/// The header boxes after the codestream, which OpenJPEG reads again once it
/// has decoded, are checked next, then the sRGB step is planned; all of it
/// before any tile is read.
fn read_jp2_prefix<'a>(bytes: &'a [u8], options: &DecodeOptions) -> Result<Jp2Prefix<'a>> {
    let file = jp2::parse(bytes, &options.packet_work_limit)?;
    let parsed = codestream::parse_header(file.codestream)?;
    let siz = &parsed.header.siz;
    let cap = samples::cap(&options.sample_limit, bytes.len());
    let selection = window::select(siz, options);
    if let Ok(selection) = &selection {
        samples::check_read(siz, selection, options.resolution_reduction, cap)?;
    }
    jp2::check_against_siz(&file.header, siz)?;
    // A component subset names codestream components, which come out as
    // coded: OpenJPEG bypasses the JP2 channel transforms under `-c`.
    let raw_channels = options.raw_channels || options.components.is_some();
    // A palette expands the area the decode returns. A refused selection has
    // no area, so the plan counts nothing: its other faults are found at the
    // same place whatever the image's size, and the selection's refusal
    // follows them.
    let plan_cap = if selection.is_ok() { cap } else { cap.lifted() };
    let plans = jp2::ChannelPlans::plan(
        &file.header,
        selection.as_ref().map_or(siz, |selection| &selection.area),
        options.resolution_reduction,
        plan_cap,
        raw_channels,
        jp2::is_jph(&file.file_type),
    )?;
    jp2::check_after_codestream(&file, raw_channels)?;
    let mut metadata = jp2::metadata(&file, &plans, siz)?;
    let selection = selection?;
    if selection.subset {
        metadata.channels = pick(metadata.channels, &selection.order);
    }
    let srgb = match options.color {
        ColorOutput::AsCoded => None,
        ColorOutput::Srgb => {
            let plan = jp2::plan_srgb(
                &metadata,
                &plans,
                &selection.area,
                options.resolution_reduction,
                cap,
            )?;
            // Chroma replication keys on the parity of the area's origin on
            // the reference grid (the full-resolution one, whatever the
            // reduction), so on part of the image it would not give the crop
            // of the whole image's conversion. An area equal to the image
            // (a window clipped to it, or the only tile) is the whole image.
            if plan.replicates_chroma() && selection.area.image_rect() != siz.image_rect() {
                return Err(Error::unsupported(
                    "sRGB output from sub-sampled sYCC chroma on an area smaller than the \
                     image, under `DecodeOptions::region`",
                ));
            }
            // A converted image is sRGB (or gray) and carries no profile, as
            // OpenJPEG reports it.
            plan.describe(&mut metadata);
            Some(plan)
        }
    };
    check_reduction(&parsed.header, options.resolution_reduction, None)?;
    Ok(Jp2Prefix {
        codestream: file.codestream,
        parsed,
        plans,
        metadata,
        srgb,
        selection,
    })
}

/// Decode a JP2 file: read it up to its tile data, decode the codestream,
/// and apply the channel plans, then any sRGB step, to the samples.
fn decode_jp2(
    bytes: &[u8],
    options: &DecodeOptions,
    ledger: Option<&incremental::Ledger>,
) -> Result<Image> {
    let prefix = read_jp2_prefix(bytes, options)?;
    let cs = codestream::parse_tiles_with(prefix.codestream, prefix.parsed, options.strict)?;
    let cap = samples::cap(&options.sample_limit, bytes.len());
    let image = decode_codestream(cs, options, &prefix.selection, cap, ledger)?;
    // A subset is planned raw, so the plans would pass it through, but they
    // are planned for every component.
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
    // The tests reach two of the steps that end the count: the reduction
    // check and the packet walk. The others cannot fail on a codestream that
    // parsed. `tile_header` fails only on a header the parse, which checks
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
                    .refused_at(tile.index, work));
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
        use crate::synth;
        let mut cod = synth::cod_body(1, 0, 4, 0);
        cod[0] = 1;
        cod.push(0x00);
        let siz = synth::siz_body([520 * tiles, 512, 0, 0, 520, 512, 0, 0], &[[7, 1, 1]]);
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
        // counted, so nothing past the first `SOT` is read.
        let raised = DecodeOptions::default().with_sample_limit(WorkLimit::Raised(4));
        assert_eq!(sot_reads(&bytes, &raised), 0);
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
    /// and reads tiles 0 and 1 by offset. A tile with no tile-part takes the
    /// walk to the end of the tile-parts.
    #[test]
    fn a_refusal_at_an_early_tile_walks_little() {
        let bytes = rescued_tiles(64, 16, &[1], &[]);
        let (reads, header) = sot_reads_of(&bytes, &DecodeOptions::default());
        assert!(matches!(header, Err(Error::Limit(_))), "{header:?}");
        assert_eq!(reads, 16 + 1 + 2);
        // Tile 1's tile-parts left out: with `strict` on the walk runs to
        // `EOC` to find it has none, and the file is refused with the
        // decode's error, from the decode's parse of the tile-parts, which
        // counts the first tile-parts again with a walk of its own.
        let missing = rescued_tiles(64, 16, &[], &[1]);
        let (reads, header) = sot_reads_of(&missing, &DecodeOptions::default());
        let decoded = crate::decode_with(&missing, &DecodeOptions::default());
        assert!(matches!(header, Err(Error::Codestream(_))), "{header:?}");
        assert_eq!(
            format!("{:?}", header.err()),
            format!("{:?}", decoded.err())
        );
        assert_eq!(reads, 63 * 16 + 1 + 1 + (63 * 16 + 1));
        // With it off the tile is left out and the rest are read by offset.
        let lenient = DecodeOptions::default().with_strict(false);
        assert_eq!(sot_reads(&missing, &lenient), 63 * 16 + 1 + 63);
    }
}
