//! `IncrementalDecoder` against one-shot decodes of the same bytes.
//!
//! Every image a refine returns must be the image `decode_with` gives with
//! `strict` off on the bytes fed so far, and `finish` must give what
//! `decode_with` gives on all of them, with a main header cut short turned
//! from `Error::Incomplete` into a truncation error. This runs on every
//! codestream, JP2 and JPH file under `tests/fixtures/`, fed in pieces of
//! many sizes, and cut at many points.

use std::path::{Path, PathBuf};

use rust_j2k::{
    DecodeOptions, Error, Image, IncrementalDecoder, LimitKind, Region, Window, WorkLimit,
    decode_with, read_header, read_header_with,
};

/// A splitmix64 stream: small, and the same on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A value in `0..n`.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Every codestream, JP2 and JPH file under `dir`, sorted.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
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
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn lenient() -> DecodeOptions {
    DecodeOptions::default().with_strict(false)
}

/// Whether `error` is a refusal by the packet or code-block work bound,
/// which more bytes raise: its message names the option that raises it.
fn is_work_limit(error: &Error) -> bool {
    matches!(error, Error::Limit(limit)
        if matches!(limit.kind(), LimitKind::PacketWork | LimitKind::CodeBlockWork))
}

/// What a refine on `prefix` must give: the strict-off decode, with a main
/// header cut short and a work-bound refusal as `None`.
fn refine_oracle(prefix: &[u8], options: &DecodeOptions) -> Result<Option<Image>, Error> {
    match decode_with(prefix, &options.clone().with_strict(false)) {
        Ok(image) => Ok(Some(image)),
        Err(Error::Incomplete(_)) => Ok(None),
        Err(error) if is_work_limit(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// `result` with its error as its `Debug` text, so that two results compare
/// by variant, data and message: `Error` has no `PartialEq`.
fn keyed<T>(result: Result<T, Error>) -> Result<T, String> {
    result.map_err(|error| format!("{error:?}"))
}

/// Whether `finish` on `bytes` gives what `decode_with` gives under
/// `options`, with `Incomplete` turned into a truncation error.
fn check_finish(name: &str, bytes: &[u8], decoder: IncrementalDecoder, options: &DecodeOptions) {
    let finished = decoder.finish();
    match decode_with(bytes, options) {
        Err(Error::Incomplete(why)) => match finished {
            Err(Error::Codestream(m) | Error::FileFormat(m)) => {
                assert!(
                    m.message().contains(why.message()),
                    "{name}, {} bytes: {m}",
                    bytes.len()
                );
            }
            other => panic!("{name}, {} bytes: {other:?}", bytes.len()),
        },
        expected => assert_eq!(
            keyed(finished),
            keyed(expected),
            "{name}, {} bytes",
            bytes.len()
        ),
    }
}

/// Feed `bytes` in pieces whose sizes `rng` draws, refining after each:
/// every image is the strict-off decode of the bytes fed so far, an error
/// is that decode's error, and `finish` is the decode of the whole file.
///
/// A refine must also give an image when the cadence says one is due: when
/// the bytes fed are at least twice those of the last refine that decoded
/// or tried to, and the decode of the bytes fed gives an image. Before any
/// try, the first refine with at least one byte fed tries. A try that gave
/// no image, for a main header cut short or a work bound, still counts: the
/// next one waits for twice its bytes.
fn stream(name: &str, bytes: &[u8], rng: &mut Rng, options: &DecodeOptions) -> usize {
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    let mut fed = 0;
    let mut images = 0;
    // The bytes fed at the last refine that decoded or tried to.
    let mut tried = 0;
    while fed < bytes.len() {
        // Pieces from 1 byte to about a sixteenth of the file.
        let piece = 1 + rng.below((bytes.len() / 16).max(1));
        let end = (fed + piece).min(bytes.len());
        decoder.feed(&bytes[fed..end]);
        fed = end;
        assert_eq!(decoder.bytes_fed(), fed);
        let last = tried;
        let due = fed >= 2 * last;
        if due {
            tried = fed;
        }
        match decoder.refine() {
            Ok(Some(image)) => {
                images += 1;
                tried = fed;
                assert_eq!(decoder.bytes_decoded(), fed, "{name}");
                let expected = decode_with(&bytes[..fed], &options.clone().with_strict(false));
                assert_eq!(Ok(image), keyed(expected), "{name}, {fed} bytes");
            }
            Ok(None) => {
                if due {
                    let expected = refine_oracle(&bytes[..fed], options);
                    assert!(
                        !matches!(expected, Ok(Some(_))),
                        "{name}: a refine at {fed} bytes, due after a try at {last}, \
                         gave no image"
                    );
                }
            }
            Err(error) => {
                let expected = refine_oracle(&bytes[..fed], options);
                assert_eq!(keyed(Err(error)), keyed(expected), "{name}, {fed} bytes");
                // A refine error is final, so the whole input must err too:
                // a decoder that gave up for good on a prefix of a file
                // that decodes would pass the check above.
                assert!(
                    decode_with(bytes, &options.clone().with_strict(false)).is_err(),
                    "{name}: refine failed for good at {fed} bytes, but the whole input decodes"
                );
                return images;
            }
        }
    }
    check_finish(name, bytes, decoder, options);
    images
}

/// Feed `bytes` in two halves, refining after each: both refines decode,
/// each to the strict-off decode of the bytes fed, and `finish` is the
/// decode of the whole.
fn halves(name: &str, bytes: &[u8], options: &DecodeOptions) -> usize {
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    let mut images = 0;
    for fed in [bytes.len() / 2, bytes.len()] {
        decoder.feed(&bytes[decoder.bytes_fed()..fed]);
        let refined = keyed(decoder.refine());
        let expected = keyed(refine_oracle(&bytes[..fed], options));
        if matches!(refined, Ok(Some(_))) {
            images += 1;
        }
        // A decode whose cost is in its geometry, not its bytes, can spend
        // most of the bound on the first half: then the refine of the whole
        // waits, and `finish` decodes it.
        if fed == bytes.len() && refined == Ok(None) && matches!(expected, Ok(Some(_))) {
            continue;
        }
        assert_eq!(refined, expected, "{name}, {fed} bytes");
        if refined.is_err() {
            return images;
        }
    }
    check_finish(name, bytes, decoder, options);
    images
}

/// The points to cut a file of `len` bytes at: every byte of a small file,
/// and for a larger one every eighth byte of the first 64, where the
/// headers are, and points drawn over the rest.
fn cuts(len: usize, rng: &mut Rng) -> Vec<usize> {
    if len <= 512 {
        return (0..=len).collect();
    }
    let drawn = 8;
    let mut cuts: Vec<usize> = (0..=64).step_by(8).collect();
    cuts.extend((0..drawn).map(|_| 64 + rng.below(len - 64)));
    cuts.extend([len - 2, len - 1, len]);
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

/// Cut `bytes` at each of `cuts`, and feed the prefix to a new decoder in
/// two pieces: its refine is the strict-off decode of the prefix, and its
/// `finish` the strict decode.
fn cut(name: &str, bytes: &[u8], cuts: &[usize], rng: &mut Rng, options: &DecodeOptions) {
    for &at in cuts {
        let prefix = &bytes[..at];
        let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
        let split = rng.below(at + 1);
        decoder.feed(&prefix[..split]);
        decoder.feed(&prefix[split..]);
        let refined = decoder.refine();
        assert_eq!(
            keyed(refined),
            keyed(refine_oracle(prefix, options)),
            "{name}, cut at {at}"
        );
        check_finish(name, prefix, decoder, options);
    }
}

/// How much of a fixture the test feeds.
const HEAD: usize = 96 << 10;

/// The most samples a fixture is decoded at: a larger image is decoded at a
/// resolution reduction, since each decode of a prefix reconstructs the
/// whole image, however few bytes it holds.
const SAMPLES: u64 = 1 << 14;

/// The options to decode `bytes` under: the defaults, with a reduction that
/// brings a large image under `SAMPLES`, as far as its levels allow.
fn options_for(bytes: &[u8]) -> DecodeOptions {
    let Ok(header) = read_header(bytes) else {
        return DecodeOptions::default();
    };
    let samples = |r: u8| {
        let side = |v: u32| u64::from(v.div_ceil(1 << r));
        side(header.width) * side(header.height) * header.components.len() as u64
    };
    let reduction = (0..=header.max_reduction)
        .find(|&r| samples(r) <= SAMPLES)
        .unwrap_or(header.max_reduction);
    DecodeOptions::default().with_resolution_reduction(reduction)
}

/// `options` with a window over the middle of the image, in the image's
/// grid, or `None` when the header is not whole.
fn windowed(bytes: &[u8], options: &DecodeOptions) -> Option<DecodeOptions> {
    let header = read_header(bytes).ok()?;
    let (w, h) = (header.width, header.height);
    let window = Window::new(
        w / 4,
        h / 4,
        (w - w / 4).max(w / 4 + 1),
        (h - h / 4).max(h / 4 + 1),
    );
    Some(options.clone().with_region(Region::Window(window)))
}

/// The fixture directories of files made to reach the work bounds.
const COSTLY: [&str; 4] = ["work_bound/", "tier1_work_bound/", "psot0/", "budget/"];

/// The fixtures the `share`-th of `SHARES` tests checks: every
/// `SHARES`-th file, so that the costly ones spread over the tests.
fn share(share: usize) -> Vec<PathBuf> {
    files(&fixtures())
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % SHARES == share)
        .map(|(_, path)| path)
        .collect()
}

/// How many tests the fixtures are split over, so they run in parallel.
const SHARES: usize = 4;

/// Each fixture, fed in pieces and cut at many points, decodes as the
/// one-shot decode of the same bytes does. Each refine and `finish` is
/// checked against `decode_with`, so this covers the MIXED HT files and the
/// PTERM, POC and multi-tile files the corpus holds.
fn check_share(index: usize) {
    let mut rng = Rng(217 + index as u64);
    let (mut count, mut points, mut images) = (0, 0, 0);
    for path in share(index) {
        let name = path.strip_prefix(fixtures()).unwrap().display().to_string();
        let whole = std::fs::read(&path).unwrap();
        let options = options_for(&whole);
        // A large file is fed only its first `HEAD` bytes, which keeps the
        // test's time in proportion to the fixture count, not their size.
        let bytes = &whole[..whole.len().min(HEAD)];
        // The files built to cost the most work a decode allows take up to
        // a second each to decode unoptimized, so they are fed in two
        // halves, and not cut.
        if COSTLY.iter().any(|dir| name.starts_with(dir)) {
            images += halves(&name, bytes, &options);
            count += 1;
            continue;
        }
        images += stream(&name, bytes, &mut rng, &options);
        // Every third file also streams to a window, and every third to a
        // component subset, which keeps the added time small.
        match count % 3 {
            0 => {
                if let Some(options) = windowed(bytes, &options) {
                    images += stream(&name, bytes, &mut rng, &options);
                }
            }
            1 => {
                let subset = options.clone().with_components(Some(vec![0]));
                images += stream(&name, bytes, &mut rng, &subset);
            }
            _ => {}
        }
        let cuts = cuts(bytes.len(), &mut rng);
        points += cuts.len();
        cut(&name, bytes, &cuts, &mut rng, &options);
        count += 1;
    }
    // The fixtures in the published package alone are well over 100.
    assert!(count > 100 / SHARES, "{count} fixtures");
    println!("{count} fixtures, {points} cut points, {images} streamed images");
}

#[test]
fn fixtures_0_refine_to_the_one_shot_decode_of_their_prefixes() {
    check_share(0);
}

#[test]
fn fixtures_1_refine_to_the_one_shot_decode_of_their_prefixes() {
    check_share(1);
}

#[test]
fn fixtures_2_refine_to_the_one_shot_decode_of_their_prefixes() {
    check_share(2);
}

#[test]
fn fixtures_3_refine_to_the_one_shot_decode_of_their_prefixes() {
    check_share(3);
}

/// The MIXED HT files, where each code-block is an HT or a Part 1 block,
/// refine to the one-shot decode with a reduction and a layer cap too, and
/// with `strict` off for `finish`.
#[test]
fn mixed_ht_files_refine_under_other_options() {
    let dir = fixtures().join("conformance/ht/codestreams");
    if !dir.exists() {
        // The conformance corpus is left out of the published package.
        return;
    }
    let mut rng = Rng(15444);
    let mut seen = 0;
    for path in files(&dir) {
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        if !name.starts_with("ds0_hm_") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        for options in [
            DecodeOptions::default().with_resolution_reduction(1),
            DecodeOptions::default().with_max_quality_layers(Some(1)),
            lenient(),
        ] {
            assert!(stream(&name, &bytes, &mut rng, &options) > 0, "{name}");
        }
        seen += 1;
    }
    assert_eq!(seen, 3, "the MIXED file count changed");
}

fn layers_file(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join("layers").join(name)).unwrap()
}

/// A PTERM file decoded to a prefix, then fed the rest, finishes to the
/// one-shot strict decode, and the image of the prefix has the later layer
/// missing.
#[test]
fn a_pterm_file_resumes_from_a_prefix_to_the_strict_decode() {
    let bytes = layers_file("pterm_cut.j2k");
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes[..226]);
    let prefix = decoder.refine().unwrap().expect("the main header is whole");
    assert!(!prefix.report.is_complete());
    assert_eq!(
        Some(prefix),
        refine_oracle(&bytes[..226], &lenient()).unwrap()
    );
    decoder.feed(&bytes[226..]);
    // Whether this refine decodes is the cadence's choice; if it does, the
    // image is the one-shot decode of all 231 bytes.
    if let Some(image) = decoder.refine().unwrap() {
        assert_eq!(Some(image), refine_oracle(&bytes, &lenient()).unwrap());
    }
    let finished = decoder.finish().unwrap();
    assert_eq!(
        finished,
        decode_with(&bytes, &DecodeOptions::default()).unwrap()
    );
    let refined = refine_oracle(&bytes, &lenient()).unwrap().unwrap();
    assert!(refined.report.is_complete());
    assert_eq!(finished.components, refined.components);
}

/// A damaged PTERM segment is refused once the bytes hold it whole: by
/// `refine` when the segment is not the block's last, and by the strict
/// `finish` when it is.
#[test]
fn a_damaged_pterm_segment_is_refused() {
    // `bypass_pterm.j2k` with byte 188 flipped damages an MQ segment of the
    // LL block that layer 0 brings whole.
    let mut bad = layers_file("bypass_pterm.j2k");
    bad[188] ^= 0x10;
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bad[..300]);
    match decoder.refine() {
        Err(Error::Codestream(m)) => {
            assert!(m.message().contains("predictable termination"), "{m}")
        }
        other => panic!("{other:?}"),
    }

    // `pterm_cut.j2k` with byte 84 flipped leaves its one segment with
    // bytes unconsumed: a prefix whose block can still take passes
    // decodes, and the end of the input makes it an error.
    let mut bad = layers_file("pterm_cut.j2k");
    bad[84] ^= 0x10;
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    for piece in bad.chunks(16) {
        decoder.feed(piece);
        if let Err(error) = decoder.refine() {
            assert_eq!(
                keyed(Err(error)),
                keyed(refine_oracle(&bad[..decoder.bytes_fed()], &lenient()))
            );
        }
    }
    match decoder.finish() {
        Err(Error::Codestream(m)) => {
            assert!(m.message().contains("predictable termination"), "{m}")
        }
        other => panic!("{other:?}"),
    }
}

