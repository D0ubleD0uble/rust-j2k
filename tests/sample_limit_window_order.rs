//! An invalid window or tile is reported as `InvalidOptions` even when the
//! image is over the sample limit: a `Limit` there would carry a hint to
//! raise the limit that cannot help. A valid window of the same image is
//! still held to the tiles it reads.

#[path = "support/synth.rs"]
mod synth;
use rust_j2k::{DecodeOptions, Error, Window, decode_with, read_header_with};
use synth::seg;

/// The main header and first `SOT` marker of a 40000x40000 image with its
/// origin at (100, 200), in 17000x17000 tiles, of two 8-bit components:
/// component 0 at full size, component 1 sub-sampled 2x3. The image area is
/// 39900x39800, over the default sample limit.
fn large_header_only() -> Vec<u8> {
    header_only(40000)
}

/// [`large_header_only`] for a `side` x `side` image.
fn header_only(side: u32) -> Vec<u8> {
    let siz = synth::siz_body(
        [side, side, 100, 200, 17000, 17000, 0, 0],
        &[[7, 1, 1], [7, 2, 3]],
    );
    let cod = synth::cod_body(1, 6, 0, 0);
    let qcd = synth::qcd_none_body(2, &[8; 19]);
    [
        vec![0xFF, 0x4F],
        seg(0xFF51, &siz),
        seg(0xFF52, &cod),
        seg(0xFF5C, &qcd),
        vec![0xFF, 0x90],
    ]
    .concat()
}

/// `codestream` in a minimal JP2 file whose `ihdr` matches the image area.
fn in_jp2(codestream: &[u8]) -> Vec<u8> {
    let ihdr = synth::bx(b"ihdr", &synth::ihdr_payload(39800, 39900, 2, [7, 7, 0, 0]));
    let colr = synth::bx(b"colr", &synth::colr_enum(17));
    synth::jp2_file(&[ihdr, colr], &[], codestream, &[])
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

/// Like `in_jp2`, with a palette that expands component 0 to three 8-bit
/// channels beside component 1, so the expanded image is over the limit.
fn in_paletted_jp2(codestream: &[u8]) -> Vec<u8> {
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
    let pclr = [0, 1, 3, 7, 7, 7, 10, 20, 30];
    let cmap = [0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 2, 0, 1, 0, 0];
    let jp2h = [
        bx(b"ihdr", &ihdr),
        bx(b"colr", &[1, 0, 0, 0, 0, 0, 16]),
        bx(b"pclr", &pclr),
        bx(b"cmap", &cmap),
    ]
    .concat();
    [
        vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A],
        bx(b"ftyp", b"jp2 \0\0\0\0jp2 "),
        bx(b"jp2h", &jp2h),
        bx(b"jp2c", codestream),
    ]
    .concat()
}

#[test]
fn a_paletted_jp2_file_reports_an_invalid_window_before_the_limit() {
    let bytes = in_paletted_jp2(&large_header_only());
    assert_invalid_options(read_header_with(&bytes, &empty_for_component_1()));
    assert_invalid_options(decode_with(&bytes, &empty_for_component_1()));
    let srgb = empty_for_component_1().with_color(rust_j2k::ColorOutput::Srgb);
    assert_invalid_options(read_header_with(&bytes, &srgb));
    assert_invalid_options(decode_with(&bytes, &srgb));
}

/// A JP2 file of the `header_only(side)` codestream with `boxes` in `jp2h`
/// after `ihdr` and `colr`.
fn jp2_with(side: u32, boxes: &[Vec<u8>]) -> Vec<u8> {
    let mut ihdr = (side - 200).to_be_bytes().to_vec();
    ihdr.extend_from_slice(&(side - 100).to_be_bytes());
    ihdr.extend_from_slice(&2u16.to_be_bytes());
    ihdr.extend_from_slice(&[7, 7, 0, 0]);
    let jp2h = [
        bx(b"ihdr", &ihdr),
        bx(b"colr", &[1, 0, 0, 0, 0, 0, 17]),
        boxes.concat(),
    ]
    .concat();
    [
        vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A],
        bx(b"ftyp", b"jp2 \0\0\0\0jp2 "),
        bx(b"jp2h", &jp2h),
        bx(b"jp2c", &header_only(side)),
    ]
    .concat()
}

fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = (8 + payload.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

fn assert_file_format<T: std::fmt::Debug>(result: Result<T, Error>) {
    match result {
        Err(Error::FileFormat(_)) => {}
        other => panic!("expected Error::FileFormat, got {other:?}"),
    }
}

/// Two `cdef` boxes in `jp2h`, a fault of the file's own.
fn two_cdefs() -> Vec<Vec<u8>> {
    let cdef = bx(b"cdef", &[0, 2, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 2]);
    vec![cdef.clone(), cdef]
}

/// The file's own faults come ahead of a refused selection, and the order
/// does not depend on the image's size.
#[test]
fn a_fault_of_the_file_comes_before_a_refused_window_on_any_size() {
    for side in [1000, 40000] {
        let bytes = jp2_with(side, &two_cdefs());
        assert_file_format(read_header_with(&bytes, &empty_for_component_1()));
        assert_file_format(decode_with(&bytes, &empty_for_component_1()));
    }
}

/// The same with a palette, whose expansion is over the limit on the big
/// image: the palette's count must not hide the fault.
#[test]
fn a_fault_of_a_paletted_file_comes_before_a_refused_window_on_any_size() {
    let pclr = bx(b"pclr", &[0, 1, 3, 7, 7, 7, 10, 20, 30]);
    let cmap = bx(b"cmap", &[0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 2, 0, 1, 0, 0]);
    for side in [1000, 40000] {
        let mut boxes = vec![pclr.clone(), cmap.clone()];
        boxes.extend(two_cdefs());
        let bytes = jp2_with(side, &boxes);
        assert_file_format(read_header_with(&bytes, &empty_for_component_1()));
        assert_file_format(decode_with(&bytes, &empty_for_component_1()));
    }
}

/// A tile or a component that does not exist is as refused as a window.
#[test]
fn a_missing_tile_or_component_is_invalid_options_on_a_large_image() {
    let codestream = large_header_only();
    let jp2 = in_jp2(&codestream);
    let tile = DecodeOptions::default().with_tile(Some(99));
    let component = DecodeOptions::default().with_components(Some(vec![5]));
    for bytes in [&codestream, &jp2] {
        for options in [&tile, &component] {
            assert_invalid_options(read_header_with(bytes, options));
            assert_invalid_options(decode_with(bytes, options));
        }
    }
}
