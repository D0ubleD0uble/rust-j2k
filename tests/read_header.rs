//! `read_header` against `decode`, over every committed codestream and JP2
//! file: the Part 4 JP2 reader files, the GDAL `jp2_real` samples, the Part 1
//! conformance codestreams, and the synthetic fixtures.
//!
//! `read_header` runs the same header checks `decode` runs before it reads
//! any tile data, so the two agree on every header (dimensions, each output
//! channel's depth, sign and sub-sampling, and the JP2 metadata), and on every
//! error found in a header. They differ only where `decode` meets a fault in
//! the tile data, which `read_header` never reads; such a file must be listed
//! in [`TILE_FAULTS`].

use std::path::{Path, PathBuf};

use rust_j2k::{ColourSpace, Error, Header, Image, Jp2Metadata, Ratio, Res, read_header};

/// Files where `decode` may fail on tile data while `read_header` succeeds,
/// and why. Today none of them differs: `rgb16_ecwsdk.jp2` is refused for its
/// `cdef` box by both, because that guard is a header check. Once `cdef` is
/// applied, `decode` fails on its tile-part ("not terminated by EOC"; OpenJPEG
/// fails there too) while its header reads cleanly.
///
/// The other expected difference has no committed file: a tile-part COD that
/// gives a component another wavelet than the main header is refused by
/// `decode` only.
const TILE_FAULTS: &[(&str, &str)] = &[(
    "jp2_real/rgb16_ecwsdk.jp2",
    "its codestream has no EOC; refused for `cdef` until cdef is applied",
)];

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Every `.j2k` and `.jp2` under `dir`, recursively, sorted.
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read fixtures").flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(&path, out);
        } else if path.extension().is_some_and(|e| e == "j2k" || e == "jp2") {
            out.push(path);
        }
    }
}

/// What `read_header` and `decode` must agree on: the image area, each
/// channel's depth, sign and sub-sampling, and the JP2 metadata.
type Shape<'a> = (u32, u32, Vec<(u8, bool, u8, u8)>, Option<&'a Jp2Metadata>);

fn shape_of_image(image: &Image) -> Shape<'_> {
    let channels = image
        .components
        .iter()
        .map(|c| (c.bit_depth, c.signed, c.x_sampling, c.y_sampling))
        .collect();
    (image.width, image.height, channels, image.jp2.as_ref())
}

fn shape_of_header(header: &Header) -> Shape<'_> {
    let channels = header
        .components
        .iter()
        .map(|c| (c.bit_depth, c.signed, c.x_sampling, c.y_sampling))
        .collect();
    (header.width, header.height, channels, header.jp2.as_ref())
}

#[test]
fn read_header_agrees_with_decode_on_every_committed_file() {
    let root = fixtures();
    let mut all = Vec::new();
    files(&root, &mut all);
    all.sort();
    let mut seen = [0usize; 4];
    for path in &all {
        let name = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for (i, prefix) in ["conformance/jp2/", "jp2_real/", "conformance/codestreams/"]
            .iter()
            .enumerate()
        {
            if name.starts_with(prefix) {
                seen[i] += 1;
            }
        }
        seen[3] += 1;
        let bytes = std::fs::read(path).unwrap();
        let header = read_header(&bytes);
        let decoded = rust_j2k::decode(&bytes);
        match (&header, &decoded) {
            (Ok(header), Ok(image)) => {
                assert_eq!(shape_of_header(header), shape_of_image(image), "{name}");
            }
            (Err(h), Err(d)) => assert_eq!(h, d, "{name}: the same error"),
            (Ok(_), Err(Error::Codestream(_))) => assert!(
                TILE_FAULTS.iter().any(|(listed, _)| *listed == name),
                "{name}: decode failed on tile data but read_header succeeded: {decoded:?}"
            ),
            _ => panic!("{name}: read_header {header:?} but decode {decoded:?}"),
        }
    }
    assert_eq!(seen[0], 9, "the nine Part 4 JP2 files");
    assert_eq!(seen[1], 9, "the nine jp2_real files");
    assert_eq!(seen[2], 23, "the 23 Part 1 codestreams");
}

fn part4(n: u32) -> Vec<u8> {
    std::fs::read(fixtures().join(format!("conformance/jp2/files/file{n}.jp2"))).unwrap()
}

fn real(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join("jp2_real").join(name)).unwrap()
}

