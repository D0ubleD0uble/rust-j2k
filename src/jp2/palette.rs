//! Palette expansion: the Palette box (I.5.3.4, `pclr`) and the Component
//! Mapping box (I.5.3.5, `cmap`).
//!
//! A palette turns a decoded component of indices into output channels: each
//! `cmap` entry says which component feeds a channel and whether the channel is
//! that component as coded (`MTYP 0`) or its samples looked up in one palette
//! column (`MTYP 1`). OpenJPEG 2.5.0 (`opj_jp2_read_pclr`, `opj_jp2_read_cmap`,
//! `opj_jp2_check_color`, `opj_jp2_apply_pclr`) is the reference. This module
//! follows it on what is accepted and on the output samples, with the two
//! exceptions listed after these rules:
//!
//! - The `cmap` box holds one entry per palette column (`NPC` entries); a
//!   shorter box is an error and bytes past the last entry are ignored.
//! - A `cmap` must come after the `pclr` in file order; at most one of each.
//! - A palette column mapped by two channels, or a palette channel `i` whose
//!   column `i` no channel maps (a direct channel counts as mapping column 0),
//!   is an error.
//! - A palette channel `i` must look up column `i`. OpenJPEG has no general
//!   mapping, so another column is [`Error::Unsupported`].
//! - A single-component file whose mapping leaves a palette column unused is
//!   read as the identity mapping (channel `i` from column `i`), OpenJPEG's
//!   correction for files with a broken `cmap`.
//! - A palette index is clamped to `0..NE`.
//! - An entry is read as OpenJPEG reads it: at most 4 bytes, unsigned, so a
//!   signed column is not sign-extended and stray high bits are kept.
//! - A palette channel takes its depth and sign from its column's `Bi`; a
//!   direct channel `i` takes them from `Bi` of column `i`. Samples may fall
//!   outside that declared range.
//!
//! The exceptions:
//!
//! - A palette column deeper than the `i32` sample container (over 31 bits
//!   unsigned, 32 signed) is [`Error::Unsupported`] when the palette is
//!   applied, as such a codestream component is. OpenJPEG decodes it.
//! - Each channel is built on its own source component's grid, so palette and
//!   direct channels from components of different sub-sampling decode
//!   correctly. OpenJPEG sizes every channel by the first one's grid and
//!   crashes or reads uninitialised memory.
//!
//! [`plan`] reads the boxes and the main header into a [`PalettePlan`] before
//! any tile is decoded; [`PalettePlan::apply`] then builds the channels from
//! the decoded components. The box layout is checked whenever a palette is
//! present. The mapping, the column depths, and the sample budget are checked
//! only when the palette is applied, so a caller asking for the raw channels
//! gets them whatever the `cmap` says, as OpenJPEG does with its "ignore
//! palette" flag. The budget bounds the expanded channels; with the decoded
//! components still held, the peak is about twice it (`check_budget`).

use super::header::Jp2Header;
use super::metadata::{Association, ChannelDefinition, ChannelType, Jp2Metadata};
use crate::codestream::MAX_IMAGE_SAMPLES;
use crate::codestream::markers::Siz;
use crate::{Component, ComponentInfo, Error, Result};

/// The most entries a palette may hold (I.5.3.4: `NE` is 1..=1024).
const MAX_ENTRIES: u16 = 1024;

/// One palette column's sample format, from its `Bi` byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PaletteColumn {
    /// Bit depth, `(Bi & 0x7F) + 1`: 1 to 128.
    pub bit_depth: u8,
    /// Whether the column's values are signed (`Bi & 0x80`).
    pub signed: bool,
}

impl PaletteColumn {
    /// Bytes per stored entry as OpenJPEG reads the table: the depth rounded
    /// up to whole bytes, but at most 4. A column deeper than 32 bits is
    /// therefore read at a 4-byte stride; such a column is never applied (see
    /// [`PaletteColumn::fits_i32`]), so this only decides how long the table must be.
    fn entry_bytes(self) -> usize {
        usize::from(self.bit_depth).div_ceil(8).min(4)
    }

    /// Whether the column's range fits the `i32` sample container (signed up
    /// to 32 bits, unsigned up to 31), as the codestream's components must.
    fn fits_i32(self) -> bool {
        if self.signed {
            self.bit_depth <= 32
        } else {
            self.bit_depth <= 31
        }
    }
}

