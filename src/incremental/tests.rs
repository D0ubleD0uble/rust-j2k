//! The incremental decoder's bounds and its kept packet walks, against
//! one-shot decodes of the same bytes.

use super::*;
use crate::error::keyed;
use crate::tier2::Meter;
use crate::tier2::resume::{REPEAT_BOUNDS, Repeats, Walks};
use crate::{LimitKind, WorkLimit, codestream, tier1, tier2};
use std::cell::Cell;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// The packet-visit and code-block work bounds one decode of `bytes` has
/// under the default options.
fn one_shot_bounds(bytes: &[u8]) -> (u64, u64) {
    let parsed = codestream::parse_header(bytes).unwrap();
    let cs = codestream::parse_tiles_with(bytes, parsed, false).unwrap();
    let tile_bytes = crate::pipeline::decode::tile_bytes(&cs);
    (
        tier2::decode_visits(tile_bytes),
        tier1::decode_work(tile_bytes),
    )
}

fn whole_strict(bytes: &[u8]) -> Image {
    decode_with(bytes, &DecodeOptions::default()).unwrap()
}

/// What a refine on `prefix` must give: the strict-off decode, with a main
/// header cut short and a work-bound refusal as `None`.
fn oracle(prefix: &[u8], options: &DecodeOptions) -> Result<Option<Image>> {
    match decode_with(prefix, &options.clone().with_strict(false)) {
        Ok(image) => Ok(Some(image)),
        Err(Error::Incomplete(_)) => Ok(None),
        Err(Error::Limit(m))
            if matches!(m.kind(), LimitKind::PacketWork | LimitKind::CodeBlockWork) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// `options` without a code-block work bound. That is the bound the
/// earlier refines cut, so without it every refine must match the one-shot
/// decode of its bytes, however many came before it.
fn uncut(options: DecodeOptions) -> DecodeOptions {
    options.with_code_block_work_limit(WorkLimit::Unlimited)
}

/// Refine now, whatever the cadence says, with a wait on an error of the
/// tiles as that error.
fn refine_now(decoder: &mut IncrementalDecoder) -> Result<Option<Image>> {
    refine_held(decoder).0
}

/// [`refine_now`], and whether the refine waited on an error of the tiles
/// ([`Waiting::Held`]), which more bytes may replace.
fn refine_held(decoder: &mut IncrementalDecoder) -> (Result<Option<Image>>, bool) {
    decoder.tried = 0;
    match decoder.refine() {
        Ok(Refined::Waiting(Waiting::Held { error })) => (Err(error), true),
        refined => (refined.map(Refined::image), false),
    }
}

/// Feed `bytes` up to each of `ends` in turn, refining at each: every
/// refine is the one-shot decode of the bytes fed, and after an error each
/// later refine gives that error again. The number of images.
fn refine_at(name: &str, bytes: &[u8], ends: &[usize], options: &DecodeOptions) -> usize {
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    let mut images = 0;
    for &end in ends {
        let before = decoder.bytes_fed();
        decoder.feed(&bytes[before..end]);
        let (refined, held) = refine_held(&mut decoder);
        let refined = keyed(refined);
        assert_eq!(
            refined,
            keyed(oracle(&bytes[..end], options)),
            "{name}, {end} bytes after {before}"
        );
        match refined {
            Ok(Some(_)) => images += 1,
            Ok(None) => {}
            // More bytes may replace it with a fault that comes first.
            Err(_) if held => {}
            Err(error) => {
                decoder.feed(&bytes[end..]);
                assert_eq!(
                    keyed(refine_now(&mut decoder)),
                    Err(error),
                    "{name}, poisoned"
                );
                return images;
            }
        }
    }
    images
}

/// The ends of the pieces of `len` bytes cut every `stride` bytes.
fn every(stride: usize, len: usize) -> Vec<usize> {
    let mut ends: Vec<usize> = (stride..len).step_by(stride.max(1)).collect();
    ends.push(len);
    ends
}

/// Where marker `code` (`0xFF`, `code`) sits in `bytes`, past the first
/// `SOT`. The delimiters searched for here cannot appear inside coded
/// data, whose bit stuffing keeps any byte after `0xFF` below `0x90`.
fn markers(bytes: &[u8], code: u8) -> Vec<usize> {
    let start = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    (start..bytes.len() - 1)
        .filter(|&i| bytes[i] == 0xFF && bytes[i + 1] == code)
        .collect()
}

/// Each refine of a file fed a byte at a time goes on from the packet
/// walks the earlier refines kept, and gives the one-shot decode of the
/// bytes fed: through cuts inside packet headers and bodies, tile-part
/// headers and between tile-parts, with `SOP` and `EPH`, `PPM` and `PPT`,
/// `PLT` and `TLM`, and `POC` volumes in the main and tile-part headers.
#[test]
fn every_byte_refines_to_the_one_shot_decode_of_its_prefix() {
    for name in [
        "layers/lrcp.j2k",
        "layers/rpcl.j2k",
        "layers/cprl_irreversible.j2k",
        "layers/bypass_pterm.j2k",
        "layers/pterm_cut.j2k",
        "layers/pterm_poc_gap.j2k",
        "delimiters_sop_eph_lossless.j2k",
        "delimiters_sop_lossless.j2k",
        "delimiters_eph_lossless.j2k",
        "packed_overrun/ppm_whole.j2k",
        "packed_overrun/ppt_whole.j2k",
        "packed_overrun/ppm_short.j2k",
        "packed_overrun/ppt_short.j2k",
        "jp2_real/byte_tlm_plt.jp2",
        "poc_precedence/poc_main_and_split_tile.j2k",
        "poc_precedence/poc_main_then_later_tile_part.j2k",
        "poc_precedence/poc_tile_leaves_out_packets.j2k",
        "poc_coverage/poc_res_tileparts.j2k",
    ] {
        if crate::skip::excluded_path(name) {
            continue;
        }
        let bytes = fixture(name);
        let options = uncut(DecodeOptions::default());
        let images = refine_at(name, &bytes, &every(1, bytes.len()), &options);
        assert!(images > 0, "{name}: no image");
    }
}

/// Larger files, and the walk's other inputs: pieces of several sizes,
/// under a reduction, a layer cap and `strict` off, on files with several
/// tiles and tile-parts.
#[test]
fn pieces_of_any_size_refine_to_the_one_shot_decode_of_their_prefix() {
    for name in [
        "tiles_multipart_lossless.j2k",
        "layers/precincts_pterm.j2k",
        "multilayer_multicomponent_lossless.j2k",
        "precincts_tiled_cprl_lossless.j2k",
        "tile_header/tlm_32_16k.j2k",
    ] {
        let bytes = fixture(name);
        let strides = if bytes.len() > 8192 {
            [251, 1009, bytes.len() / 5 + 1]
        } else {
            [3, 61, bytes.len() / 5 + 1]
        };
        for options in [
            DecodeOptions::default(),
            DecodeOptions::default().with_resolution_reduction(1),
            DecodeOptions::default().with_max_quality_layers(Some(1)),
        ] {
            let options = uncut(options);
            for stride in strides {
                let ends = every(stride, bytes.len());
                // A file of 1x1 tiles has no level to reduce.
                let images = refine_at(name, &bytes, &ends, &options);
                assert!(images > 0 || options.resolution_reduction > 0, "{name}");
            }
        }
    }
}

/// The MIXED HT files, where each code-block is an HT or a Part 1 block,
/// cut at points drawn over all of each.
#[test]
fn mixed_ht_files_refine_to_the_one_shot_decode_of_their_prefix() {
    // The conformance corpus is left out of the published package.
    if crate::skip::excluded() {
        return;
    }
    let dir = format!(
        "{}/tests/fixtures/conformance/ht/codestreams",
        env!("CARGO_MANIFEST_DIR")
    );
    let entries = std::fs::read_dir(&dir).expect("the HT corpus directory");
    let mut seen = 0;
    for entry in entries {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        if !name.starts_with("ds0_hm_") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let options = uncut(DecodeOptions::default().with_resolution_reduction(1));
        for stride in [97, 1009] {
            refine_at(&name, &bytes, &every(stride, bytes.len()), &options);
        }
        seen += 1;
    }
    assert_eq!(seen, 3, "the MIXED file count changed");
}

/// A file with packed headers (`PPM`) and `SOP` markers, cut where the data
/// ends right after a packet: the packets read with no data after them may
/// yet have an `SOP` before them, which the next refine finds.
#[test]
fn packed_headers_with_sop_markers_refine_to_the_one_shot_decode() {
    // The conformance corpus is left out of the published package.
    if crate::skip::excluded() {
        return;
    }
    let path = format!(
        "{}/tests/fixtures/conformance/codestreams/p1_05.j2k",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(&path).expect("read p1_05");
    let options = uncut(DecodeOptions::default().with_resolution_reduction(2));
    // Each cut ends a piece right before an `SOP`.
    let mut ends: Vec<usize> = markers(&bytes, 0x91).into_iter().step_by(97).collect();
    ends.push(bytes.len());
    ends.dedup();
    assert!(refine_at("p1_05.j2k", &bytes, &ends, &options) > 2);
}

/// The cut points the walk stops at, one of each kind, each followed by
/// the rest of the file in two pieces.
fn stop_at(name: &str, bytes: &[u8], at: usize) {
    let options = uncut(DecodeOptions::default());
    let rest = at + (bytes.len() - at) / 2;
    let images = refine_at(name, bytes, &[at, rest, bytes.len()], &options);
    assert_eq!(images, 3, "{name}, cut at {at}");
}

/// A 1 x 1 sample under one wavelet level and two LRCP layers, with `SOP`
/// markers and no `EPH`, its four packets empty: each an `SOP` and `00`.
fn empty_packets_with_sop() -> Vec<u8> {
    let mut cs = vec![0xFF, 0x4F];
    // SIZ: one 1 x 1 tile of one 8-bit component.
    cs.extend_from_slice(&[0xFF, 0x51, 0, 41, 0, 0]);
    for v in [1u32, 1, 0, 0, 1, 1, 0, 0] {
        cs.extend_from_slice(&v.to_be_bytes());
    }
    cs.extend_from_slice(&[0, 1, 7, 1, 1]);
    // COD: SOP, LRCP, 2 layers, 1 level, 64 x 64 code-blocks, 5/3.
    cs.extend_from_slice(&[0xFF, 0x52, 0, 12, 0x02, 0, 0, 2, 0, 1, 4, 4, 0, 1]);
    // QCD: no quantization, for 4 bands.
    cs.extend_from_slice(&[0xFF, 0x5C, 0, 7, 0x40, 0x40, 0x40, 0x40, 0x40]);
    let packets: Vec<u8> = (0..4u8)
        .flat_map(|n| [0xFF, 0x91, 0, 4, 0, n, 0x00])
        .collect();
    let psot = 14 + packets.len() as u32;
    cs.extend_from_slice(&[0xFF, 0x90, 0, 10, 0, 0]);
    cs.extend_from_slice(&psot.to_be_bytes());
    cs.extend_from_slice(&[0, 1, 0xFF, 0x93]);
    cs.extend_from_slice(&packets);
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

/// A header of 65535 one-sample tiles of 300 components, and tile 0 alone:
/// the charge for the missing tiles' tile-components passes the packet
/// bound, though the file is a few kilobytes.
fn missing_tiles_past_the_bound() -> Vec<u8> {
    let components: u16 = 300;
    let mut cs = vec![0xFF, 0x4F, 0xFF, 0x51];
    cs.extend_from_slice(&(38 + 3 * components).to_be_bytes());
    cs.extend_from_slice(&[0, 0]);
    for v in [65535u32, 1, 0, 0, 1, 1, 0, 0] {
        cs.extend_from_slice(&v.to_be_bytes());
    }
    cs.extend_from_slice(&components.to_be_bytes());
    for _ in 0..components {
        cs.extend_from_slice(&[7, 1, 1]);
    }
    cs.extend_from_slice(&[0xFF, 0x52, 0, 12, 0x00, 0, 0, 1, 0, 0, 4, 4, 0, 1]);
    cs.extend_from_slice(&[0xFF, 0x5C, 0, 4, 0x40, 0x40]);
    cs.extend_from_slice(&[0xFF, 0x90, 0, 10, 0, 0]);
    cs.extend_from_slice(&(14 + u32::from(components)).to_be_bytes());
    cs.extend_from_slice(&[0, 1, 0xFF, 0x93]);
    cs.extend(std::iter::repeat_n(0u8, usize::from(components)));
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

/// The charge for missing tiles is a work bound too: a refine whose
/// missing tiles cost more than the packet bound waits with `MissingTiles`,
/// though no packet is read, as the one-shot decode of the same bytes is
/// refused, and carries that decode's refusal.
#[test]
fn a_missing_tile_charge_past_the_bound_waits() {
    let bytes = missing_tiles_past_the_bound();
    let lenient = DecodeOptions::default().with_strict(false);
    let expected = match decode_with(&bytes, &lenient) {
        Err(Error::Limit(message)) => {
            assert_eq!(message.kind(), LimitKind::PacketWork, "{message}");
            message
        }
        other => panic!("{other:?}"),
    };
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes);
    match decoder.refine() {
        Ok(Refined::Waiting(Waiting::MissingTiles { limit, .. })) => {
            assert_eq!(format!("{limit:?}"), format!("{expected:?}"));
            assert_eq!(limit.retry_with(), expected.retry_with());
            assert!(limit.retry_with().is_some(), "{limit}");
        }
        other => panic!("{other:?}"),
    }
    assert!(decoder.refused);
    assert_eq!(decoder.bytes_decoded(), 0);
}

/// A header of 1024 one-sample tiles of 16384 components, one LRCP layer
/// and no wavelet levels, and tile 0 alone: its 16384 empty packets, then
/// `padding` zero bytes. The 1023 missing tiles' charge plus tile 0's walk
/// is just past the floor of the packet bound, so a prefix is refused for
/// work until tile 0's bytes raise the bound past it, at about 513 KB.
fn missing_tiles_and_padding(padding: u32) -> Vec<u8> {
    let components: u16 = 16384;
    let mut cs = vec![0xFF, 0x4F, 0xFF, 0x51];
    cs.extend_from_slice(&(38 + 3 * u32::from(components)).to_be_bytes()[2..]);
    cs.extend_from_slice(&[0, 0]);
    for v in [1024u32, 1, 0, 0, 1, 1, 0, 0] {
        cs.extend_from_slice(&v.to_be_bytes());
    }
    cs.extend_from_slice(&components.to_be_bytes());
    for _ in 0..components {
        cs.extend_from_slice(&[7, 1, 1]);
    }
    cs.extend_from_slice(&[0xFF, 0x52, 0, 12, 0x00, 0, 0, 1, 0, 0, 4, 4, 0, 1]);
    cs.extend_from_slice(&[0xFF, 0x5C, 0, 4, 0x40, 0x40]);
    cs.extend_from_slice(&[0xFF, 0x90, 0, 10, 0, 0]);
    cs.extend_from_slice(&(14 + u32::from(components) + padding).to_be_bytes());
    cs.extend_from_slice(&[0, 1, 0xFF, 0x93]);
    cs.extend(std::iter::repeat_n(
        0u8,
        usize::from(components) + padding as usize,
    ));
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

/// A refine the packet bound refuses still counts as a try: the next one
/// waits until the bytes fed are twice those it tried, though the one-shot
/// decode of fewer bytes gives an image by then.
#[test]
fn a_refused_refine_puts_off_the_next_until_the_bytes_double() {
    let bytes = missing_tiles_and_padding(1 << 20);
    let lenient = DecodeOptions::default().with_strict(false);
    let header = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    // Fed up to `tried`, the refine is refused for work; at `between`, the
    // one-shot decode gives an image, but the refine does not try.
    let tried = header + (300 << 10);
    let between = header + (600 << 10);
    assert!(between < 2 * tried);
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes[..tried]);
    // Tile 0's walk, not the charge for the missing tiles, passes the bound.
    assert!(matches!(
        decoder.refine(),
        Ok(Refined::Waiting(Waiting::WorkLimit { limit: m, .. })) if m.kind() == LimitKind::PacketWork
    ));
    assert!(decoder.refused);
    assert!(matches!(
        decode_with(&bytes[..tried], &lenient),
        Err(Error::Limit(m)) if m.kind() == LimitKind::PacketWork
    ));
    decoder.feed(&bytes[tried..between]);
    assert!(decode_with(&bytes[..between], &lenient).is_ok());
    assert!(matches!(
        decoder.refine(),
        Ok(Refined::Waiting(Waiting::MoreBytes { until, .. })) if until == 2 * tried
    ));
    decoder.feed(&bytes[between..2 * tried - 1]);
    assert!(matches!(
        decoder.refine(),
        Ok(Refined::Waiting(Waiting::MoreBytes { until, .. })) if until == 2 * tried
    ));
    assert_eq!(decoder.bytes_decoded(), 0);
    decoder.feed(&bytes[2 * tried - 1..2 * tried]);
    // The image is the one-shot decode of its bytes, as every refine's is
    // (the tests above); decoding it again here would double the time.
    assert!(
        decoder.refine().unwrap().image().is_some(),
        "the bytes have doubled"
    );
    assert_eq!(decoder.bytes_decoded(), 2 * tried);
}

/// `tiles` tiles of one sample each in a row, with no wavelet levels and
/// two LRCP layers, each tile's two packets empty: `00`.
fn one_sample_tiles(tiles: u32) -> Vec<u8> {
    let mut cs = vec![0xFF, 0x4F];
    cs.extend_from_slice(&[0xFF, 0x51, 0, 41, 0, 0]);
    for v in [tiles, 1, 0, 0, 1, 1, 0, 0] {
        cs.extend_from_slice(&v.to_be_bytes());
    }
    cs.extend_from_slice(&[0, 1, 7, 1, 1]);
    cs.extend_from_slice(&[0xFF, 0x52, 0, 12, 0x00, 0, 0, 2, 0, 0, 4, 4, 0, 1]);
    cs.extend_from_slice(&[0xFF, 0x5C, 0, 4, 0x40, 0x40]);
    for index in 0..tiles {
        cs.extend_from_slice(&[0xFF, 0x90, 0, 10]);
        cs.extend_from_slice(&(index as u16).to_be_bytes());
        cs.extend_from_slice(&16u32.to_be_bytes());
        cs.extend_from_slice(&[0, 1, 0xFF, 0x93, 0, 0]);
    }
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

/// A file of many tiles whose packet state is large beside the samples
/// they hold: the decoder keeps the walks of some, within about twice what
/// one decode holds, and walks the rest from the start on each refine,
/// which still gives the one-shot decode of the bytes fed.
#[test]
fn walks_past_the_room_are_walked_again() {
    let bytes = one_sample_tiles(64);
    whole_strict(&bytes);
    let options = uncut(DecodeOptions::default());
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    for end in every(7, bytes.len()) {
        let before = decoder.bytes_fed();
        decoder.feed(&bytes[before..end]);
        assert_eq!(
            keyed(refine_now(&mut decoder)),
            keyed(oracle(&bytes[..end], &options)),
            "{end}"
        );
    }
    let (kept, walked) = decoder.spent.walks.kept();
    assert!(kept > 0 && walked > 0, "{kept} kept, {walked} walked again");
}

/// One tile of 1024 x 1024 samples with 3 wavelet levels, one layer and
/// code-blocks of 4 x 4 samples, each of its 4 packets empty (`00`), with
/// `comments` `COM` segments of 60000 bytes in its tile-part header: 94
/// bytes without them, whose packet walk holds the state of 65536
/// code-blocks.
fn large_tile(comments: usize) -> Vec<u8> {
    const COMMENT: usize = 60000;
    let mut cs = vec![0xFF, 0x4F];
    cs.extend_from_slice(&[0xFF, 0x51, 0, 41, 0, 0]);
    for v in [1024u32, 1024, 0, 0, 1024, 1024, 0, 0] {
        cs.extend_from_slice(&v.to_be_bytes());
    }
    cs.extend_from_slice(&[0, 1, 7, 1, 1]);
    cs.extend_from_slice(&[0xFF, 0x52, 0, 12, 0x00, 0, 0, 1, 0, 3, 0, 0, 0, 1]);
    cs.extend_from_slice(&[0xFF, 0x5C, 0, 13, 0x40]);
    cs.extend_from_slice(&[0x40; 10]);
    let psot = 18 + comments * (2 + COMMENT);
    cs.extend_from_slice(&[0xFF, 0x90, 0, 10, 0, 0]);
    cs.extend_from_slice(&(psot as u32).to_be_bytes());
    cs.extend_from_slice(&[0, 1]);
    for _ in 0..comments {
        // `Lcom` counts itself, `Rcom` (0, binary) and the comment.
        cs.extend_from_slice(&[0xFF, 0x64]);
        cs.extend_from_slice(&(COMMENT as u16).to_be_bytes());
        cs.extend_from_slice(&[0, 0]);
        cs.extend(std::iter::repeat_n(0x20, COMMENT - 4));
    }
    cs.extend_from_slice(&[0xFF, 0x93, 0, 0, 0, 0]);
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

/// A short file whose tile's packet walk is past the room its bytes give
/// keeps no walk between refines, though its image gives room for it; the
/// same tile with 240 KB of comments in its header keeps it. Each refine is
/// the one-shot decode of the bytes fed.
#[test]
fn a_decoder_keeps_the_walks_its_bytes_make_room_for() {
    use crate::tier2::resume::kept_room;
    for (comments, kept) in [(0, (0, 1)), (4, (1, 0))] {
        let bytes = large_tile(comments);
        whole_strict(&bytes);
        let options = DecodeOptions::default();
        let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
        decoder.feed(&bytes);
        assert_eq!(
            keyed(refine_now(&mut decoder)),
            keyed(oracle(&bytes, &options)),
            "{comments} comments"
        );
        let walks = &decoder.spent.walks;
        assert_eq!(walks.kept(), kept, "{comments} comments");
        let held = walks.kept_bytes();
        assert!(held <= kept_room(bytes.len() as u64), "{comments}: {held}");
        assert_eq!(held > 0, kept.0 == 1, "{comments}: {held}");
    }
}

/// A walk larger than the room the bytes fed give is not kept, whatever
/// room the image would give it, and is kept once the bytes fed make room
/// for it. Either way the walk charges what the one-shot walk charges,
/// and each of the three walks is charged to the walks repeated: the walks
/// not kept, and the walk built once there is room, which reads the tile
/// from the start again.
#[test]
fn a_walk_past_the_room_of_the_bytes_fed_waits_for_more_bytes() {
    use crate::tier2::resume::kept_room;
    let bytes = large_tile(0);
    assert_eq!(bytes.len(), 94);
    whole_strict(&bytes);
    let parsed = codestream::parse_header(&bytes).unwrap();
    let cs = codestream::parse_tiles_with(&bytes, parsed, false).unwrap();
    let poc_bitmap = tier2::decode_visits(crate::pipeline::decode::tile_bytes(&cs));
    let tile = &cs.tiles[0];
    let header = cs.tile_header(0).unwrap();
    let once = Meter::new(u64::MAX / 2);
    tier2::decode_packets_partial(&header, tile, &once, poc_bitmap, 0, None).unwrap();
    let mut walks = Walks::default();
    let repeats = Repeats::default();
    // An image room of 1 GB, and the room of 94 bytes fed, then of 1 MB.
    for (fed, kept) in [(94, 0), (94, 0), (1 << 20, 1)] {
        let meter = Meter::new(u64::MAX / 2);
        walks
            .walk(
                &header,
                tile,
                &meter,
                poc_bitmap,
                0,
                None,
                1 << 30,
                fed,
                &repeats,
            )
            .unwrap();
        assert_eq!(meter.spent(), once.spent(), "{fed} bytes fed");
        assert_eq!(walks.kept(), (kept, 1 - kept), "{fed} bytes fed");
        assert!(walks.kept_bytes() <= kept_room(fed), "{fed} bytes fed");
        assert_eq!(walks.kept_bytes() > 0, kept == 1, "{fed} bytes fed");
    }
    assert_eq!(repeats.visits.get(), 3 * once.spent());
    assert!(!repeats.refused.get());
}

/// A feed that ends one byte into an `SOP`, headers inline: the one-shot
/// decode of those bytes reads the `0xFF` as an empty packet's header, and
/// the refine does too, but puts the packet back, so that the next refine
/// reads the marker that more data shows there.
#[test]
fn a_feed_can_end_one_byte_into_an_sop() {
    let bytes = empty_packets_with_sop();
    assert_eq!(bytes.len(), 112);
    whole_strict(&bytes);
    let sops = markers(&bytes, 0x91);
    assert_eq!(sops.len(), 4);
    let options = uncut(DecodeOptions::default());
    for &sop in &sops {
        refine_at(
            "empty packets with SOP",
            &bytes,
            &[sop + 1, bytes.len()],
            &options,
        );
    }
    // And after every `0xFF` in turn.
    let ends: Vec<usize> = (0..bytes.len())
        .filter(|&i| bytes[i] == 0xFF)
        .map(|i| i + 1)
        .chain([bytes.len()])
        .collect();
    refine_at("empty packets with SOP", &bytes, &ends, &options);
}

/// A feed that ends inside a packet header: past the `SOP` of a packet,
/// before its `EPH`.
#[test]
fn a_feed_can_end_inside_a_packet_header() {
    let bytes = fixture("delimiters_sop_eph_lossless.j2k");
    let ephs = markers(&bytes, 0x92);
    // A packet whose header is more than a byte.
    let sop = *markers(&bytes, 0x91)
        .iter()
        .find(|&&s| ephs.iter().find(|&&e| e > s).is_some_and(|&e| e > s + 7))
        .unwrap();
    stop_at("a packet header", &bytes, sop + 7);
}

/// A feed that ends inside a packet body: past the `EPH` of a packet,
/// before the next packet's `SOP`.
#[test]
fn a_feed_can_end_inside_a_packet_body() {
    let bytes = fixture("delimiters_sop_eph_lossless.j2k");
    let ephs = markers(&bytes, 0x92);
    let sops = markers(&bytes, 0x91);
    let (eph, next) = ephs
        .iter()
        .find_map(|&e| {
            let next = *sops.iter().find(|&&s| s > e)?;
            (next > e + 3).then_some((e, next))
        })
        .unwrap();
    stop_at("a packet body", &bytes, eph + 3);
    stop_at("the end of a packet body", &bytes, next - 1);
}

/// A feed that ends inside a tile-part header, and one that ends between
/// two tile-parts.
#[test]
fn a_feed_can_end_inside_or_between_tile_parts() {
    let bytes = fixture("tiles_multipart_lossless.j2k");
    let sots = markers(&bytes, 0x90);
    assert!(sots.len() > 2);
    for &sot in &sots[1..] {
        stop_at("between tile-parts", &bytes, sot);
        stop_at("a tile-part header", &bytes, sot + 6);
        stop_at("past a tile-part header", &bytes, sot + 14);
    }
}

/// After a refine fails, each later refine gives the same error, however
/// many bytes come, and `finish` decodes the whole input under the
/// decoder's options.
#[test]
fn a_refine_error_poisons_the_later_refines() {
    let bytes = fixture("packed_overrun/ppt_short.j2k");
    let error = oracle(&bytes, &DecodeOptions::default()).unwrap_err();
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes);
    let error = format!("{error:?}");
    assert_eq!(
        keyed(decoder.refine().map(Refined::image)),
        Err(error.clone())
    );
    for _ in 0..3 {
        decoder.feed(&[0; 64]);
        assert_eq!(keyed(refine_now(&mut decoder)), Err(error.clone()));
    }
    let more = [&bytes[..], &[0; 192]].concat();
    assert_eq!(
        keyed(decoder.finish()),
        keyed(decode_with(&more, &DecodeOptions::default()))
    );
}

/// A feeder that hands over one byte at a time and refines after each
/// spends, over all its refines, no more code-block work than one decode
/// of the whole file is allowed, since the ledger cuts each refine's bound
/// by what the earlier ones spent. Its packet walks read each packet once:
/// they spend the visits of one walk of the whole file. The last image
/// comes within the last doubling of the input, as the cadence promises.
#[test]
fn a_byte_at_a_time_feeder_stays_within_one_decodes_bounds() {
    for name in [
        "layers/lrcp.j2k",
        "layers/rpcl.j2k",
        "layers/bypass_pterm.j2k",
        "jpeg2000_regular_latlon.j2k",
        "poc_precedence/poc_main_and_tile.j2k",
    ] {
        let bytes = fixture(name);
        let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
        let mut images = 0;
        crate::pipeline::decode::tests::BLOCK_WORK.with(|spent| spent.set(0));
        for byte in &bytes {
            decoder.feed(std::slice::from_ref(byte));
            if decoder.refine().unwrap().image().is_some() {
                images += 1;
            }
        }
        let block_work = crate::pipeline::decode::tests::BLOCK_WORK.with(Cell::get);
        let (packet_bound, block_bound) = one_shot_bounds(&bytes);
        let packets = decoder.spent.walks.visits();
        let blocks = decoder.spent.blocks;
        assert!(packets > 0 && blocks > 0, "{name}: nothing was decoded");
        assert_eq!(blocks, block_work, "{name}: the ledger missed block work");
        // One refine of the whole file walks it once.
        let mut once = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
        once.feed(&bytes);
        assert!(once.refine().unwrap().image().is_some(), "{name}");
        let one_walk = once.spent.walks.visits();
        assert_eq!(packets, one_walk, "{name}: the walks read packets again");
        // Every walk is kept, so none is charged as one walked again.
        assert_eq!(decoder.spent.walks.kept().1, 0, "{name}");
        assert_eq!(decoder.spent.repeats, 0, "{name}: a kept walk was charged");
        assert!(
            packets <= packet_bound,
            "{name}: {packets} > {packet_bound}"
        );
        assert!(blocks <= block_bound, "{name}: {blocks} > {block_bound}");
        assert!(images > 0, "{name}: no image");
        assert!(
            2 * decoder.bytes_decoded() > bytes.len(),
            "{name}: the last image is of {} of {} bytes",
            decoder.bytes_decoded(),
            bytes.len()
        );
        assert_eq!(decoder.finish().unwrap(), whole_strict(&bytes), "{name}");
    }
}

/// A file whose packet walk costs about the same however few of its bytes
/// are there, fed in two halves under a bound that covers one walk: both
/// refines decode, since the second goes on from the first's walk, and the
/// two spend no more visits than one decode of the file is allowed.
#[test]
fn a_costly_walk_fed_in_halves_walks_once() {
    // Its walk takes about twice the default bound, at any length.
    let bytes = fixture("work_bound/sweep_rpcl_1024_64.j2k");
    let options = DecodeOptions::default().with_packet_work_limit(WorkLimit::Raised(4));
    let packet_bound = 4 * one_shot_bounds(&bytes).0;
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    let half = bytes.len() / 2;
    decoder.feed(&bytes[..half]);
    assert_eq!(
        keyed(decoder.refine().map(Refined::image)),
        keyed(oracle(&bytes[..half], &options))
    );
    let first = decoder.spent.walks.visits();
    assert!(4 * first > packet_bound, "{first} of {packet_bound}");
    decoder.feed(&bytes[half..]);
    let image = decoder.refine().unwrap().image().expect("the walk goes on");
    assert_eq!(
        image,
        decode_with(&bytes, &options.clone().with_strict(false)).unwrap()
    );
    let packets = decoder.spent.walks.visits();
    assert!(packets <= packet_bound, "{packets} > {packet_bound}");
    assert!(2 * first > packets, "{first}, then {packets}");
    let finished = decoder.finish().unwrap();
    assert_eq!(finished, decode_with(&bytes, &options).unwrap());
}

/// A refine whose walk a decode of its bytes on its own is refused for is
/// put off, and `finish` still decodes the file under a bound of its own.
#[test]
fn a_walk_past_the_packet_bound_waits() {
    let bytes = fixture("work_bound/sweep_rpcl_1024_64.j2k");
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes);
    assert!(matches!(
        decoder.refine(),
        Ok(Refined::Waiting(Waiting::WorkLimit { limit: m, .. })) if m.kind() == LimitKind::PacketWork
    ));
    assert!(decoder.refused);
    assert_eq!(decoder.bytes_decoded(), 0);
    let options = DecodeOptions::default().with_packet_work_limit(WorkLimit::Raised(4));
    assert!(decode_with(&bytes, &options).is_ok());
}

/// Whether `refined` waits on `BoundSpent`, with a code-block detail and
/// no hint.
fn is_bound_spent(refined: &crate::Result<Refined>) -> bool {
    matches!(
        refined,
        Ok(Refined::Waiting(Waiting::BoundSpent { limit: m, .. }))
            if m.kind() == LimitKind::CodeBlockWork && m.retry_with().is_none()
    )
}

/// A refine that the rest of the code-block bound cannot cover, though its
/// own work fits the whole bound, waits on `BoundSpent`, and `finish`
/// still decodes the file under a bound of its own. Meanwhile a decode of
/// the bytes fed now with the decoder's options, `strict` off, has a bound
/// of its own too and gives the image the refine waits on, at any cut: the
/// way out of a wait on the shared bound.
#[test]
fn a_refine_past_the_rest_of_the_bound_waits() {
    let bytes = fixture("layers/lrcp.j2k");
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.spent.blocks = u64::MAX - 1;
    decoder.feed(&bytes);
    let refined = decoder.refine();
    assert!(is_bound_spent(&refined), "{refined:?}");
    assert_eq!(decoder.bytes_decoded(), 0);
    let now = |decoder: &IncrementalDecoder| {
        decode_with(
            decoder.bytes(),
            &decoder.options().clone().with_strict(false),
        )
    };
    assert_eq!(
        keyed(now(&decoder).map(Some)),
        keyed(oracle(&bytes, &DecodeOptions::default()))
    );
    assert_eq!(decoder.finish().unwrap(), whole_strict(&bytes));

    // The same with half the bound spent, as the issue measured, on a cut
    // file whose decode-now image is partial.
    let half = bytes.len() / 2;
    let mut decoder =
        IncrementalDecoder::with_code_block_work_spent(DecodeOptions::default(), u64::MAX / 2)
            .unwrap();
    decoder.feed(&bytes[..half]);
    let refined = decoder.refine();
    assert!(is_bound_spent(&refined), "{refined:?}");
    let image = now(&decoder).unwrap();
    assert!(!image.report.is_complete());
    assert_eq!(
        Some(image),
        oracle(&bytes[..half], &DecodeOptions::default()).unwrap()
    );
}

/// A refine that the rest of the code-block bound just covers decodes:
/// the bound is cut by what the earlier refines spent, and no more.
#[test]
fn a_refine_the_rest_of_the_bound_covers_decodes() {
    let bytes = fixture("layers/lrcp.j2k");
    let (_, block_bound) = one_shot_bounds(&bytes);
    let mut once = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    once.feed(&bytes);
    assert!(once.refine().unwrap().image().is_some());
    let work = once.spent.blocks;
    assert!(8 * work < block_bound, "{work} of {block_bound}");

    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    // The earlier refines left just what the decode of the file takes.
    decoder.spent.blocks = block_bound - work;
    decoder.feed(&bytes);
    assert!(
        decoder.refine().unwrap().image().is_some(),
        "{work} of {block_bound}"
    );
    assert_eq!(decoder.spent.blocks, block_bound);
    decoder.feed(&[]);
    assert!(
        matches!(refine_now(&mut decoder), Ok(None)),
        "nothing is left"
    );
}

/// A kept walk over a `POC` sequence whose volumes repeat each other is
/// charged, at each cut, the packet visits the one-shot walk of the same
/// bytes spends: the packets it skips as emitted before, from the data
/// and past its end, cost what they cost that walk.
#[test]
fn a_kept_walk_charges_poc_repeats_as_the_one_shot_walk_does() {
    let bytes = fixture("work_bound/pocdup_4x4_1024_31.j2k");
    let mut walks = Walks::default();
    let mut checked = 0;
    for end in every(bytes.len() / 9, bytes.len()) {
        let prefix = &bytes[..end];
        let Ok(parsed) = codestream::parse_header(prefix) else {
            continue;
        };
        let cs = codestream::parse_tiles_with(prefix, parsed, false).unwrap();
        let poc_bitmap = tier2::decode_visits(crate::pipeline::decode::tile_bytes(&cs));
        for (position, tile) in cs.tiles.iter().enumerate() {
            let header = cs.tile_header(position).unwrap();
            let once = Meter::new(u64::MAX / 2);
            let expected =
                tier2::decode_packets_partial(&header, tile, &once, poc_bitmap, 0, None).is_ok();
            let kept = Meter::new(u64::MAX / 2);
            let walked = walks
                .walk(
                    &header,
                    tile,
                    &kept,
                    poc_bitmap,
                    0,
                    None,
                    u64::MAX / 4,
                    u64::MAX / 4,
                    &Repeats::default(),
                )
                .is_ok();
            assert_eq!(walked, expected, "{end} bytes");
            assert_eq!(kept.spent(), once.spent(), "{end} bytes");
            checked += 1;
        }
    }
    assert!(checked > 5, "{checked}");
}

/// A refine the code-block bound refuses at its second tile is charged the
/// work of the first tile, which it did, and not the work of the tile it
/// refused. Its own work fits the whole bound, so it waits on
/// `BoundSpent`.
#[test]
fn a_refused_refine_is_charged_the_tiles_before_the_cut() {
    let bytes = fixture("tiles_even_origin_lossless.j2k");
    let work_of = |region: Option<crate::Region>| {
        let mut options = DecodeOptions::default();
        if let Some(region) = region {
            options = options.with_region(region);
        }
        let mut decoder = IncrementalDecoder::new(options).unwrap();
        decoder.feed(&bytes);
        assert!(decoder.refine().unwrap().image().is_some());
        (decoder.spent.blocks, decoder.spent.bound.unwrap())
    };
    let (whole, bound) = work_of(None);
    let (first, tile_bound) = work_of(Some(crate::Region::Tile(0)));
    let (second, _) = work_of(Some(crate::Region::Tile(1)));
    assert_eq!(tile_bound, bound);
    assert!(
        first > 0 && second > 0 && first + second <= whole,
        "{first} {second} {whole}"
    );

    // What is left of the bound covers the first tile and no more.
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.spent.blocks = bound - first;
    decoder.feed(&bytes);
    let refined = decoder.refine();
    assert!(is_bound_spent(&refined), "{refined:?}");
    assert!(decoder.refused);
    assert_eq!(decoder.spent.blocks, bound, "{first} of tile 0 is charged");
}

/// The code-block work a refine of `bytes` under `options`, with nothing
/// spent, spends, after checking that it gives the image.
fn own_work(bytes: &[u8], options: &DecodeOptions) -> u64 {
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    decoder.feed(bytes);
    assert!(decoder.refine().unwrap().image().is_some());
    decoder.spent.blocks
}

/// `refine` on a decoder whose earlier refines spent `spent`, fed `bytes`.
fn refine_after(bytes: &[u8], options: &DecodeOptions, spent: u64) -> crate::Result<Refined> {
    let mut decoder =
        IncrementalDecoder::with_code_block_work_spent(options.clone(), spent).unwrap();
    decoder.feed(bytes);
    decoder.refine()
}

/// `options` under a code-block work cap of `cap` (see
/// [`crate::fuzz::with_code_block_work_cap`]).
fn capped(cap: u64) -> DecodeOptions {
    crate::fuzz::with_code_block_work_cap(DecodeOptions::default(), cap)
}

/// At the tile the rest of the bound cannot cover, a refine whose own work
/// is exactly the whole bound waits on `BoundSpent`: under a cap of `W`,
/// the work of `small_blocks_128_b4.j2k`, with 1 spent. With nothing spent
/// it decodes, and under a cap of `W - 1` its own work passes the bound, so
/// it waits on `WorkLimit`, as a decode of it on its own is refused.
#[test]
fn a_refine_whose_own_work_is_the_whole_bound_waits_on_bound_spent() {
    if crate::skip::excluded() {
        return;
    }
    let bytes = fixture("bench/small_blocks_128_b4.j2k");
    let work = own_work(&bytes, &capped(u64::MAX / 8));
    let options = capped(work);
    assert!(matches!(
        refine_after(&bytes, &options, 0),
        Ok(Refined::Image(_))
    ));
    let refined = refine_after(&bytes, &options, 1);
    assert!(is_bound_spent(&refined), "{work}: {refined:?}");
    let under = capped(work - 1);
    assert!(decode_with(&bytes, &under.clone().with_strict(false)).is_err());
    let refined = refine_after(&bytes, &under, 1);
    assert!(
        matches!(
            &refined,
            Ok(Refined::Waiting(Waiting::WorkLimit { limit: m, .. })) if m.kind() == LimitKind::CodeBlockWork
        ),
        "{refined:?}"
    );
}

/// Past the tile the rest of the bound cannot cover, a refine counts the
/// later tiles' work against the whole bound, and waits on `BoundSpent`
/// whether its own work reaches exactly the whole bound at its last tile or
/// passes it there: it did not decode the tiles from the cut on, which a
/// decode of the file on its own decodes before it is refused. Only the
/// message differs: where the whole bound refuses the last tile, it names
/// that tile. On `tiles_even_origin_lossless.j2k`, under a cap of `W`, the
/// work of all nine tiles, what the earlier refines left covers one less
/// than tile 0.
#[test]
fn a_counted_refine_whose_own_work_reaches_the_whole_bound_waits_on_bound_spent() {
    let bytes = fixture("tiles_even_origin_lossless.j2k");
    let measure = capped(u64::MAX / 8);
    let whole = own_work(&bytes, &measure);
    let first = own_work(&bytes, &measure.with_region(crate::Region::Tile(0)));
    assert!(0 < first && first < whole, "{first} of {whole}");
    let refined = refine_after(&bytes, &capped(whole), whole - first + 1);
    assert!(is_bound_spent(&refined), "{first} of {whole}: {refined:?}");
    let Ok(Refined::Waiting(Waiting::BoundSpent { limit, .. })) = refined else {
        unreachable!()
    };
    assert!(
        limit
            .message()
            .starts_with("the code-blocks of the tiles counted"),
        "{limit}"
    );
    let under = capped(whole - 1);
    let Err(Error::Limit(_)) = decode_with(&bytes, &under.clone().with_strict(false)) else {
        panic!("the file fits one less");
    };
    let refined = refine_after(&bytes, &under, whole - first);
    assert!(is_bound_spent(&refined), "{first} of {whole}: {refined:?}");
    let Ok(Refined::Waiting(Waiting::BoundSpent { limit, .. })) = refined else {
        unreachable!()
    };
    assert!(limit.message().starts_with("tile 8 needs "), "{limit}");
}

/// A refine whose own code-block work passes the whole bound waits on
/// `WorkLimit`, whatever the earlier refines spent, and carries the refusal
/// a decode of its bytes on its own gives, hint included: not one counted
/// against what the earlier refines left.
#[test]
fn a_refine_past_the_whole_bound_waits_with_the_one_shot_refusal() {
    let bytes = fixture("tier1_work_bound/t1bomb_1024_64_88.j2k");
    let Err(Error::Limit(expected)) =
        decode_with(&bytes, &DecodeOptions::default().with_strict(false))
    else {
        panic!("the bomb decodes");
    };
    assert_eq!(expected.kind(), LimitKind::CodeBlockWork);
    let (_, bound) = one_shot_bounds(&bytes);
    for spent in [0, 1, bound / 2, bound - 1, bound, u64::MAX / 2] {
        let mut decoder =
            IncrementalDecoder::with_code_block_work_spent(DecodeOptions::default(), spent)
                .unwrap();
        decoder.feed(&bytes);
        match decoder.refine() {
            Ok(Refined::Waiting(Waiting::WorkLimit { limit, .. })) => {
                assert_eq!(format!("{limit:?}"), format!("{expected:?}"), "{spent}");
            }
            other => panic!("{spent} spent: {other:?}"),
        }
    }
}

/// A refine refused for what earlier refines spent, whose count of the
/// later tiles passes the whole bound, waits on `BoundSpent`, whose message
/// names the tile the whole bound refuses: it did not decode the tiles it
/// counted, and a decode of its bytes on its own can fail in one of them.
/// On `incremental/segsym_wide_flipped.j2k` under a cap of 2^22, a decode
/// with nothing spent fails in tile 11, before tile 12 would pass the
/// bound. A refine with work spent is cut at an earlier tile and counts
/// through tile 11 to tile 12.
#[test]
fn a_count_past_the_whole_bound_waits_on_bound_spent() {
    let bytes = fixture("incremental/segsym_wide_flipped.j2k");
    let error = decode_with(&bytes, &DecodeOptions::default().with_strict(false)).unwrap_err();
    let bound = 1 << 22;
    let options = crate::fuzz::with_code_block_work_cap(DecodeOptions::default(), bound);
    let mut fresh = IncrementalDecoder::new(options.clone()).unwrap();
    fresh.feed(&bytes);
    assert_eq!(
        keyed(fresh.refine().map(Refined::image)),
        keyed(Err(error.clone()))
    );
    for spent in [1 << 20, 1 << 21, 3 << 20, 4_000_000] {
        let mut decoder =
            IncrementalDecoder::with_code_block_work_spent(options.clone(), spent).unwrap();
        decoder.feed(&bytes);
        let refined = decoder.refine();
        assert!(is_bound_spent(&refined), "{spent}: {refined:?}");
        let Ok(Refined::Waiting(Waiting::BoundSpent { limit, .. })) = refined else {
            unreachable!()
        };
        assert!(
            limit.message().starts_with("tile 12 needs "),
            "{spent}: {limit}"
        );
        // Only the tiles before the cut were decoded, and only they are
        // spent.
        let (blocks, _) = decoder.code_block_work();
        assert!(blocks < bound, "{spent}: {blocks}");
    }
}

/// The visits a one-shot walk of tile `index` of `bytes` spends, with
/// `strict` off and no bound.
fn one_shot_tile_visits(bytes: &[u8], index: u16) -> u64 {
    let parsed = codestream::parse_header(bytes).unwrap();
    let cs = codestream::parse_tiles_with(bytes, parsed, false).unwrap();
    let poc_bitmap = tier2::decode_visits(crate::pipeline::decode::tile_bytes(&cs));
    let position = cs
        .tiles
        .iter()
        .position(|t| t.index == u32::from(index))
        .unwrap();
    let header = cs.tile_header(position).unwrap();
    let meter = Meter::new(u64::MAX / 2);
    let _ =
        tier2::decode_packets_partial(&header, &cs.tiles[position], &meter, poc_bitmap, 0, None);
    meter.spent()
}

/// A file whose last tile's walk is too large to keep, and costs about
/// half the packet bound however few of its bytes are in: each refine
/// walks it from the start. Refined at 24 points, the walks of it over all
/// the refines spend no more than 5 packet bounds (4 the refines may
/// spend, and the walk that goes past them): the refines after that
/// return `Ok(None)`, though a one-shot decode of their bytes gives an
/// image, and `finish` still decodes the file. Without the charge, the 24
/// walks spend about 12 bounds (#416).
#[test]
fn the_walks_of_a_tile_not_kept_stay_within_a_few_bounds() {
    const HEAD: u16 = 2;
    let bytes = crate::synth::repeat_walk_tiles(HEAD, 16000, 4096);
    let options = DecodeOptions::default().with_strict(false);
    let (bound, _) = one_shot_bounds(&bytes);
    let whole = one_shot_tile_visits(&bytes, HEAD);
    assert!(2 * whole < bound && 3 * whole > bound, "{whole} of {bound}");
    // The refines start once the last tile-part's header is in.
    let header = 14 + bytes.windows(2).rposition(|w| w == [0xFF, 0x90]).unwrap();
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    let (mut walked, mut images, mut put_off) = (0, 0, 0);
    for step in 1..=24 {
        let end = header + (bytes.len() - header) * step / 24;
        decoder.feed(&bytes[decoder.bytes_fed()..end]);
        let refined = refine_now(&mut decoder).unwrap();
        let (kept, _) = decoder.spent.walks.kept();
        assert_eq!(
            kept, HEAD as usize,
            "{end}: only the first tiles' walks are kept"
        );
        if refined.is_some() {
            images += 1;
            walked += one_shot_tile_visits(&bytes[..end], HEAD);
        } else {
            assert!(decoder.refused, "{end}");
            put_off += 1;
        }
    }
    assert!(
        walked <= 5 * bound,
        "{images} images walked {walked} of {bound}"
    );
    // The ledger counts those walks, and the one refused after its walk,
    // and no walk after it.
    let repeats = decoder.spent.repeats;
    assert!(
        walked < repeats && repeats <= 5 * bound,
        "{walked} walked, {repeats} charged, of {bound}"
    );
    assert!(
        images > 0 && put_off > 0,
        "{images} images, {put_off} put off"
    );
    assert!(oracle(&bytes, &options).unwrap().is_some());
    assert!(decoder.finish().is_ok());
}

/// A tile whose later tile-parts each add a `POC` volume has its kept walk
/// started again at each, and the walk started again reads the tile from
/// the start. Fed 64 bytes at a time, the walks started again are charged
/// as walks of tiles not kept: the walks of the refines that give an image
/// spend at most 6 packet bounds (the first walk, which is not charged,
/// then the 4 bounds the refines may spend and the walk that goes past
/// them). Without the charge, it gives 12 images, whose walks spend 9.6
/// bounds.
#[test]
fn a_walk_started_again_is_charged_as_a_walk_not_kept() {
    let bytes = crate::synth::restarted_poc_tile(4, 13, 400);
    let options = DecodeOptions::default().with_strict(false);
    let (bound, _) = one_shot_bounds(&bytes);
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    let (mut walked, mut images, mut put_off) = (0, 0, 0);
    for end in every(64, bytes.len()) {
        decoder.feed(&bytes[decoder.bytes_fed()..end]);
        let tried = decoder.tried;
        match decoder.refine().unwrap() {
            Refined::Image(_) => {
                images += 1;
                walked += one_shot_tile_visits(&bytes[..end], 0);
            }
            // Only the charge refuses a try, and the refine names it, not
            // the packet bound: a decode of the same bytes on its own gives
            // an image.
            Refined::Waiting(Waiting::RepeatLimit { .. }) => {
                assert!(
                    decoder.tried != tried && decoder.refused_by_repeats,
                    "{end}"
                );
                // A decode of the bytes fed now is not held to that bound:
                // it is the way to an image meanwhile.
                let now = decode_with(
                    decoder.bytes(),
                    &decoder.options().clone().with_strict(false),
                );
                assert!(now.is_ok(), "{end}: {now:?}");
                put_off += 1;
            }
            Refined::Waiting(Waiting::MoreBytes { until, .. }) => {
                assert_eq!((decoder.tried, until), (tried, 2 * tried), "{end}");
            }
            Refined::Waiting(Waiting::HeaderIncomplete { .. }) => {
                assert!(!decoder.refused, "{end}")
            }
            other => panic!("{end}: {other:?}"),
        }
    }
    assert_eq!(decoder.spent.walks.kept(), (1, 0), "the walk is kept");
    assert!(
        walked <= 6 * bound,
        "{images} images walked {walked} of {bound}"
    );
    let repeats = decoder.spent.repeats;
    assert!(
        repeats > 0 && repeats <= 5 * bound,
        "{repeats} charged, of {bound}"
    );
    assert!(
        images > 0 && put_off > 0,
        "{images} images, {put_off} put off"
    );
    assert!(oracle(&bytes, &options).unwrap().is_some());
    assert!(decoder.finish().is_ok());
}

/// The bound on the walks of tiles not kept follows the packet work limit:
/// with no limit, or a limit ten times the default, every one of the 24
/// refines of `the_walks_of_a_tile_not_kept_stay_within_a_few_bounds`
/// gives an image, since their walks spend about 12 default bounds.
#[test]
fn the_bound_on_walks_not_kept_follows_the_packet_work_limit() {
    let bytes = crate::synth::repeat_walk_tiles(2, 16000, 4096);
    let header = 14 + bytes.windows(2).rposition(|w| w == [0xFF, 0x90]).unwrap();
    for limit in [WorkLimit::Unlimited, WorkLimit::Raised(10)] {
        let options = DecodeOptions::default()
            .with_strict(false)
            .with_packet_work_limit(limit.clone());
        let mut decoder = IncrementalDecoder::new(options).unwrap();
        let mut images = 0;
        for step in 1..=24 {
            let end = header + (bytes.len() - header) * step / 24;
            decoder.feed(&bytes[decoder.bytes_fed()..end]);
            if refine_now(&mut decoder).unwrap().is_some() {
                images += 1;
            }
        }
        assert_eq!(images, 24, "{limit:?}");
        assert!(!decoder.refused_by_repeats, "{limit:?}");
    }
}

/// A file made to reach the packet work bound, whose tiles' walks are too
/// large to keep, fed 256 bytes at a time: each refine that is due gives
/// an image where the one-shot decode of its bytes gives one. Its walks
/// spend about 2 bounds over all the refines, more than one and below the
/// 4 they may: held to one bound, the refine of 16384 bytes is refused.
#[test]
fn a_work_bound_file_fed_in_pieces_keeps_every_image() {
    let bytes = fixture("work_bound/t2skip_2048x1024_40000.j2k");
    let options = DecodeOptions::default();
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    let mut images = 0;
    for end in every(256, bytes.len()) {
        decoder.feed(&bytes[decoder.bytes_fed()..end]);
        let tried = decoder.tried;
        let refined = decoder.refine().unwrap().image();
        if decoder.tried == tried {
            continue;
        }
        let expected = oracle(&bytes[..end], &options).unwrap();
        assert_eq!(refined.is_some(), expected.is_some(), "{end} bytes");
        assert!(!decoder.refused_by_repeats, "{end} bytes");
        images += usize::from(refined.is_some());
    }
    let (_, walked) = decoder.spent.walks.kept();
    assert!(
        images > 1 && walked > 0,
        "{images} images, {walked} walked again"
    );
    let (bound, _) = one_shot_bounds(&bytes);
    assert!(
        decoder.spent.repeats > bound,
        "{} of {bound}",
        decoder.spent.repeats
    );
}

/// The packet visits a one-shot walk with `strict` off of every tile of
/// `bytes` spends under `reduction` and `layers`, with no bound.
fn one_shot_walk_visits(bytes: &[u8], reduction: u8, layers: Option<u16>) -> u64 {
    let parsed = codestream::parse_header(bytes).unwrap();
    let cs = codestream::parse_tiles_with(bytes, parsed, false).unwrap();
    let poc_bitmap = tier2::decode_visits(crate::pipeline::decode::tile_bytes(&cs));
    (0..cs.tiles.len())
        .map(|position| {
            let header = cs.tile_header(position).unwrap();
            let meter = Meter::new(u64::MAX / 2);
            let tile = &cs.tiles[position];
            let _ =
                tier2::decode_packets_partial(&header, tile, &meter, poc_bitmap, reduction, layers);
            meter.spent()
        })
        .sum()
}

/// Set the decoder's reduction, layer cap and region.
fn set_options(decoder: &mut IncrementalDecoder, options: &DecodeOptions) {
    decoder
        .set_resolution_reduction(options.resolution_reduction)
        .unwrap();
    decoder
        .set_max_quality_layers(options.max_quality_layers)
        .unwrap();
    decoder.set_region(options.region.clone()).unwrap();
}

/// A refine under options changed since the last one is the one-shot
/// strict-off decode of its bytes under the new options. Before each
/// refine the reduction, the layer cap or both change, and the region
/// moves between the whole image, tile 0 and the top-left quarter of the
/// image, on files with several tiles and tile-parts, `TLM` and `PLT`, a
/// `POC` volume in a later tile-part, and the MIXED HT files.
#[test]
fn options_changed_between_refines_refine_to_the_one_shot_decode_of_their_prefix() {
    let names: Vec<&str> = [
        "tiles_multipart_lossless.j2k",
        "layers/precincts_pterm.j2k",
        "multilayer_multicomponent_lossless.j2k",
        "precincts_tiled_cprl_lossless.j2k",
        "tile_header/tlm_32_16k.j2k",
        "jp2_real/byte_tlm_plt.jp2",
        "poc_precedence/poc_main_then_later_tile_part.j2k",
        // The conformance corpus and `jp2_real` are left out of the
        // published package.
        "conformance/ht/codestreams/ds0_hm_15_b8.j2k",
    ]
    .into();
    for name in names {
        if crate::skip::excluded_path(name) {
            continue;
        }
        let bytes = fixture(name);
        let stride = if bytes.len() > 8192 { 1009 } else { 61 };
        let mut decoder = IncrementalDecoder::new(uncut(DecodeOptions::default())).unwrap();
        let (mut images, mut errors) = (0, 0);
        for (step, end) in every(stride, bytes.len()).into_iter().enumerate() {
            decoder.feed(&bytes[decoder.bytes_fed()..end]);
            // Each step changes the reduction or the layer cap, so an error
            // of the step before does not carry over.
            let quarter = decoder.header().ok().flatten().map_or(Region::Whole, |h| {
                let a = h.image_area;
                Region::Window(crate::Window::new(
                    a.x0,
                    a.y0,
                    a.x0 + a.x1.div_ceil(2) - a.x0 / 2,
                    a.y0 + a.y1.div_ceil(2) - a.y0 / 2,
                ))
            });
            let options = DecodeOptions::default();
            let options = match step % 6 {
                0 => options,
                1 => options.with_resolution_reduction(1),
                2 => options.with_max_quality_layers(Some(1)),
                3 => options
                    .with_resolution_reduction(1)
                    .with_region(Region::Tile(0)),
                4 => options.with_region(quarter),
                _ => options
                    .with_resolution_reduction(1)
                    .with_max_quality_layers(Some(1))
                    .with_region(quarter),
            };
            set_options(&mut decoder, &options);
            let refined = keyed(refine_now(&mut decoder));
            assert_eq!(
                refined,
                keyed(oracle(&bytes[..end], &uncut(options))),
                "{name}, {end} bytes, step {step}"
            );
            match refined {
                Ok(Some(_)) => images += 1,
                Ok(None) => {}
                Err(_) => errors += 1,
            }
        }
        assert!(images > 0, "{name}: {images} images, {errors} errors");
    }
}

/// A file of 3 x 2 tiles whose walks fit the room of a decode of its top
/// row as well as of the whole image, so a change of region keeps them all.
const SIX_TILES: &str = "precincts_tiled_cprl_lossless.j2k";

/// A change of region reuses the walks kept: the tiles it reads go on from
/// where their walks stopped, and the walks of the tiles it does not read
/// stay until the region comes back to them. Fed in halves, with a change
/// to the top row of tiles between them and back to the whole image after,
/// each packet is read once, as by one walk of the file, and nothing is
/// charged as a walk read again.
#[test]
fn a_change_of_region_keeps_the_walks() {
    let bytes = fixture(SIX_TILES);
    let whole = DecodeOptions::default();
    let top = whole
        .clone()
        .with_region(Region::Window(crate::Window::new(0, 0, 128, 48)));
    let mut decoder = IncrementalDecoder::new(whole.clone()).unwrap();
    let half = bytes.len() / 2;
    decoder.feed(&bytes[..half]);
    assert_eq!(
        refine_now(&mut decoder).unwrap(),
        oracle(&bytes[..half], &whole).unwrap()
    );
    // The data of tiles 4 and 5 is not in yet.
    assert_eq!(decoder.spent.walks.kept(), (4, 0));
    decoder.set_region(top.region.clone()).unwrap();
    decoder.feed(&bytes[half..]);
    assert_eq!(
        refine_now(&mut decoder).unwrap(),
        oracle(&bytes, &top).unwrap()
    );
    // Tile 3's walk stays, where the first refine left it.
    assert_eq!(decoder.spent.walks.kept(), (4, 0));
    decoder.set_region(Region::Whole).unwrap();
    assert_eq!(
        refine_now(&mut decoder).unwrap(),
        oracle(&bytes, &whole).unwrap()
    );
    assert_eq!(decoder.spent.repeats, 0, "a kept walk was charged");
    let mut once = IncrementalDecoder::new(whole).unwrap();
    once.feed(&bytes);
    assert!(once.refine().unwrap().image().is_some());
    assert_eq!(
        decoder.spent.walks.visits(),
        once.spent.walks.visits(),
        "the walks read packets again"
    );
}

/// A region whose image leaves the walks kept past the room lets go of the
/// walks of the tiles it does not read, from the first, until the rest
/// fit, and keeps the walk of the tile it reads. A later walk of a tile let
/// go is charged as a walk read again.
///
/// The region is one sample of tile 0, so the room is about twice the
/// largest walk. The six walks hold more than that on a target of any
/// pointer width, so several are let go; how many follows the walks' size
/// in bytes, which the test reads from the decoder.
#[test]
fn a_smaller_region_lets_go_of_the_walks_of_the_tiles_it_does_not_read() {
    let bytes = fixture(SIX_TILES);
    let whole = DecodeOptions::default();
    let mut decoder = IncrementalDecoder::new(whole.clone()).unwrap();
    decoder.feed(&bytes);
    assert!(refine_now(&mut decoder).unwrap().is_some());
    let visits = decoder.spent.walks.visits();
    let held = decoder.spent.walks.kept_bytes();
    let largest = decoder.spent.walks.largest();
    assert!(
        held > 3 * largest,
        "{held} bytes in walks of at most {largest}"
    );
    let sample = Region::Window(crate::Window::new(0, 0, 1, 1));
    decoder.set_region(sample.clone()).unwrap();
    assert_eq!(
        refine_now(&mut decoder).unwrap(),
        oracle(&bytes, &whole.clone().with_region(sample)).unwrap()
    );
    // The tiles let go, from the first; tile 0's walk goes on, as no walk
    // was read again.
    let (kept, unkept) = decoder.spent.walks.kept();
    assert_eq!(unkept, 0);
    let gone: Vec<u16> = (1..).take(6 - kept).collect();
    assert!(gone.len() >= 2, "only {gone:?} let go");
    assert!(decoder.spent.walks.kept_bytes() < held);
    assert_eq!(decoder.spent.repeats, 0);
    assert_eq!(decoder.spent.walks.visits(), visits);
    decoder.set_region(Region::Whole).unwrap();
    assert_eq!(
        refine_now(&mut decoder).unwrap(),
        oracle(&bytes, &whole).unwrap()
    );
    assert_eq!(decoder.spent.walks.kept(), (6, 0));
    assert_eq!(
        decoder.spent.repeats,
        gone.iter()
            .map(|&tile| one_shot_tile_visits(&bytes, tile))
            .sum::<u64>()
    );
}

/// A change of the layer cap lets go of every walk, and the next refine
/// builds them again, charged as walks read again: by what a one-shot walk
/// of the tiles under the new options spends. So does a change of both the
/// layer cap and the reduction. A change back before the next refine, or a
/// set to the value the decoder has, keeps them.
#[test]
fn a_change_of_layers_walks_the_tiles_again_and_is_charged() {
    let bytes = fixture(SIX_TILES);
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes);
    assert!(refine_now(&mut decoder).unwrap().is_some());
    assert_eq!(decoder.spent.repeats, 0);
    let mut charged = 0;
    for (reduction, layers) in [(1, Some(1)), (2, None), (0, Some(1)), (0, None)] {
        let options = DecodeOptions::default()
            .with_resolution_reduction(reduction)
            .with_max_quality_layers(layers);
        set_options(&mut decoder, &options);
        assert_eq!(
            refine_now(&mut decoder).unwrap(),
            oracle(&bytes, &options).unwrap()
        );
        charged += one_shot_walk_visits(&bytes, reduction, layers);
        assert!(charged > 0);
        assert_eq!(decoder.spent.repeats, charged, "{reduction} {layers:?}");
        // The room is that of the new options: the largest walk, which the
        // room follows, and the walks kept are those a decoder made with
        // them has. A layer change forgets every walk, and with them the
        // largest walk of the old options.
        let mut fresh = IncrementalDecoder::new(options).unwrap();
        fresh.feed(&bytes);
        assert!(fresh.refine().unwrap().image().is_some());
        assert_eq!(
            decoder.spent.walks.largest(),
            fresh.spent.walks.largest(),
            "{reduction} {layers:?}"
        );
        let kept = fresh.spent.walks.kept();
        assert_eq!(decoder.spent.walks.kept(), kept, "{reduction} {layers:?}");
        assert_eq!(
            decoder.spent.walks.kept_bytes(),
            fresh.spent.walks.kept_bytes()
        );
    }
    // There and back between two refines, and the value it has: no walk is
    // let go.
    decoder.set_max_quality_layers(Some(1)).unwrap();
    decoder.set_max_quality_layers(None).unwrap();
    decoder.set_resolution_reduction(1).unwrap();
    decoder.set_resolution_reduction(0).unwrap();
    let options = DecodeOptions::default();
    assert_eq!(
        refine_now(&mut decoder).unwrap(),
        oracle(&bytes, &options).unwrap()
    );
    assert_eq!(decoder.spent.repeats, charged);
}

/// A 64 x 64 codestream of two 8-bit components, the second subsampled by
/// 2, so each holds other code-blocks in the resolutions a reduction drops:
/// 5/3, LRCP, one layer, two wavelet levels, 4x4 code-blocks, and an empty
/// packet for each of its six packets.
fn two_sizes() -> Vec<u8> {
    use crate::synth::{cod_body, qcd_none_body, seg, siz_body, tile_part};
    let siz = siz_body([64, 64, 0, 0, 64, 64, 0, 0], &[[7, 1, 1], [7, 2, 2]]);
    [
        vec![0xFF, 0x4F],
        seg(0xFF51, &siz),
        seg(0xFF52, &cod_body(1, 2, 0, 0)),
        seg(0xFF5C, &qcd_none_body(2, &[8; 7])),
        tile_part(0, 0, 1, &[], &[0; 6]),
        vec![0xFF, 0xD9],
    ]
    .concat()
}

/// A change of the reduction alone keeps every walk: the next refine goes
/// on from them, charged nothing as walks read again, and gives the
/// one-shot decode under the new reduction. Each walk kept then charges the
/// code-blocks the new reduction drops, as a walk built for it does. Fed in
/// halves, with the reduction changed before each refine, every packet is
/// read once, as by one walk of the file.
///
/// The room for the walks kept is that of the decode's image, which a
/// larger reduction makes smaller, so on a file of several small tiles a
/// larger reduction can let walks go as a smaller region does (see
/// `a_smaller_region_lets_go_of_the_walks_of_the_tiles_it_does_not_read`).
/// Here the six tiles go to a smaller reduction, and the one-tile files,
/// whose one walk always fits, go back and forth.
#[test]
fn a_change_of_reduction_keeps_the_walks() {
    for (name, bytes, reductions) in [
        (SIX_TILES, fixture(SIX_TILES), &[1, 0][..]),
        ("two_sizes", two_sizes(), &[1, 0, 2, 0, 1][..]),
        (
            "multilayer_multicomponent_lossless.j2k",
            fixture("multilayer_multicomponent_lossless.j2k"),
            &[2, 0, 1][..],
        ),
    ] {
        let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
        // The first refine has half the file, or for `two_sizes`, whose
        // header is most of it, all but its last two packets.
        let first_end = if name == "two_sizes" {
            bytes.len() - 4
        } else {
            bytes.len() / 2
        };
        let ends = [first_end, bytes.len()];
        // Per tile, the code-blocks dropped at the walk's first decode.
        let mut first: Vec<u64> = Vec::new();
        for (step, &reduction) in reductions.iter().enumerate() {
            let end = ends[step.min(1)];
            decoder.feed(&bytes[decoder.bytes_fed()..end]);
            let options = DecodeOptions::default().with_resolution_reduction(reduction);
            set_options(&mut decoder, &options);
            assert_eq!(
                refine_now(&mut decoder).unwrap(),
                oracle(&bytes[..end], &options).unwrap(),
                "{name}, step {step}"
            );
            assert_eq!(decoder.spent.repeats, 0, "{name}, step {step}");
            assert_eq!(decoder.spent.walks.kept().1, 0, "{name}, step {step}");
            // What each walk kept charges for the resolutions dropped is
            // what a walk built for this reduction charges.
            let mut fresh = IncrementalDecoder::new(options).unwrap();
            fresh.feed(&bytes[..end]);
            assert!(fresh.refine().unwrap().image().is_some());
            let dropped = decoder.spent.walks.dropped();
            assert_eq!(dropped, fresh.spent.walks.dropped(), "{name}, step {step}");
            if reduction > 0 {
                assert!(
                    dropped.iter().flatten().flatten().any(|&d| d > 0),
                    "{name}, step {step}"
                );
            }
            if name == "two_sizes" {
                let blocks = |side: u64| {
                    (1..=u64::from(reduction))
                        .map(|l| 3 * (side >> l).pow(2) / 16)
                        .sum()
                };
                assert_eq!(
                    dropped,
                    [Some(vec![blocks(64), blocks(32)])],
                    "{name}, step {step}"
                );
            }
            for walk in dropped.iter().skip(first.len()).flatten() {
                first.push(walk.iter().sum());
            }
        }
        // A walk's visits count each packet once, and the code-blocks
        // dropped at its first decode. A walk of the file at no reduction,
        // which drops none, reads every packet once.
        let mut once = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
        once.feed(&bytes);
        assert!(once.refine().unwrap().image().is_some());
        assert_eq!(once.spent.walks.kept().1, 0, "{name}");
        assert_eq!(
            decoder.spent.walks.visits(),
            once.spent.walks.visits() + first.iter().sum::<u64>(),
            "{name}: the walks read packets again"
        );
    }
}

/// Changing the layer cap back and forth on every refine walks every tile
/// again each time, and those walks stay within the bound on walks read
/// again: once they pass 4 packet bounds, a refine returns `Ok(None)`
/// though a one-shot decode of its bytes gives an image, and `finish`
/// still decodes the file. The walks of this file spend a small share of a
/// bound, so the decoder starts with most of the 4 bounds spent, as by
/// earlier refines.
#[test]
fn options_changed_back_and_forth_stay_within_the_bound_on_walks_read_again() {
    let bytes = fixture(SIX_TILES);
    let (bound, _) = one_shot_bounds(&bytes);
    let walk = one_shot_walk_visits(&bytes, 0, None);
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.spent.repeats = REPEAT_BOUNDS * bound - 3 * walk;
    decoder.feed(&bytes);
    assert!(refine_now(&mut decoder).unwrap().is_some());
    let (mut images, mut put_off) = (0, 0);
    for step in 0..8 {
        let layers = if step % 2 == 0 { Some(1) } else { None };
        let options = DecodeOptions::default().with_max_quality_layers(layers);
        set_options(&mut decoder, &options);
        match refine_now(&mut decoder).unwrap() {
            Some(image) => {
                assert_eq!(image, oracle(&bytes, &options).unwrap().unwrap(), "{step}");
                images += 1;
            }
            None => {
                assert!(decoder.refused_by_repeats, "{step}");
                assert!(oracle(&bytes, &options).unwrap().is_some());
                put_off += 1;
            }
        }
    }
    assert!(
        images > 0 && put_off > 0,
        "{images} images, {put_off} put off"
    );
    assert!(
        decoder.spent.repeats <= (REPEAT_BOUNDS + 1) * bound,
        "{} of {bound}",
        decoder.spent.repeats
    );
    let options = decoder.options.clone();
    assert_eq!(
        decoder.finish().unwrap(),
        decode_with(&bytes, &options).unwrap()
    );
}

/// What `header` must give on `prefix`: a header read from the start of
/// those bytes, with nothing kept, and a main header cut short as `None`.
fn fresh_header(prefix: &[u8], options: &DecodeOptions) -> String {
    let lenient = options.clone().with_strict(false);
    let read = match crate::pipeline::decode::read_header_streamed(prefix, &lenient, None) {
        Ok(header) => Ok(Some(header)),
        Err(Error::Incomplete(_)) => Ok(None),
        Err(error) => Err(error),
    };
    format!("{read:?}")
}

/// The main-header bytes and the top-level box bytes the reads on this
/// thread have walked since the last call.
fn walked() -> u64 {
    let main = codestream::tests::MAIN_HEADER_BYTES.with(|walked| walked.replace(0));
    let boxes = crate::jp2::tests::BOX_BYTES.with(|walked| walked.replace(0));
    main + boxes
}

/// A codestream of one 64 x 64 tile whose main header holds `segments`
/// `PPM` markers of about 4 KiB each, then a tile-part with no data.
fn ppm_codestream(segments: u8) -> Vec<u8> {
    use crate::synth;
    let siz = synth::siz_body([64, 64, 0, 0, 64, 64, 0, 0], &[[7, 1, 1]]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0)),
        synth::seg(0xFF5C, &synth::qcd_none_body(0, &[8])),
    ]
    .concat();
    for index in 0..segments {
        // `Zppm`, then whole records of an `Nppm` of 0.
        let mut body = vec![index];
        body.resize(1 + 4 * 1024, 0);
        bytes.extend(synth::seg(0xFF60, &body));
    }
    bytes.extend(synth::tile_part(0, 0, 1, &[], &[]));
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// A JP2 file of `codestream` with `padding` boxes of 8 bytes, and an
/// `xml ` box after each 1000 of them, between its `jp2h` and its `jp2c`.
fn padded_jp2(codestream: &[u8], padding: usize) -> Vec<u8> {
    use crate::synth;
    let between: Vec<Vec<u8>> = (0..padding)
        .map(|index| {
            if index % 1000 == 999 {
                synth::bx(b"xml ", b"<a/>")
            } else {
                synth::bx(b"free", &[])
            }
        })
        .collect();
    synth::jp2_file(
        &[
            synth::ihdr_for(codestream),
            synth::bx(b"colr", &synth::colr_enum(17)),
        ],
        &between,
        codestream,
        &[],
    )
}

/// Feed `bytes` in pieces of `piece` bytes, calling `header` after each,
/// with each call checked against a read from the start of the same bytes.
/// Returns the bytes the calls walked in all.
fn header_walk_after_each_feed(bytes: &[u8], piece: usize) -> u64 {
    let options = DecodeOptions::default();
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    walked();
    let mut spent = 0;
    for chunk in bytes.chunks(piece) {
        decoder.feed(chunk);
        let got = format!("{:?}", decoder.header());
        spent += walked();
        let fed = decoder.bytes_fed();
        assert_eq!(
            got,
            fresh_header(decoder.bytes(), &options),
            "at {fed} bytes"
        );
        walked();
    }
    assert!(matches!(decoder.header(), Ok(Some(_))));
    spent
}

/// A main header of 256 KiB of `PPM` data, fed 256 bytes at a time with a
/// `header` call after each: each call goes on from where the last one
/// stopped, so the calls walk the header about once in all.
#[test]
fn header_calls_on_a_growing_ppm_main_header_walk_it_about_once() {
    let bytes = ppm_codestream(64);
    let spent = header_walk_after_each_feed(&bytes, 256);
    let len = bytes.len() as u64;
    println!("{len} bytes, {spent} walked");
    assert!(spent <= 3 * len, "{spent} bytes walked for {len}");
}

/// The same with a main header of 64 KiB in a JP2 file, after 8000 boxes
/// of 8 bytes before its `jp2c`, fed 64 bytes at a time: the calls walk the
/// boxes before the codestream and its main header about once each in all.
#[test]
fn header_calls_on_growing_boxes_and_main_header_walk_them_about_once() {
    let bytes = padded_jp2(&ppm_codestream(16), 8000);
    let spent = header_walk_after_each_feed(&bytes, 64);
    let len = bytes.len() as u64;
    println!("{len} bytes, {spent} walked");
    assert!(spent <= 4 * len, "{spent} bytes walked for {len}");
}

/// Where the first `SOT` marker after the first `SOC` is in `bytes`: about
/// the end of the main header.
fn first_sot(bytes: &[u8]) -> Option<usize> {
    let soc = bytes.windows(2).position(|code| code == [0xFF, 0x4F])?;
    bytes[soc..]
        .windows(2)
        .position(|code| code == [0xFF, 0x90])
        .map(|at| soc + at)
}

/// Every codestream, JP2 and JPH file under `tests/fixtures`: `header`
/// after each feed that doubles the bytes fed, and after each byte fed
/// around the end of the main header and the start of the `jp2c` box, gives
/// what a read from the start of the same bytes gives, with a reduction set
/// and cleared between the calls.
#[test]
fn header_after_doubling_and_byte_feeds_is_a_read_from_the_start() {
    let root = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let mut dirs = vec![std::path::PathBuf::from(root)];
    let mut files = Vec::new();
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e, "j2k" | "j2c" | "jp2" | "jph"))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(files.len() > 100, "{} files", files.len());
    let reduced = DecodeOptions::default().with_resolution_reduction(1);
    let whole_options = DecodeOptions::default();
    for path in &files {
        let name = path.display().to_string();
        let whole = std::fs::read(path).unwrap();
        // Each read from the start costs its prefix, so a large file is fed
        // only its first 2 MiB.
        let bytes = &whole[..whole.len().min(2 << 20)];
        let mut points = vec![];
        let mut at = 1;
        while at < bytes.len() {
            points.push(at);
            at *= 2;
        }
        points.push(bytes.len());
        for near in [
            first_sot(bytes),
            bytes.windows(4).position(|kind| kind == b"jp2c"),
        ]
        .into_iter()
        .flatten()
        {
            points.extend(near.saturating_sub(40).max(1)..(near + 40).min(bytes.len()));
        }
        points.sort_unstable();
        points.dedup();
        let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
        let mut fed = 0;
        for (index, &point) in points.iter().enumerate() {
            decoder.feed(&bytes[fed..point]);
            fed = point;
            // The reduction is set for 16 calls, then cleared for 16.
            let options = if index % 32 >= 16 {
                &reduced
            } else {
                &whole_options
            };
            decoder
                .set_resolution_reduction(options.resolution_reduction)
                .unwrap();
            assert_eq!(
                format!("{:?}", decoder.header()),
                fresh_header(&bytes[..point], options),
                "{name} at {point} bytes"
            );
        }
    }
}

