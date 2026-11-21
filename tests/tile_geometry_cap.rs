//! The per-tile caps on code-blocks and precincts grow with the sample
//! limit: a tile may hold one code-block for each 512 samples the limit
//! allows and one precinct for each 2048, and at least 2^20 and 2^18. A
//! refusal names a `WorkLimit::Raised` factor of `DecodeOptions::sample_limit`
//! to retry with, or none under `WorkLimit::AtMost`, and `read_header_with`
//! refuses the tile as the decode does.
//!
//! The files are built here: one 8-bit component in one tile, no wavelet
//! levels, 4x4 code-blocks, and an empty packet per precinct, so the decode's
//! cost is its geometry and its buffers. A file over 64 MiB raises the
//! default sample limit past its 2^29 floor, and the caps with it; zero bytes
//! after `EOC` make it that long. The 40000 x 40000 file the caps were
//! raised for is `forty_thousand_square_in_32x32_blocks_decodes_at_the_defaults`,
//! ignored, as it needs about 9 GB.

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{
    DecodeOptions, Error, IncrementalDecoder, LimitDetail, LimitKind, Refined, Region, Waiting,
    WorkLimit, decode_with, read_header_with,
};

const FLOOR_BLOCKS: u64 = 1 << 20;

/// One `width` x `height` tile with no wavelet levels in 4x4 code-blocks
/// and the maximal precinct partition, `len` bytes long.
fn blocks_tile(width: u32, height: u32, len: usize) -> Vec<u8> {
    synth::empty_tiles(
        [width, height, 0, 0, width, height, 0, 0],
        0,
        0,
        &[],
        &[1],
        len,
    )
}

/// Its code-blocks.
fn blocks_of(width: u32, height: u32) -> u64 {
    u64::from(width.div_ceil(4)) * u64::from(height.div_ceil(4))
}

/// One `width` x `height` tile with no wavelet levels and 1x1 precincts,
/// so 1x1 code-blocks, one each: `width * height` precincts.
fn precincts_tile(width: u32, height: u32) -> Vec<u8> {
    let packets = (width * height) as usize;
    synth::empty_tiles(
        [width, height, 0, 0, width, height, 0, 0],
        0,
        0,
        &[0],
        &[packets],
        0,
    )
}

/// The refusal of `bytes` under `options`, from the decode, after checking
/// that `read_header_with` gives the same one.
fn refusal(bytes: &[u8], options: &DecodeOptions) -> LimitDetail {
    let header = read_header_with(bytes, options).map(|h| (h.width, h.height));
    let decoded = decode_with(bytes, options).map(|i| (i.width, i.height));
    assert_eq!(
        format!("{header:?}"),
        format!("{decoded:?}"),
        "read_header_with and decode_with refuse alike"
    );
    match decoded {
        Err(Error::Limit(limit)) => limit,
        other => panic!("{other:?}"),
    }
}

/// The tile and the amount a refusal's `refused_at` gives.
fn refused_at(limit: &LimitDetail) -> (u32, u64) {
    let at = limit.refused_at().expect("the tile and its amount");
    (at.tile(), at.amount())
}

/// That `bytes` decode under `options` to `width` x `height`, and that
/// `read_header_with` admits them too.
fn decodes(bytes: &[u8], options: &DecodeOptions, width: u32, height: u32) {
    let header = read_header_with(bytes, options).expect("the header is admitted");
    assert_eq!((header.width, header.height), (width, height));
    let image = decode_with(bytes, options).expect("the decode is admitted");
    assert_eq!((image.width, image.height), (width, height));
    assert!(image.components[0].samples.iter().all(|&v| v == 128));
}

/// A tile over the 2^20 floor is refused under a small input's default,
/// with `Raised(2)`, and decodes under it. Padded past 64 MiB, where the
/// default limit is 8 samples a byte, the same file decodes at the defaults:
/// the cap is then one code-block for each 512 samples of that limit.
#[test]
fn a_tile_past_the_floor_decodes_once_the_input_raises_the_limit() {
    let (width, height) = (4096, 4160);
    let blocks = blocks_of(width, height);
    assert!(blocks > FLOOR_BLOCKS);

    let small = blocks_tile(width, height, 0);
    let limit = refusal(&small, &DecodeOptions::default());
    assert_eq!(limit.kind(), LimitKind::TileCodeBlocks, "{limit}");
    assert_eq!(limit.retry_with(), Some(WorkLimit::Raised(2)), "{limit}");
    assert_eq!(refused_at(&limit), (0, blocks), "{limit}");
    assert!(
        limit.message().starts_with(&format!(
            "tile 0 holds {blocks} code-blocks, more than the 1048576 a tile may hold"
        )),
        "{limit}"
    );
    assert!(
        limit.message().contains("`DecodeOptions::sample_limit`"),
        "{limit}"
    );
    let retry = DecodeOptions::default()
        .with_retry(&limit)
        .expect("the hint names the sample limit");
    assert_eq!(retry.sample_limit, WorkLimit::Raised(2));
    decodes(&small, &retry, width, height);

    // 70,000,000 bytes allow 560,000,000 samples, so 1,093,750 code-blocks.
    let padded = blocks_tile(width, height, 70_000_000);
    decodes(&padded, &DecodeOptions::default(), width, height);
}

/// Padded to 70,000,000 bytes, a tile of 1,572,864 code-blocks needs about
/// 1.4 times the cap of 1,093,750 the default allows. It is refused with
/// `Raised(2)` at the defaults, and decodes under it.
#[test]
fn a_tile_that_needs_twice_the_scaled_cap_gets_raised_2() {
    let (width, height) = (4096, 6144);
    let bytes = blocks_tile(width, height, 70_000_000);
    let cap = 70_000_000 * 8 / 512;
    assert!(blocks_of(width, height) > cap && blocks_of(width, height) <= 2 * cap);

    let limit = refusal(&bytes, &DecodeOptions::default());
    assert_eq!(limit.kind(), LimitKind::TileCodeBlocks, "{limit}");
    assert_eq!(limit.retry_with(), Some(WorkLimit::Raised(2)), "{limit}");
    assert_eq!(refused_at(&limit), (0, 1_572_864), "{limit}");
    assert!(
        limit.message().starts_with(&format!(
            "tile 0 holds 1572864 code-blocks, more than the {cap} a tile may hold under the \
             sample limit of 560000000 samples"
        )),
        "{limit}"
    );
    let retry = DecodeOptions::default().with_retry(&limit).unwrap();
    decodes(&bytes, &retry, width, height);
}

/// Under `AtMost(n)` the cap is one code-block for each 512 samples of `n`,
/// whatever the input, and a refusal names no retry.
#[test]
fn under_at_most_the_code_block_cap_scales_from_n_with_no_retry() {
    let (width, height) = (4096, 4160);
    let blocks = blocks_of(width, height);
    let bytes = blocks_tile(width, height, 0);

    let exact = DecodeOptions::default().with_sample_limit(WorkLimit::AtMost(blocks * 512));
    decodes(&bytes, &exact, width, height);

    let under = WorkLimit::AtMost(blocks * 512 - 1);
    let options = DecodeOptions::default().with_sample_limit(under.clone());
    let limit = refusal(&bytes, &options);
    assert_eq!(limit.kind(), LimitKind::TileCodeBlocks, "{limit}");
    assert_eq!(limit.retry_with(), None, "{limit}");
    assert_eq!(refused_at(&limit), (0, blocks), "{limit}");
    assert!(options.with_retry(&limit).is_none());
    assert_eq!(
        limit.message(),
        format!(
            "tile 0 holds {blocks} code-blocks, more than the {} a tile may hold under \
             `WorkLimit::AtMost({})` (see `DecodeOptions::sample_limit`)",
            blocks - 1,
            blocks * 512 - 1
        )
    );

    // Padding does not move the cap under `AtMost`.
    let padded = blocks_tile(width, height, 70_000_000);
    let padded_limit = refusal(&padded, &options);
    assert_eq!(padded_limit.message(), limit.message());

    // Below 2^29 the cap is the floor: an image within it decodes under a
    // ceiling just over its own samples.
    let (width, height) = (4096, 4096);
    let samples = u64::from(width) * u64::from(height);
    let floor = blocks_tile(width, height, 0);
    let options = DecodeOptions::default().with_sample_limit(WorkLimit::AtMost(samples));
    decodes(&floor, &options, width, height);
}