/// A JP2 file's palette, from its `pclr` box: a table of `NE` entries (1 to
/// 1024), each a row of one value per column (1 to 255 columns).
///
/// Entry `j`, column `i` is `entry(j)[i]`. A value is the entry's stored
/// bytes (the column's depth rounded up to whole bytes, at most 4) read as a
/// big-endian unsigned number and cast to `i32`, as OpenJPEG does: a signed
/// column's values are not sign-extended. A lookup clamps its index to
/// `0..entry_count()`, as the decoder does.
///
/// The table is at most 1024 entries by 255 columns of `i32`, about 1 MiB, and
/// is bounded by the bytes the `pclr` box holds. It is `#[non_exhaustive]` and
/// built only by the decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Palette {
    /// `NE`, 1..=1024.
    entries: u16,
    /// One per column (`NPC`, 1..=255).
    columns: Vec<PaletteColumn>,
    /// `NE * NPC` values, entry-major: entry `j` column `i` at
    /// `j * NPC + i`.
    values: Vec<i32>,
}

impl Palette {
    /// Parse a `pclr` box: `NE` (2 bytes), `NPC` (1 byte), `NPC` `Bi` bytes,
    /// then `NE` rows of `NPC` entries (see [`PaletteColumn::entry_bytes`]).
    pub(super) fn parse(payload: &[u8]) -> Result<Self> {
        let &[ne0, ne1, npc, ref rest @ ..] = payload else {
            return Err(Error::FileFormat(format!(
                "the `pclr` box holds {} bytes, fewer than its 3-byte header",
                payload.len()
            )));
        };
        let entries = u16::from_be_bytes([ne0, ne1]);
        if !(1..=MAX_ENTRIES).contains(&entries) {
            return Err(Error::FileFormat(format!(
                "the `pclr` box declares {entries} entries; a palette holds 1 to {MAX_ENTRIES}"
            )));
        }
        if npc == 0 {
            return Err(Error::FileFormat(
                "the `pclr` box declares 0 palette columns".into(),
            ));
        }
        let Some((bi, table)) = rest.split_at_checked(usize::from(npc)) else {
            return Err(Error::FileFormat(format!(
                "the `pclr` box declares {npc} columns but ends inside their `Bi` bytes"
            )));
        };
        let columns: Vec<PaletteColumn> = bi
            .iter()
            .map(|&b| PaletteColumn {
                bit_depth: (b & 0x7F) + 1,
                signed: b & 0x80 != 0,
            })
            .collect();

        // Bound the table by the bytes the box holds before allocating it.
        let row_bytes: usize = columns.iter().map(|c| c.entry_bytes()).sum();
        let needed = row_bytes.checked_mul(usize::from(entries)).ok_or_else(|| {
            Error::FileFormat("the `pclr` box's entry table size overflows".into())
        })?;
        if table.len() < needed {
            return Err(Error::FileFormat(format!(
                "the `pclr` box's {entries} x {npc} entries need {needed} bytes but it holds {}",
                table.len()
            )));
        }

        let mut values = Vec::with_capacity(usize::from(entries) * columns.len());
        let mut at = 0;
        for _ in 0..entries {
            for column in &columns {
                let width = column.entry_bytes();
                let bytes = table
                    .get(at..at + width)
                    .ok_or_else(|| Error::Inconsistent("palette entry past its table".into()))?;
                at += width;
                values.push(entry_value(bytes));
            }
        }
        Ok(Palette {
            entries,
            columns,
            values,
        })
    }

    /// Each column's depth and sign (`NPC` of them), in column order.
    #[must_use]
    pub fn columns(&self) -> &[PaletteColumn] {
        &self.columns
    }

    /// The number of entries, `NE`.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        usize::from(self.entries)
    }

    /// Entry `index`: one value per column, or `None` past the last entry.
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<&[i32]> {
        let width = self.columns.len();
        self.values.get(index * width..(index + 1) * width)
    }

    /// The value of entry `index` (clamped to `0..NE`) in `column`.
    fn lookup(&self, index: i32, column: usize) -> i32 {
        let top = i32::from(self.entries) - 1;
        let row = index.clamp(0, top) as usize;
        self.values[row * self.columns.len() + column]
    }
}

/// An entry's stored bytes (1 to 4 of them), big-endian and unsigned, then
/// reinterpreted as `i32` as OpenJPEG's `(OPJ_INT32)` cast does.
fn entry_value(bytes: &[u8]) -> i32 {
    let value = bytes.iter().fold(0u32, |acc, &b| (acc << 8) | u32::from(b));
    value as i32
}