/// The end of the input turns a main header cut short into a truncation
/// error: `Error::Codestream` inside a codestream's main header, and
/// `Error::FileFormat` inside a JP2 file's boxes.
#[test]
fn finish_refuses_a_header_cut_short() {
    let codestream = std::fs::read(fixtures().join("jpeg2000_regular_latlon.j2k")).unwrap();
    let jp2 = std::fs::read(fixtures().join("jp2_cdef_rgba.jp2")).unwrap();
    for (bytes, cut, box_layer) in [
        (&codestream, 0, false),
        (&codestream, 40, false),
        (&jp2, 20, true),
    ] {
        let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
        decoder.feed(&bytes[..cut]);
        assert!(matches!(decoder.refine(), Ok(None)));
        match (decoder.finish(), box_layer) {
            (Err(Error::Codestream(m)), false) | (Err(Error::FileFormat(m)), true) => {
                assert!(m.message().starts_with("the input is truncated: "), "{m}");
            }
            other => panic!("{cut} bytes: {other:?}"),
        }
    }
}

/// Options that are invalid whatever the input are refused when the
/// decoder is made, as `decode_with` refuses them before it reads a byte.
#[test]
fn invalid_options_are_refused_up_front() {
    let options = DecodeOptions::default().with_max_quality_layers(Some(0));
    assert!(matches!(
        IncrementalDecoder::new(options),
        Err(Error::InvalidOptions(_))
    ));
}

