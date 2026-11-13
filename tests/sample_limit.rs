//! The sample limit, [`DecodeOptions::sample_limit`]: how many samples a
//! decoded image may hold. The default is 2^28 samples, counted at the
//! decode's resolution reduction; `WorkLimit::Raised` and
//! `WorkLimit::Unlimited` raise or lift it.
//!
//! The fixtures in `tests/fixtures/sample_limit/` are written by
//! `scripts/gen-sample-limit-fixtures.py` (see the README there).

use std::path::PathBuf;

use rust_j2k::{
    DecodeOptions, Error, WorkLimit, decode, decode_with, read_header, read_header_with,
};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sample_limit")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The flat 16385x16385 file: 268468225 samples, just over 2^28.
fn flat() -> Vec<u8> {
    fixture("flat_16385_tiled_lossless.j2k")
}

/// The flat file's main header and the first `SOT` marker, without its tile
/// data. A decode that gets past the sample limit fails on the missing data,
/// before it allocates any plane.
fn flat_header_only() -> Vec<u8> {
    let mut bytes = flat();
    let sot = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    bytes.truncate(sot + 2);
    bytes
}

/// `codestream` in a minimal JP2 file whose `ihdr` matches the flat image.
fn in_jp2(codestream: &[u8]) -> Vec<u8> {
    let bx = |kind: &[u8; 4], payload: &[u8]| {
        let mut out = (8 + payload.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    };
    let mut ihdr = 16385u32.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&16385u32.to_be_bytes());
    ihdr.extend_from_slice(&[0, 1, 7, 7, 0, 0]);
    let jp2h = [bx(b"ihdr", &ihdr), bx(b"colr", &[1, 0, 0, 0, 0, 0, 17])].concat();
    [
        vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A],
        bx(b"ftyp", b"jp2 \0\0\0\0jp2 "),
        bx(b"jp2h", &jp2h),
        bx(b"jp2c", codestream),
    ]
    .concat()
}

fn with_limit(limit: WorkLimit) -> DecodeOptions {
    DecodeOptions::default().with_sample_limit(limit)
}

/// Sample `(x, y)` of component `c` of the 6000x4000 picture, as
/// `scripts/gen-sample-limit-fixtures.py` draws it. The script checks that
/// `opj_decompress` decodes the file to exactly this picture.
fn picture(c: usize, x: i32, y: i32) -> i32 {
    [(x >> 8) * 10, (y >> 8) * 15, ((x >> 9) ^ (y >> 9)) * 30][c] & 255
}

/// A 6000x4000 RGB image is 72 M samples: refused by the old 2^26 limit,
/// decoded at the default, and bit-exact against `opj_decompress`.
#[test]
fn a_6000x4000_rgb_image_decodes_at_defaults_and_matches_openjpeg() {
    let image = decode(&fixture("rgb_6000x4000.jp2")).expect("decodes at defaults");
    assert_eq!((image.width, image.height), (6000, 4000));
    assert_eq!(image.components.len(), 3);
    for (c, component) in image.components.iter().enumerate() {
        assert_eq!((component.width, component.height), (6000, 4000));
        assert_eq!((component.bit_depth, component.signed), (8, false));
        let wrong = (0..4000)
            .flat_map(|y| (0..6000).map(move |x| (x, y)))
            .zip(&component.samples)
            .filter(|&((x, y), &s)| s != picture(c, x, y))
            .count();
        assert_eq!(wrong, 0, "plane {c}: {wrong} samples differ from OpenJPEG");
    }
}

/// Over the default limit, the decode and the header read are refused with
/// [`Error::Limit`], and the message names the option to raise and the
/// factor that would admit the file.
#[test]
fn over_the_default_limit_is_refused_and_the_message_names_the_option() {
    for (what, bytes) in [
        ("header only", flat_header_only()),
        ("JP2 header only", in_jp2(&flat_header_only())),
        ("file", flat()),
    ] {
        for result in [decode(&bytes).map(|_| ()), read_header(&bytes).map(|_| ())] {
            match result {
                Err(Error::Limit(m)) => {
                    assert!(
                        m.starts_with("image area 16385×16385 needs 268468225 samples, over the sample limit of 268435456"),
                        "{what}: {m}"
                    );
                    assert!(m.contains("`DecodeOptions::sample_limit`"), "{what}: {m}");
                    assert!(m.contains("`WorkLimit::Raised(2)`"), "{what}: {m}");
                    assert!(m.contains("`WorkLimit::Unlimited`"), "{what}: {m}");
                }
                other => panic!("{what}: {other:?}"),
            }
        }
    }
}