/// One channel of a file's component mapping (`cmap` box): where an output
/// channel's samples come from.
///
/// The decoder's output channels follow these entries in order, with one
/// exception: for a one-component file whose `cmap` leaves a palette column
/// unused, OpenJPEG reads the mapping as every channel looking up its own
/// column, and so does this decoder. Under
/// [`raw_channels`](crate::DecodeOptions::raw_channels) the entries are
/// reported as the box has them, unchecked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChannelMapping {
    /// `CMP`: the codestream component this channel reads.
    pub component: u16,
    /// `MTYP` and `PCOL`: how the component becomes the channel.
    pub kind: MappingKind,
}

/// How a mapped channel is made from its component (`MTYP`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MappingKind {
    /// `MTYP` 0: the component's samples as coded.
    Direct,
    /// `MTYP` 1: the component's samples are indexes into this palette
    /// column (`PCOL`).
    Palette {
        /// The column to look up, counting from 0.
        column: u8,
    },
    /// An `MTYP` other than 0 and 1, which the standard reserves. Only
    /// reported under [`raw_channels`](crate::DecodeOptions::raw_channels),
    /// which does not check the mapping; a normal decode refuses such a file.
    Reserved {
        /// The `MTYP` value.
        mtyp: u8,
        /// The `PCOL` value.
        pcol: u8,
    },
}

/// `cmap` entries as public data, in order.
fn public_mapping(entries: &[(u16, u8, u8)]) -> Vec<ChannelMapping> {
    entries
        .iter()
        .map(|&(component, mtyp, pcol)| ChannelMapping {
            component,
            kind: match mtyp {
                0 => MappingKind::Direct,
                1 => MappingKind::Palette { column: pcol },
                mtyp => MappingKind::Reserved { mtyp, pcol },
            },
        })
        .collect()
}

/// Where one output channel's samples come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// `MTYP 0`: codestream component `component` as coded.
    Direct { component: u16 },
    /// `MTYP 1`: codestream component `component`'s samples looked up in
    /// palette column `column`.
    Palette { component: u16, column: u8 },
}

impl Source {
    fn component(self) -> u16 {
        match self {
            Source::Direct { component } | Source::Palette { component, .. } => component,
        }
    }
}

/// One output channel of a palettised file, known from the boxes and the main
/// header alone: its source and its sample format. The grid size comes from
/// the source component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlannedChannel {
    pub(crate) source: Source,
    pub(crate) bit_depth: u8,
    pub(crate) signed: bool,
    pub(crate) x_sampling: u8,
    pub(crate) y_sampling: u8,
}

/// What the palette boxes do to a file's channels. Computed before any tile is
/// decoded, so a header-only read can report the output channels too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PalettePlan {
    /// No palette applies: no `pclr`, or a `pclr` without a `cmap` (I.5.3.4:
    /// both or neither, so the palette is dropped). The channels are the
    /// components as coded.
    None,
    /// The caller asked for the raw channels of a file whose palette would
    /// have applied. `index_components[k]` is set for each component a
    /// palette channel would have looked up, so it can be labelled as an
    /// index plane rather than a colour.
    Skipped {
        index_components: Vec<bool>,
        palette: Palette,
        mapping: Vec<ChannelMapping>,
    },
    /// The palette applies: one output channel per palette column.
    Apply {
        palette: Palette,
        channels: Vec<PlannedChannel>,
    },
}

