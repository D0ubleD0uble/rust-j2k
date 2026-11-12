//! The metadata of a `raw_channels` decode is enough to rebuild the default
//! decode, in order: expand the index planes through `Jp2Metadata::palette`
//! with `channel_mapping`, then order the channels and label them with
//! `channel_definitions`. The recipe below is the one the `raw_channels`
//! rustdoc gives, and uses nothing but the raw decode and its metadata.

use rust_j2k::{
    Association, ChannelDefinition, ChannelType, DecodeOptions, Jp2Metadata, MappingKind, decode,
    decode_with,
};

const FIXTURES: [&str; 12] = [
    // Palette only.
    "jp2_palette_8bit",
    "jp2_palette_12bit",
    "jp2_palette_clamp",
    "jp2_palette_corrected",
    "jp2_palette_mixed",
    "jp2_palette_subsampled_big_first",
    "jp2_palette_subsampled_small_first",
    // Palette and cdef.
    "jp2_cdef_palette",
    // cdef only.
    "jp2_cdef_alpha_first",
    "jp2_cdef_reversed",
    "jp2_cdef_rgba",
    "jp2_cdef_unspecified",
];

fn read(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.jp2"));
    std::fs::read(path).unwrap()
}

/// One rebuilt channel: its grid size, samples, label, and depth and sign.
type Rebuilt = ((u32, u32), Vec<i32>, ChannelDefinition, (u8, bool));

/// A channel after step 1: its grid size, samples, and depth and sign.
type Expanded = ((u32, u32), Vec<i32>, (u8, bool));

/// Step 1 and 2 of the `raw_channels` recipe.
fn rebuild(raw: &rust_j2k::Image) -> Vec<Rebuilt> {
    let meta: &Jp2Metadata = raw.jp2.as_ref().unwrap();

    // 1. Expand the index planes.
    let mut channels: Vec<Expanded> = Vec::new();
    match &meta.channel_mapping {
        None => {
            assert!(meta.palette.is_none());
            for c in &raw.components {
                channels.push((
                    (c.width, c.height),
                    c.samples.clone(),
                    (c.bit_depth, c.signed),
                ));
            }
        }
        Some(mapping) => {
            let palette = meta
                .palette
                .as_ref()
                .expect("a mapping comes with a palette");
            for (i, entry) in mapping.iter().enumerate() {
                let source = &raw.components[usize::from(entry.component)];
                // A palette channel takes its column's depth and sign, and a
                // direct channel those of the column at its own position.
                let format_column = match entry.kind {
                    MappingKind::Palette { column } => usize::from(column),
                    _ => i,
                };
                let format = &palette.columns()[format_column];
                let samples = match entry.kind {
                    MappingKind::Direct => source.samples.clone(),
                    MappingKind::Palette { column } => source
                        .samples
                        .iter()
                        .map(|&index| {
                            let at = usize::try_from(index.max(0))
                                .unwrap()
                                .min(palette.entry_count() - 1);
                            palette.entry(at).unwrap()[usize::from(column)]
                        })
                        .collect(),
                    _ => panic!("an unexpected mapping kind"),
                };
                channels.push((
                    (source.width, source.height),
                    samples,
                    (format.bit_depth, format.signed),
                ));
            }
        }
    }

    // 2. Order and label them with the channel definitions.
    let count = channels.len();
    let mut order: Vec<usize> = (0..count).collect();
    let mut labels = vec![
        ChannelDefinition {
            kind: ChannelType::Color,
            association: Association::None,
        };
        count
    ];
    if let Some(entries) = &meta.channel_definitions {
        for entry in entries {
            let channel = usize::from(entry.channel);
            labels[channel] = entry.definition;
            if let (ChannelType::Color, Association::Color(n)) =
                (entry.definition.kind, entry.definition.association)
            {
                let target = usize::from(n) - 1;
                let from = order.iter().position(|&c| c == channel).unwrap();
                order.swap(from, target);
            }
        }
    }
    order
        .iter()
        .map(|&c| {
            (
                channels[c].0,
                channels[c].1.clone(),
                labels[c],
                channels[c].2,
            )
        })
        .collect()
}

#[test]
fn the_raw_metadata_rebuilds_the_default_decode_in_order() {
    for name in FIXTURES {
        let bytes = read(name);
        let image = decode(&bytes).unwrap();
        let raw = decode_with(&bytes, &DecodeOptions::default().with_raw_channels(true)).unwrap();

        let rebuilt = rebuild(&raw);
        assert_eq!(rebuilt.len(), image.components.len(), "{name}");
        for (i, (size, samples, _, format)) in rebuilt.iter().enumerate() {
            let want = &image.components[i];
            assert_eq!(*size, (want.width, want.height), "{name} channel {i}");
            assert_eq!(samples, &want.samples, "{name} channel {i}");
            assert_eq!(*format, (want.bit_depth, want.signed), "{name} channel {i}");
        }
        // The labels are the default decode's too, for the files with a cdef.
        if raw.jp2.as_ref().unwrap().channel_definitions.is_some() {
            let want = &image.jp2.as_ref().unwrap().channels;
            let got: Vec<ChannelDefinition> = rebuilt.iter().map(|r| r.2).collect();
            assert_eq!(&got, want, "{name}: labels");
        }
    }
}