/// The precinct cap follows the same rule, one precinct for each 2048
/// samples and at least 2^18, and is checked before the code-blocks.
#[test]
fn the_precinct_cap_scales_with_the_sample_limit() {
    let (width, height) = (1024, 512);
    let precincts = u64::from(width) * u64::from(height);
    let bytes = precincts_tile(width, height);

    let limit = refusal(&bytes, &DecodeOptions::default());
    assert_eq!(limit.kind(), LimitKind::TilePrecincts, "{limit}");
    assert_eq!(limit.retry_with(), Some(WorkLimit::Raised(2)), "{limit}");
    assert_eq!(refused_at(&limit), (0, precincts), "{limit}");
    assert!(
        limit.message().starts_with(&format!(
            "tile 0 holds {precincts} precincts, more than the 262144 a tile may hold under \
             the sample limit of 536870912 samples, one for each 2048 samples it allows"
        )),
        "{limit}"
    );
    let retry = DecodeOptions::default().with_retry(&limit).unwrap();
    decodes(&bytes, &retry, width, height);

    let exact = DecodeOptions::default().with_sample_limit(WorkLimit::AtMost(precincts * 2048));
    decodes(&bytes, &exact, width, height);
    let under = DecodeOptions::default().with_sample_limit(WorkLimit::AtMost(precincts * 2048 - 1));
    let limit = refusal(&bytes, &under);
    assert_eq!(limit.kind(), LimitKind::TilePrecincts, "{limit}");
    assert_eq!(limit.retry_with(), None, "{limit}");
    assert_eq!(refused_at(&limit), (0, precincts), "{limit}");

    // A tile over both caps is refused for its precincts.
    let both = precincts_tile(2048, 1024);
    let limit = refusal(&both, &DecodeOptions::default());
    assert_eq!(limit.kind(), LimitKind::TilePrecincts, "{limit}");
    assert_eq!(limit.retry_with(), Some(WorkLimit::Raised(8)), "{limit}");
    assert_eq!(refused_at(&limit), (0, 2048 * 1024), "{limit}");
}

/// Of two tiles, the one over the cap is the one named, by the header read
/// and the decode alike, and a region that reads only the other admits it.
#[test]
fn the_tile_over_the_cap_is_named_alike() {
    // Tiles 4096 x 4160 from x = 0; the image starts at x = 2048, so tile 0
    // is half a tile and tile 1 a whole one.
    let (side, height) = (4096, 4160);
    let bytes = synth::empty_tiles(
        [2 * side, height, side / 2, 0, side, height, 0, 0],
        0,
        0,
        &[],
        &[1, 1],
        0,
    );
    let limit = refusal(&bytes, &DecodeOptions::default());
    assert_eq!(limit.kind(), LimitKind::TileCodeBlocks, "{limit}");
    assert!(
        limit.message().starts_with(&format!(
            "tile 1 holds {} code-blocks",
            blocks_of(side, height)
        )),
        "{limit}"
    );
    assert_eq!(limit.retry_with(), Some(WorkLimit::Raised(2)), "{limit}");
    assert_eq!(refused_at(&limit), (1, blocks_of(side, height)), "{limit}");

    let tile0 = DecodeOptions::default().with_region(rust_j2k::Region::Tile(0));
    decodes(&bytes, &tile0, side / 2, height);
}

/// A tile is counted under its own header, as the decode counts it: a
/// tile-part `COD` of 64x64 code-blocks brings a tile the main header gives
/// 4x4 code-blocks within the cap, and the header read admits it too.
#[test]
fn a_tile_part_cod_is_counted_as_the_decode_counts_it() {
    let (width, height) = (4096, 4160);
    let main = blocks_tile(width, height, 0);
    // The main header, without the tile-part of one empty packet and `EOC`.
    let mut bytes = main[..main.len() - 17].to_vec();
    let cod = synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0));
    bytes.extend(synth::tile_part(0, 0, 1, &cod, &[0]));
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    decodes(&bytes, &DecodeOptions::default(), width, height);
    // Under the main header alone it is over the cap.
    let limit = refusal(&main, &DecodeOptions::default());
    assert_eq!(limit.kind(), LimitKind::TileCodeBlocks, "{limit}");

    // Of two tiles, the second over the cap under the main header and its
    // tile-part `COD` bringing it within: the header read finds that
    // tile-part after the first tile's.
    let mut two = synth::empty_tiles(
        [2 * width, height, width / 2, 0, width, height, 0, 0],
        0,
        0,
        &[],
        &[],
        0,
    );
    two.truncate(two.len() - 2);
    two.extend(synth::tile_part(0, 0, 1, &[], &[0]));
    two.extend(synth::tile_part(1, 0, 1, &cod, &[0]));
    two.extend_from_slice(&[0xFF, 0xD9]);
    decodes(&two, &DecodeOptions::default(), 3 * width / 2, height);

    // The reverse: a main header of 64x64 code-blocks and a tile-part `COD`
    // of 4x4. The header read counts under the main header and admits it;
    // the decode counts the tile under its own and refuses it, with a hint
    // that admits it.
    let mut wide = synth::empty_tiles(
        [width, height, 0, 0, width, height, 0, 0],
        0,
        4,
        &[],
        &[1],
        0,
    );
    wide.truncate(wide.len() - 17);
    let cod = synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0));
    wide.extend(synth::tile_part(0, 0, 1, &cod, &[0]));
    wide.extend_from_slice(&[0xFF, 0xD9]);
    let header =
        read_header_with(&wide, &DecodeOptions::default()).expect("the header is admitted");
    assert_eq!((header.width, header.height), (width, height));
    let limit = match decode_with(&wide, &DecodeOptions::default()) {
        Err(Error::Limit(limit)) => limit,
        other => panic!("{other:?}"),
    };
    assert_eq!(limit.kind(), LimitKind::TileCodeBlocks, "{limit}");
    assert_eq!(refused_at(&limit), (0, blocks_of(width, height)), "{limit}");
    let retry = DecodeOptions::default().with_retry(&limit).unwrap();
    decodes(&wide, &retry, width, height);
}

/// A tile's precincts include one for each of its resolutions, which a
/// small tile of many components and levels holds mostly: a 1x1 image of
/// 16384 components and 32 levels has 540,672 precincts, one a resolution,
/// over the 2^18 floor. The header read counts it, though the tile is a
/// single sample a component.
#[test]
fn a_tile_of_many_resolutions_is_counted() {
    let components = 16384;
    let siz = synth::siz_body([1, 1, 0, 0, 1, 1, 0, 0], &vec![[7, 1, 1]; components]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &synth::cod_body(1, 32, 0, 0)),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8; 97])),
    ]
    .concat();
    bytes.extend(synth::tile_part(0, 0, 1, &[], &[]));
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    let limit = refusal(&bytes, &DecodeOptions::default());
    assert_eq!(limit.kind(), LimitKind::TilePrecincts, "{limit}");
    assert!(
        limit.message().starts_with("tile 0 holds 540672 precincts"),
        "{limit}"
    );
    assert_eq!(refused_at(&limit), (0, 540_672), "{limit}");
}