/// Read the file's palette boxes into a [`PalettePlan`], for a decode at
/// `reduction` resolution levels down. The box layout is always checked; the
/// mapping, the column depths and the expanded sample count only when the
/// palette is applied.
pub(crate) fn plan(
    header: &Jp2Header<'_>,
    siz: &Siz,
    reduction: u8,
    raw_channels: bool,
) -> Result<PalettePlan> {
    let (pclrs, cmaps) = (header.pclr(), header.cmap());
    if pclrs.len() > 1 {
        return Err(Error::FileFormat(
            "the file has more than one `pclr` box".into(),
        ));
    }
    if cmaps.len() > 1 {
        return Err(Error::FileFormat(
            "the file has more than one `cmap` box".into(),
        ));
    }
    let Some(pclr) = pclrs.first() else {
        if cmaps.is_empty() {
            return Ok(PalettePlan::None);
        }
        return Err(Error::FileFormat(
            "the file has a `cmap` box but no `pclr` box".into(),
        ));
    };
    if cmap_before_pclr(header) {
        return Err(Error::FileFormat(
            "the `cmap` box comes before the `pclr` box it maps".into(),
        ));
    }
    let palette = Palette::parse(pclr)?;
    let Some(cmap) = cmaps.first() else {
        return Ok(PalettePlan::None);
    };
    let entries = parse_cmap(cmap, palette.columns.len())?;
    if raw_channels {
        let mut index_components = vec![false; siz.components.len()];
        for &(component, mtyp, _) in &entries {
            if let (1, Some(slot)) = (mtyp, index_components.get_mut(usize::from(component))) {
                *slot = true;
            }
        }
        return Ok(PalettePlan::Skipped {
            index_components,
            palette,
            mapping: public_mapping(&entries),
        });
    }
    let sources = map_channels(&entries, palette.columns.len(), siz.components.len())?;
    let channels: Vec<PlannedChannel> = sources
        .into_iter()
        .enumerate()
        .map(|(i, source)| plan_channel(i, source, &palette, siz))
        .collect::<Result<_>>()?;
    check_budget(&channels, siz, reduction)?;
    Ok(PalettePlan::Apply { palette, channels })
}

/// Whether the first `cmap` comes before the first `pclr` in read order (every
/// `jp2h`, then the header boxes misplaced after it). OpenJPEG needs the
/// palette first to know how many `cmap` entries to read.
fn cmap_before_pclr(header: &Jp2Header<'_>) -> bool {
    let position = |kind: &[u8; 4]| header.children.iter().position(|c| &c.kind == kind);
    match (position(b"cmap"), position(b"pclr")) {
        (Some(cmap), Some(pclr)) => cmap < pclr,
        _ => false,
    }
}

/// Parse a `cmap` box into `(CMP, MTYP, PCOL)` triples, one per palette
/// column. Bytes after the last of them are ignored, as OpenJPEG does.
fn parse_cmap(payload: &[u8], count: usize) -> Result<Vec<(u16, u8, u8)>> {
    let needed = count * 4;
    let Some(body) = payload.get(..needed) else {
        return Err(Error::FileFormat(format!(
            "the `cmap` box holds {} bytes; mapping {count} palette columns needs {needed}",
            payload.len()
        )));
    };
    Ok(body
        .chunks_exact(4)
        .map(|e| (u16::from_be_bytes([e[0], e[1]]), e[2], e[3]))
        .collect())
}

/// Check the `cmap` entries the way `opj_jp2_check_color` does, and turn them
/// into channel sources.
///
/// OpenJPEG tracks which columns are used: a palette channel uses its column,
/// and a direct channel (whose `PCOL` is 0) marks column 0. A malformed entry,
/// a column used by two palette channels, or a palette channel `i` whose
/// column `i` ends up unused is [`Error::FileFormat`]; those come first, so a
/// malformed file reports its fault. A palette channel `i` that looks up
/// another column is then [`Error::Unsupported`] (OpenJPEG's "implementation
/// limitation"). A single-component file that still leaves a column unused is
/// read as the identity mapping.
fn map_channels(
    entries: &[(u16, u8, u8)],
    columns: usize,
    components: usize,
) -> Result<Vec<Source>> {
    let mut used = vec![false; columns];
    for (i, &(component, mtyp, pcol)) in entries.iter().enumerate() {
        if usize::from(component) >= components {
            return Err(Error::FileFormat(format!(
                "`cmap` channel {i} maps component {component}, but the codestream has {components}"
            )));
        }
        let column = usize::from(pcol);
        if mtyp > 1 {
            return Err(Error::FileFormat(format!(
                "`cmap` channel {i} has mapping type {mtyp}; only 0 (direct) and 1 (palette) exist"
            )));
        }
        if column >= columns {
            return Err(Error::FileFormat(format!(
                "`cmap` channel {i} names palette column {pcol}, but the palette has {columns}"
            )));
        }
        if mtyp == 1 && used[column] {
            return Err(Error::FileFormat(format!(
                "`cmap` maps palette column {pcol} twice"
            )));
        }
        if mtyp == 0 && pcol != 0 {
            return Err(Error::FileFormat(format!(
                "`cmap` channel {i} uses component {component} directly but names palette \
                 column {pcol}; a direct channel's PCOL must be 0"
            )));
        }
        used[column] = true;
    }
    for (i, &(_, mtyp, _)) in entries.iter().enumerate() {
        if mtyp != 0 && !used[i] {
            return Err(Error::FileFormat(format!(
                "`cmap` channel {i} is a palette channel but palette column {i} is not mapped"
            )));
        }
    }
    for (i, &(_, mtyp, pcol)) in entries.iter().enumerate() {
        if mtyp == 1 && usize::from(pcol) != i {
            return Err(Error::Unsupported(format!(
                "`cmap` channel {i} looks up palette column {pcol}; only channel i from \
                 column i is decoded"
            )));
        }
    }

    // OpenJPEG's correction for a broken single-component `cmap` ("Component
    // mapping seems wrong. Trying to correct."): every channel becomes a
    // lookup in its own column.
    let correct = components == 1 && used.contains(&false);
    entries
        .iter()
        .enumerate()
        .map(|(i, &(component, mtyp, pcol))| {
            Ok(if correct {
                let column = u8::try_from(i)
                    .map_err(|_| Error::Inconsistent(format!("palette column {i} exceeds NPC")))?;
                Source::Palette { component, column }
            } else if mtyp == 0 {
                Source::Direct { component }
            } else {
                Source::Palette {
                    component,
                    column: pcol,
                }
            })
        })
        .collect()
}

