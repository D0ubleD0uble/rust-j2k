//! Channel definitions: the Channel Definition box (I.5.3.6, `cdef`).
//!
//! A `cdef` box says, for each output channel, what it is (`Typ`: colour,
//! opacity, premultiplied opacity, unspecified) and which colour it belongs to
//! (`Asoc`). It describes the channels after the palette, when one applies.
//! OpenJPEG 2.5.0 (`opj_jp2_read_cdef`, `opj_jp2_check_color`,
//! `opj_jp2_apply_cdef`) is the reference, and this module follows it on what
//! is accepted and on the output order:
//!
//! - The box holds `N` (2 bytes), then `N` entries of `Cn`, `Typ`, `Asoc`
//!   (2 bytes each). `N = 0`, or fewer bytes than `N` entries need, is an
//!   error; bytes past the last entry are ignored. A second `cdef` is an
//!   error.
//! - Checked against the channel count after the palette: a `Cn` past the
//!   last channel, an `Asoc` other than 0 and 65535 whose `Asoc - 1` is past
//!   the last channel, or a channel no entry names is an error. An entry may
//!   repeat a channel, as long as every channel is named.
//! - The entries are applied in order. An entry of type colour (`Typ 0`)
//!   whose channel is not yet at position `Asoc - 1` swaps it there, and the
//!   later entries follow their channels through the swap. Nothing else
//!   moves a channel. For the usual layouts this puts colour `n` at position
//!   `n - 1` and leaves opacity channels where the file put them.
//!
//! Each output channel is labelled with its own entry (the last one that
//! names it), so the label moves with the samples. OpenJPEG keeps only the
//! type, in a field its tools never write out, and can leave it on the wrong
//! channel after a swap; that field is not an output, so it is not followed.
//!
//! [`plan`] reads the box into a [`ChannelPlan`] before any tile is decoded;
//! [`ChannelPlan::apply`] then reorders the channels the palette step built,
//! and [`ChannelPlan::describe`] labels them.

use super::header::Jp2Header;
use super::metadata::{Association, ChannelDefinition, ChannelType, Jp2Metadata};
use crate::{Error, Result};

/// The bytes of one `cdef` entry: `Cn`, `Typ`, `Asoc`.
const ENTRY_BYTES: usize = 6;

/// One `cdef` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    /// `Cn`: the channel the entry describes.
    channel: u16,
    /// `Typ`.
    kind: u16,
    /// `Asoc`.
    association: u16,
}

impl Entry {
    /// The colour position this entry moves its channel to: `Asoc - 1` for a
    /// colour channel associated with a colour. `None` when the entry moves
    /// nothing (another type, `Asoc` 0 or 65535).
    fn target(self) -> Option<usize> {
        match (self.kind, self.association) {
            (0, 1..=65534) => Some(usize::from(self.association) - 1),
            _ => None,
        }
    }

    fn definition(self) -> ChannelDefinition {
        ChannelDefinition {
            kind: match self.kind {
                0 => ChannelType::Colour,
                1 => ChannelType::Opacity,
                2 => ChannelType::PremultipliedOpacity,
                65535 => ChannelType::Unspecified,
                other => ChannelType::Other(other),
            },
            association: match self.association {
                0 => Association::WholeImage,
                65535 => Association::None,
                n => Association::Colour(n),
            },
        }
    }
}

/// Parse a `cdef` payload into its entries. The length is checked before
/// anything is allocated.
fn parse(payload: &[u8]) -> Result<Vec<Entry>> {
    let &[n0, n1, ref body @ ..] = payload else {
        return Err(Error::FileFormat(format!(
            "the `cdef` box holds {} bytes, fewer than its 2-byte count",
            payload.len()
        )));
    };
    let count = u16::from_be_bytes([n0, n1]);
    if count == 0 {
        return Err(Error::FileFormat(
            "the `cdef` box declares 0 channel definitions".into(),
        ));
    }
    let needed = usize::from(count)
        .checked_mul(ENTRY_BYTES)
        .ok_or_else(|| Error::FileFormat("the `cdef` box's size overflows".into()))?;
    let Some(entries) = body.get(..needed) else {
        return Err(Error::FileFormat(format!(
            "the `cdef` box declares {count} channel definitions ({needed} bytes) but holds {}",
            body.len()
        )));
    };
    Ok(entries
        .chunks_exact(ENTRY_BYTES)
        .map(|e| Entry {
            channel: u16::from_be_bytes([e[0], e[1]]),
            kind: u16::from_be_bytes([e[2], e[3]]),
            association: u16::from_be_bytes([e[4], e[5]]),
        })
        .collect())
}

/// Check a `cdef` payload's layout only, as OpenJPEG reads the box.
pub(super) fn check_layout(payload: &[u8]) -> Result<()> {
    parse(payload).map(|_| ())
}

/// What the `cdef` box does to the file's channels. Computed before any tile
/// is decoded, so a header-only read can report the output channels too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChannelPlan {
    /// No `cdef`, or the caller asked for the raw channels: the channels keep
    /// their order and their default labels.
    None,
    /// The `cdef` applies.
    Apply {
        /// `order[p]` is the input channel that ends up at position `p`.
        order: Vec<usize>,
        /// The label of the channel at each output position.
        definitions: Vec<ChannelDefinition>,
    },
}

