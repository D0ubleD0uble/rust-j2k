//! Palette expansion: the Palette box (I.5.3.4, `pclr`) and the Component
//! Mapping box (I.5.3.5, `cmap`).
//!
//! A palette turns a decoded component of indices into output channels: each
//! `cmap` entry says which component feeds a channel and whether the channel is
//! that component as coded (`MTYP 0`) or its samples looked up in one palette
//! column (`MTYP 1`). OpenJPEG 2.5.0 (`opj_jp2_read_pclr`, `opj_jp2_read_cmap`,
//! `opj_jp2_check_color`, `opj_jp2_apply_pclr`) is the reference, and this
//! module follows it on what is accepted and on every output sample:
//!
//! - The `cmap` box holds one entry per palette column (`NPC` entries); a
//!   shorter box is an error and bytes past the last entry are ignored.
//! - A `cmap` must come after the `pclr` in file order; at most one of each.
//! - A palette column mapped by two channels, or a palette channel `i` whose
//!   column `i` no channel maps (a direct channel counts as mapping column 0),
//!   is an error.
//! - A palette index is clamped to `0..NE`.
//! - An entry is its stored bytes as an unsigned number: a signed column is
//!   not sign-extended and stray high bits are kept.
//! - A palette channel takes its depth and sign from its column's `Bi`; a
//!   direct channel `i` takes them from `Bi` of column `i`, as OpenJPEG does.
//! - A single-component file whose mapping leaves a palette column unused is
//!   read as the identity mapping (channel `i` from column `i`), OpenJPEG's
//!   correction for files with a broken `cmap`.
//!
//! Two rules differ. OpenJPEG requires channel `i` of a palette mapping to use
//! column `i`, an implementation limit of its own; any column is accepted
//! here, and such a file is mapped as its `cmap` says. And a palette column
//! deeper than the `i32` sample container (over 31 bits unsigned, 32 signed)
//! is [`Error::Unsupported`], as such a codestream component is; OpenJPEG
//! decodes it.
//!
//! [`plan`] reads the boxes and the main header into a [`PalettePlan`] before
//! any tile is decoded; [`PalettePlan::apply`] then builds the channels from
//! the decoded components. The box layout is checked whenever a palette is
//! present. The mapping is checked against the codestream only when the
//! palette is applied, so a caller asking for the raw channels gets them from
//! a file whose `cmap` names a component that does not exist, as OpenJPEG does
//! with its "ignore palette" flag.

use super::header::Jp2Header;
use super::metadata::{Association, ChannelDefinition, ChannelType, Jp2Metadata};
use crate::codestream::MAX_IMAGE_SAMPLES;
use crate::codestream::markers::Siz;
use crate::{Component, Error, Result};

/// The most entries a palette may hold (I.5.3.4: `NE` is 1..=1024).
const MAX_ENTRIES: u16 = 1024;

/// One palette column's sample format, from its `Bi` byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Column {
    /// Bit depth, `(Bi & 0x7F) + 1`: 1..=128.
    depth: u8,
    /// Whether the column's values are signed (`Bi & 0x80`).
    signed: bool,
}

impl Column {
    /// Bytes per stored entry: the depth rounded up to whole bytes.
    fn entry_bytes(self) -> usize {
        usize::from(self.depth).div_ceil(8)
    }

    /// Whether the column's range fits the `i32` sample container (signed up
    /// to 32 bits, unsigned up to 31), as the codestream's components must.
    fn fits_i32(self) -> bool {
        if self.signed {
            self.depth <= 32
        } else {
            self.depth <= 31
        }
    }
}

/// A parsed `pclr` box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Palette {
    /// `NE`, 1..=1024.
    entries: u16,
    /// One per column (`NPC`, 1..=255).
    columns: Vec<Column>,
    /// `NE * NPC` values, entry-major: entry `j` column `i` at
    /// `j * NPC + i`.
    values: Vec<i32>,
}

