//! The `header_split` target's [`replaceable::replaceable`] where the
//! suffix that changes a held error is a box of length 0 after the
//! codestream run past 2^32 bytes, which OpenJPEG skips unread, and those
//! bytes also raise the default sample limit, to about 2^35 at 2^32 bytes
//! and without bound short of what the target can address as more follow.
//!
//! On `header_split_colr_lbox0_after_codestream_cap_lifts_tile_part_fault.seed`,
//! at a feed of 2999 bytes, `read_header_with` refuses a tile over a cap and
//! so meets a tile-part fault, which it gives. A raised limit counts no tile
//! over a cap, and the fault is not met. The input ends inside a `colr` of
//! length 0 after the codestream, so `header` holds the fault. On a long
//! enough suffix the header read no longer meets it, though `finish` and
//! `decode_with`, which parse every tile-part, still refuse. Under
//! [`SampleLimit::AtMost`] no byte raises the limit, so the same fault is
//! final, and `replaceable` must find no suffix there.
//!
//! The decode passes such a `colr` with any bytes more, so where no byte
//! raises the cap, under `AtMost` or under `Unlimited` (where the read
//! meets the fault through a tile past the address space), `header` gives
//! the fault at once, and `refine` gives its decode's fault under any
//! limit. A `bpcc` there can still turn at fault, so the fault is still
//! held behind one.
//!
//! Under a limit that grows, `header` holds only a stop a larger cap takes
//! away: one the count still meets at the samples the target can address,
//! such as a tile past the address space, is given at once.

#[path = "support/inconsistent.rs"]
mod inconsistent;
#[path = "support/pieces.rs"]
mod pieces;
#[path = "support/replaceable.rs"]
mod replaceable;
#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{
    DecodeOptions, Error, IncrementalDecoder, LimitKind, SampleLimit, decode_with, read_header_with,
};

/// The seed's file and the options its piece stream sets by the feed that
/// ends at `fed`: the reduction of its first control byte, then the
/// changes before that feed.
fn at_feed(name: &str, fed: usize) -> (Vec<u8>, DecodeOptions) {
    let seed = synth::read_file(synth::fixtures(&format!("incremental/{name}.seed")));
    let (control, data) = seed.split_at(12);
    let mut options = DecodeOptions::default()
        .with_strict(false)
        .with_resolution_reduction(control[0] % 3);
    let mut reached = false;
    for feed in pieces::Pieces::new(control, data.len()) {
        match feed.change {
            Some(pieces::Change::Reduction(levels)) => {
                options = options.with_resolution_reduction(levels);
            }
            Some(pieces::Change::Layers(layers)) => {
                options = options.with_max_quality_layers(Some(layers));
            }
            // The seed's region changes are to the whole image.
            Some(pieces::Change::Region { code, .. }) => assert_eq!(code % 4, 0, "{name}"),
            None => {}
        }
        if feed.to == fed {
            reached = true;
            break;
        }
    }
    assert!(reached, "{name}: no feed ends at {fed}");
    (data[..fed].to_vec(), options)
}

const SEED: &str = "header_split_colr_lbox0_after_codestream_cap_lifts_tile_part_fault";

const FAULT: &str = "tile 28 tile-part index is 2, but 0 of its tile-parts have been read";

/// The message of a [`Error::Codestream`] refusal, if `result` is one.
fn fault_of<T>(result: Result<T, Error>) -> Option<&'static str> {
    match result {
        Err(Error::Codestream(detail)) if detail.to_string() == FAULT => Some(FAULT),
        Err(Error::Codestream(_)) => Some("another codestream fault"),
        _ => None,
    }
}

/// At 2999 bytes the read meets the fault under the default limit and not
/// under a raised one, and `replaceable` finds that a suffix that skips
/// the `colr` of length 0, read under the limit of the longest such suffix,
/// changes it. Before it read that stand-in under that limit, it found no
/// suffix, and the target failed.
#[test]
fn a_tile_part_fault_met_through_a_cap_gives_way_to_the_bytes_that_skip_the_box() {
    let (prefix, options) = at_feed(SEED, 2999);
    assert_eq!(options.resolution_reduction(), 1);
    assert_eq!(options.max_quality_layers(), Some(1));
    let read = |options: &DecodeOptions| read_header_with(&prefix, options).err();
    assert_eq!(fault_of(read_header_with(&prefix, &options)), Some(FAULT));
    assert!(read(&options.clone().with_sample_limit(SampleLimit::Unlimited)).is_none());
    assert!(read(&options.clone().with_sample_limit(SampleLimit::Raised(2))).is_none());
    assert!(replaceable::replaceable(&prefix, &options));
    // The decoder holds it there, which is what the seed tests.
    let mut decoder = IncrementalDecoder::new(options).unwrap();
    decoder.feed(&prefix);
    assert!(matches!(decoder.header(), Ok(Some(_))));
}

