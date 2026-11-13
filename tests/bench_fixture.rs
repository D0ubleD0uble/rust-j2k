//! The GRIB2-shaped benchmark fixture decodes to what OpenJPEG decodes.
//!
//! `tests/fixtures/bench/grib2_shape_1440x721_u16.j2k` is a synthetic 1440x721
//! 16-bit field encoded by `opj_compress` in the shape of a GRIB2 template 5.40
//! message (`scripts/gen-grib2-shape-bench-fixture.py`). The expected file holds
//! an FNV-1a digest of `opj_decompress`'s samples, so the fixture stays small.

use std::path::PathBuf;

/// FNV-1a over the samples as little-endian `u16`, in raster order. The
/// generator script computes the same digest.
fn fnv1a64(samples: &[i32]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &sample in samples {
        let value = u16::try_from(sample).expect("an unsigned 16-bit sample");
        for byte in value.to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

#[test]
fn grib2_shape_bench_fixture_matches_opj_decompress() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bench");
    // `tests/fixtures/bench/` is left out of the published package.
    let Ok(bytes) = std::fs::read(dir.join("grib2_shape_1440x721_u16.j2k")) else {
        eprintln!("bench_fixture: fixture not present (packaged crate?); skipping");
        return;
    };
    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("grib2_shape_1440x721_u16.expected.json")).unwrap(),
    )
    .unwrap();
    let image = rust_j2k::decode(&bytes).expect("the bench fixture decodes");
    assert_eq!(image.components.len(), 1);
    let component = &image.components[0];
    assert_eq!(
        (component.width, component.height, component.bit_depth),
        (1440, 721, 16)
    );
    assert_eq!(
        fnv1a64(&component.samples),
        expected["fnv1a64"].as_str().unwrap()
    );
}