/// Feed `bytes` in pieces of `piece` bytes, calling `header` after each,
/// and check each call against a read from the start of the same bytes.
fn header_matches_after_each_feed(name: &str, bytes: &[u8], piece: usize) {
    let options = DecodeOptions::default();
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    for chunk in bytes.chunks(piece) {
        decoder.feed(chunk);
        assert_eq!(
            format!("{:?}", decoder.header()),
            fresh_header(decoder.bytes(), &options),
            "{name} at {} bytes",
            decoder.bytes_fed()
        );
    }
}

/// A fault that comes in after the calls have found the main header cut
/// short is the error a read from the start gives, as soon as its bytes are
/// in: a duplicate `SIZ`, a segment length below 2 or a Part 2 marker in
/// the middle of a `PPM` header (whose refusal waits for the header to end),
/// and in a JP2 file a `jp2h` with no `ihdr`, or a box shorter than its
/// header, among the boxes before the codestream.
#[test]
fn a_fault_after_a_cut_main_header_is_given_once_its_bytes_are_in() {
    use crate::synth;
    let header = ppm_codestream(16);
    // After the 8th `PPM` segment.
    let at = header
        .windows(2)
        .enumerate()
        .filter(|(_, code)| *code == [0xFF, 0x60])
        .nth(8)
        .map(|(at, _)| at)
        .unwrap();
    let siz = synth::siz_body([64, 64, 0, 0, 64, 64, 0, 0], &[[7, 1, 1]]);
    for (what, insert) in [
        ("a duplicate SIZ", synth::seg(0xFF51, &siz)),
        ("a length below 2", vec![0xFF, 0x64, 0x00, 0x01]),
        ("a Part 2 marker", synth::seg(0xFF74, &[0, 0])),
        ("a COM", synth::seg(0xFF64, &[0, 1, b'x'])),
    ] {
        let mut bytes = header.clone();
        bytes.splice(at..at, insert);
        for piece in [1, 97, 1000] {
            header_matches_after_each_feed(what, &bytes[..bytes.len().min(40_000)], piece);
        }
        header_matches_after_each_feed(what, &bytes, 1000);
        let jp2 = padded_jp2(&bytes, 300);
        header_matches_after_each_feed(what, &jp2, 61);
    }
    let codestream = ppm_codestream(1);
    let plain = padded_jp2(&codestream, 300);
    // After the 150th box of padding.
    let at = plain
        .windows(4)
        .enumerate()
        .filter(|(_, kind)| *kind == b"free")
        .nth(150)
        .map(|(at, _)| at - 4)
        .unwrap();
    for (what, insert) in [
        (
            "a jp2h with no ihdr",
            synth::bx(b"jp2h", &synth::bx(b"colr", &synth::colr_enum(17))),
        ),
        (
            "a second jp2h",
            synth::bx(b"jp2h", &synth::ihdr_for(&codestream)),
        ),
        (
            "a box of length 4",
            vec![0, 0, 0, 4, b'f', b'r', b'e', b'e'],
        ),
        (
            "a box of length 0",
            vec![0, 0, 0, 0, b'f', b'r', b'e', b'e'],
        ),
        (
            "a jp2h of length 0",
            [&[0u8, 0, 0, 0][..], b"jp2h", &synth::ihdr_for(&codestream)].concat(),
        ),
    ] {
        let mut bytes = plain.clone();
        bytes.splice(at..at, insert);
        for piece in [1, 7, 64] {
            header_matches_after_each_feed(what, &bytes, piece);
        }
    }
}

