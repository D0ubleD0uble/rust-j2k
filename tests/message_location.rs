//! Error messages name the tile and tile-part they come from. Each test
//! damages a late tile of `tiles_multipart_lossless.j2k` (3x3 tiles, three
//! tile-parts each), so a message that named the wrong index, or always
//! named tile 0, would fail.

use std::path::PathBuf;

use rust_j2k::{Error, decode};

/// Offset of tile 8's last `SOT` marker; its `Psot` is 167 and the file ends
/// with `EOC` right after it, 169 bytes from the marker.
const TILE_8_PART_2: usize = 5854;
/// Offset of tile 4's last `SOT` marker; its `Psot` is 618.
const TILE_4_PART_2: usize = 3522;

fn fixture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tiles_multipart_lossless.j2k");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Read the `Psot` of the `SOT` at `sot`, checking the tile index and part.
fn psot(data: &[u8], sot: usize, tile: u16, part: u8) -> u32 {
    assert_eq!(data[sot..sot + 2], [0xFF, 0x90], "no SOT at {sot}");
    assert_eq!(u16::from_be_bytes([data[sot + 4], data[sot + 5]]), tile);
    assert_eq!(data[sot + 10], part);
    u32::from_be_bytes(data[sot + 6..sot + 10].try_into().unwrap())
}

fn set_psot(data: &mut [u8], sot: usize, value: u32) {
    data[sot + 6..sot + 10].copy_from_slice(&value.to_be_bytes());
}

fn codestream_error(data: &[u8]) -> String {
    match decode(data) {
        Err(Error::Codestream(m)) => m,
        other => panic!("expected Error::Codestream, got {other:?}"),
    }
}

#[test]
fn a_psot_past_the_end_names_its_tile_and_part() {
    let mut data = fixture();
    assert_eq!(psot(&data, TILE_8_PART_2, 8, 2), 167);
    set_psot(&mut data, TILE_8_PART_2, 1000);
    assert_eq!(
        codestream_error(&data),
        "tile 8 tile-part 2 declares `Psot` 1000, but only 169 bytes are left in the codestream"
    );
}

#[test]
fn a_psot_shorter_than_its_header_names_its_tile_and_part() {
    let mut data = fixture();
    assert_eq!(psot(&data, TILE_8_PART_2, 8, 2), 167);
    set_psot(&mut data, TILE_8_PART_2, 5);
    let message = codestream_error(&data);
    assert!(
        message.starts_with("tile 8 tile-part 2 declares `Psot` 5, shorter than its "),
        "{message}"
    );
}

#[test]
fn bytes_left_after_the_last_packet_name_their_tile() {
    let mut data = fixture();
    let stated = psot(&data, TILE_4_PART_2, 4, 2);
    assert_eq!(stated, 618);
    let end = TILE_4_PART_2 + stated as usize;
    data.splice(end..end, [0u8; 3]);
    set_psot(&mut data, TILE_4_PART_2, stated + 3);
    assert_eq!(
        codestream_error(&data),
        "tile 4 has 3 byte(s) left after the last packet"
    );
}