/// Read the file's `cdef` box into a [`ChannelPlan`] for `channels` input
/// channels: the codestream's components, or the palette's channels when the
/// palette applies. The box layout is always checked; the entries are checked
/// against `channels` only when they are applied, so a caller asking for the
/// raw channels gets them whatever the entries say, as OpenJPEG does with its
/// "ignore palette and channel definitions" flag.
pub(crate) fn plan(
    header: &Jp2Header<'_>,
    channels: usize,
    raw_channels: bool,
) -> Result<ChannelPlan> {
    let boxes = header.cdef();
    if boxes.len() > 1 {
        return Err(Error::FileFormat(
            "the file has more than one `cdef` box".into(),
        ));
    }
    let Some(payload) = boxes.first() else {
        return Ok(ChannelPlan::None);
    };
    let entries = parse(payload)?;
    if raw_channels {
        return Ok(ChannelPlan::None);
    }
    check(&entries, channels)?;
    Ok(place(&entries, channels))
}

/// Check the entries against the channel count as `opj_jp2_check_color` does:
/// each entry's `Cn` and `Asoc - 1` in range, then every channel named.
fn check(entries: &[Entry], channels: usize) -> Result<()> {
    for (i, entry) in entries.iter().enumerate() {
        if usize::from(entry.channel) >= channels {
            return Err(Error::FileFormat(format!(
                "`cdef` entry {i} describes channel {}, but the file has {channels}",
                entry.channel
            )));
        }
        if !matches!(entry.association, 0 | 65535) && usize::from(entry.association) > channels {
            return Err(Error::FileFormat(format!(
                "`cdef` entry {i} associates channel {} with colour {}, but the file has \
                 {channels} channels",
                entry.channel, entry.association
            )));
        }
    }
    let mut named = vec![false; channels];
    for entry in entries {
        // In range: checked above.
        if let Some(slot) = named.get_mut(usize::from(entry.channel)) {
            *slot = true;
        }
    }
    if let Some(missing) = named.iter().position(|&n| !n) {
        return Err(Error::FileFormat(format!(
            "the `cdef` box does not define channel {missing}; it must define every channel"
        )));
    }
    Ok(())
}

/// Run OpenJPEG's placement over checked entries: each colour entry swaps its
/// channel to `Asoc - 1`, and later entries follow their channels.
fn place(entries: &[Entry], channels: usize) -> ChannelPlan {
    // `order[p]` is the input channel at position `p`; `position[c]` is where
    // input channel `c` is now.
    let mut order: Vec<usize> = (0..channels).collect();
    let mut position: Vec<usize> = (0..channels).collect();
    // Each input channel's label: the last entry naming it. Every channel is
    // named (checked), so the placeholder never survives.
    let mut labels = vec![
        ChannelDefinition {
            kind: ChannelType::Unspecified,
            association: Association::None,
        };
        channels
    ];
    for entry in entries {
        let channel = usize::from(entry.channel);
        labels[channel] = entry.definition();
        let Some(target) = entry.target() else {
            continue;
        };
        let from = position[channel];
        if from != target {
            let displaced = order[target];
            order.swap(from, target);
            position[channel] = target;
            position[displaced] = from;
        }
    }
    let definitions = order.iter().map(|&c| labels[c]).collect();
    ChannelPlan::Apply { order, definitions }
}

impl ChannelPlan {
    /// Put the channels in their output order: the decoded components, or
    /// their formats for a header-only read.
    pub(crate) fn apply<T>(&self, components: Vec<T>) -> Result<Vec<T>> {
        let ChannelPlan::Apply { order, .. } = self else {
            return Ok(components);
        };
        if order.len() != components.len() {
            return Err(Error::Inconsistent(format!(
                "the `cdef` plan covers {} channels but {} were decoded",
                order.len(),
                components.len()
            )));
        }
        let mut slots: Vec<Option<T>> = components.into_iter().map(Some).collect();
        order
            .iter()
            .map(|&from| {
                slots.get_mut(from).and_then(Option::take).ok_or_else(|| {
                    Error::Inconsistent(format!("`cdef` places channel {from} twice"))
                })
            })
            .collect()
    }

