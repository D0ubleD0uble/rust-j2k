//! The incremental decoder's bounds and its kept packet walks, against
//! one-shot decodes of the same bytes.

use super::*;
use crate::{WorkLimit, codestream, tier1, tier2};

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
    let tile_bytes = crate::tile_bytes(&cs);
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
            if m.contains("packet_work_limit") || m.contains("code_block_work_limit") =>
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

/// Refine now, whatever the cadence says.
fn refine_now(decoder: &mut IncrementalDecoder) -> Result<Option<Image>> {
    decoder.tried = 0;
    decoder.refine()
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
        let refined = refine_now(&mut decoder);
        assert_eq!(
            refined,
            oracle(&bytes[..end], options),
            "{name}, {end} bytes after {before}"
        );
        match refined {
            Ok(Some(_)) => images += 1,
            Ok(None) => {}
            Err(error) => {
                decoder.feed(&bytes[end..]);
                assert_eq!(refine_now(&mut decoder), Err(error), "{name}, poisoned");
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
    let dir = format!(
        "{}/tests/fixtures/conformance/ht/codestreams",
        env!("CARGO_MANIFEST_DIR")
    );
    let Ok(entries) = std::fs::read_dir(&dir) else {
        // The conformance corpus is left out of the published package.
        return;
    };
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
    let path = format!(
        "{}/tests/fixtures/conformance/codestreams/p1_05.j2k",
        env!("CARGO_MANIFEST_DIR")
    );
    let Ok(bytes) = std::fs::read(&path) else {
        // The conformance corpus is left out of the published package.
        return;
    };
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
/// missing tiles cost more than the packet bound waits, though no packet
/// is read, as the one-shot decode of the same bytes is refused.
#[test]
fn a_missing_tile_charge_past_the_bound_waits() {
    let bytes = missing_tiles_past_the_bound();
    let lenient = DecodeOptions::default().with_strict(false);
    match decode_with(&bytes, &lenient) {
        Err(Error::Limit(message)) => assert!(message.contains("packet_work_limit"), "{message}"),
        other => panic!("{other:?}"),
    }
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes);
    assert_eq!(decoder.refine(), Ok(None));
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
    assert_eq!(decoder.refine(), Ok(None));
    assert!(decoder.refused);
    assert!(matches!(
        decode_with(&bytes[..tried], &lenient),
        Err(Error::Limit(m)) if m.contains("packet_work_limit")
    ));
    decoder.feed(&bytes[tried..between]);
    assert!(decode_with(&bytes[..between], &lenient).is_ok());
    assert_eq!(decoder.refine(), Ok(None));
    decoder.feed(&bytes[between..2 * tried - 1]);
    assert_eq!(decoder.refine(), Ok(None));
    assert_eq!(decoder.bytes_decoded(), 0);
    decoder.feed(&bytes[2 * tried - 1..2 * tried]);
    // The image is the one-shot decode of its bytes, as every refine's is
    // (the tests above); decoding it again here would double the time.
    assert!(
        decoder.refine().unwrap().is_some(),
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
            refine_now(&mut decoder),
            oracle(&bytes[..end], &options),
            "{end}"
        );
    }
    let (kept, walked) = decoder.spent.walks.kept();
    assert!(kept > 0 && walked > 0, "{kept} kept, {walked} walked again");
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
    assert_eq!(decoder.refine(), Err(error.clone()));
    for _ in 0..3 {
        decoder.feed(&[0; 64]);
        assert_eq!(refine_now(&mut decoder), Err(error.clone()));
    }
    let more = [&bytes[..], &[0; 192]].concat();
    assert_eq!(
        decoder.finish(),
        decode_with(&more, &DecodeOptions::default())
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
        crate::tests::BLOCK_WORK.with(|spent| spent.set(0));
        for byte in &bytes {
            decoder.feed(std::slice::from_ref(byte));
            if decoder.refine().unwrap().is_some() {
                images += 1;
            }
        }
        let block_work = crate::tests::BLOCK_WORK.with(Cell::get);
        let (packet_bound, block_bound) = one_shot_bounds(&bytes);
        let packets = decoder.spent.walks.visits();
        let blocks = decoder.spent.blocks;
        assert!(packets > 0 && blocks > 0, "{name}: nothing was decoded");
        assert_eq!(blocks, block_work, "{name}: the ledger missed block work");
        // One refine of the whole file walks it once.
        let mut once = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
        once.feed(&bytes);
        assert!(once.refine().unwrap().is_some(), "{name}");
        let one_walk = once.spent.walks.visits();
        assert_eq!(packets, one_walk, "{name}: the walks read packets again");
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
    assert_eq!(decoder.refine(), oracle(&bytes[..half], &options));
    let first = decoder.spent.walks.visits();
    assert!(4 * first > packet_bound, "{first} of {packet_bound}");
    decoder.feed(&bytes[half..]);
    let image = decoder.refine().unwrap().expect("the walk goes on");
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
    assert_eq!(decoder.refine(), Ok(None));
    assert!(decoder.refused);
    assert_eq!(decoder.bytes_decoded(), 0);
    let options = DecodeOptions::default().with_packet_work_limit(WorkLimit::Raised(4));
    assert!(decode_with(&bytes, &options).is_ok());
}

/// A refine that the rest of the code-block bound cannot cover is put off,
/// not refused, and `finish` still decodes the file under a bound of its
/// own.
#[test]
fn a_refine_past_the_rest_of_the_bound_waits() {
    let bytes = fixture("layers/lrcp.j2k");
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.spent.blocks = u64::MAX - 1;
    decoder.feed(&bytes);
    assert_eq!(decoder.refine().unwrap(), None);
    assert_eq!(decoder.bytes_decoded(), 0);
    assert_eq!(decoder.finish().unwrap(), whole_strict(&bytes));
}

/// A refine that the rest of the code-block bound just covers decodes:
/// the bound is cut by what the earlier refines spent, and no more.
#[test]
fn a_refine_the_rest_of_the_bound_covers_decodes() {
    let bytes = fixture("layers/lrcp.j2k");
    let (_, block_bound) = one_shot_bounds(&bytes);
    let mut once = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    once.feed(&bytes);
    assert!(once.refine().unwrap().is_some());
    let work = once.spent.blocks;
    assert!(8 * work < block_bound, "{work} of {block_bound}");

    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    // The earlier refines left just what the decode of the file takes.
    decoder.spent.blocks = block_bound - work;
    decoder.feed(&bytes);
    assert!(
        decoder.refine().unwrap().is_some(),
        "{work} of {block_bound}"
    );
    assert_eq!(decoder.spent.blocks, block_bound);
    decoder.feed(&[]);
    assert_eq!(refine_now(&mut decoder), Ok(None), "nothing is left");
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
        let poc_bitmap = tier2::decode_visits(crate::tile_bytes(&cs));
        for (position, tile) in cs.tiles.iter().enumerate() {
            let header = cs.tile_header(position).unwrap();
            let once = Meter::new(u64::MAX / 2);
            let expected =
                tier2::decode_packets_partial(&header, tile, &once, poc_bitmap, 0, None).is_ok();
            let kept = Meter::new(u64::MAX / 2);
            let walked = walks
                .walk(&header, tile, &kept, poc_bitmap, 0, None, u64::MAX / 4)
                .is_ok();
            assert_eq!(walked, expected, "{end} bytes");
            assert_eq!(kept.spent(), once.spent(), "{end} bytes");
            checked += 1;
        }
    }
    assert!(checked > 5, "{checked}");
}