/// Channel `i`'s sample format. A palette channel takes its column's `Bi`; a
/// direct channel takes `Bi` of column `i`, as OpenJPEG labels it (`i` is
/// below `NPC`, since `cmap` holds one entry per column). The sub-sampling is
/// the source component's.
fn plan_channel(i: usize, source: Source, palette: &Palette, siz: &Siz) -> Result<PlannedChannel> {
    let label = match source {
        Source::Direct { .. } => i,
        Source::Palette { column, .. } => usize::from(column),
    };
    let column = palette
        .columns
        .get(label)
        .ok_or_else(|| Error::Inconsistent(format!("no palette column {label}")))?;
    if !column.fits_i32() {
        return Err(Error::Unsupported(format!(
            "palette channel {i} has depth {} ({}), which exceeds the i32 sample container",
            column.bit_depth,
            if column.signed { "signed" } else { "unsigned" }
        )));
    }
    let component = siz
        .components
        .get(usize::from(source.component()))
        .ok_or_else(|| Error::Inconsistent("`cmap` component past SIZ".into()))?;
    Ok(PlannedChannel {
        source,
        bit_depth: column.bit_depth,
        signed: column.signed,
        x_sampling: component.x_sampling,
        y_sampling: component.y_sampling,
    })
}

/// Hold the expanded channels to the decoder's sample budget, from the SIZ
/// extents at `reduction`, before any tile is decoded or plane allocated.
/// The budget bounds the expanded channels alone: the decoded components are
/// held while the channels are built from them, so the peak is about twice
/// the budget (2^26 samples is 256 MiB as `i32`, so roughly 512 MiB).
fn check_budget(channels: &[PlannedChannel], siz: &Siz, reduction: u8) -> Result<()> {
    let mut total: u64 = 0;
    for channel in channels {
        let index = usize::from(channel.source.component());
        let (width, height) = siz
            .component_extent_at(index, reduction)
            .ok_or_else(|| Error::Inconsistent(format!("SIZ declares no component {index}")))?;
        total = total.saturating_add(u64::from(width) * u64::from(height));
        if total > MAX_IMAGE_SAMPLES {
            return Err(Error::Limit(format!(
                "palette expansion to {} channels totals over {MAX_IMAGE_SAMPLES} samples, \
                 above the decode guard",
                channels.len()
            )));
        }
    }
    Ok(())
}

impl PalettePlan {
    /// The format of each output channel, from the plan and SIZ alone: the
    /// palette's channels when it applies, otherwise the codestream's
    /// components as coded.
    pub(crate) fn output_channels(&self, siz: &Siz, reduction: u8) -> Vec<ComponentInfo> {
        match self {
            PalettePlan::Apply { channels, .. } => channels
                .iter()
                .map(|c| {
                    ComponentInfo::on_grid(
                        siz,
                        c.bit_depth,
                        c.signed,
                        c.x_sampling,
                        c.y_sampling,
                        reduction,
                    )
                })
                .collect(),
            PalettePlan::None | PalettePlan::Skipped { .. } => {
                ComponentInfo::of_siz(siz, reduction)
            }
        }
    }

