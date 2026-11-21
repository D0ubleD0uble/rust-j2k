//! A fault in a tile's own header is found as the tile walk reads that
//! header, so with `strict` off a fault later in the input does not take
//! its place, and every prefix moves only from incomplete to `Ok` to one
//! final error. A fault found in a tile's header names the tile by its
//! index in the grid, not by its place among the tiles the input holds.
//!
//! The first two inputs are `offset_tiled_lossless.j2k` with a tile-part
//! `QCD` of one step (`FF5C 0004 4040`) put after tile 0's `SOT`, its `Psot`
//! raised to match: tile 0 codes two levels, which need 7 steps. Then
//! either the closing `EOC` is turned into `00 00`, or a `SIZ` segment
//! (`FF51 0004 0000`) is put after the last tile-part's `SOT` the same way.
//! OpenJPEG 2.5.0 decodes both files (`opj_decompress`): refusing the short
//! `QCD` is a departure this crate already made. Only which fault wins is
//! checked here.

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{DecodeOptions, Error, IncrementalDecoder, decode_with, read_header_with};

/// The `SOT` offsets of a codestream, by the `Psot` chain.
fn sots(bytes: &[u8]) -> Vec<usize> {
    let mut at = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    let mut found = Vec::new();
    while at + 12 <= bytes.len() && bytes[at..at + 2] == [0xFF, 0x90] {
        found.push(at);
        let psot = u32::from_be_bytes(bytes[at + 6..at + 10].try_into().unwrap()) as usize;
        if psot == 0 {
            break;
        }
        at += psot;
    }
    found
}

/// `bytes` with `segment` put after the `SOT` segment at `at`, its `Psot`
/// raised to match.
fn insert_after_sot(bytes: &mut Vec<u8>, at: usize, segment: &[u8]) {
    let psot = u32::from_be_bytes(bytes[at + 6..at + 10].try_into().unwrap());
    assert_ne!(psot, 0);
    bytes[at + 6..at + 10].copy_from_slice(&(psot + segment.len() as u32).to_be_bytes());
    bytes.splice(at + 12..at + 12, segment.iter().copied());
}

/// The fixture with tile 0's `QCD` of one step.
fn short_qcd() -> Vec<u8> {
    let mut bytes = std::fs::read("tests/fixtures/offset_tiled_lossless.j2k").unwrap();
    let parts = sots(&bytes);
    assert_eq!(parts.len(), 9);
    assert_eq!(parts[0], 110);
    assert_eq!(bytes[114..116], [0, 0], "tile 0");
    insert_after_sot(&mut bytes, 110, &[0xFF, 0x5C, 0, 4, 0x40, 0x40]);
    bytes
}