/// With `strict` off, a tile with no tile-part is left out of the decode, so
/// the header read does not count it either, nor does an
/// `IncrementalDecoder`'s header. With `strict` on, the decode refuses the
/// file for the missing tile before it counts any tile, and the header read
/// gives that error too.
#[test]
fn a_missing_tile_is_counted_only_when_strict() {
    let (side, height) = (4096, 4160);
    // Tile 1, over the cap, has no tile-part.
    let bytes = synth::empty_tiles(
        [2 * side, height, side / 2, 0, side, height, 0, 0],
        0,
        0,
        &[],
        &[1],
        0,
    );
    let lenient = DecodeOptions::default().with_strict(false);
    let header = read_header_with(&bytes, &lenient).expect("the header is admitted");
    assert_eq!((header.width, header.height), (3 * side / 2, height));
    let image = decode_with(&bytes, &lenient).expect("the decode is admitted");
    assert!(!image.report.is_complete());
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes);
    assert!(decoder.header().unwrap().is_some());

    let header = read_header_with(&bytes, &DecodeOptions::default());
    assert!(matches!(header, Err(Error::Codestream(_))), "{header:?}");
    assert_same_error(&bytes, &DecodeOptions::default());
}

/// That `read_header_with` and `decode_with` refuse `bytes` under `options`
/// with the same error.
fn assert_same_error(bytes: &[u8], options: &DecodeOptions) {
    let header = read_header_with(bytes, options).map(|_| ());
    let decoded = decode_with(bytes, options).map(|_| ());
    assert!(header.is_err(), "{header:?}");
    assert_eq!(format!("{header:?}"), format!("{decoded:?}"));
}

/// A tile over the cap under the main header whose tile-part the input ends
/// before, inside the `SOT` that would start it (a `header` fuzz find, the
/// seed `tests/fixtures/header/rescue_cut_sot.j2k`): with `strict` on, the
/// decode refuses the file for the cut `SOT` before it counts any tile, and
/// the header read gives that error too, not the cap's. With `strict` off both
/// admit the file, the tile left out.
#[test]
fn a_tile_behind_a_cut_sot_gets_the_decode_error() {
    let bytes = include_bytes!("fixtures/header/rescue_cut_sot.j2k");
    let header = read_header_with(bytes, &DecodeOptions::default());
    assert!(
        matches!(&header, Err(Error::Codestream(detail)) if detail.message() == "truncated marker segment"),
        "{header:?}"
    );
    assert_same_error(bytes, &DecodeOptions::default());
    // The walk stopped at the other faults it stops for: a `SOT` of the
    // wrong length, and another marker where a `SOT` should be. And where
    // the chain is whole but carries tile 1 only, the decode refuses the
    // file for the missing tile 0.
    let sot = bytes.len() - 2;
    for tail in [
        &[0xFF, 0x90, 0, 9, 0, 0, 0, 0, 0, 14, 0, 1][..],
        &[0xFF, 0x64, 0, 2],
        &[
            0xFF, 0x90, 0, 10, 0, 1, 0, 0, 0, 14, 0, 1, 0xFF, 0x93, 0xFF, 0xD9,
        ],
    ] {
        let cut = [&bytes[..sot], tail].concat();
        assert_same_error(&cut, &DecodeOptions::default());
    }
    let lenient = DecodeOptions::default().with_strict(false);
    let header = read_header_with(bytes, &lenient).expect("the header is admitted");
    let image = decode_with(bytes, &lenient).expect("the decode is admitted");
    assert_eq!((header.width, header.height), (image.width, image.height));
    assert!(!image.report.is_complete());
}

/// The decode's error for the tile-parts is the first fault its parse finds,
/// which need not be in the tile counted: reading tile 1 of two, over the
/// cap under the main header and with no tile-part, where tile 0's
/// tile-part `COD` is illegal (code-blocks of 2^11 samples a side, or
/// progression order 9), the decode refuses the file for that `COD` with
/// `Error::Marker`, and the header read gives that error too.
#[test]
fn a_fault_in_another_tiles_header_is_the_decode_error() {
    let mut cod = synth::cod_body(1, 0, 4, 0);
    cod[0] = 1;
    cod.push(0x00);
    let siz = synth::siz_body([2 * 520, 512, 0, 0, 520, 512, 0, 0], &[[7, 1, 1]]);
    let main = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
    ]
    .concat();
    let large_blocks = synth::cod_body(1, 0, 9, 0);
    let mut order_9 = synth::cod_body(1, 0, 4, 0);
    order_9[1] = 9;
    let options = DecodeOptions::default().with_region(Region::Tile(1));
    for own in [large_blocks, order_9] {
        let part = synth::tile_part(0, 0, 1, &synth::seg(0xFF52, &own), &[0]);
        let bytes = [&main[..], &part, &[0xFF, 0xD9]].concat();
        let header = read_header_with(&bytes, &options);
        assert!(matches!(header, Err(Error::Marker(_))), "{header:?}");
        assert_same_error(&bytes, &options);
    }
}

/// A tile-part header the decode finds at fault is the decode's to report,
/// so the header read does not count its tile under the main header, over
/// whose caps it is: not where the input ends inside its `COD` (which the
/// decode leaves out with `strict` off), nor where the header runs past its
/// `Psot` or `Psot` is shorter than the `SOT` (which the decode refuses).
#[test]
fn a_tile_part_header_at_fault_is_left_to_the_decode() {
    let (width, height) = (4096, 4160);
    let main = blocks_tile(width, height, 0);
    let main = &main[..main.len() - 17];
    let cod = synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0));
    let part = synth::tile_part(0, 0, 1, &cod, &[0]);
    let with_psot = |psot: u32| {
        let mut part = part.clone();
        part[6..10].copy_from_slice(&psot.to_be_bytes());
        [main, &part, &[0xFF, 0xD9]].concat()
    };
    let cut = [main, &part[..12 + 6]].concat();
    let lenient = DecodeOptions::default().with_strict(false);
    // With `strict` off the decode leaves the cut tile out, and the header
    // read admits the file too.
    let header = read_header_with(&cut, &lenient).expect("the header is admitted");
    assert_eq!((header.width, header.height), (width, height));
    let image = decode_with(&cut, &lenient).expect("the decode is admitted");
    assert!(!image.report.is_complete());
    for bytes in [&cut, &with_psot(14), &with_psot(8)] {
        let header =
            read_header_with(bytes, &DecodeOptions::default()).expect("the header is admitted");
        assert_eq!((header.width, header.height), (width, height));
        let decoded = decode_with(bytes, &DecodeOptions::default());
        assert!(matches!(decoded, Err(Error::Codestream(_))), "{decoded:?}");
    }
    for psot in [14, 8] {
        let bytes = with_psot(psot);
        let header = read_header_with(&bytes, &lenient).expect("the header is admitted");
        assert_eq!((header.width, header.height), (width, height));
        let decoded = decode_with(&bytes, &lenient);
        assert!(matches!(decoded, Err(Error::Codestream(_))), "{decoded:?}");
    }
}