    /// The output channels from the decoded components, in SIZ order. Each
    /// channel is built on its source component's grid. A direct channel
    /// takes its component's plane when no later channel reads it.
    pub(crate) fn apply(&self, mut components: Vec<Component>) -> Result<Vec<Component>> {
        let PalettePlan::Apply { palette, channels } = self else {
            return Ok(components);
        };
        let mut reads_left = vec![0usize; components.len()];
        for channel in channels {
            let index = usize::from(channel.source.component());
            let slot = reads_left.get_mut(index).ok_or_else(|| {
                Error::Inconsistent(format!(
                    "`cmap` names component {index}, which was not decoded"
                ))
            })?;
            *slot += 1;
        }

        let mut out = Vec::with_capacity(channels.len());
        for channel in channels {
            let index = usize::from(channel.source.component());
            reads_left[index] -= 1;
            let from = &mut components[index];
            let samples = match channel.source {
                Source::Direct { .. } if reads_left[index] == 0 => {
                    std::mem::take(&mut from.samples)
                }
                Source::Direct { .. } => from.samples.clone(),
                Source::Palette { column, .. } => from
                    .samples
                    .iter()
                    .map(|&k| palette.lookup(k, usize::from(column)))
                    .collect(),
            };
            out.push(Component::new(
                from.width,
                from.height,
                channel.bit_depth,
                channel.signed,
                channel.x_sampling,
                channel.y_sampling,
                samples,
            ));
        }
        Ok(out)
    }