/// A limit more bytes cannot clear is an error from the first refine that
/// reaches it, not a wait: an image past the sample limit is refused as
/// soon as its header is in.
#[test]
fn a_limit_more_bytes_cannot_clear_is_an_error() {
    let path = fixtures().join("sample_limit/flat_32769x16385_tiled_lossless.j2k");
    let bytes = std::fs::read(&path).unwrap();
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    for piece in bytes.chunks(4096) {
        decoder.feed(piece);
        match decoder.refine() {
            Ok(None) => continue,
            Err(Error::Limit(_)) => return,
            other => panic!("{} bytes: {other:?}", decoder.bytes_fed()),
        }
    }
    panic!("no refine reached the sample limit");
}

/// A work-bound refusal on a prefix is a wait, not an error: the bounds
/// grow with the bytes. `t1bomb_1024_64_88.j2k` with 6000 zero bytes added
/// to its tile-part needs more code-block work than the floor of the bound
/// allows on some prefixes, though the whole file is within its bound.
#[test]
fn a_work_bound_a_prefix_reaches_is_a_wait() {
    let path = fixtures().join("tier1_work_bound/t1bomb_1024_64_88.j2k");
    let mut bytes = std::fs::read(&path).unwrap();
    // The codestream ends with EOC; put the zeros before it, in the one
    // tile-part, and grow its Psot to match.
    assert_eq!(&bytes[bytes.len() - 2..], &[0xFF, 0xD9]);
    let sot = bytes
        .windows(2)
        .position(|w| w == [0xFF, 0x90])
        .expect("an SOT marker");
    let psot = u32::from_be_bytes(bytes[sot + 6..sot + 10].try_into().unwrap());
    assert_eq!(psot as usize, bytes.len() - 2 - sot, "one tile-part to EOC");
    let eoc = bytes.len() - 2;
    bytes.splice(eoc..eoc, std::iter::repeat_n(0u8, 6000));
    bytes[sot + 6..sot + 10].copy_from_slice(&(psot + 6000).to_be_bytes());

    let lenient = lenient();
    // The premise: a prefix is refused for work that the whole file is not.
    let refused = (1..=bytes.len() / 256)
        .any(|k| decode_with(&bytes[..256 * k], &lenient).is_err_and(|e| is_work_limit(&e)));
    assert!(refused, "no prefix reaches the work bound");
    assert!(decode_with(&bytes, &lenient).is_ok());

    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    for piece in bytes.chunks(256) {
        decoder.feed(piece);
        if let Err(error) = decoder.refine() {
            panic!("{} bytes: {error:?}", decoder.bytes_fed());
        }
    }
    assert_eq!(
        keyed(decoder.finish()),
        keyed(decode_with(&bytes, &DecodeOptions::default()))
    );
}