/// file1 has an `xml ` box before `jp2h` and one between it and `jp2c`.
/// file8 has one before `jp2c` and one after it. Sizes from a box dump.
#[test]
fn the_part4_files_report_their_xml_boxes() {
    let xml_lengths = |n| {
        let jp2 = read_header(&part4(n)).unwrap().jp2.unwrap();
        assert!(jp2.uuid.is_empty(), "file{n}");
        jp2.xml.iter().map(Vec::len).collect::<Vec<_>>()
    };
    assert_eq!(xml_lengths(1), [431, 741]);
    assert_eq!(xml_lengths(8), [377, 902]);
    let file8 = part4(8);
    let after = read_header(&file8).unwrap().jp2.unwrap().xml[1].clone();
    assert!(
        file8.ends_with(&after),
        "file8's second xml is after the codestream"
    );
}

/// Files 5 and 7 are JP2-compatible JPX files (`jpx ` brand, `jp2 ` in the
/// compatibility list, an `rreq` box). The `rreq` is skipped, and the
/// metadata is read: the first `colr` (a restricted ICC profile of 546 and
/// 13332 bytes, which is what each profile's own size field says) and three
/// colour channels.
#[test]
fn the_jpx_compatible_files_report_their_metadata() {
    for (n, profile) in [(5, 546), (7, 13332)] {
        let bytes = part4(n);
        let header = read_header(&bytes).unwrap_or_else(|e| panic!("file{n}: {e}"));
        let jp2 = header.jp2.clone().unwrap();
        match &jp2.colour {
            ColourSpace::RestrictedIcc(icc) => {
                assert_eq!(icc.len(), profile, "file{n}");
                assert_eq!(
                    u32::from_be_bytes(icc[..4].try_into().unwrap()) as usize,
                    profile,
                    "file{n}: the profile's size field"
                );
            }
            other => panic!("file{n}: {other:?}"),
        }
        assert_eq!(jp2.channels.len(), 3, "file{n}");
        assert!(jp2.xml.is_empty() && jp2.uuid.is_empty(), "file{n}");
        assert_eq!(rust_j2k::decode(&bytes).unwrap().jp2, Some(jp2), "file{n}");
    }
}

/// The GDAL samples' resolution boxes, as `opj_dump`-independent box dumps
/// read them: `resc 10000/254 e0` and `resd 12901/32768 e4`.
#[test]
fn the_gdal_samples_report_their_resolution() {
    let ratio = |numerator, denominator, exponent| Ratio {
        numerator,
        denominator,
        exponent,
    };
    let tlm = read_header(&real("byte_tlm_plt.jp2")).unwrap().jp2.unwrap();
    let dpi = ratio(10000, 254, 0);
    assert_eq!(
        tlm.resolution.capture,
        Some(Res {
            vertical: dpi,
            horizontal: dpi,
        })
    );
    assert_eq!(tlm.resolution.display, None);
    let per_inch = dpi.grid_points_per_metre().unwrap() * 0.0254;
    assert!(
        (per_inch - 1.0).abs() < 1e-12,
        "one point per inch: {per_inch}"
    );

    let kakadu = read_header(&real("rgbwcmyk01_YeGeo_kakadu.jp2"))
        .unwrap()
        .jp2
        .unwrap();
    let display = ratio(12901, 32768, 4);
    assert_eq!(kakadu.resolution.capture, None);
    assert_eq!(
        kakadu.resolution.display,
        Some(Res {
            vertical: display,
            horizontal: display,
        })
    );
}

/// A component too deep for the `i32` sample container is a header fault:
/// `read_header` refuses it as `decode` does, and before any tile data, so a
/// broken tile-part does not change the error (OpenJPEG refuses a depth over
/// 31 bits when it reads SIZ).
#[test]
fn a_component_too_deep_for_i32_is_refused_from_the_header() {
    let mut bytes = std::fs::read(fixtures().join("regular_latlon_lossy.j2k")).unwrap();
    // SIZ: Csiz at 40, then the first component's Ssiz (depth - 1, sign bit).
    assert_eq!(&bytes[40..42], &[0, 1]);
    bytes[42] = 31; // unsigned, 32 bits
    let refused = |bytes: &[u8]| match read_header(bytes) {
        Err(Error::Unsupported(message)) => {
            assert!(message.contains("exceeds the i32"), "{message}");
            assert_eq!(rust_j2k::decode(bytes), Err(Error::Unsupported(message)));
        }
        other => panic!("expected Unsupported, got {other:?}"),
    };
    refused(&bytes);
    let cut = bytes.len() - 10;
    refused(&bytes[..cut]);
}