/// Under `AtMost(2^29)` the read meets the same fault, and no byte raises
/// the limit: a harness that read the stand-in as `Unlimited` here would
/// pass a decoder that held a final fault.
#[test]
fn under_a_ceiling_the_same_fault_is_final() {
    let (prefix, options) = at_feed(SEED, 2999);
    let ceiling = options.with_sample_limit(SampleLimit::AtMost(1 << 29));
    assert_eq!(fault_of(read_header_with(&prefix, &ceiling)), Some(FAULT));
    assert!(!replaceable::replaceable(&prefix, &ceiling));
}

/// The `colr` seed with the sample limit `AtMost(1 + 255 * 2^16)`: control
/// byte 1 is `06` and byte 4 is `ff`.
const AT_MOST: &str = "header_split_colr_lbox0_after_codestream_tile_part_fault_at_most";

/// The `colr` seed under `Unlimited` (control byte 1 is `04`), its image
/// 2^31 x 2^27 in tiles 2^25 x 2^27, past what the target can address.
const ADDRESS_SPACE: &str =
    "header_split_colr_lbox0_after_codestream_address_space_tile_part_fault";

/// The seed `name`: its 12 control bytes, then the file.
fn seed_of(name: &str) -> Vec<u8> {
    synth::read_file(synth::fixtures(&format!("incremental/{name}.seed")))
}

/// What `header()` must give on `prefix`, as the target has it:
/// `read_header_with` with `strict` off, an incomplete main header as
/// `None`, as text.
fn oracle(prefix: &[u8], options: &DecodeOptions) -> String {
    format!(
        "{:?}",
        match inconsistent::checked(read_header_with(prefix, options)) {
            Ok(header) => Ok(Some(header)),
            Err(Error::Incomplete(_)) => Ok(None),
            Err(error) => Err(error),
        }
    )
}

/// `refine` on a decoder fed `prefix` under `options`, as an error.
fn refined(prefix: &[u8], options: &DecodeOptions) -> Result<(), Error> {
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    decoder.feed(prefix);
    decoder.refine().map(|_| ())
}

/// Under a limit no byte raises, the fault the read meets through a cap at
/// 2999 bytes is final: the `colr` of length 0 the input ends inside passes
/// whatever follows, and from 2^32 bytes on it is skipped, which raises no
/// cap here. So `header` gives it at once, and `refine`, whose decode meets
/// it in its parse of the tile-parts, too. From there on each call of a
/// decoder fed the seed's pieces gives what `read_header_with` gives on the
/// bytes fed, and so does a second call.
fn a_fault_no_byte_lifts_is_given_at_once(name: &str, limit: SampleLimit) {
    let (prefix, options) = at_feed(name, 2999);
    let options = options.with_sample_limit(limit.clone());
    assert_eq!(fault_of(read_header_with(&prefix, &options)), Some(FAULT));
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    decoder.feed(&prefix);
    assert_eq!(fault_of(decoder.header()), Some(FAULT), "{name}");
    assert_eq!(fault_of(decoder.header()), Some(FAULT), "{name}: again");
    assert_eq!(
        fault_of(refined(&prefix, &options)),
        Some(FAULT),
        "{name}: refine"
    );
    // The same through what the decoder keeps, fed in the seed's pieces.
    let seed = seed_of(name);
    let (control, file) = seed.split_at(12);
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    for feed in pieces::Pieces::new(control, file.len()) {
        decoder.feed(&file[decoder.bytes_fed()..feed.to]);
        let got = format!("{:?}", inconsistent::checked(decoder.header()));
        if feed.to >= 2999 {
            let at = format!("{name} at {}", feed.to);
            assert_eq!(got, oracle(&file[..feed.to], &options), "{at}");
            assert_eq!(format!("{:?}", decoder.header()), got, "{at}: again");
        }
    }
}

/// Under `AtMost` the cap is a ceiling the caller set. Before, `header`
/// held the fault, as the box of length 0 was of a type the decode checks,
/// and the `header_split` target failed on the seed at 2999 bytes.
#[test]
fn under_a_ceiling_header_gives_a_fault_met_through_a_cap_at_once() {
    a_fault_no_byte_lifts_is_given_at_once(AT_MOST, SampleLimit::AtMost(1 + (255 << 16)));
}

