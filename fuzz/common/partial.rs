//! The differential `decode_partial` and `tile_body` run on a decode with
//! `strict` off.

use rust_j2k::{DecodeOptions, Image, ReportEntry, Result};

/// Decode `bytes` with `strict` off under `options` (which must be strict),
/// given `strict`, the strict decode of the same bytes under `options`, and
/// check what the two must agree on:
///
/// - when the strict decode gives an image, the decode with `strict` off
///   gives the same image with the complete report;
/// - a strict image always has the complete report;
/// - the report holds at most five entries per tile-part plus two, and every
///   tile-part starts with the bytes `FF 90`, so their count bounds it.
pub fn check(bytes: &[u8], options: &DecodeOptions, strict: &Result<Image>) {
    let lenient = rust_j2k::decode_with(bytes, &options.clone().with_strict(false));
    if let Ok(image) = strict {
        assert_eq!(image.report, rust_j2k::DecodeReport::default());
        let partial = lenient.as_ref().expect("strict decodes, so partial does");
        assert_eq!(
            (image.width, image.height, &image.components, &image.jp2),
            (partial.width, partial.height, &partial.components, &partial.jp2),
            "the same image"
        );
        assert!(partial.report.entries.is_empty(), "{:?}", partial.report);
        assert_eq!(partial.report.complete_reduction, Some(0));
        assert!(partial.report.is_complete());
    }
    if let Ok(image) = &lenient {
        let report = &image.report;
        let sots = bytes.windows(2).filter(|w| *w == [0xFF, 0x90]).count();
        assert!(
            report.entries.len() <= 5 * sots + 2,
            "{} entries for at most {sots} tile-parts",
            report.entries.len()
        );
        // A tile with bytes left over is complete at no reduction.
        let leftover = report
            .entries
            .iter()
            .any(|e| matches!(e, ReportEntry::LeftoverBytes { .. }));
        if leftover {
            assert_eq!(report.complete_reduction, None);
        }
        if report.is_complete() {
            assert!(!report
                .entries
                .iter()
                .any(|e| matches!(e, ReportEntry::MissingTiles { .. })));
        }
    }
}