/// A file the default code-block work bound refuses.
fn bomb() -> Vec<u8> {
    std::fs::read(fixtures().join("tier1_work_bound/t1bomb_1024_64_88.j2k")).unwrap()
}

/// After `finish` refuses a work limit, `into_bytes` hands the bytes back,
/// and decoding them with the limit raised gives what `decode_with` gives.
#[test]
fn into_bytes_recovers_the_input_after_a_refused_work_limit() {
    let bytes = bomb();
    let options = DecodeOptions::default();
    let refused = decode_with(&bytes, &options).unwrap_err();
    assert!(is_work_limit(&refused), "{refused:?}");

    let mut decoder = IncrementalDecoder::new(options).unwrap();
    for piece in bytes.chunks(4096) {
        decoder.feed(piece);
    }
    let error = decoder.finish().unwrap_err();
    assert!(is_work_limit(&error), "{error:?}");

    let recovered = decoder.into_bytes();
    assert_eq!(recovered, bytes);
    let raised = DecodeOptions::default().with_code_block_work_limit(WorkLimit::Unlimited);
    let expected = decode_with(&bytes, &raised);
    assert!(expected.is_ok());
    assert_eq!(
        keyed(decode_with(&recovered, &raised)),
        keyed(expected.clone())
    );
    let mut again = IncrementalDecoder::new(raised).unwrap();
    again.feed(&recovered);
    assert_eq!(keyed(again.finish()), keyed(expected));
}