/// `Raised` and `Unlimited` admit the file. The header reads; the decode
/// passes the limit and fails only on the missing tile data, before any
/// plane is allocated, so the test stays cheap.
#[test]
fn raised_and_unlimited_admit_an_image_over_the_default() {
    let codestream = flat_header_only();
    let file = in_jp2(&codestream);
    for (bytes, limit) in [
        (&codestream, WorkLimit::Raised(2)),
        (&codestream, WorkLimit::Unlimited),
        (&file, WorkLimit::Raised(2)),
        (&file, WorkLimit::Unlimited),
    ] {
        let options = with_limit(limit.clone());
        let header = read_header_with(bytes, &options).unwrap_or_else(|e| panic!("{limit:?}: {e}"));
        assert_eq!((header.width, header.height), (16385, 16385), "{limit:?}");
        match decode_with(bytes, &options) {
            Err(Error::Codestream(_)) => {}
            other => panic!("{limit:?}: expected the missing tile data, got {other:?}"),
        }
    }
    // `Raised(1)` is the default limit.
    assert!(matches!(
        read_header_with(&file, &with_limit(WorkLimit::Raised(1))),
        Err(Error::Limit(_))
    ));
}

/// The limit counts the samples at the decode's reduction: three levels down
/// the flat image is 2049x2049, which fits, and it decodes as
/// `opj_decompress -r 3` does (all zero). Two levels down still fits; none
/// does not.
#[test]
fn a_reduced_decode_of_an_image_over_the_limit_is_admitted() {
    let bytes = flat();
    let reduced = DecodeOptions::default().with_resolution_reduction(3);
    let image = decode_with(&bytes, &reduced).expect("fits once reduced");
    assert_eq!((image.width, image.height), (2049, 2049));
    let [component] = &image.components[..] else {
        panic!("{} components", image.components.len());
    };
    assert_eq!(component.samples.len(), 2049 * 2049);
    assert!(component.samples.iter().all(|&s| s == 0));

    let header = read_header_with(&bytes, &reduced).expect("header fits once reduced");
    assert_eq!((header.width, header.height), (2049, 2049));
    let header_only = flat_header_only();
    let one = DecodeOptions::default().with_resolution_reduction(1);
    assert!(read_header_with(&header_only, &one).is_ok());
    assert!(matches!(
        decode_with(&header_only, &one),
        Err(Error::Codestream(_))
    ));
}

/// The reduction's own levels appear in the message when the reduced image is
/// still over the limit.
#[test]
fn a_reduced_image_still_over_the_limit_names_the_reduction() {
    // 40000 x 40000 at one level down is 20000 x 20000: 4e8 samples, which a
    // 32-bit target can still address.
    let mut bytes = flat_header_only();
    // SOC, then SIZ: marker, Lsiz, Rsiz, then Xsiz at 8, Ysiz, XOsiz, YOsiz,
    // XTsiz, YTsiz.
    assert_eq!(&bytes[2..4], &[0xFF, 0x51]);
    for at in [8, 12, 24, 28] {
        bytes[at..at + 4].copy_from_slice(&40_000u32.to_be_bytes());
    }
    let options = DecodeOptions::default().with_resolution_reduction(1);
    match read_header_with(&bytes, &options) {
        Err(Error::Limit(m)) => assert!(
            m.starts_with(
                "image area 20000×20000 at resolution reduction 1 needs 400000000 samples"
            ) && m.contains("`WorkLimit::Raised(2)`"),
            "{m}"
        ),
        other => panic!("{other:?}"),
    }
}

/// `Raised(0)` raises nothing: it is a contradiction in the options, refused
/// whatever the input.
#[test]
fn a_sample_limit_raised_by_zero_is_invalid_options() {
    let options = with_limit(WorkLimit::Raised(0));
    for bytes in [flat(), b"not a codestream".to_vec()] {
        for result in [
            decode_with(&bytes, &options).map(|_| ()),
            read_header_with(&bytes, &options).map(|_| ()),
        ] {
            match result {
                Err(Error::InvalidOptions(m)) => {
                    assert_eq!(
                        m,
                        "a sample limit raised by a factor of 0; the default is \
                         `WorkLimit::Default`"
                    );
                }
                other => panic!("{other:?}"),
            }
        }
    }
}

/// The default is `WorkLimit::Default`, and the setter sets the field.
#[test]
fn the_sample_limit_defaults_to_the_default_bound() {
    assert_eq!(DecodeOptions::default().sample_limit, WorkLimit::Default);
    assert_eq!(
        with_limit(WorkLimit::Unlimited).sample_limit,
        WorkLimit::Unlimited
    );
}