/// A tile-part header is read within its `Psot`: where each tile-part's
/// `Psot` ends inside a `COM` that holds the next `SOT`, so that each header
/// read past its `Psot` would run through every `COM` after it, the header
/// read still takes time linear in the file. 1024 tiles over the precinct
/// cap and 4 MiB of `COM` would take minutes in a debug build so.
#[test]
fn a_tile_part_header_is_read_within_its_psot() {
    let tiles = 1024u32;
    let mut cod = synth::cod_body(1, 0, 0, 0);
    cod[0] = 1;
    cod.push(0x00);
    let (tw, th) = (520, 512);
    let siz = synth::siz_body([tw * tiles, th, 0, 0, tw, th, 0, 0], &[[7, 1, 1]]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
    ]
    .concat();
    for tile in 0..tiles {
        // `SOT` and `COM` marker and length in 16 bytes; the `COM` body is
        // the next tile-part.
        let psot = if tile + 1 < tiles { 16 } else { 14 };
        bytes.extend(synth::sot(tile as u16, psot, 0, 1));
        if tile + 1 < tiles {
            bytes.extend_from_slice(&[0xFF, 0x64, 0, 14]);
        }
    }
    for _ in 0..1 << 20 {
        bytes.extend_from_slice(&[0xFF, 0x64, 0, 2]);
    }
    bytes.extend_from_slice(&[0xFF, 0x93, 0xFF, 0xD9]);
    for strict in [true, false] {
        let options = DecodeOptions::default().with_strict(strict);
        let start = std::time::Instant::now();
        let header = read_header_with(&bytes, &options);
        let spent = start.elapsed();
        // Every tile-part header runs past its `Psot`, so none is counted.
        assert!(header.is_ok(), "{header:?}");
        assert!(spent.as_secs() < 5, "{spent:?}");
        assert!(matches!(
            decode_with(&bytes, &options),
            Err(Error::Codestream(_))
        ));
    }
}

/// The header read's count of the tiles' own headers has a budget, and
/// reading a header that copies the main header's coding parameters is
/// charged for each component: of 1000 tiles of 4096 components over the
/// precinct cap under the main header, the first 999 with a tile-part `COD`
/// that brings them within, the budget runs out before the last, which is
/// over the cap under its own header too, so the header read leaves it to
/// the decode.
#[test]
fn the_count_of_the_tiles_own_headers_has_a_budget() {
    // `tiles` tiles of 8x16 and 4096 components in 1x1 precincts, 524,288
    // a tile, all but the last with a tile-part `COD` of the maximal ones.
    let file = |tiles: u32| {
        let mut cod = synth::cod_body(1, 0, 0, 0);
        cod[0] = 1;
        cod.push(0x00);
        let siz = synth::siz_body([8 * tiles, 16, 0, 0, 8, 16, 0, 0], &vec![[7, 1, 1]; 4096]);
        let mut bytes = [
            vec![0xFF, 0x4F],
            synth::seg(0xFF51, &siz),
            synth::seg(0xFF52, &cod),
            synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
        ]
        .concat();
        let own = synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0));
        for tile in 0..tiles - 1 {
            bytes.extend(synth::tile_part(tile as u16, 0, 1, &own, &[]));
        }
        bytes.extend(synth::tile_part(tiles as u16 - 1, 0, 1, &[], &[]));
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    };
    // Within the budget the last tile is counted and refused.
    match read_header_with(&file(8), &DecodeOptions::default()) {
        Err(Error::Limit(limit)) => {
            assert_eq!(limit.kind(), LimitKind::TilePrecincts, "{limit}");
            assert!(limit.message().starts_with("tile 7 "), "{limit}");
        }
        other => panic!("{other:?}"),
    }
    let bytes = file(1000);
    let header =
        read_header_with(&bytes, &DecodeOptions::default()).expect("the header is admitted");
    assert_eq!(header.width, 8000);
    assert!(decode_with(&bytes, &DecodeOptions::default()).is_err());
}

/// The header read rescues the tiles over a cap under the main header from
/// one walk of the tile-parts, not one a tile: 1980 tiles over the precinct
/// cap under the main header, each brought within by its first tile-part's
/// `COD`, with 254 more tile-parts each, about 505,000 `SOT`s in 6.9 MiB. A
/// walk a tile would take minutes in a debug build and seconds in release.
#[test]
fn the_tiles_over_a_cap_are_rescued_in_few_walks() {
    let (tw, th, nx, ny, parts) = (520, 512, 44, 45, 255);
    let mut cod = synth::cod_body(1, 0, 4, 0);
    cod[0] = 1;
    cod.push(0x00);
    let siz = synth::siz_body([tw * nx, th * ny, 0, 0, tw, th, 0, 0], &[[7, 1, 1]]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
    ]
    .concat();
    let own = synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0));
    for tile in 0..nx * ny {
        bytes.extend(synth::tile_part(tile as u16, 0, parts, &own, &[]));
    }
    for part in 1..parts {
        for tile in 0..nx * ny {
            bytes.extend(synth::tile_part(tile as u16, part, parts, &[], &[]));
        }
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    let start = std::time::Instant::now();
    let header =
        read_header_with(&bytes, &DecodeOptions::default()).expect("the header is admitted");
    let spent = start.elapsed();
    assert_eq!((header.width, header.height), (tw * nx, th * ny));
    assert!(spent.as_secs() < 4, "{spent:?}");
}

/// The header read counts the tiles' own headers in index order, so its
/// result does not depend on the order of the tile-parts. Of 100 tiles of
/// 4096 components over the precinct cap under the main header, every tile
/// but 66 is brought within by a tile-part `COD`. The budget runs out at
/// tile 85, so tile 66 is counted and refused whether the tile-parts come in
/// increasing or decreasing order, with the decode's own refusal.
#[test]
fn where_the_budget_runs_out_does_not_depend_on_the_order() {
    let tiles = 100u16;
    let refused = 66;
    let file = |order: &mut dyn Iterator<Item = u16>| {
        let mut cod = synth::cod_body(1, 0, 0, 0);
        cod[0] = 1;
        cod.push(0x00);
        let siz = synth::siz_body(
            [8 * u32::from(tiles), 16, 0, 0, 8, 16, 0, 0],
            &vec![[7, 1, 1]; 4096],
        );
        let mut bytes = [
            vec![0xFF, 0x4F],
            synth::seg(0xFF51, &siz),
            synth::seg(0xFF52, &cod),
            synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
        ]
        .concat();
        let own = synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0));
        let packets = vec![0; 4096];
        for tile in order {
            // An empty packet for each component of a tile brought within.
            let (extra, data): (&[u8], &[u8]) = if tile == refused {
                (&[], &[])
            } else {
                (&own, &packets)
            };
            bytes.extend(synth::tile_part(tile, 0, 1, extra, data));
        }
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    };
    for strict in [true, false] {
        let options = DecodeOptions::default().with_strict(strict);
        for bytes in [file(&mut (0..tiles)), file(&mut (0..tiles).rev())] {
            match read_header_with(&bytes, &options) {
                Err(Error::Limit(limit)) => {
                    assert_eq!(limit.kind(), LimitKind::TilePrecincts, "{limit}");
                    assert!(
                        limit.message().starts_with(&format!("tile {refused} ")),
                        "{limit}"
                    );
                }
                other => panic!("{other:?}"),
            }
        }
    }
    // The decode refuses tile 66 with the same error, here read alone to
    // keep the test cheap (a decode of the 66 tiles before it takes seconds
    // in a debug build).
    let bytes = file(&mut (0..tiles).rev());
    let header = read_header_with(&bytes, &DecodeOptions::default()).map(|h| h.width);
    let alone = DecodeOptions::default().with_region(Region::Tile(u32::from(refused)));
    let decoded = decode_with(&bytes, &alone).map(|i| i.width);
    assert_eq!(format!("{header:?}"), format!("{decoded:?}"));
}