/// Feed `bytes` 509 bytes at a time, calling `header` after each, with
/// `before` run on the decoder first and `hold` saying whether to hold the
/// lock on what `header` keeps through the call: each call gives what a
/// read from the start gives.
fn header_around_the_cache(
    hold: impl Fn(usize) -> bool,
    before: impl Fn(&mut IncrementalDecoder, usize),
) {
    let bytes = padded_jp2(&ppm_codestream(16), 2000);
    let options = DecodeOptions::default();
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    for (index, chunk) in bytes.chunks(509).enumerate() {
        decoder.feed(chunk);
        before(&mut decoder, index);
        let held = hold(index).then(|| {
            decoder
                .header_kept
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        });
        let got = format!("{:?}", decoder.header());
        drop(held);
        assert_eq!(
            got,
            fresh_header(decoder.bytes(), &options),
            "at {} bytes",
            decoder.bytes_fed()
        );
    }
}

/// A call while another holds what `header` keeps reads from the start,
/// and the calls after it go on as before.
#[test]
fn header_while_the_cache_is_held_is_a_read_from_the_start() {
    header_around_the_cache(|index| index % 5 == 1, |_, _| {});
}

/// A call after one that panicked holding what `header` keeps reads from
/// the start, and the calls after it go on as before.
#[test]
#[cfg_attr(
    target_os = "wasi",
    ignore = "a panic aborts on wasm32-wasip1, so `catch_unwind` cannot catch it"
)]
fn header_after_a_poisoned_cache_is_a_read_from_the_start() {
    header_around_the_cache(
        |index| index % 5 == 1,
        |decoder, index| {
            if index % 7 == 3 {
                let poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _kept = decoder.header_kept.lock().unwrap();
                    panic!("poison the cache");
                }));
                assert!(poison.is_err());
                assert!(decoder.header_kept.is_poisoned());
            }
        },
    );
}

