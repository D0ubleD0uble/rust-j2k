//! `Jp2Metadata::palette` and `channel_mapping` are enough to rebuild a
//! palettised decode from the raw index planes: expand each raw channel
//! through the palette as the mapping says, and the result is the channels of
//! the default decode.

use rust_j2k::{DecodeOptions, MappingKind, decode, decode_with};

const PALETTE_FIXTURES: [&str; 8] = [
    "jp2_palette_8bit",
    "jp2_palette_12bit",
    "jp2_palette_clamp",
    "jp2_palette_corrected",
    "jp2_palette_mixed",
    "jp2_palette_subsampled_big_first",
    "jp2_palette_subsampled_small_first",
    "jp2_cdef_palette",
];

fn read(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.jp2"));
    std::fs::read(path).unwrap()
}

#[test]
fn expanding_the_raw_planes_gives_the_default_channels() {
    for name in PALETTE_FIXTURES {
        let bytes = read(name);
        let image = decode(&bytes).unwrap();
        let raw = decode_with(&bytes, &DecodeOptions::default().with_raw_channels(true)).unwrap();
        let applied = image.jp2.as_ref().unwrap();
        let skipped = raw.jp2.as_ref().unwrap();
        assert!(
            applied.palette_applied && !skipped.palette_applied,
            "{name}"
        );

        // The palette is the same either way.
        let palette = applied.palette.as_ref().expect(name);
        assert_eq!(applied.palette, skipped.palette, "{name}");
        assert!(palette.entry_count() >= 1 && palette.entry(palette.entry_count()).is_none());
        for j in 0..palette.entry_count() {
            assert_eq!(palette.entry(j).unwrap().len(), palette.columns().len());
        }

        let mapping = applied.channel_mapping.as_ref().expect(name);
        assert_eq!(mapping.len(), palette.columns().len(), "{name}");
        assert_eq!(mapping.len(), image.components.len(), "{name}");

        // Only the single-component correction changes the mapping.
        let as_boxed = skipped.channel_mapping.as_ref().expect(name);
        if name == "jp2_palette_corrected" {
            assert_ne!(mapping, as_boxed, "{name}: the correction is reported");
        } else {
            assert_eq!(mapping, as_boxed, "{name}");
        }

        // Rebuild each channel from the raw planes and the mapping.
        let mut rebuilt = Vec::new();
        for entry in mapping {
            let source = &raw.components[usize::from(entry.component)];
            let samples: Vec<i32> = match entry.kind {
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
                _ => panic!("{name}: an unexpected mapping kind"),
            };
            rebuilt.push(samples);
        }
        // A `cdef` box reorders the output channels, so match as a set.
        let mut remaining: Vec<&Vec<i32>> = image.components.iter().map(|c| &c.samples).collect();
        for samples in &rebuilt {
            let at = remaining
                .iter()
                .position(|d| *d == samples)
                .unwrap_or_else(|| panic!("{name}: a rebuilt channel matches no decoded one"));
            remaining.swap_remove(at);
        }
    }
}

#[test]
fn a_file_without_a_palette_has_neither_field() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut checked = 0;
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.starts_with("jp2_cdef_") || !name.ends_with(".jp2") || name.contains("palette") {
            continue;
        }
        let image = decode(&std::fs::read(&path).unwrap()).unwrap();
        let jp2 = image.jp2.unwrap();
        assert!(
            jp2.palette.is_none() && jp2.channel_mapping.is_none(),
            "{name}"
        );
        checked += 1;
    }
    assert!(checked > 0, "no jp2_cdef_ fixtures found");
}