/// Where the tile-parts come out of index order, the header read's refusal
/// names the lowest tile refused, as the decode's does. Each tile is 520 x
/// 512 in 1x1 precincts under the main header, over the precinct cap, and
/// the tiles not refused have a tile-part `COD` of the maximal precincts.
#[test]
fn a_refusal_names_the_lowest_tile_whatever_the_order() {
    let file = |tiles: u32, order: &[u16], refused: &[u16]| {
        let mut cod = synth::cod_body(1, 0, 4, 0);
        cod[0] = 1;
        cod.push(0x00);
        let siz = synth::siz_body([520 * tiles, 512, 0, 0, 520, 512, 0, 0], &[[7, 1, 1]]);
        let mut bytes = [
            vec![0xFF, 0x4F],
            synth::seg(0xFF51, &siz),
            synth::seg(0xFF52, &cod),
            synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
        ]
        .concat();
        let own = synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0));
        for &tile in order {
            let extra: &[u8] = if refused.contains(&tile) { &[] } else { &own };
            bytes.extend(synth::tile_part(tile, 0, 1, extra, &[0]));
        }
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    };
    for (tiles, order, refused, named) in [
        (4, &[3, 2, 1, 0][..], &[1, 2][..], 1),
        // Tile 4 is seen before tile 6, and both before tile 3.
        (8, &[0, 1, 2, 4, 6, 3, 5, 7][..], &[4, 6][..], 4),
    ] {
        let limit = refusal(&file(tiles, order, refused), &DecodeOptions::default());
        assert_eq!(limit.kind(), LimitKind::TilePrecincts, "{limit}");
        assert!(
            limit.message().starts_with(&format!("tile {named} ")),
            "{limit}"
        );
    }
}

/// A tile-part `COD` that takes a lower tile over a cap is counted before a
/// higher tile over a cap under the main header (a `header` fuzz find, the
/// seed `tests/fixtures/header/own_cod_lower_tile.j2k`). Of two tiles, tile
/// 0 is 600 x 4096 and within the caps under the main header's 4x4
/// code-blocks and maximal precincts, and its tile-part `COD` gives it 1x1
/// precincts; tile 1, 4200 x 4096, is over the code-block cap under the main
/// header. The decode refuses tile 0 for its precincts, and the header read
/// now does too (it refused tile 1 for its code-blocks, with `Raised(2)`).
#[test]
fn a_tile_part_cod_over_a_cap_in_a_lower_tile_is_refused_first() {
    let bytes = include_bytes!("fixtures/header/own_cod_lower_tile.j2k");
    for strict in [true, false] {
        let options = DecodeOptions::default().with_strict(strict);
        let limit = refusal(bytes, &options);
        assert_eq!(limit.kind(), LimitKind::TilePrecincts, "{limit}");
        assert!(limit.message().starts_with("tile 0 "), "{limit}");
        assert_eq!(limit.retry_with(), Some(WorkLimit::Raised(10)), "{limit}");
    }
}

/// Two rows of two tiles: a row of 4200 x 512, within the caps under the
/// main header's 4x4 code-blocks and maximal precincts, over a row of
/// 4200 x 4096, over the code-block cap there. Each tile in `own` has a
/// tile-part `COD` of 1x1 precincts, over the precinct cap, and every tile
/// one empty packet (none for a tile refused). The tile-parts come in
/// `order`, and the main header has `levels` levels.
fn two_rows(order: &[u16], own: &[u16], levels: u8, own_levels: u8) -> Vec<u8> {
    let siz = synth::siz_body(
        [2 * 4200, 2 * 4096, 0, 4096 - 512, 4200, 4096, 0, 0],
        &[[7, 1, 1]],
    );
    let exponents = vec![8; 3 * usize::from(levels) + 1];
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &synth::cod_body(1, levels, 0, 0)),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &exponents)),
    ]
    .concat();
    let mut cod = synth::cod_body(1, own_levels, 0, 0);
    cod[0] = 1;
    cod.extend(vec![0; usize::from(own_levels) + 1]);
    let cod = synth::seg(0xFF52, &cod);
    for &tile in order {
        let (extra, data): (&[u8], &[u8]) = if own.contains(&tile) {
            (&cod, &[])
        } else {
            (&[], &[0])
        };
        bytes.extend(synth::tile_part(tile, 0, 1, extra, data));
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// The header read counts the lower tiles under their own headers in index
/// order, whatever the order of the tile-parts, and names the lowest tile
/// refused, as the decode does: tile 0 where tiles 0 and 1 have a tile-part
/// `COD` over the precinct cap, tile 1 where only tile 1 has (tile 0 is
/// decoded first), and tile 2, over the code-block cap under the main
/// header, where neither has.
#[test]
fn the_lowest_tile_a_tile_part_cod_takes_over_a_cap_is_refused() {
    for order in [&[0, 1, 2, 3][..], &[3, 2, 1, 0], &[2, 1, 3, 0]] {
        for (own, named, kind) in [
            (&[0, 1][..], 0, LimitKind::TilePrecincts),
            (&[1], 1, LimitKind::TilePrecincts),
            (&[], 2, LimitKind::TileCodeBlocks),
        ] {
            for strict in [true, false] {
                let options = DecodeOptions::default().with_strict(strict);
                let limit = refusal(&two_rows(order, own, 0, 0), &options);
                assert_eq!(limit.kind(), kind, "{order:?} {own:?}: {limit}");
                assert!(
                    limit.message().starts_with(&format!("tile {named} ")),
                    "{order:?} {own:?}: {limit}"
                );
            }
        }
    }
    // Where that `COD` also has fewer levels than the reduction takes, the
    // tile is refused for the reduction, as the decode checks a tile's
    // reduction before its caps.
    let bytes = two_rows(&[3, 2, 1, 0], &[0], 1, 0);
    let options = DecodeOptions::default().with_resolution_reduction(1);
    let header = read_header_with(&bytes, &options);
    assert!(
        matches!(&header, Err(Error::InvalidOptions(d)) if d.message().ends_with("in tile 0")),
        "{header:?}"
    );
    assert_same_error(&bytes, &options);
    // Under a reduction a lower tile is counted whatever its size: here tile
    // 0 is 8 x 4096, within the caps under any header, and its tile-part
    // `COD` (or a `COC` for its one component) has no levels where the main
    // header's has one. Tile 1, 4200 x 4096, is over the code-block cap.
    let siz = synth::siz_body([2 * 4200, 4096, 4192, 0, 4200, 4096, 0, 0], &[[7, 1, 1]]);
    let cod = synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0));
    let coc = synth::seg(0xFF53, &[0, 0, 0, 0, 0, 0, 1]);
    for own in [cod, coc] {
        let bytes = [
            vec![0xFF, 0x4F],
            synth::seg(0xFF51, &siz),
            synth::seg(0xFF52, &synth::cod_body(1, 1, 0, 0)),
            synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8; 4])),
            synth::tile_part(0, 0, 1, &own, &[]),
            synth::tile_part(1, 0, 1, &[], &[]),
            vec![0xFF, 0xD9],
        ]
        .concat();
        for strict in [true, false] {
            let options = options.clone().with_strict(strict);
            let header = read_header_with(&bytes, &options);
            assert!(
                matches!(&header, Err(Error::InvalidOptions(d)) if d.message().ends_with("in tile 0")),
                "{header:?}"
            );
            assert_same_error(&bytes, &options);
        }
    }
}