    /// Label the output channels with their definitions.
    pub(crate) fn describe(&self, metadata: &mut Jp2Metadata) {
        if let ChannelPlan::Apply { definitions, .. } = self {
            metadata.channels.clone_from(definitions);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Component;

    fn entries(triples: &[(u16, u16, u16)]) -> Vec<Entry> {
        triples
            .iter()
            .map(|&(channel, kind, association)| Entry {
                channel,
                kind,
                association,
            })
            .collect()
    }

    fn order(triples: &[(u16, u16, u16)], channels: usize) -> Vec<usize> {
        let entries = entries(triples);
        check(&entries, channels).expect("valid entries");
        match place(&entries, channels) {
            ChannelPlan::Apply { order, .. } => order,
            ChannelPlan::None => unreachable!(),
        }
    }

    /// Each order measured with `opj_decompress` 2.5.0 on a three-component
    /// file (see `tests/fixtures/README.md` for the harness).
    #[test]
    fn placement_follows_openjpeg() {
        type Case = (&'static [(u16, u16, u16)], [usize; 3]);
        let cases: &[Case] = &[
            (&[(0, 0, 1), (1, 0, 2), (2, 0, 3)], [0, 1, 2]),
            (&[(0, 0, 3), (1, 0, 2), (2, 0, 1)], [2, 1, 0]),
            (&[(2, 0, 1), (1, 0, 2), (0, 0, 3)], [2, 1, 0]),
            (&[(0, 0, 2), (1, 0, 3), (2, 0, 1)], [2, 0, 1]),
            // Two entries claim colour 2: the second swaps the first back.
            (&[(0, 0, 2), (1, 0, 2), (2, 0, 3)], [0, 1, 2]),
            // An opacity channel moves only when a colour channel takes its
            // place.
            (&[(1, 1, 0), (0, 0, 2), (2, 0, 3)], [1, 0, 2]),
            (&[(0, 1, 3), (1, 0, 2), (2, 0, 1)], [2, 1, 0]),
            // Asoc 0 and 65535 move nothing, whatever the type.
            (&[(0, 0, 0), (1, 0, 65535), (2, 0, 3)], [0, 1, 2]),
            (&[(0, 7, 1), (1, 0, 2), (2, 0, 3)], [0, 1, 2]),
            (
                &[(0, 65535, 65535), (1, 65535, 65535), (2, 65535, 65535)],
                [0, 1, 2],
            ),
            // A repeated channel is fine once every channel is named.
            (&[(0, 0, 1), (1, 0, 2), (2, 0, 3), (2, 1, 0)], [0, 1, 2]),
        ];
        for (triples, want) in cases {
            assert_eq!(order(triples, 3), want, "{triples:?}");
        }
    }

    /// Opacity first (an ARGB layout): every colour entry swaps, and the
    /// opacity channel is pushed to the end. Its label goes with it.
    #[test]
    fn an_opacity_channel_pushed_along_keeps_its_label() {
        let entries = entries(&[(0, 1, 0), (1, 0, 1), (2, 0, 2), (3, 0, 3)]);
        check(&entries, 4).unwrap();
        let ChannelPlan::Apply { order, definitions } = place(&entries, 4) else {
            unreachable!()
        };
        assert_eq!(order, [1, 2, 3, 0]);
        let colour = |n| ChannelDefinition {
            kind: ChannelType::Colour,
            association: Association::Colour(n),
        };
        assert_eq!(
            definitions,
            [
                colour(1),
                colour(2),
                colour(3),
                ChannelDefinition {
                    kind: ChannelType::Opacity,
                    association: Association::WholeImage,
                },
            ]
        );
    }

    #[test]
    fn the_last_entry_naming_a_channel_labels_it() {
        let entries = entries(&[(0, 0, 1), (0, 2, 0)]);
        let ChannelPlan::Apply { definitions, .. } = place(&entries, 1) else {
            unreachable!()
        };
        assert_eq!(definitions[0].kind, ChannelType::PremultipliedOpacity);
        assert_eq!(definitions[0].association, Association::WholeImage);
    }

    #[test]
    fn types_and_associations_map_to_their_variants() {
        let label = |kind, association| {
            Entry {
                channel: 0,
                kind,
                association,
            }
            .definition()
        };
        assert_eq!(label(0, 2).kind, ChannelType::Colour);
        assert_eq!(label(1, 0).kind, ChannelType::Opacity);
        assert_eq!(label(2, 0).kind, ChannelType::PremultipliedOpacity);
        assert_eq!(label(65535, 0).kind, ChannelType::Unspecified);
        assert_eq!(label(3, 0).kind, ChannelType::Other(3));
        assert_eq!(label(65534, 0).kind, ChannelType::Other(65534));
        assert_eq!(label(0, 0).association, Association::WholeImage);
        assert_eq!(label(0, 65535).association, Association::None);
        assert_eq!(label(0, 7).association, Association::Colour(7));
    }

    #[test]
    fn apply_moves_each_channel_once() {
        let plan = ChannelPlan::Apply {
            order: vec![2, 0, 1],
            definitions: vec![],
        };
        let plane = |v| Component::new(1, 1, 8, false, 1, 1, vec![v]);
        let out = plan.apply(vec![plane(10), plane(11), plane(12)]).unwrap();
        let samples: Vec<i32> = out.iter().map(|c| c.samples[0]).collect();
        assert_eq!(samples, [12, 10, 11]);
        assert!(matches!(
            plan.apply(vec![plane(10), plane(11)]),
            Err(Error::Inconsistent(_))
        ));
    }

    #[test]
    fn parse_reads_entries_and_ignores_trailing_bytes() {
        let payload = [0, 1, 0, 2, 0, 1, 0, 0, 0xFF, 0xFF];
        assert_eq!(parse(&payload).unwrap(), entries(&[(2, 1, 0)]));
    }
}