impl Palette {
    /// Parse a `pclr` box: `NE` (2 bytes), `NPC` (1 byte), `NPC` `Bi` bytes,
    /// then `NE` rows of `NPC` entries, each `ceil(depth / 8)` bytes.
    fn parse(payload: &[u8]) -> Result<Self> {
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
        let columns: Vec<Column> = bi
            .iter()
            .map(|&b| Column {
                depth: (b & 0x7F) + 1,
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

    /// The value of entry `index` (clamped to `0..NE`) in `column`.
    fn lookup(&self, index: i32, column: usize) -> i32 {
        let top = i32::from(self.entries) - 1;
        let row = index.clamp(0, top) as usize;
        self.values[row * self.columns.len() + column]
    }
}

/// An entry's stored bytes as OpenJPEG reads them: big-endian, unsigned, at
/// most the last four bytes significant, then reinterpreted as `i32`. Columns
/// deeper than 32 bits never reach the output (see [`Column::fits_i32`]).
fn entry_value(bytes: &[u8]) -> i32 {
    let low = &bytes[bytes.len().saturating_sub(4)..];
    let value = low.iter().fold(0u32, |acc, &b| (acc << 8) | u32::from(b));
    value as i32
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
    Skipped { index_components: Vec<bool> },
    /// The palette applies: one output channel per entry.
    Apply {
        palette: Palette,
        channels: Vec<PlannedChannel>,
    },
}

/// Read the file's palette boxes into a [`PalettePlan`]. The box layout is
/// always checked; the mapping is checked against `siz` only when the palette
/// is applied.
pub(crate) fn plan(header: &Jp2Header<'_>, siz: &Siz, raw_channels: bool) -> Result<PalettePlan> {
    if header.pclr.len() > 1 {
        return Err(Error::FileFormat(
            "the file has more than one `pclr` box".into(),
        ));
    }
    if header.cmap.len() > 1 {
        return Err(Error::FileFormat(
            "the file has more than one `cmap` box".into(),
        ));
    }
    let Some(pclr) = header.pclr.first() else {
        if header.cmap.is_empty() {
            return Ok(PalettePlan::None);
        }
        return Err(Error::FileFormat(
            "the file has a `cmap` box but no `pclr` box".into(),
        ));
    };
    if header.cmap_before_pclr {
        return Err(Error::FileFormat(
            "the `cmap` box comes before the `pclr` box it maps".into(),
        ));
    }
    let palette = Palette::parse(pclr)?;
    let Some(cmap) = header.cmap.first() else {
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
        return Ok(PalettePlan::Skipped { index_components });
    }
    let sources = map_channels(&entries, palette.columns.len(), siz.components.len())?;
    let channels = sources
        .into_iter()
        .enumerate()
        .map(|(i, source)| plan_channel(i, source, &palette, siz))
        .collect::<Result<_>>()?;
    Ok(PalettePlan::Apply { palette, channels })
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

/// Check the `cmap` entries the way `opj_jp2_check_color` does, less its
/// "channel `i` uses column `i`" limit, and turn them into channel sources.
///
/// OpenJPEG tracks which columns are used: a palette channel uses its column,
/// and a direct channel (whose `PCOL` is 0) marks column 0. A column used by
/// two palette channels, or a palette channel `i` whose column `i` ends up
/// unused, is refused. A single-component file that still leaves a column
/// unused is then read as the identity mapping.
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
            column.depth,
            if column.signed { "signed" } else { "unsigned" }
        )));
    }
    let component = siz
        .components
        .get(usize::from(source.component()))
        .ok_or_else(|| Error::Inconsistent("`cmap` component past SIZ".into()))?;
    Ok(PlannedChannel {
        source,
        bit_depth: column.depth,
        signed: column.signed,
        x_sampling: component.x_sampling,
        y_sampling: component.y_sampling,
    })
}

impl PalettePlan {
    /// The output channels from the decoded components, in SIZ order.
    pub(crate) fn apply(&self, components: Vec<Component>) -> Result<Vec<Component>> {
        let PalettePlan::Apply { palette, channels } = self else {
            return Ok(components);
        };
        let source = |channel: &PlannedChannel| {
            let index = channel.source.component();
            components.get(usize::from(index)).ok_or_else(|| {
                Error::Inconsistent(format!(
                    "`cmap` names component {index}, which was not decoded"
                ))
            })
        };

        // The expansion allocates a plane per channel: hold the total to the
        // decoder's sample budget before allocating any of them.
        let mut total: u64 = 0;
        for channel in channels {
            total = total.saturating_add(source(channel)?.samples.len() as u64);
            if total > MAX_IMAGE_SAMPLES {
                return Err(Error::Limit(format!(
                    "palette expansion to {} channels totals over {MAX_IMAGE_SAMPLES} samples, \
                     above the decode guard",
                    channels.len()
                )));
            }
        }

        channels
            .iter()
            .map(|channel| {
                let from = source(channel)?;
                let samples = match channel.source {
                    Source::Direct { .. } => from.samples.clone(),
                    Source::Palette { column, .. } => from
                        .samples
                        .iter()
                        .map(|&index| palette.lookup(index, usize::from(column)))
                        .collect(),
                };
                Ok(Component::new(
                    from.width,
                    from.height,
                    channel.bit_depth,
                    channel.signed,
                    channel.x_sampling,
                    channel.y_sampling,
                    samples,
                ))
            })
            .collect()
    }

    /// Record the plan on the file's metadata: whether the palette was
    /// applied, and, when the caller skipped it, which raw channels are index
    /// planes. An index plane is `Unspecified` with no association: it is not
    /// a colour until it goes through the palette.
    pub(crate) fn describe(&self, metadata: &mut Jp2Metadata) {
        metadata.palette_applied = matches!(self, PalettePlan::Apply { .. });
        if let PalettePlan::Skipped { index_components } = self {
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
                Column {
                    depth: 8,
                    signed: false
                },
                Column {
                    depth: 12,
                    signed: false
                },
                Column {
                    depth: 1,
                    signed: false
                },
            ]
        );
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

    /// 255 channels over a 300 000-sample component is 76.5 M samples, over
    /// the 2^26 budget: refused before any channel is allocated.
    #[test]
    fn the_expansion_is_held_to_the_sample_budget() {
        let palette = Palette::parse(&pclr(1, &[7; 255], &[0; 255])).unwrap();
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
        let plan = PalettePlan::Apply {
            palette,
            channels: (0..=254).map(channel).collect(),
        };
        let index = Component::new(600, 500, 8, false, 1, 1, vec![0; 300_000]);
        assert!(matches!(
            plan.apply(vec![index.clone()]),
            Err(Error::Limit(_))
        ));

        // Within budget the same plan expands.
        let PalettePlan::Apply { palette, channels } = plan else {
            unreachable!()
        };
        let small = PalettePlan::Apply {
            palette,
            channels: channels[..2].to_vec(),
        };
        assert_eq!(small.apply(vec![index]).unwrap().len(), 2);
    }

    #[test]
    fn a_wide_entry_keeps_its_last_four_bytes() {
        assert_eq!(entry_value(&[0xAA, 0x01, 0x02, 0x03, 0x04]), 0x0102_0304);
        assert_eq!(entry_value(&[0xFF, 0xFF, 0xFF, 0xFF]), -1);
        assert_eq!(entry_value(&[]), 0);
    }
}