/// Where the header read refuses a tile, the decode can refuse the file
/// sooner, for what only decoding finds: here the packets of tile 0 with
/// `strict` on (a byte left after its last packet), or with `strict` off the
/// charge for the tiles read that have no tile-part, one step a
/// tile-component, past the packet work bound. The header read reads
/// neither, and names the tile over a cap.
#[test]
fn what_only_decoding_finds_in_a_lower_tile_comes_first_in_the_decode() {
    let mut bytes = two_rows(&[0, 1, 2, 3], &[], 0, 0);
    // Tile 0's tile-part, the first of four of 15 bytes, now with a byte
    // after its packet.
    let at = bytes.len() - 2 - 4 * 15;
    bytes.splice(at..at + 15, synth::tile_part(0, 0, 1, &[], &[0, 0]));
    // The `header` fuzz seeds: this file, and one with no data in tile 0.
    assert_eq!(
        bytes[..],
        include_bytes!("fixtures/header/lower_tile_data_long.j2k")[..]
    );
    let short = include_bytes!("fixtures/header/lower_tile_data_short.j2k");
    for (bytes, fault) in [
        (
            &bytes[..],
            "tile 0 has 1 byte(s) left after the last packet",
        ),
        (&short[..], "the data of tile 0 ends before the last packet"),
    ] {
        let header = read_header_with(bytes, &DecodeOptions::default());
        assert!(
            matches!(&header, Err(Error::Limit(l)) if l.message().starts_with("tile 2 ")),
            "{header:?}"
        );
        let decoded = decode_with(bytes, &DecodeOptions::default());
        assert!(
            matches!(&decoded, Err(Error::Codestream(d)) if d.message() == fault),
            "{decoded:?}"
        );
    }

    // 1024 x 2 tiles of 16384 components: a row of 1x1 tiles, within the
    // caps by their size, over a row of 1 x 17, over the precinct cap under
    // the main header's 1x1 precincts. Only tile 1024 has a tile-part.
    let siz = synth::siz_body([1024, 34, 0, 16, 1, 17, 0, 0], &vec![[7, 1, 1]; 16384]);
    let mut cod = synth::cod_body(1, 0, 0, 0);
    cod[0] = 1;
    cod.push(0x00);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
    ]
    .concat();
    bytes.extend(synth::tile_part(1024, 0, 1, &[], &[]));
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    let lenient = DecodeOptions::default().with_strict(false);
    let header = read_header_with(&bytes, &lenient);
    assert!(
        matches!(&header, Err(Error::Limit(l)) if l.kind() == LimitKind::TilePrecincts),
        "{header:?}"
    );
    let decoded = decode_with(&bytes, &lenient);
    assert!(
        matches!(&decoded, Err(Error::Limit(l)) if l.kind() == LimitKind::PacketWork),
        "{decoded:?}"
    );
    // With `strict` on, both refuse the file for the missing tile 0: the
    // header read counts it, as a tile-part header could take it over a cap.
    assert_same_error(&bytes, &DecodeOptions::default());
}

/// A JP2 file is held to the same caps, and its header read refuses as its
/// decode does.
#[test]
fn a_jp2_file_is_refused_alike() {
    let (width, height) = (4096, 4160);
    let codestream = blocks_tile(width, height, 0);
    let header = [
        synth::ihdr_for(&codestream),
        synth::bx(b"colr", &synth::colr_enum(17)),
    ];
    let bytes = synth::jp2_file(&header, &[], &codestream, &[]);
    let limit = refusal(&bytes, &DecodeOptions::default());
    assert_eq!(limit.kind(), LimitKind::TileCodeBlocks, "{limit}");
    assert_eq!(limit.retry_with(), Some(WorkLimit::Raised(2)), "{limit}");
    let retry = DecodeOptions::default().with_retry(&limit).unwrap();
    decodes(&bytes, &retry, width, height);
}

/// The caps grow with the bytes fed, so an `IncrementalDecoder` waits on a
/// tile past them, with `SampleLimit`, and gives the image once the bytes
/// raise the caps. Under `AtMost` the refusal is an error, and it sticks.
#[test]
fn a_refine_waits_on_a_tile_past_the_caps() {
    let (width, height) = (4096, 4160);
    let bytes = blocks_tile(width, height, 70_000_000);
    let (head, rest) = bytes.split_at(1 << 20);
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(head);
    match decoder.refine() {
        Ok(Refined::Waiting(Waiting::SampleLimit { limit, .. })) => {
            assert_eq!(limit.kind(), LimitKind::TileCodeBlocks, "{limit}");
            assert_eq!(limit.retry_with(), Some(WorkLimit::Raised(2)), "{limit}");
        }
        other => panic!("{other:?}"),
    }
    decoder.feed(rest);
    match decoder.refine() {
        Ok(Refined::Image(image)) => assert_eq!((image.width, image.height), (width, height)),
        other => panic!("{other:?}"),
    }

    let ceiling = WorkLimit::AtMost(blocks_of(width, height) * 512 - 1);
    let mut decoder =
        IncrementalDecoder::new(DecodeOptions::default().with_sample_limit(ceiling)).unwrap();
    decoder.feed(head);
    let first = decoder.refine();
    assert!(
        matches!(&first, Err(Error::Limit(l)) if l.kind() == LimitKind::TileCodeBlocks),
        "{first:?}"
    );
    decoder.feed(rest);
    assert_eq!(format!("{:?}", decoder.refine()), format!("{first:?}"));
}

/// The file the caps were raised for: one 40000 x 40000 tile of one
/// component in 32x32 code-blocks with 5 levels (2,083,325 code-blocks),
/// padded to 210,000,000 bytes, so the default limit of 1.68 x 10^9 samples
/// admits its 1.6 x 10^9 and 3,281,250 code-blocks. It decodes at the
/// defaults, at full resolution and at reduction 3. Under 2^20 code-blocks
/// it was refused at every option.
#[test]
#[ignore = "decodes 1.6 x 10^9 samples; about 9 GB and a minute in release"]
fn forty_thousand_square_in_32x32_blocks_decodes_at_the_defaults() {
    let side = 40_000;
    let bytes = synth::empty_tiles(
        [side, side, 0, 0, side, side, 0, 0],
        5,
        3,
        &[],
        // A packet for each resolution, and four at the finest, which the
        // maximal 2^15 precincts cut in two each way.
        &[9],
        210_000_000,
    );
    if let Ok(path) = std::env::var("TILE_GEOMETRY_CAP_WRITE") {
        std::fs::write(path, &bytes).unwrap();
    }
    let reduced = DecodeOptions::default().with_resolution_reduction(3);
    let header = read_header_with(&bytes, &reduced).unwrap();
    assert_eq!((header.width, header.height), (5000, 5000));
    let image = decode_with(&bytes, &reduced).unwrap();
    assert_eq!((image.width, image.height), (5000, 5000));
    assert!(image.components[0].samples.iter().all(|&v| v == 128));
    drop(image);
    let image = decode_with(&bytes, &DecodeOptions::default()).unwrap();
    assert_eq!((image.width, image.height), (side, side));
}