/// `finish` borrows the decoder, and calling it again, also after a refine,
/// gives the same result.
#[test]
fn finish_twice_gives_equal_results() {
    let bytes = bomb();
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes);
    let first = decoder.finish();
    assert!(first.is_err());
    assert_eq!(keyed(decoder.finish()), keyed(first));

    let file =
        std::fs::read(fixtures().join("tier1_work_bound/opj_flat2048_16bit_n1.j2k")).unwrap();
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&file[..file.len() / 2]);
    let _ = decoder.refine();
    let first = decoder.finish();
    assert_eq!(keyed(decoder.finish()), keyed(first));
    assert_eq!(decoder.bytes_fed(), file.len() / 2);
}

/// `header` is the header `read_header_with` reads from the bytes fed so
/// far under the decoder's options, `Ok(None)` while the main header is cut
/// short, and `bytes` borrows the bytes fed: fed a byte at a time through
/// the main header of a codestream and the boxes of a JP2 file.
#[test]
fn header_and_bytes_follow_the_bytes_fed() {
    for (name, options) in [
        ("layers/lrcp.j2k", DecodeOptions::default()),
        (
            "jp2_real/byte_tlm_plt.jp2",
            DecodeOptions::default().with_resolution_reduction(1),
        ),
    ] {
        let bytes = std::fs::read(fixtures().join(name)).unwrap();
        let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
        assert_eq!(decoder.header().unwrap(), None, "{name}");
        // Past the first tile-part's header, and a little of its data.
        let sot = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
        let mut headers = 0;
        for end in 1..sot + 32 {
            decoder.feed(&bytes[end - 1..end]);
            assert_eq!(decoder.bytes(), &bytes[..end], "{name}");
            let expected = match read_header_with(&bytes[..end], &options) {
                Err(Error::Incomplete(_)) => Ok(None),
                read => read.map(Some),
            };
            let header = decoder.header();
            assert_eq!(
                keyed(header.clone()),
                keyed(expected),
                "{name}, {end} bytes"
            );
            headers += usize::from(header.unwrap().is_some());
        }
        assert!(headers > 0, "{name}");
        // A change of options shows in the header.
        decoder.set_resolution_reduction(0).unwrap();
        assert_eq!(
            decoder.header().unwrap(),
            Some(read_header(decoder.bytes()).unwrap())
        );
    }
    // A header that cannot be read is the error `read_header` gives, and
    // so is a reduction the main header cannot take.
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(b"not a codestream");
    assert_eq!(
        keyed(decoder.header()),
        keyed(read_header(b"not a codestream").map(Some))
    );
    let bytes = std::fs::read(fixtures().join("layers/lrcp.j2k")).unwrap();
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes);
    decoder.set_resolution_reduction(30).unwrap();
    assert!(matches!(decoder.header(), Err(Error::InvalidOptions(_))));
}