/// A `jp2c` box whose length ends it inside its main header, then boxes
/// after it, whole or at fault: each call gives what a read from the start
/// gives, as the boxes after the `jp2c` come in.
#[test]
fn header_on_a_jp2c_box_ending_inside_its_main_header_is_a_read_from_the_start() {
    use crate::synth;
    let whole = ppm_codestream(4);
    let codestream = &whole[..9000];
    for (what, after) in [
        ("nothing", vec![]),
        ("an xml box", vec![synth::bx(b"xml ", b"<a/>")]),
        (
            "a box of length 4",
            vec![vec![0, 0, 0, 4, b'f', b'r', b'e', b'e']],
        ),
        (
            "a box of length 0",
            vec![vec![0, 0, 0, 0, b'f', b'r', b'e', b'e']],
        ),
        ("a jp2h", vec![synth::bx(b"jp2h", &synth::ihdr_for(&whole))]),
        ("a second jp2c", vec![synth::bx(b"jp2c", &whole)]),
    ] {
        let bytes = synth::jp2_file(
            &[
                synth::ihdr_for(&whole),
                synth::bx(b"colr", &synth::colr_enum(17)),
            ],
            &[],
            codestream,
            &after,
        );
        for piece in [1, 13, 500] {
            header_matches_after_each_feed(what, &bytes, piece);
        }
    }
}

