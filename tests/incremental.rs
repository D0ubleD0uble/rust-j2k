//! `IncrementalDecoder` against one-shot decodes of the same bytes.
//!
//! Every image a refine returns must be the image `decode_with` gives with
//! `strict` off on the bytes fed so far, and `finish` must give what
//! `decode_with` gives on all of them, with a main header cut short turned
//! from `Error::Incomplete` into a truncation error. This runs on every
//! codestream, JP2 and JPH file under `tests/fixtures/`, fed in pieces of
//! many sizes, and cut at many points.

use std::path::{Path, PathBuf};

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{
    DecodeOptions, Error, Image, IncrementalDecoder, LimitKind, Refined, Region, Waiting, Window,
    WorkLimit, decode_with, read_header, read_header_with,
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

/// Whether `error` is a refusal by the sample limit, which more bytes raise
/// too.
fn is_sample_limit(error: &Error) -> bool {
    matches!(error, Error::Limit(limit) if limit.kind() == LimitKind::Samples)
}

/// What a refine on `prefix` must give: the strict-off decode, with a main
/// header cut short and a refusal by a work bound or the sample limit as
/// `None`.
fn refine_oracle(prefix: &[u8], options: &DecodeOptions) -> Result<Option<Image>, Error> {
    match decode_with(prefix, &options.clone().with_strict(false)) {
        Ok(image) => Ok(Some(image)),
        Err(Error::Incomplete(_)) => Ok(None),
        Err(error) if is_work_limit(&error) || is_sample_limit(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// `result` with its error as its `Debug` text, so that two results compare
/// by variant, data and message: `Error` has no `PartialEq`.
fn keyed<T>(result: Result<T, Error>) -> Result<T, String> {
    result.map_err(|error| format!("{error:?}"))
}

/// A refine, with the reason it waited dropped.
fn refine_image(decoder: &mut IncrementalDecoder) -> Result<Option<Image>, Error> {
    decoder.refine().map(Refined::image)
}

/// Check `why`, the reason a refine that tried on `prefix` waited, against
/// the strict-off decode of `prefix` under `options`. A header cut short is
/// `HeaderIncomplete`, and a refusal by the sample limit or the charge for
/// missing tiles, or by the packet bound of that decode, is the same
/// refusal, under the variant that names it: the charge for missing tiles
/// is `MissingTiles`, and the packet bound is a `WorkLimit`. A `WorkLimit`
/// of the code-block bound is that decode's refusal too, or stands in for
/// an error it gives later in the file. `BoundSpent` comes only where that
/// decode passes the code-block bound: it gives an image, or an error that
/// is not a limit. The bound on the walks done again can refuse whatever
/// that decode gives.
fn check_waiting(name: &str, why: &Waiting, prefix: &[u8], options: &DecodeOptions) {
    let one_shot = decode_with(prefix, &options.clone().with_strict(false));
    let at = prefix.len();
    match (why, &one_shot) {
        (Waiting::HeaderIncomplete, Err(Error::Incomplete(_))) => {}
        (
            Waiting::SampleLimit(limit) | Waiting::MissingTiles(limit) | Waiting::WorkLimit(limit),
            Err(Error::Limit(expected)),
        ) if limit.kind() != LimitKind::CodeBlockWork => {
            assert_eq!(
                format!("{limit:?}"),
                format!("{expected:?}"),
                "{name}, {at} bytes"
            );
            let kind = match why {
                Waiting::SampleLimit(_) => LimitKind::Samples,
                _ => LimitKind::PacketWork,
            };
            assert_eq!(limit.kind(), kind, "{name}, {at} bytes: {why:?}");
            // The two packet-work refusals carry the same kind; the
            // one-shot message says which it was.
            if kind == LimitKind::PacketWork {
                let missing = expected.message().contains("the missing tiles cost");
                assert_eq!(
                    matches!(why, Waiting::MissingTiles(_)),
                    missing,
                    "{name}, {at} bytes: {why:?}"
                );
            }
        }
        (Waiting::WorkLimit(limit), Err(Error::Limit(expected)))
            if limit.kind() == LimitKind::CodeBlockWork =>
        {
            assert_eq!(
                format!("{limit:?}"),
                format!("{expected:?}"),
                "{name}, {at} bytes"
            );
        }
        (Waiting::WorkLimit(limit), Err(error))
            if limit.kind() == LimitKind::CodeBlockWork
                && !matches!(error, Error::Incomplete(_)) => {}
        (Waiting::BoundSpent(limit), _)
            if !matches!(one_shot, Err(Error::Limit(_) | Error::Incomplete(_))) =>
        {
            assert_eq!(limit.kind(), LimitKind::CodeBlockWork, "{name}, {at} bytes");
            assert_eq!(limit.retry_with(), None, "{name}, {at} bytes");
        }
        (Waiting::RepeatLimit { .. }, _) => {}
        _ => panic!("{name}, {at} bytes: waited for {why:?}, where the decode gives {one_shot:?}"),
    }
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
            Ok(Refined::Image(image)) => {
                images += 1;
                tried = fed;
                assert_eq!(decoder.bytes_decoded(), fed, "{name}");
                let expected = decode_with(&bytes[..fed], &options.clone().with_strict(false));
                assert_eq!(Ok(image), keyed(expected), "{name}, {fed} bytes");
            }
            Ok(Refined::Waiting(why)) => {
                if due {
                    check_waiting(name, &why, &bytes[..fed], options);
                    let expected = refine_oracle(&bytes[..fed], options);
                    assert!(
                        !matches!(expected, Ok(Some(_))),
                        "{name}: a refine at {fed} bytes, due after a try at {last}, \
                         gave no image"
                    );
                } else {
                    // Not due: the refine waits for the bytes to double.
                    assert!(
                        matches!(why, Waiting::MoreBytes { until, .. } if until == 2 * last),
                        "{name}, {fed} bytes after a try at {last}: {why:?}"
                    );
                }
            }
            Ok(_) => unreachable!("a new variant of `Refined`"),
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
        let refined = keyed(refine_image(&mut decoder));
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
        match &refined {
            Ok(Refined::Waiting(Waiting::MoreBytes { until: 1, .. })) if at == 0 => {}
            Ok(Refined::Waiting(why)) => check_waiting(name, why, prefix, options),
            _ => {}
        }
        assert_eq!(
            keyed(refined.map(Refined::image)),
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
    let prefix = decoder
        .refine()
        .unwrap()
        .image()
        .expect("the main header is whole");
    assert!(!prefix.report.is_complete());
    assert_eq!(
        Some(prefix),
        refine_oracle(&bytes[..226], &lenient()).unwrap()
    );
    decoder.feed(&bytes[226..]);
    // Whether this refine decodes is the cadence's choice; if it does, the
    // image is the one-shot decode of all 231 bytes.
    if let Some(image) = decoder.refine().unwrap().image() {
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

/// A refine of a main header cut short waits with `HeaderIncomplete`: cut
/// inside the SIZ marker segment of a codestream, and inside the boxes of a
/// JP2 file. With no bytes fed it waits for the first byte. The end of the
/// input turns the cut into a truncation error: `Error::Codestream` inside a
/// codestream's main header, and `Error::FileFormat` inside a JP2 file's
/// boxes.
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
        let refined = decoder.refine().unwrap();
        if cut == 0 {
            assert!(
                matches!(
                    refined,
                    Refined::Waiting(Waiting::MoreBytes { until: 1, .. })
                ),
                "{refined:?}"
            );
        } else {
            assert!(
                matches!(refined, Refined::Waiting(Waiting::HeaderIncomplete)),
                "{cut} bytes: {refined:?}"
            );
        }
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
/// reaches it, not a wait: an image too large for the target to address is
/// refused as soon as its header is in, and every later refine gives the
/// same error.
#[test]
fn a_limit_more_bytes_cannot_clear_is_an_error() {
    let side = u32::MAX;
    let siz = synth::siz_body([side, side, 0, 0, side, side, 0, 0], &[[7, 1, 1]]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &synth::cod_body(1, 5, 4, 0)),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8; 16])),
    ]
    .concat();
    bytes.extend(synth::tile_part(0, 0, 1, &[], &[0; 4000]));
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    let mut first = None;
    for piece in bytes.chunks(64) {
        decoder.feed(piece);
        match (decoder.refine(), &first) {
            (Ok(Refined::Waiting(_)), None) => {}
            (Err(Error::Limit(limit)), None) => {
                assert_eq!(limit.kind(), LimitKind::AddressSpace, "{limit}");
                first = Some(format!("{limit:?}"));
            }
            (Err(Error::Limit(limit)), Some(first)) => {
                assert_eq!(&format!("{limit:?}"), first);
            }
            other => panic!("{} bytes: {other:?}", decoder.bytes_fed()),
        }
    }
    assert!(first.is_some(), "no refine reached the limit");
}

/// The sample limit grows with the bytes, so a prefix it refuses is a
/// wait: every refine of the flat 32769x16385 file, whose 27 KB hold the
/// limit to 2^29, waits, and `finish` reports the limit, as the one-shot
/// decode does. Each refine that tries once the header is in waits with
/// `SampleLimit`, carrying the refusal the one-shot decode gives.
#[test]
fn a_sample_limit_refusal_is_a_wait() {
    let path = fixtures().join("sample_limit/flat_32769x16385_tiled_lossless.j2k");
    let bytes = std::fs::read(&path).unwrap();
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    let (mut refused, mut waited) = (0, 0);
    for piece in bytes.chunks(4096) {
        decoder.feed(piece);
        let prefix = &bytes[..decoder.bytes_fed()];
        let one_shot = decode_with(prefix, &lenient());
        match (decoder.refine(), &one_shot) {
            (Ok(Refined::Waiting(Waiting::SampleLimit(limit))), Err(Error::Limit(expected))) => {
                assert_eq!(format!("{limit:?}"), format!("{expected:?}"));
                assert!(limit.retry_with().is_some(), "{limit}");
                waited += 1;
            }
            (Ok(Refined::Waiting(Waiting::MoreBytes { .. } | Waiting::HeaderIncomplete)), _) => {}
            (other, _) => panic!("{} bytes: {other:?}", prefix.len()),
        }
        if one_shot.is_err_and(|e| is_sample_limit(&e)) {
            refused += 1;
        }
    }
    assert!(refused > 0, "no prefix reached the sample limit");
    assert!(waited > 0, "no refine waited on the sample limit");
    match decoder.finish() {
        Err(Error::Limit(limit)) => assert_eq!(limit.kind(), LimitKind::Samples),
        other => panic!("{other:?}"),
    }
}

/// Each refine is held to the sample limit of its own prefix: a window
/// inside the first tile of `synth::two_large_tiles` counts one tile just
/// over 2^29 samples, which the first half of the file is too short for
/// and the whole file is not. The refine of the half waits; the refine of
/// the whole gives the strict-off decode of it.
#[cfg(target_pointer_width = "64")]
#[test]
fn each_refine_is_held_to_the_sample_limit_of_its_prefix() {
    let bytes = synth::two_large_tiles(synth::LARGE_TILE_BYTES + 1);
    let options = DecodeOptions::default().with_region(Region::Window(Window::new(0, 0, 16, 16)));
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    let half = bytes.len() / 2;
    decoder.feed(&bytes[..half]);
    assert!(
        decode_with(&bytes[..half], &options.clone().with_strict(false))
            .is_err_and(|e| is_sample_limit(&e))
    );
    assert!(matches!(
        decoder.refine(),
        Ok(Refined::Waiting(Waiting::SampleLimit(_)))
    ));
    assert!(matches!(decoder.header(), Err(Error::Limit(_))));
    decoder.feed(&bytes[half..]);
    let refined = decoder
        .refine()
        .expect("decodes")
        .image()
        .expect("an image");
    assert_eq!(decoder.bytes_decoded(), bytes.len());
    assert_eq!((refined.width, refined.height), (16, 16));
    let expected = refine_oracle(&bytes, &options).unwrap().unwrap();
    assert_eq!(keyed(Ok(refined)), keyed(Ok(expected)));
    assert!(decoder.header().unwrap().is_some());
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
    let mut waited = 0;
    for piece in bytes.chunks(256) {
        decoder.feed(piece);
        match decoder.refine() {
            Ok(Refined::Waiting(Waiting::WorkLimit(limit))) => {
                assert_eq!(limit.kind(), LimitKind::CodeBlockWork, "{limit}");
                waited += 1;
            }
            Ok(_) => {}
            Err(error) => panic!("{} bytes: {error:?}", decoder.bytes_fed()),
        }
    }
    assert!(waited > 0, "no refine waited on the work bound");
    assert_eq!(
        keyed(decoder.finish()),
        keyed(decode_with(&bytes, &DecodeOptions::default()))
    );
}

/// A tile whose walk starts again at each of its tile-parts, fed 64 bytes
/// at a time with a refine after each: the due refines that the bound on
/// the walks done again refuses wait with `RepeatLimit`, though the
/// one-shot decode of their bytes gives an image, and every due refine that
/// waits passes `check_waiting`. A decode of the bytes fed then, with the
/// decoder's options and `strict` off, gives that image.
#[test]
fn a_restarted_walk_streamed_waits_with_repeat_limit() {
    let bytes = synth::restarted_poc_tile(4, 13, 400);
    let options = lenient();
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    let (mut tried, mut repeats) = (0, 0);
    for piece in bytes.chunks(64) {
        decoder.feed(piece);
        let fed = decoder.bytes_fed();
        let due = fed >= 2 * tried;
        if due {
            tried = fed;
        }
        match decoder.refine().unwrap() {
            Refined::Waiting(why) if due => {
                check_waiting("restarted_poc_tile", &why, &bytes[..fed], &options);
                if matches!(why, Waiting::RepeatLimit { .. }) {
                    repeats += 1;
                    let now = decode_with(
                        decoder.bytes(),
                        &decoder.options().clone().with_strict(false),
                    );
                    assert!(now.is_ok(), "{fed} bytes: {now:?}");
                }
            }
            Refined::Waiting(why) => assert!(
                matches!(why, Waiting::MoreBytes { until, .. } if until == 2 * tried),
                "{fed} bytes: {why:?}"
            ),
            _ => {}
        }
    }
    assert!(repeats > 0, "no refine waited on the repeat bound");
}

/// A file the default code-block work bound refuses.
fn bomb() -> Vec<u8> {
    std::fs::read(fixtures().join("tier1_work_bound/t1bomb_1024_64_88.j2k")).unwrap()
}

/// A refine that a work bound refuses waits with `WorkLimit`, carrying the
/// refusal the one-shot decode of its bytes gives, hint included: the
/// code-block bound on `t1bomb_1024_64_88.j2k`, and the packet bound on
/// `sweep_rpcl_1024_64.j2k`. Retries with `with_retry` of each hint, a new
/// decoder each, reach a refine that gives the image.
#[test]
fn a_work_bound_refusal_waits_with_the_hint_the_decode_gives() {
    let sweep = std::fs::read(fixtures().join("work_bound/sweep_rpcl_1024_64.j2k")).unwrap();
    for (name, bytes, kind) in [
        ("t1bomb", bomb(), LimitKind::CodeBlockWork),
        ("sweep", sweep, LimitKind::PacketWork),
    ] {
        let expected = match decode_with(&bytes, &lenient()) {
            Err(Error::Limit(limit)) => limit,
            other => panic!("{name}: {other:?}"),
        };
        let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
        decoder.feed(&bytes);
        let limit = match decoder.refine() {
            Ok(Refined::Waiting(Waiting::WorkLimit(limit))) => limit,
            other => panic!("{name}: {other:?}"),
        };
        assert_eq!(limit.kind(), kind, "{name}");
        assert_eq!(limit.retry_with(), expected.retry_with(), "{name}");
        assert!(limit.retry_with().is_some(), "{name}");
        assert_eq!(format!("{limit:?}"), format!("{expected:?}"), "{name}");
        // Retry with the hint in a loop, as with `decode_with`: a new
        // decoder for each, as the decoder keeps its options' bounds.
        let bytes = decoder.into_bytes();
        let mut limit = limit;
        let mut options = DecodeOptions::default();
        let mut retries = 0;
        let image = loop {
            options = options.with_retry(&limit).unwrap();
            retries += 1;
            let mut again = IncrementalDecoder::new(options.clone()).unwrap();
            again.feed(&bytes);
            match again.refine() {
                Ok(Refined::Image(image)) => break image,
                Ok(Refined::Waiting(Waiting::WorkLimit(next))) if retries < 4 => limit = next,
                other => panic!("{name}, retry {retries}: {other:?}"),
            }
        };
        assert_eq!(
            image,
            decode_with(&bytes, &options.with_strict(false)).unwrap(),
            "{name}"
        );
    }
}

/// A refine that does not decode waits with `MoreBytes`, naming the bytes
/// at which a refine decodes again: 1 before any byte is fed, then twice
/// those of the last try. Meanwhile a decode of the bytes fed with the
/// decoder's options and `strict` off gives the image of those bytes now.
#[test]
fn a_short_prefix_waits_for_more_bytes() {
    let bytes = layers_file("lrcp.j2k");
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    assert!(matches!(
        decoder.refine(),
        Ok(Refined::Waiting(Waiting::MoreBytes { until: 1, .. }))
    ));
    let half = bytes.len() / 2;
    decoder.feed(&bytes[..half]);
    let first = decoder.refine().unwrap().image().expect("the header is in");
    assert_eq!(decoder.bytes_decoded(), half);
    for end in [half + 1, 2 * half - 1] {
        decoder.feed(&bytes[decoder.bytes_fed()..end]);
        match decoder.refine() {
            Ok(Refined::Waiting(Waiting::MoreBytes { until, .. })) => assert_eq!(until, 2 * half),
            other => panic!("{end} bytes: {other:?}"),
        }
        let now = decode_with(
            decoder.bytes(),
            &decoder.options().clone().with_strict(false),
        )
        .unwrap();
        assert_eq!(Some(now), refine_oracle(&bytes[..end], &lenient()).unwrap());
    }
    assert_eq!(decoder.bytes_decoded(), half, "no refine decoded");
    decoder.feed(&bytes[2 * half - 1..2 * half]);
    let second = decoder
        .refine()
        .unwrap()
        .image()
        .expect("the bytes doubled");
    assert_ne!(first, second);
    assert_eq!(
        Some(second),
        refine_oracle(&bytes[..2 * half], &lenient()).unwrap()
    );
}

/// A refine that waits on `MoreBytes` after a try on a cut header leaves
/// the header cut: a decode of the bytes fed then gives `Error::Incomplete`,
/// not an image.
#[test]
fn more_bytes_after_a_cut_header_decodes_now_as_incomplete() {
    let bytes = layers_file("lrcp.j2k");
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes[..10]);
    assert!(matches!(
        decoder.refine(),
        Ok(Refined::Waiting(Waiting::HeaderIncomplete))
    ));
    decoder.feed(&bytes[10..15]);
    assert!(matches!(
        decoder.refine(),
        Ok(Refined::Waiting(Waiting::MoreBytes { until: 20, .. }))
    ));
    let now = decode_with(
        decoder.bytes(),
        &decoder.options().clone().with_strict(false),
    );
    assert!(matches!(now, Err(Error::Incomplete(_))), "{now:?}");
}

/// A refine that the code-block bound refuses before any refine spent from
/// it waits on what a decode on its own refuses too: a decode of the bytes
/// fed then gives the same `CodeBlockWork` refusal, as an error.
#[test]
fn a_code_block_wait_with_nothing_spent_decodes_now_as_the_same_refusal() {
    let bytes = bomb();
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(&bytes);
    let limit = match decoder.refine() {
        Ok(Refined::Waiting(Waiting::WorkLimit(limit))) => limit,
        other => panic!("{other:?}"),
    };
    assert_eq!(limit.kind(), LimitKind::CodeBlockWork);
    let now = decode_with(
        decoder.bytes(),
        &decoder.options().clone().with_strict(false),
    );
    match now {
        Err(Error::Limit(expected)) => {
            assert_eq!(format!("{limit:?}"), format!("{expected:?}"));
        }
        other => panic!("{other:?}"),
    }
}

/// A refine refused only for the code-block work the earlier refines
/// spent waits on `BoundSpent`, with no hint, where a decode of its bytes
/// on its own passes the bound. `t1bomb_1024_64_88.j2k` decodes under
/// `Raised(2)` with work to spare, but not twice over: fed whole, the first
/// refine gives the image; fed again to twice the bytes, with zeros after
/// its end, the second is past what the first left. A decode of the bytes
/// fed then, and a new decoder fed them, give the image. The bomb with
/// nothing spent waits on `WorkLimit` instead (see
/// `a_code_block_wait_with_nothing_spent_decodes_now_as_the_same_refusal`).
#[test]
fn a_refine_refused_for_what_earlier_refines_spent_waits_on_bound_spent() {
    let bytes = bomb();
    let options = DecodeOptions::default().with_code_block_work_limit(WorkLimit::Raised(2));
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    decoder.feed(&bytes);
    let first = decoder
        .refine()
        .unwrap()
        .image()
        .expect("the bound covers it once");
    decoder.feed(&vec![0; bytes.len()]);
    let why = match decoder.refine() {
        Ok(Refined::Waiting(why)) => why,
        other => panic!("{other:?}"),
    };
    let Waiting::BoundSpent(limit) = &why else {
        panic!("{why:?}");
    };
    assert_eq!(limit.kind(), LimitKind::CodeBlockWork);
    assert_eq!(limit.retry_with(), None);
    check_waiting("t1bomb padded", &why, decoder.bytes(), &options);
    let now = decode_with(
        decoder.bytes(),
        &decoder.options().clone().with_strict(false),
    )
    .unwrap();
    assert_eq!(now.components, first.components);
    let mut fresh = IncrementalDecoder::new(options).unwrap();
    fresh.feed(decoder.bytes());
    assert!(
        fresh.refine().unwrap().image().is_some(),
        "a new decoder decodes it"
    );
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
    // The reduction it has changes nothing: the error stays, without a
    // decode.
    decoder.set_resolution_reduction(30).unwrap();
    decoder.feed(&bytes[half..half + 1]);
    assert!(matches!(decoder.refine(), Err(Error::InvalidOptions(_))));
    let options = DecodeOptions::default()
        .with_resolution_reduction(1)
        .with_max_quality_layers(Some(2))
        .with_region(Region::Window(Window::new(0, 0, 9, 7)));
    decoder.set_resolution_reduction(1).unwrap();
    decoder.set_max_quality_layers(Some(2)).unwrap();
    decoder.set_region(options.region.clone()).unwrap();
    // Not twice the bytes yet.
    decoder.feed(&bytes[half + 1..half + 2]);
    assert!(matches!(
        decoder.refine(),
        Ok(Refined::Waiting(Waiting::MoreBytes { until, .. })) if until == 2 * half
    ));
    decoder.feed(&bytes[half + 2..]);
    let image = decoder
        .refine()
        .unwrap()
        .image()
        .expect("the refine is due");
    assert_eq!(Some(image), refine_oracle(&bytes, &options).unwrap());
    assert!(matches!(
        decoder.refine(),
        Ok(Refined::Waiting(Waiting::MoreBytes { until, .. })) if until == 2 * bytes.len()
    ));
    assert_eq!(
        keyed(decoder.finish()),
        keyed(decode_with(&bytes, &options))
    );
}

/// An image right after a change of options, from the bytes and the options
/// the decoder reports with `strict` off, is the one-shot strict-off decode
/// of the same bytes under the new options. `finish` on those bytes errors.
#[test]
fn decode_with_the_decoders_options_gives_an_image_after_a_change() {
    let file = std::fs::read(fixtures().join("layers/lrcp.j2k")).unwrap();
    let prefix = &file[..file.len() - 5];
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(prefix);
    decoder.set_resolution_reduction(1).unwrap();
    let lenient = decoder.options().clone().with_strict(false);
    let image = decode_with(decoder.bytes(), &lenient).unwrap();
    let expected = decode_with(
        prefix,
        &DecodeOptions::default()
            .with_resolution_reduction(1)
            .with_strict(false),
    )
    .unwrap();
    assert_eq!(image, expected);
    // Strict `finish` refuses the cut file.
    assert!(decoder.finish().is_err());
    assert!(decoder.options().strict);
}

/// `options()` follows each setter, keeps the caller's `strict`, and is
/// unchanged by a setter that returns `InvalidOptions`.
#[test]
fn options_reflects_each_setter_and_survives_a_refused_one() {
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    assert_eq!(decoder.options().resolution_reduction, 0);
    assert!(decoder.options().strict);
    decoder.set_resolution_reduction(2).unwrap();
    assert_eq!(decoder.options().resolution_reduction, 2);
    decoder.set_max_quality_layers(Some(3)).unwrap();
    assert_eq!(decoder.options().max_quality_layers, Some(3));
    let region = Region::Window(Window::new(1, 2, 3, 4));
    decoder.set_region(region.clone()).unwrap();
    assert_eq!(decoder.options().region, region);
    assert!(matches!(
        decoder.set_max_quality_layers(Some(0)),
        Err(Error::InvalidOptions(_))
    ));
    assert!(matches!(
        decoder.set_region(Region::Window(Window::new(4, 4, 4, 8))),
        Err(Error::InvalidOptions(_))
    ));
    let options = decoder.options();
    assert_eq!(options.resolution_reduction, 2);
    assert_eq!(options.max_quality_layers, Some(3));
    assert_eq!(options.region, region);
    assert!(options.strict);

    let lenient = DecodeOptions::default().with_strict(false);
    let decoder = IncrementalDecoder::new(lenient).unwrap();
    assert!(!decoder.options().strict);
}

/// The same with a partial image: 1500 bytes short, the last layer holds
/// only the lowest resolutions, so the image at reduction 1 is incomplete.
#[test]
fn decode_with_the_decoders_options_gives_a_partial_image_after_a_change() {
    let file = std::fs::read(fixtures().join("layers/lrcp.j2k")).unwrap();
    let prefix = &file[..file.len() - 1500];
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default()).unwrap();
    decoder.feed(prefix);
    assert!(decoder.refine().unwrap().image().is_some());
    decoder.set_resolution_reduction(1).unwrap();
    assert!(decoder.options().strict);
    let image = decode_with(
        decoder.bytes(),
        &decoder.options().clone().with_strict(false),
    )
    .unwrap();
    assert!(!image.report.is_complete());
    let expected = decode_with(
        prefix,
        &DecodeOptions::default()
            .with_resolution_reduction(1)
            .with_strict(false),
    )
    .unwrap();
    assert_eq!(image, expected);
    assert!(decoder.finish().is_err());
}