/// Under `Unlimited` the read meets the fault through a tile past the
/// address space, which no limit admits. Before, `header` held it, and the
/// target failed on the seed at 2999 bytes.
#[test]
fn under_unlimited_header_gives_a_fault_met_past_the_address_space_at_once() {
    a_fault_no_byte_lifts_is_given_at_once(ADDRESS_SPACE, SampleLimit::Unlimited);
}

/// Under the default limit `header` still holds the fault, which a longer
/// input lifts (above), but `refine`, whose decode parses every tile-part
/// whatever the limit, gives it at once: no box after the codestream can
/// turn at fault before it.
#[test]
fn under_the_default_limit_only_header_holds_the_fault() {
    let (prefix, options) = at_feed(SEED, 2999);
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    decoder.feed(&prefix);
    assert!(matches!(decoder.header(), Ok(Some(_))));
    assert_eq!(fault_of(refined(&prefix, &options)), Some(FAULT));
}

/// A `bpcc` of length 0 in place of the `colr`, holding the one byte the
/// file's one component takes, passes now, and one byte more puts it at
/// fault, which the decode meets before the tile-parts. So under a ceiling
/// `header` still holds the fault it meets through a cap there, and
/// `refine` waits on it.
#[test]
fn a_bpcc_of_length_0_after_the_codestream_can_still_turn_at_fault() {
    let (prefix, options) = at_feed(AT_MOST, 2999);
    let options = options.with_sample_limit(SampleLimit::AtMost(1 + (255 << 16)));
    let colr = prefix.len() - 72;
    assert_eq!(&prefix[colr..colr + 8], b"\0\0\0\0colr");
    let bpcc = [&prefix[..colr], b"\0\0\0\0bpcc", &[7]].concat();
    assert_eq!(fault_of(read_header_with(&bpcc, &options)), Some(FAULT));
    let longer = [&bpcc[..], &[7]].concat();
    assert!(matches!(
        read_header_with(&longer, &options),
        Err(Error::FileFormat(_))
    ));
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    decoder.feed(&bpcc);
    assert!(matches!(decoder.header(), Ok(Some(_))));
    assert!(refined(&bpcc, &options).is_ok());
}

/// The bytes of a `colr` of length 0 that passes whatever follows: an
/// enumerated color space, sRGB, and nothing after it.
const PASSING_COLR: &[u8] = b"\0\0\0\0colr\x01\0\0\0\0\0\x11";

/// A JP2 file of the codestream `main`, `tile_part` and `EOC`, then
/// [`PASSING_COLR`] and the end of the input. Its `jp2h` holds an `ihdr`
/// that matches the `SIZ` and an sRGB `colr`.
fn jp2_ending_in_a_passing_colr(main: &[u8], tile_part: &[u8]) -> Vec<u8> {
    let codestream = [main, tile_part, &[0xFF, 0xD9]].concat();
    let header = [
        synth::ihdr_for(&codestream),
        synth::bx(b"colr", &synth::colr_enum(16)),
    ];
    let mut file = synth::jp2_file(&header, &[], &codestream, &[]);
    file.extend_from_slice(PASSING_COLR);
    file
}

/// A main header: a `SIZ` of the eight `grid` fields and `components`
/// 8-bit components, a `COD` of `levels` levels and code-blocks of
/// `4 << block` samples square, and a `QCD`.
fn main_header(grid: [u32; 8], components: usize, levels: u8, block: u8) -> Vec<u8> {
    let mut main = vec![0xFF, 0x4F];
    main.extend(synth::seg(
        0xFF51,
        &synth::siz_body(grid, &vec![[7, 1, 1]; components]),
    ));
    main.extend(synth::seg(0xFF52, &synth::cod_body(1, levels, block, 0)));
    let exponents = vec![9; 1 + 3 * usize::from(levels)];
    main.extend(synth::seg(0xFF5C, &synth::qcd_none_body(2, &exponents)));
    main
}

/// The repro of #740: one 4-component tile of (2^32 - 1)^2 under 20 levels
/// and 4x4 code-blocks, read under a reduction of 19. Its code-blocks are
/// past what the target can address, so no sample limit admits it.
fn past_the_address_space() -> (Vec<u8>, DecodeOptions) {
    let side = u32::MAX;
    let main = main_header([side, side, 0, 0, side, side, 0, 0], 4, 20, 0);
    let file = jp2_ending_in_a_passing_colr(&main, &synth::tile_part(0, 0, 1, &[], &[]));
    let options = DecodeOptions::default()
        .with_strict(false)
        .with_resolution_reduction(19);
    (file, options)
}