/// A `jp2c` box with no `jp2h` before it, cut inside its 8-byte extended
/// length: once its type is in, each call gives the "no `jp2h`" error a
/// read from the start gives, not a wait for more bytes.
#[test]
fn header_on_a_jp2c_with_no_jp2h_cut_in_its_extended_length_is_an_error() {
    use crate::synth;
    let mut bytes = synth::JP2_SIGNATURE.to_vec();
    bytes.extend(synth::ftyp_jp2());
    // `LBox` 1: the length is in the 8-byte `XLBox` after the type.
    bytes.extend([0, 0, 0, 1]);
    bytes.extend(b"jp2c");
    bytes.extend([0, 0, 0, 0, 0, 0]);
    let fresh = fresh_header(&bytes, &DecodeOptions::default());
    assert!(fresh.starts_with("Err(FileFormat("), "{fresh}");
    for piece in [1, 4] {
        header_matches_after_each_feed("a cut XLBox", &bytes, piece);
    }
}

/// A `jp2c` box that ends inside its own main header, between two marker
/// segments, then 200 boxes of 8 bytes, fed a box at a time. The bytes after
/// the box do not go on with a marker segment, so the kept parse of the
/// main header is dropped, and each call reads the boxes and the main
/// header from the start. This pins the cost as it is; an issue tracks
/// going on from the kept parse here too.
#[test]
fn header_calls_after_a_jp2c_box_ending_inside_its_main_header_read_it_each_time() {
    use crate::synth;
    let whole = ppm_codestream(16);
    // Before the 9th `PPM` segment.
    let cut = whole
        .windows(2)
        .position(|code| code == [0xFF, 0x60])
        .unwrap()
        + 8 * (4 + 4097);
    assert_eq!(whole[cut..cut + 2], [0xFF, 0x60]);
    let codestream = &whole[..cut];
    let after: Vec<Vec<u8>> = (0..200).map(|_| synth::bx(b"free", &[])).collect();
    let bytes = synth::jp2_file(
        &[
            synth::ihdr_for(&whole),
            synth::bx(b"colr", &synth::colr_enum(17)),
        ],
        &[],
        codestream,
        &after,
    );
    let options = DecodeOptions::default();
    let box_end = bytes.len() - 8 * after.len();
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    decoder.feed(&bytes[..box_end]);
    assert_eq!(format!("{:?}", decoder.header()), "Ok(None)");
    walked();
    for chunk in bytes[box_end..].chunks(8) {
        decoder.feed(chunk);
        let got = format!("{:?}", decoder.header());
        let spent = walked();
        assert_eq!(got, fresh_header(decoder.bytes(), &options));
        walked();
        let len = codestream.len() as u64;
        assert!(spent >= len, "{spent} bytes walked for a {len}-byte jp2c");
    }
}