    /// Record the plan on the file's metadata: whether the palette was
    /// applied, and, when the caller skipped it, which raw channels are index
    /// planes. An index plane is `Unspecified` with no association: it is not
    /// a colour until it goes through the palette.
    pub(crate) fn describe(&self, metadata: &mut Jp2Metadata) {
        metadata.palette_applied = matches!(self, PalettePlan::Apply { .. });
        match self {
            PalettePlan::None => {}
            PalettePlan::Apply { palette, channels } => {
                metadata.palette = Some(palette.clone());
                metadata.channel_mapping = Some(
                    channels
                        .iter()
                        .map(|c| match c.source {
                            Source::Direct { component } => ChannelMapping {
                                component,
                                kind: MappingKind::Direct,
                            },
                            Source::Palette { component, column } => ChannelMapping {
                                component,
                                kind: MappingKind::Palette { column },
                            },
                        })
                        .collect(),
                );
            }
            PalettePlan::Skipped {
                palette, mapping, ..
            } => {
                metadata.palette = Some(palette.clone());
                metadata.channel_mapping = Some(mapping.clone());
            }
        }
        if let PalettePlan::Skipped {
            index_components, ..
        } = self
        {
            for (channel, &index) in metadata.channels.iter_mut().zip(index_components) {
                if index {
                    *channel = ChannelDefinition {
                        kind: ChannelType::Unspecified,
                        association: Association::None,
                    };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codestream::markers::SizComponent;

    fn pclr(entries: u16, bi: &[u8], table: &[u8]) -> Vec<u8> {
        let mut out = entries.to_be_bytes().to_vec();
        out.push(u8::try_from(bi.len()).unwrap());
        out.extend_from_slice(bi);
        out.extend_from_slice(table);
        out
    }

    #[test]
    fn entries_are_read_at_their_byte_widths() {
        // 2 entries, columns of 8, 12 and 1 bits: 1 + 2 + 1 bytes per row.
        let palette =
            Palette::parse(&pclr(2, &[7, 11, 0], &[1, 0x0A, 0xBC, 1, 2, 0x0F, 0xFF, 0])).unwrap();
        assert_eq!(palette.values, [1, 0xABC, 1, 2, 0xFFF, 0]);
        assert_eq!(
            palette.columns,
            [
                PaletteColumn {
                    bit_depth: 8,
                    signed: false
                },
                PaletteColumn {
                    bit_depth: 12,
                    signed: false
                },
                PaletteColumn {
                    bit_depth: 1,
                    signed: false
                },
            ]
        );
    }

    /// OpenJPEG reads a column deeper than 32 bits at a 4-byte stride, so a
    /// 38-bit column's table is 4 bytes per entry.
    #[test]
    fn a_deep_column_is_read_at_a_four_byte_stride() {
        let palette = Palette::parse(&pclr(2, &[37], &[0, 0, 0, 7, 0xFF, 0xFF, 0xFF, 0xFF]))
            .expect("4 bytes per entry is enough");
        assert_eq!(palette.values, [7, -1]);
        assert!(matches!(
            Palette::parse(&pclr(2, &[37], &[0; 7])),
            Err(Error::FileFormat(_))
        ));
    }

    /// Measured with `opj_decompress` 2.5.0: a signed 12-bit column holding
    /// `0xFFF` decodes to 4095, not -1, and a 12-bit entry `0xF000` keeps its
    /// high bits.
    #[test]
    fn entries_are_unsigned_and_unmasked_as_in_openjpeg() {
        let palette = Palette::parse(&pclr(1, &[0x8B, 11], &[0x0F, 0xFF, 0xF0, 0x00])).unwrap();
        assert!(palette.columns[0].signed);
        assert_eq!(palette.values, [4095, 0xF000]);
    }

    #[test]
    fn indices_clamp_to_the_palette() {
        let palette = Palette::parse(&pclr(3, &[7], &[10, 20, 30])).unwrap();
        let looked: Vec<i32> = [-5, 0, 1, 2, 3, 1000, i32::MIN, i32::MAX]
            .iter()
            .map(|&k| palette.lookup(k, 0))
            .collect();
        assert_eq!(looked, [10, 10, 20, 30, 30, 30, 10, 30]);
    }

    fn siz(width: u32, height: u32) -> Siz {
        Siz {
            x_size: width,
            y_size: height,
            x_offset: 0,
            y_offset: 0,
            tile_width: width,
            tile_height: height,
            tile_x_offset: 0,
            tile_y_offset: 0,
            components: vec![SizComponent {
                bit_depth: 8,
                signed: false,
                x_sampling: 1,
                y_sampling: 1,
            }],
        }
    }

    /// 255 channels over a 600x500 component is 76.5 M samples, over the 2^26
    /// budget: refused from SIZ alone. A reduced decode, or fewer channels,
    /// fits.
    #[test]
    fn the_expansion_is_held_to_the_sample_budget() {
        let channel = |column| PlannedChannel {
            source: Source::Palette {
                component: 0,
                column,
            },
            bit_depth: 8,
            signed: false,
            x_sampling: 1,
            y_sampling: 1,
        };
        let channels: Vec<PlannedChannel> = (0..=254).map(channel).collect();
        let siz = siz(600, 500);
        assert!(matches!(
            check_budget(&channels, &siz, 0),
            Err(Error::Limit(_))
        ));
        assert!(check_budget(&channels, &siz, 1).is_ok());
        assert!(check_budget(&channels[..200], &siz, 0).is_ok());
    }

    /// A direct channel moves its component's plane when nothing later reads
    /// it, and copies it otherwise; either way the samples are the component's.
    #[test]
    fn a_direct_channel_moves_or_copies_its_plane() {
        let palette = Palette::parse(&pclr(2, &[7, 7], &[10, 20, 30, 40])).unwrap();
        let direct = PlannedChannel {
            source: Source::Direct { component: 0 },
            bit_depth: 8,
            signed: false,
            x_sampling: 1,
            y_sampling: 1,
        };
        let lookup = PlannedChannel {
            source: Source::Palette {
                component: 0,
                column: 1,
            },
            ..direct
        };
        let index = Component::new(2, 1, 8, false, 1, 1, vec![0, 1]);
        for channels in [
            vec![direct, lookup],
            vec![lookup, direct],
            vec![direct, direct],
        ] {
            let plan = PalettePlan::Apply {
                palette: palette.clone(),
                channels: channels.clone(),
            };
            let out = plan.apply(vec![index.clone()]).unwrap();
            for (channel, got) in channels.iter().zip(&out) {
                let want = match channel.source {
                    Source::Direct { .. } => vec![0, 1],
                    Source::Palette { .. } => vec![20, 40],
                };
                assert_eq!(got.samples, want, "{channels:?}");
            }
        }
    }

    #[test]
    fn entries_are_big_endian_and_cast_to_i32() {
        assert_eq!(entry_value(&[0x01, 0x02, 0x03, 0x04]), 0x0102_0304);
        assert_eq!(entry_value(&[0xFF, 0xFF, 0xFF, 0xFF]), -1);
        assert_eq!(entry_value(&[0x0A, 0xBC]), 0xABC);
    }
}