/// Whether `result` refuses a tile past the address space.
fn past_the_address_space_refused<T>(result: Result<T, Error>) -> bool {
    matches!(result, Err(Error::Limit(limit)) if limit.kind() == LimitKind::AddressSpace)
}

/// After a passing `colr` of length 0, `header` held a refusal of a tile
/// past the address space while the sample limit could grow, under the
/// default or a `Raised` limit, though no limit admits that tile. Now it
/// gives it at once, as `read_header_with`, `refine` and `decode_with` do.
#[test]
fn header_gives_a_tile_past_the_address_space_at_once_under_a_limit_that_grows() {
    let (file, options) = past_the_address_space();
    for limit in [SampleLimit::Default, SampleLimit::Raised(8)] {
        let options = options.clone().with_sample_limit(limit.clone());
        let at = format!("{limit:?}");
        assert!(
            past_the_address_space_refused(read_header_with(&file, &options)),
            "{at}"
        );
        assert!(
            past_the_address_space_refused(decode_with(&file, &options)),
            "{at}"
        );
        let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
        decoder.feed(&file);
        let got = format!("{:?}", inconsistent::checked(decoder.header()));
        assert_eq!(got, oracle(&file, &options), "{at}: header");
        assert!(past_the_address_space_refused(decoder.header()), "{at}");
        assert!(
            past_the_address_space_refused(decoder.refine()),
            "{at}: refine"
        );
    }
}

/// With an `xml ` box of length 0 in place of the `colr`, a box the decode
/// does not check, `header` gave the refusal before too.
#[test]
fn an_xml_box_of_length_0_there_gives_the_same() {
    let (file, options) = past_the_address_space();
    let colr = file.len() - PASSING_COLR.len();
    let xml = [&file[..colr], b"\0\0\0\0xml ", &PASSING_COLR[8..]].concat();
    assert!(past_the_address_space_refused(read_header_with(
        &xml, &options
    )));
    let mut decoder = IncrementalDecoder::new(options).unwrap();
    decoder.feed(&xml);
    assert!(past_the_address_space_refused(decoder.header()));
}

/// 1024 tiles of 16x16, each of 1024 components under 5 levels. A tile's
/// size does not show it within the default cap on precincts, so each is
/// counted under the main header, at 6144 steps a tile, and the count stops
/// at its budget, though no tile is over a cap. Tile 28's first tile-part
/// is part 2, which the decode's parse of the tile-parts refuses, and which
/// the read meets where the count stops.
fn a_budget_stop_meets_a_tile_part_fault() -> Vec<u8> {
    let main = main_header([512, 512, 0, 0, 16, 16, 0, 0], 1024, 5, 4);
    jp2_ending_in_a_passing_colr(&main, &synth::tile_part(28, 2, 3, &[], &[]))
}

/// Whether `result` is the tile-part fault of
/// [`a_budget_stop_meets_a_tile_part_fault`].
fn part_fault<T>(result: Result<T, Error>) -> bool {
    matches!(result, Err(Error::Codestream(detail))
        if detail.to_string() == "tile 28 tile-part index is 2, but 0 of its tile-parts have been read")
}

/// Under the default limit the read meets the tile-part fault through the
/// stop at the budget, and a larger cap, under which each tile's size
/// admits it, takes that stop away. So after a passing `colr` of length 0
/// `header` holds it (a hold of only a refusal would not), and `refine`,
/// whose decode parses every tile-part, gives it. Under a ceiling no byte
/// raises the cap, and `header` gives it at once.
#[test]
fn a_stop_at_the_budget_a_larger_cap_takes_away_is_held() {
    let file = a_budget_stop_meets_a_tile_part_fault();
    let options = DecodeOptions::default().with_strict(false);
    assert!(part_fault(read_header_with(&file, &options)));
    let unlimited = options.clone().with_sample_limit(SampleLimit::Unlimited);
    assert!(read_header_with(&file, &unlimited).is_ok());
    assert!(replaceable::replaceable(&file, &options));
    let mut decoder = IncrementalDecoder::new(options).unwrap();
    decoder.feed(&file);
    assert!(matches!(decoder.header(), Ok(Some(_))));
    assert!(matches!(decoder.header(), Ok(Some(_))), "again");
    assert!(part_fault(decoder.refine()));
    let ceiling = DecodeOptions::default()
        .with_strict(false)
        .with_sample_limit(SampleLimit::AtMost(1 << 29));
    assert!(!replaceable::replaceable(&file, &ceiling));
    let mut decoder = IncrementalDecoder::new(ceiling).unwrap();
    decoder.feed(&file);
    assert!(part_fault(decoder.header()));
}
