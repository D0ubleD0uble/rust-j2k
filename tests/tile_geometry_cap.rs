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
    DecodeOptions, Error, IncrementalDecoder, LimitDetail, LimitKind, Refined, Waiting, WorkLimit,
    decode_with, read_header_with,
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

    // A tile over both caps is refused for its precincts.
    let both = precincts_tile(2048, 1024);
    let limit = refusal(&both, &DecodeOptions::default());
    assert_eq!(limit.kind(), LimitKind::TilePrecincts, "{limit}");
    assert_eq!(limit.retry_with(), Some(WorkLimit::Raised(8)), "{limit}");
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
}

/// With `strict` off, a tile with no tile-part is left out of the decode, so
/// the header read does not count it either, nor does an
/// `IncrementalDecoder`'s header. With `strict` on, the header read counts it
/// under the main header and refuses it; the decode refuses the file too.
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

    match read_header_with(&bytes, &DecodeOptions::default()) {
        Err(Error::Limit(limit)) => assert_eq!(limit.kind(), LimitKind::TileCodeBlocks),
        other => panic!("{other:?}"),
    }
    assert!(decode_with(&bytes, &DecodeOptions::default()).is_err());
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
        Ok(Refined::Waiting(Waiting::SampleLimit(limit))) => {
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