/// `jp2_palette_mixed` with its direct channel moved to position 1 and the
/// palette's column 1 made signed. Every fixture's direct channel shares its
/// format with column 0 and with its component, so only a file like this one
/// tells the three apart. `opj_decompress` 2.5.0 writes channel 1 of this
/// file as signed 8-bit (`PG ML - 8`), column 1's format.
fn direct_channel_mid_palette() -> Vec<u8> {
    let mut bytes = read("jp2_palette_mixed");
    // The `pclr` payload: NE = 256, NPC = 3, then B0..B2 (8-bit unsigned).
    assert_eq!(bytes[81..85], *b"pclr");
    assert_eq!(bytes[85..91], [0x01, 0x00, 0x03, 0x07, 0x07, 0x07]);
    bytes[89] = 0x87; // column 1: 8-bit signed
    // The `cmap` payload: channel 0 from column 0 of component 0, channel 1
    // direct from component 2, channel 2 from column 2 of component 0.
    assert_eq!(bytes[863..867], *b"cmap");
    bytes[867..879].copy_from_slice(&[0, 0, 1, 0, 0, 2, 0, 0, 0, 0, 1, 2]);
    bytes
}

#[test]
fn a_direct_channel_takes_the_format_of_the_column_at_its_position() {
    let bytes = direct_channel_mid_palette();
    let image = decode(&bytes).unwrap();
    let raw = decode_with(&bytes, &DecodeOptions::default().with_raw_channels(true)).unwrap();
    let formats: Vec<(u8, bool)> = image
        .components
        .iter()
        .map(|c| (c.bit_depth, c.signed))
        .collect();
    assert_eq!(formats, [(8, false), (8, true), (8, false)]);
    assert_eq!(
        (raw.components[2].bit_depth, raw.components[2].signed),
        (8, false),
        "the direct channel's component is unsigned"
    );
    assert_eq!(image.components[1].samples, raw.components[2].samples);

    let rebuilt = rebuild(&raw);
    assert_eq!(rebuilt.len(), image.components.len());
    for (i, (size, samples, _, format)) in rebuilt.iter().enumerate() {
        let want = &image.components[i];
        assert_eq!(*size, (want.width, want.height), "channel {i}");
        assert_eq!(samples, &want.samples, "channel {i}");
        assert_eq!(*format, (want.bit_depth, want.signed), "channel {i}");
    }
}

#[test]
fn the_mapping_and_the_palette_are_the_same_in_both_modes() {
    for name in FIXTURES {
        let bytes = read(name);
        let image = decode(&bytes).unwrap();
        let raw = decode_with(&bytes, &DecodeOptions::default().with_raw_channels(true)).unwrap();
        let (applied, skipped) = (image.jp2.unwrap(), raw.jp2.unwrap());
        assert_eq!(applied.palette, skipped.palette, "{name}");
        assert_eq!(applied.channel_mapping, skipped.channel_mapping, "{name}");
        assert_eq!(
            applied.channel_definitions, skipped.channel_definitions,
            "{name}"
        );
        assert_eq!(
            applied.palette_applied,
            applied.palette.is_some(),
            "{name}: the palette applied"
        );
        assert!(!skipped.palette_applied, "{name}");
    }
}

/// The correction for a single-component file is what the metadata reports:
/// every entry looks up its own column.
#[test]
fn the_corrected_mapping_is_the_one_reported() {
    let image = decode(&read("jp2_palette_corrected")).unwrap();
    let meta = image.jp2.unwrap();
    let mapping = meta.channel_mapping.unwrap();
    for (i, entry) in mapping.iter().enumerate() {
        assert_eq!(
            entry.kind,
            MappingKind::Palette {
                column: u8::try_from(i).unwrap()
            },
            "channel {i}"
        );
    }
}

#[test]
fn a_file_without_a_palette_has_no_palette_or_mapping() {
    for name in ["jp2_cdef_alpha_first", "jp2_cdef_reversed", "jp2_cdef_rgba"] {
        let meta = decode(&read(name)).unwrap().jp2.unwrap();
        assert!(
            meta.palette.is_none() && meta.channel_mapping.is_none(),
            "{name}"
        );
        assert!(meta.channel_definitions.is_some(), "{name}");
    }
}

#[test]
fn a_palette_entry_past_the_end_is_none_without_overflow() {
    let raw = decode_with(
        &read("jp2_palette_mixed"),
        &DecodeOptions::default().with_raw_channels(true),
    )
    .unwrap();
    let palette = raw.jp2.as_ref().unwrap().palette.as_ref().unwrap();
    let count = palette.entry_count();
    let width = palette.columns().len();
    assert!(width > 1, "the wrap case needs more than one column");

    assert!(palette.entry(count - 1).is_some());
    assert_eq!(palette.entry(count), None);
    assert_eq!(palette.entry(usize::MAX), None);
    assert_eq!(palette.entry(usize::MAX / width + 1), None);
    assert_eq!(palette.entry(usize::MAX / width), None);
}
