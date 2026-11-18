//! Decode orchestration: from the input bytes to an [`Image`] or its
//! [`Header`].
//!
//! The crate root's [`decode_with`](crate::decode_with) and
//! [`read_header_with`](crate::read_header_with) are thin wrappers over
//! [`decode_metered`] and [`read_header`]. The incremental decoder calls
//! [`decode_metered`] with its [`Ledger`](incremental::Ledger), and
//! [`check_options`] when it is built.

use super::report::{Scope, report_part, report_tile};
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
    decode_codestream(cs, options, &selection, ledger)
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
    let image = decode_codestream(cs, options, &prefix.selection, ledger)?;
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
/// names.
fn decode_codestream(
    cs: codestream::Codestream<'_>,
    options: &DecodeOptions,
    selection: &window::Selection,
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
    let meter = tier2::Meter::new(packet_bound);
    let mut block_work = match &options.code_block_work_limit {
        WorkLimit::Unlimited => None,
        limit => {
            let bound = limit.apply(tier1::decode_work(bytes));
            Some(ledger.map_or(bound, |l| l.blocks_left(bound)))
        }
    };
    let first_work = block_work.unwrap_or(0);
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
            ledger.refuse();
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
        let header = cs.tile_header(position)?;

        // A tile COD can give one tile a shallower pyramid than the rest, so
        // the reduction is checked per tile, not once for the image.
        check_reduction(&header, reduction, Some(index))?;

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
            Err(tier2::Refused::Work(work)) => {
                if let Some(ledger) = ledger {
                    ledger.refuse();
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
        let footprints = coded
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
            .collect::<Result<Vec<_>>>()?;
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
                report_part(
                    &header,
                    tile,
                    &reach,
                    &scope,
                    reduction,
                    layers,
                    &mut entries,
                )?
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
            let work = tier1::work(&header, &coded, reduction, &wanted);
            #[cfg(test)]
            tests::BLOCK_WORK.with(|spent| spent.set(spent.get() + work));
            let Some(rest) = left.checked_sub(work) else {
                if let Some(ledger) = ledger {
                    ledger.refuse();
                }
                return Err(limit_hint::code_blocks(
                    index,
                    work,
                    *left,
                    first_work.saturating_sub(*left),
                    tier1::decode_work(bytes),
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
}