/// A 60-byte header declaring one tile whose geometry no sample limit can
/// hold: under `Unlimited` the decode refuses it as past the address space,
/// a kind no option raises, with no retry, and `read_header_with` agrees.
/// The first is `2^28` x `2^28` in 4x4 code-blocks (`2^52` of them), the
/// second `2^25 + 1` x `2^25` in 1x1 precincts.
fn past_the_address_space() -> [(&'static str, Vec<u8>); 2] {
    let side = 1 << 28;
    let (wide, high) = ((1 << 25) + 1, 1 << 25);
    [
        (
            "code-blocks",
            synth::empty_tiles([side, side, 0, 0, side, side, 0, 0], 0, 0, &[], &[0], 0),
        ),
        (
            "precincts",
            synth::empty_tiles([wide, high, 0, 0, wide, high, 0, 0], 0, 0, &[0], &[0], 0),
        ),
    ]
}

#[test]
fn a_tile_past_the_address_space_is_not_adjustable() {
    let unlimited = DecodeOptions::default().with_sample_limit(WorkLimit::Unlimited);
    for (noun, bytes) in past_the_address_space() {
        assert!(bytes.len() < 100, "{noun}: {}", bytes.len());
        let limit = refusal(&bytes, &unlimited);
        assert_eq!(limit.kind(), LimitKind::AddressSpace, "{noun}: {limit}");
        assert!(!limit.kind().is_adjustable(), "{noun}: {limit}");
        assert_eq!(limit.retry_with(), None, "{noun}: {limit}");
        assert!(limit.message().starts_with("tile 0 holds "), "{limit}");
        assert_eq!(limit.refused_at(), None, "{noun}: {limit}");
    }
}

/// A sample limit refusal names no tile: it counts the whole image.
#[test]
fn a_sample_limit_refusal_names_no_tile() {
    let small = blocks_tile(64, 64, 0);
    let under = DecodeOptions::default().with_sample_limit(WorkLimit::AtMost(100));
    let limit = refusal(&small, &under);
    assert_eq!(limit.kind(), LimitKind::Samples, "{limit}");
    assert_eq!(limit.refused_at(), None, "{limit}");
}

/// `refine` returns that refusal as an error, and every later refine returns
/// it again, however many bytes come. It does not wait on `SampleLimit`.
#[test]
fn a_refine_does_not_wait_on_a_tile_past_the_address_space() {
    let unlimited = DecodeOptions::default().with_sample_limit(WorkLimit::Unlimited);
    for (noun, bytes) in past_the_address_space() {
        let mut decoder = IncrementalDecoder::new(unlimited.clone()).unwrap();
        decoder.feed(&bytes);
        let first = decoder.refine();
        match &first {
            Err(Error::Limit(limit)) => {
                assert_eq!(limit.kind(), LimitKind::AddressSpace, "{noun}: {limit}");
            }
            other => panic!("{noun}: {other:?}"),
        }
        decoder.feed(&[0; 64]);
        assert_eq!(format!("{:?}", decoder.refine()), format!("{first:?}"));
    }
}

/// A ceiling past the address space does not turn the refusal into the
/// ceiling's: the tile is `AddressSpace` under `AtMost(u64::MAX)` too, and
/// a refine returns it as an error.
#[test]
fn a_ceiling_past_the_address_space_does_not_change_the_kind() {
    let ceiling = DecodeOptions::default().with_sample_limit(WorkLimit::AtMost(u64::MAX));
    for (noun, bytes) in past_the_address_space() {
        let limit = refusal(&bytes, &ceiling);
        assert_eq!(limit.kind(), LimitKind::AddressSpace, "{noun}: {limit}");
        assert_eq!(limit.retry_with(), None, "{noun}: {limit}");
        let mut decoder = IncrementalDecoder::new(ceiling.clone()).unwrap();
        decoder.feed(&bytes);
        assert!(
            matches!(decoder.refine(), Err(Error::Limit(l)) if l.kind() == LimitKind::AddressSpace),
            "{noun}"
        );
    }
}

/// A refusal that only `Unlimited` lifts still waits: one tile of
/// 4(2^26 - 1) x 4(2^26 + 1) in 4x4 code-blocks is over the cap of
/// `Raised(u32::MAX)` and within the address space, so it names `Unlimited`,
/// which admits it, and a refine waits on it with that retry.
#[test]
fn a_refusal_only_unlimited_lifts_is_a_wait() {
    let (width, height) = (4 * ((1u32 << 26) - 1), 4 * ((1u32 << 26) + 1));
    let bytes = blocks_tile(width, height, 0);
    let raised = DecodeOptions::default().with_sample_limit(WorkLimit::Raised(u32::MAX));
    let limit = refusal(&bytes, &raised);
    assert_eq!(limit.kind(), LimitKind::TileCodeBlocks, "{limit}");
    assert_eq!(limit.retry_with(), Some(WorkLimit::Unlimited), "{limit}");
    let mut decoder = IncrementalDecoder::new(raised).unwrap();
    decoder.feed(&bytes);
    match decoder.refine() {
        Ok(Refined::Waiting(Waiting::SampleLimit { limit, .. })) => {
            assert_eq!(limit.retry_with(), Some(WorkLimit::Unlimited), "{limit}");
        }
        other => panic!("{other:?}"),
    }
    let unlimited = DecodeOptions::default().with_retry(&limit).unwrap();
    let header = read_header_with(&bytes, &unlimited).expect("Unlimited admits the tile");
    assert_eq!((header.width, header.height), (width, height));
}

/// A tile over a cap under the main header that has no tile-part, with a
/// lower tile its own header refuses (a review find, the seed
/// `tests/fixtures/header/strict_missing_over_cap.j2k`). Tile 0 is 8 wide,
/// with a tile-part `COD` of no levels; tile 1, 1000 x 1000 and over the
/// precinct cap, is missing. With `strict` on the decode refuses the file for
/// the missing tile before it counts any tile, so at reduction 1 the header
/// read gives that error too, not tile 0's reduction refusal.
#[test]
fn a_missing_tile_over_a_cap_comes_before_a_lower_tile_refused() {
    let bytes = include_bytes!("fixtures/header/strict_missing_over_cap.j2k");
    let reduced = DecodeOptions::default().with_resolution_reduction(1);
    let header = read_header_with(bytes, &reduced);
    assert!(
        matches!(&header, Err(Error::Codestream(d))
            if d.message() == "the codestream carries no tile-part for tile 1"),
        "{header:?}"
    );
    assert_same_error(bytes, &reduced);
    // With `strict` off the decode leaves tile 1 out and refuses tile 0.
    let lenient = reduced.clone().with_strict(false);
    let header = read_header_with(bytes, &lenient);
    assert!(
        matches!(&header, Err(Error::InvalidOptions(d)) if d.message().ends_with("in tile 0")),
        "{header:?}"
    );
    assert_same_error(bytes, &lenient);
    // The same for a missing tile between the lower tile refused and the
    // tile over a cap: tile 1, within the caps, of two rows whose tile 0
    // has a tile-part `COD` of no levels.
    let bytes = two_rows(&[0, 2, 3], &[0], 1, 0);
    let header = read_header_with(&bytes, &reduced);
    assert!(
        matches!(&header, Err(Error::Codestream(d))
            if d.message() == "the codestream carries no tile-part for tile 1"),
        "{header:?}"
    );
    assert_same_error(&bytes, &reduced);
    let header = read_header_with(&bytes, &lenient);
    assert!(
        matches!(&header, Err(Error::InvalidOptions(d)) if d.message().ends_with("in tile 0")),
        "{header:?}"
    );
    assert_same_error(&bytes, &lenient);
}

/// A tile over a cap with no tile-part after one its own header admits,
/// with a lower tile refused (a review find, the seed
/// `tests/fixtures/header/later_missing_over_cap.j2k`). Tile 0 is 8 wide,
/// with a tile-part `COD` of no levels; tile 1, 1000 x 1000 and over the
/// precinct cap under the main header, has a tile-part `COD` that admits
/// it; tile 2, the same size, is missing. With `strict` on the decode
/// refuses the file for tile 2 before it counts any tile, so at reduction 1
/// the header read looks past tile 1 and gives that error too.
#[test]
fn a_missing_tile_over_a_cap_after_one_admitted_comes_before_a_lower_tile_refused() {
    let bytes = include_bytes!("fixtures/header/later_missing_over_cap.j2k");
    let reduced = DecodeOptions::default().with_resolution_reduction(1);
    let header = read_header_with(bytes, &reduced);
    assert!(
        matches!(&header, Err(Error::Codestream(d))
            if d.message() == "the codestream carries no tile-part for tile 2"),
        "{header:?}"
    );
    assert_same_error(bytes, &reduced);
    // With `strict` off the decode leaves tile 2 out and refuses tile 0.
    let lenient = reduced.clone().with_strict(false);
    let header = read_header_with(bytes, &lenient);
    assert!(
        matches!(&header, Err(Error::InvalidOptions(d)) if d.message().ends_with("in tile 0")),
        "{header:?}"
    );
    assert_same_error(bytes, &lenient);
}

/// `components` gray components over a row of `width` tiles of 1x1 and a
/// row of `width` tiles of 1 x `tall`, with `levels` levels and precincts
/// of 1x1 at resolution 0 and 2x2 above.
/// Every tile has a tile-part with no `COD` or `COC` and no data.
fn small_row_over_tall_row(components: usize, width: u32, tall: u32, levels: u8) -> Vec<u8> {
    let siz = synth::siz_body(
        [width, 2 * tall, 0, tall - 1, 1, tall, 0, 0],
        &vec![[7, 1, 1]; components],
    );
    let mut cod = synth::cod_body(1, levels, 0, 0);
    cod[0] = 1;
    cod.push(0x00);
    cod.extend(vec![0x11; usize::from(levels)]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(
            0xFF5C,
            &synth::qcd_none_body(2, &vec![8; 3 * usize::from(levels) + 1]),
        ),
    ]
    .concat();
    for tile in 0..2 * width {
        bytes.extend(synth::tile_part(tile as u16, 0, 1, &[], &[]));
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// Lower tiles whose tile-parts carry no `COD` or `COC` are coded as the
/// main header has it, so the header read neither reads nor charges them,
/// and its budget is left for the tile over a cap: 400 small tiles of 1024
/// components under a reduction, and 64 of 16384 components (where any
/// tile-part header could take a small tile over a cap), are refused at
/// the first tall tile, as the decode refuses them once the packet work
/// bound is lifted (it reads every lower tile's packets first).
#[test]
fn lower_tiles_with_no_tile_part_cod_spend_no_budget() {
    for (components, width, tall, reduction) in [(1024, 400, 4096, 1), (16384, 64, 17, 0)] {
        let bytes = small_row_over_tall_row(components, width, tall, 1);
        let options = DecodeOptions::default()
            .with_strict(false)
            .with_packet_work_limit(WorkLimit::Unlimited)
            .with_resolution_reduction(reduction);
        let limit = refusal(&bytes, &options);
        assert_eq!(limit.kind(), LimitKind::TilePrecincts, "{limit}");
        let tile = format!("tile {width} ");
        assert!(limit.message().starts_with(&tile), "{limit}");
    }
}

/// An over-cap tile refused by its own reduction check, before a tile with
/// no tile-part (a review find, the seed
/// `tests/fixtures/header/own_reduction_then_missing.j2k`). Tile 0 is 8
/// wide; tile 1, 1000 x 1000 and over the precinct cap under the main
/// header, has a tile-part `COD` of no levels; tile 2 is missing. With
/// `strict` on the decode refuses the file for tile 2 before it counts any
/// tile, so at reduction 1 the header read gives that error too, for the
/// whole image and for a window that reads tile 1 alone.
#[test]
fn a_missing_tile_comes_before_an_own_reduction_refusal() {
    let bytes = include_bytes!("fixtures/header/own_reduction_then_missing.j2k");
    let reduced = DecodeOptions::default().with_resolution_reduction(1);
    let window = reduced
        .clone()
        .with_region(Region::Window(rust_j2k::Window::new(1000, 0, 1100, 100)));
    for options in [&reduced, &window] {
        let header = read_header_with(bytes, options);
        assert!(
            matches!(&header, Err(Error::Codestream(d))
                if d.message() == "the codestream carries no tile-part for tile 2"),
            "{header:?}"
        );
        assert_same_error(bytes, options);
    }
    // With `strict` off the decode leaves tile 2 out and refuses tile 1.
    let lenient = reduced.clone().with_strict(false);
    let header = read_header_with(bytes, &lenient);
    assert!(
        matches!(&header, Err(Error::InvalidOptions(d)) if d.message().ends_with("in tile 1")),
        "{header:?}"
    );
    assert_same_error(bytes, &lenient);
}

/// `components` gray components over `rows` rows of two tiles: a 1 x 16
/// tile (2r) with a tile-part `COD` of one level, or of none where `refuse`
/// names the row, then a 16 x 16 tile (2r + 1), over the precinct cap under
/// the main header and brought within by its tile-part `COD`. The tiles
/// `missing` names have no tile-part.
fn narrow_and_square(components: usize, rows: u32, refuse: &[u32], missing: &[u16]) -> Vec<u8> {
    let siz = synth::siz_body(
        [32, 16 * rows, 15, 0, 16, 16, 0, 0],
        &vec![[7, 1, 1]; components],
    );
    let mut cod = synth::cod_body(1, 1, 0, 0);
    cod[0] = 1;
    cod.extend([0x00, 0x11]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8; 4])),
    ]
    .concat();
    let one = synth::seg(0xFF52, &synth::cod_body(1, 1, 0, 0));
    let none = synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0));
    for tile in 0..2 * rows as u16 {
        if missing.contains(&tile) {
            continue;
        }
        let refused = tile % 2 == 0 && refuse.contains(&u32::from(tile / 2));
        let own = if refused { &none } else { &one };
        bytes.extend(synth::tile_part(tile, 0, 1, own, &vec![0; 2 * components]));
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// With `strict` on, a tile with no tile-part comes before a lower tile's
/// refusal and before a stop at the count's budget (review finds): 60 rows
/// of 4096 components, where counting the lower tiles under their own
/// headers spends the budget before the missing tile. Tile 119 is missing
/// with no tile refused, or with tile 40 refused under a reduction; tile
/// 118, within the caps, is missing with tile 0 refused.
#[test]
fn a_missing_tile_comes_before_a_lower_refusal_or_a_budget_stop() {
    let cases: [(&[u32], u16, u8); 4] = [
        (&[], 119, 0),
        (&[20], 119, 0),
        (&[20], 119, 1),
        (&[0], 118, 1),
    ];
    for (refuse, missing, reduction) in cases {
        let bytes = narrow_and_square(4096, 60, refuse, &[missing]);
        let options = DecodeOptions::default().with_resolution_reduction(reduction);
        let header = read_header_with(&bytes, &options);
        let expected = format!("the codestream carries no tile-part for tile {missing}");
        assert!(
            matches!(&header, Err(Error::Codestream(d)) if d.message() == expected),
            "{refuse:?} {missing} r{reduction}: {header:?}"
        );
        assert_same_error(&bytes, &options);
    }
}
