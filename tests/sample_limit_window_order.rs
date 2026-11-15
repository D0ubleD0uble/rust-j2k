//! An invalid window or tile is reported as `InvalidOptions` even when the
//! image is over the sample limit: a `Limit` there would carry a hint to
//! raise the limit that cannot help. A valid window of the same image is
//! still held to the tiles it reads.

use rust_j2k::{DecodeOptions, Error, Window, decode_with, read_header_with};

fn segment(marker: u16, body: &[u8]) -> Vec<u8> {
    let mut out = marker.to_be_bytes().to_vec();
    out.extend_from_slice(&(body.len() as u16 + 2).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// The main header and first `SOT` marker of a 40000x40000 image with its
/// origin at (100, 200), in 17000x17000 tiles, of two 8-bit components:
/// component 0 at full size, component 1 sub-sampled 2x3. The image area is
/// 39900x39800, over the default sample limit.
fn large_header_only() -> Vec<u8> {
    let mut siz = vec![0, 0];
    for field in [40000u32, 40000, 100, 200, 17000, 17000, 0, 0] {
        siz.extend_from_slice(&field.to_be_bytes());
    }
    siz.extend_from_slice(&2u16.to_be_bytes());
    siz.extend_from_slice(&[7, 1, 1, 7, 2, 3]);
    let mut cod = vec![0, 0];
    cod.extend_from_slice(&1u16.to_be_bytes());
    cod.extend_from_slice(&[0, 6, 0, 0, 0, 1]);
    let qcd = [&[0x40][..], &[8 << 3; 19]].concat();
    [
        vec![0xFF, 0x4F],
        segment(0xFF51, &siz),
        segment(0xFF52, &cod),
        segment(0xFF5C, &qcd),
        vec![0xFF, 0x90],
    ]
    .concat()
}

/// `codestream` in a minimal JP2 file whose `ihdr` matches the image area.
fn in_jp2(codestream: &[u8]) -> Vec<u8> {
    let bx = |kind: &[u8; 4], payload: &[u8]| {
        let mut out = (8 + payload.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    };
    let mut ihdr = 39800u32.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&39900u32.to_be_bytes());
    ihdr.extend_from_slice(&2u16.to_be_bytes());
    ihdr.extend_from_slice(&[7, 7, 0, 0]);
    let jp2h = [bx(b"ihdr", &ihdr), bx(b"colr", &[1, 0, 0, 0, 0, 0, 17])].concat();
    [
        vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A],
        bx(b"ftyp", b"jp2 \0\0\0\0jp2 "),
        bx(b"jp2h", &jp2h),
        bx(b"jp2c", codestream),
    ]
    .concat()
}

fn window(x0: u32, y0: u32, x1: u32, y1: u32) -> DecodeOptions {
    DecodeOptions::default().with_window(Some(Window::new(x0, y0, x1, y1)))
}

/// Holds no sample of component 1, which is sub-sampled 2x3.
fn empty_for_component_1() -> DecodeOptions {
    window(16999, 16999, 17001, 17001)
}

fn assert_invalid_options<T: std::fmt::Debug>(result: Result<T, Error>) {
    match result {
        Err(Error::InvalidOptions(_)) => {}
        other => panic!("expected Error::InvalidOptions, got {other:?}"),
    }
}

#[test]
fn read_header_with_reports_an_invalid_window_before_the_limit() {
    assert_invalid_options(read_header_with(
        &large_header_only(),
        &empty_for_component_1(),
    ));
}

#[test]
fn decode_with_reports_an_invalid_window_before_the_limit() {
    assert_invalid_options(decode_with(&large_header_only(), &empty_for_component_1()));
}

#[test]
fn a_jp2_file_reports_an_invalid_window_before_the_limit() {
    let bytes = in_jp2(&large_header_only());
    assert_invalid_options(read_header_with(&bytes, &empty_for_component_1()));
    assert_invalid_options(decode_with(&bytes, &empty_for_component_1()));
}

#[test]
fn a_valid_window_of_the_same_image_is_still_held_to_the_tiles_read() {
    let codestream = large_header_only();
    let jp2 = in_jp2(&codestream);
    // Inside the first tile, which is 16900x16800: over the limit by itself.
    let options = window(200, 300, 300, 400);
    for bytes in [&codestream, &jp2] {
        for result in [
            read_header_with(bytes, &options).map(drop),
            decode_with(bytes, &options).map(drop),
        ] {
            match result {
                Err(Error::Limit(m)) => {
                    assert!(m.contains("the area of the tiles read (16900x16800"), "{m}")
                }
                other => panic!("expected Error::Limit, got {other:?}"),
            }
        }
    }
}