/// A change of options that are invalid whatever the input is refused, and
/// the decoder keeps the options it had: `finish` decodes under them.
#[test]
fn a_change_to_invalid_options_is_refused() {
    let bytes = std::fs::read(fixtures().join("layers/lrcp.j2k")).unwrap();
    let options = DecodeOptions::default().with_max_quality_layers(Some(1));
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    decoder.feed(&bytes);
    assert!(matches!(
        decoder.set_max_quality_layers(Some(0)),
        Err(Error::InvalidOptions(_))
    ));
    assert!(matches!(
        decoder.set_region(Region::Window(Window::new(4, 4, 4, 8))),
        Err(Error::InvalidOptions(_))
    ));
    assert_eq!(
        keyed(decoder.finish()),
        keyed(decode_with(&bytes, &options))
    );
}

/// A change of options applies to the next refine that decodes, at the
/// usual cadence, and to `finish`, and clears an error the old options
/// gave: a reduction past the file's levels is refused, and after a change
/// to one it can take, the refine of twice the bytes gives the image.
#[test]
fn a_change_of_options_applies_to_the_later_refines_and_clears_their_error() {
    let bytes = std::fs::read(fixtures().join("layers/lrcp.j2k")).unwrap();
    let mut decoder =
        IncrementalDecoder::new(DecodeOptions::default().with_resolution_reduction(30)).unwrap();
    let half = bytes.len() / 2;
    decoder.feed(&bytes[..half]);
    assert!(matches!(decoder.refine(), Err(Error::InvalidOptions(_))));
    let options = DecodeOptions::default()
        .with_resolution_reduction(1)
        .with_max_quality_layers(Some(2))
        .with_region(Region::Window(Window::new(0, 0, 9, 7)));
    decoder.set_resolution_reduction(1).unwrap();
    decoder.set_max_quality_layers(Some(2)).unwrap();
    decoder.set_region(options.region.clone()).unwrap();
    // Not twice the bytes yet.
    decoder.feed(&bytes[half..half + 1]);
    assert!(matches!(decoder.refine(), Ok(None)));
    decoder.feed(&bytes[half + 1..]);
    assert_eq!(
        keyed(decoder.refine()),
        keyed(refine_oracle(&bytes, &options))
    );
    assert!(decoder.refine().unwrap().is_none());
    assert_eq!(
        keyed(decoder.finish()),
        keyed(decode_with(&bytes, &options))
    );
}