/// The input for the fault in tile 0's header and the one after it.
fn cases() -> [(&'static str, Vec<u8>); 2] {
    let mut junk = short_qcd();
    let end = junk.len() - 2;
    assert_eq!(junk[end..], [0xFF, 0xD9]);
    junk[end..].fill(0);
    let mut siz = short_qcd();
    let last = *sots(&siz).last().unwrap();
    insert_after_sot(&mut siz, last, &[0xFF, 0x51, 0, 4, 0, 0]);
    [("junk for EOC", junk), ("a SIZ in the last tile-part", siz)]
}

const SHORT_QCD: &str = "Marker(\"component 0 carries 1 quantization step entries but its 2 \
                         decomposition levels need 7\")";

fn lenient() -> DecodeOptions {
    DecodeOptions::default().with_strict(false)
}

/// With `strict` off, every prefix of each input is incomplete, then
/// decodes, then fails with one error, the fault in tile 0's header, which
/// is also the whole file's error with `strict` on. The header read of
/// each prefix gives no other error, and the two tile-part walks agree on
/// each.
#[test]
fn a_tile_header_fault_is_the_final_error_of_every_longer_prefix() {
    for (name, bytes) in cases() {
        for strict in [true, false] {
            let options = DecodeOptions::default().with_strict(strict);
            let whole = decode_with(&bytes, &options).map(|_| ());
            assert_eq!(format!("{whole:?}"), format!("Err({SHORT_QCD})"), "{name}");
        }
        // 0 incomplete, 1 decodes, 2 fails.
        let mut stage = 0;
        for length in 0..=bytes.len() {
            let prefix = &bytes[..length];
            rust_j2k::fuzz::tile_parts_walks_agree(prefix);
            let now = match decode_with(prefix, &lenient()) {
                Err(Error::Incomplete(_)) => 0,
                Ok(_) => 1,
                Err(error) => {
                    assert_eq!(format!("{error:?}"), SHORT_QCD, "{name} at {length}");
                    2
                }
            };
            assert!(now >= stage, "{name} at {length}: {now} after {stage}");
            stage = now;
            match read_header_with(prefix, &lenient()) {
                Ok(_) | Err(Error::Incomplete(_)) => {}
                Err(error) => assert_eq!(format!("{error:?}"), SHORT_QCD, "{name} at {length}"),
            }
        }
        assert_eq!(stage, 2, "{name}");
    }
}

/// Fed in pieces, a refine's lasting error is `finish`'s: the fault in
/// tile 0's header, from the feed that holds that header on.
#[test]
fn a_refine_gives_the_error_finish_gives() {
    for (name, bytes) in cases() {
        for piece in [1, 64, 256] {
            let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
            let mut first = None;
            for chunk in bytes.chunks(piece) {
                decoder.feed(chunk);
                let header = decoder.header().map(|_| ());
                if let Err(error) = decoder.refine() {
                    let error = format!("{error:?}");
                    assert_eq!(
                        error,
                        SHORT_QCD,
                        "{name}, {piece}, at {}",
                        decoder.bytes_fed()
                    );
                    first.get_or_insert(decoder.bytes_fed());
                } else {
                    assert!(first.is_none(), "{name}, {piece}: Ok after an error");
                }
                if let Err(error) = header {
                    assert_eq!(format!("{error:?}"), SHORT_QCD, "{name}, {piece}");
                }
            }
            assert!(first.is_some(), "{name}, {piece}");
            let finished = decoder.finish().map(|_| ());
            assert_eq!(
                format!("{finished:?}"),
                format!("Err({SHORT_QCD})"),
                "{name}"
            );
        }
    }
}

/// Tile 0 has no tile-part, and tile 1 codes the 9/7 wavelet under a 5/3
/// main header. With `strict` off the decode and the header read both name
/// tile 1, the tile the fault is in.
#[test]
fn a_tile_header_fault_names_the_tile_by_its_index() {
    let hex = "ff4fff510029000000000010000000080000000000000000000000080000000800000000000000\
               000001070101ff52000c00000001000100000001ff5c00074040404040ff90000a0001000000\
               2a0001ff52000c00000001000100000000ff5c000b424000400040004000ff9300ffd9";
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
        .collect();
    rust_j2k::fuzz::tile_parts_walks_agree(&bytes);
    let want = "Unsupported(\"tile 1 codes component 0 with the Irreversible97 wavelet where \
                the main header uses Reversible53; tiles that disagree on a component's \
                wavelet are not decoded\")";
    let decoded = decode_with(&bytes, &lenient()).map(|_| ());
    assert_eq!(format!("{decoded:?}"), format!("Err({want})"));
    // With `strict` on, tile 1's fault comes before the walk finds tile 0
    // missing at the end.
    let strict = decode_with(&bytes, &DecodeOptions::default()).map(|_| ());
    assert_eq!(format!("{strict:?}"), format!("Err({want})"));
}

/// The same fault where the header read walks the tile-parts for the
/// decode's error: three tiles of 1000 x 1000, over the precinct cap under
/// the main header's 1x1 precincts. Tile 0 has no tile-part, tile 1 codes
/// the 9/7 wavelet with its own precincts, within the cap, and tile 2 is
/// refused. With `strict` off the header read, as the decode, names tile 1.
#[test]
fn the_header_read_names_the_tile_at_fault_by_its_index() {
    let siz = synth::siz_body([3000, 1000, 0, 0, 1000, 1000, 0, 0], &[[7, 1, 1]]);
    let mut cod = synth::cod_body(1, 1, 0, 0);
    cod[0] = 1;
    cod.extend([0x00, 0x11]);
    let mut wavelet_97 = synth::cod_body(1, 1, 0, 0);
    *wavelet_97.last_mut().unwrap() = 0;
    // Scalar expounded, one step for each of the four subbands.
    let steps = [0x42, 0x40, 0, 0x40, 0, 0x40, 0, 0x40, 0];
    let own = [synth::seg(0xFF52, &wavelet_97), synth::seg(0xFF5C, &steps)].concat();
    let bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8; 4])),
        synth::tile_part(1, 0, 1, &own, &[]),
        synth::tile_part(2, 0, 1, &[], &[]),
        vec![0xFF, 0xD9],
    ]
    .concat();
    rust_j2k::fuzz::tile_parts_walks_agree(&bytes);
    let want = "Err(Unsupported(\"tile 1 codes component 0 with the Irreversible97 wavelet";
    let header = format!("{:?}", read_header_with(&bytes, &lenient()).map(|_| ()));
    assert!(header.starts_with(want), "{header}");
    let decoded = format!("{:?}", decode_with(&bytes, &lenient()).map(|_| ()));
    assert_eq!(decoded, header);
}
