//! `IncrementalDecoder` against one-shot decodes of the same bytes.
//!
//! Every image a refine returns must be the image `decode_with` gives with
//! `strict` off on the bytes fed so far, and `finish` must give what
//! `decode_with` gives on all of them, with a main header cut short turned
//! from `Error::Incomplete` into a truncation error. This runs on every
//! codestream, JP2 and JPH file under `tests/fixtures/`, fed in pieces of
//! many sizes, and cut at many points.

use std::path::{Path, PathBuf};

use rust_j2k::{DecodeOptions, Error, Image, IncrementalDecoder, decode_with, read_header};

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

/// What a refine on `prefix` must give: the strict-off decode, with a main
/// header cut short and a limit as `None`.
fn refine_oracle(prefix: &[u8], options: &DecodeOptions) -> Result<Option<Image>, Error> {
    match decode_with(prefix, &options.clone().with_strict(false)) {
        Ok(image) => Ok(Some(image)),
        Err(Error::Incomplete(_) | Error::Limit(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Whether `finish` on `bytes` gives what `decode_with` gives under
/// `options`, with `Incomplete` turned into a truncation error.
fn check_finish(name: &str, bytes: &[u8], decoder: IncrementalDecoder, options: &DecodeOptions) {
    let finished = decoder.finish();
    match decode_with(bytes, options) {
        Err(Error::Incomplete(why)) => match finished {
            Err(Error::Codestream(m) | Error::FileFormat(m)) => {
                assert!(m.contains(&why), "{name}, {} bytes: {m}", bytes.len());
            }
            other => panic!("{name}, {} bytes: {other:?}", bytes.len()),
        },
        expected => assert_eq!(finished, expected, "{name}, {} bytes", bytes.len()),
    }
}

/// Feed `bytes` in pieces whose sizes `rng` draws, refining after each:
/// every image is the strict-off decode of the bytes fed so far, an error
/// is that decode's error, and `finish` is the decode of the whole file.
fn stream(name: &str, bytes: &[u8], rng: &mut Rng, options: &DecodeOptions) -> usize {
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    let mut fed = 0;
    let mut images = 0;
    while fed < bytes.len() {
        // Pieces from 1 byte to about a sixteenth of the file.
        let piece = 1 + rng.below((bytes.len() / 16).max(1));
        let end = (fed + piece).min(bytes.len());
        decoder.feed(&bytes[fed..end]);
        fed = end;
        assert_eq!(decoder.bytes_fed(), fed);
        match decoder.refine() {
            Ok(Some(image)) => {
                images += 1;
                assert_eq!(decoder.bytes_decoded(), fed, "{name}");
                let expected = decode_with(&bytes[..fed], &options.clone().with_strict(false));
                assert_eq!(Ok(image), expected, "{name}, {fed} bytes");
            }
            Ok(None) => {}
            Err(error) => {
                let expected = refine_oracle(&bytes[..fed], options);
                assert_eq!(Err(error), expected, "{name}, {fed} bytes");
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
        let refined = decoder.refine();
        let expected = refine_oracle(&bytes[..fed], options);
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
        assert_eq!(refined, refine_oracle(prefix, options), "{name}, cut at {at}");
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

/// The fixture directories of files made to reach the work bounds.
const COSTLY: [&str; 4] = ["work_bound/", "tier1_work_bound/", "psot0/", "budget/"];

/// Every fixture, fed in pieces and cut at many points, decodes as the
/// one-shot decode of the same bytes does. Each refine and `finish` is
/// checked against `decode_with`, so this covers the MIXED HT files and the
/// PTERM, POC and multi-tile files the corpus holds.
#[test]
fn every_fixture_refines_to_the_one_shot_decode_of_its_prefix() {
    let mut rng = Rng(217);
    let (mut count, mut points, mut images) = (0, 0, 0);
    for path in files(&fixtures()) {
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
        let cuts = cuts(bytes.len(), &mut rng);
        points += cuts.len();
        cut(&name, bytes, &cuts, &mut rng, &options);
        count += 1;
    }
    // The fixtures in the published package alone are well over 100.
    assert!(count > 100, "{count} fixtures");
    println!("{count} fixtures, {points} cut points, {images} streamed images");
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
    assert_eq!(Some(prefix), refine_oracle(&bytes[..226], &lenient()).unwrap());
    decoder.feed(&bytes[226..]);
    // 231 bytes are not twice 226: no new refine until the end.
    assert_eq!(decoder.refine(), Ok(None));
    let finished = decoder.finish().unwrap();
    assert_eq!(finished, decode_with(&bytes, &DecodeOptions::default()).unwrap());
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
        Err(Error::Codestream(m)) => assert!(m.contains("predictable termination"), "{m}"),
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
            assert_eq!(Err(error), refine_oracle(&bad[..decoder.bytes_fed()], &lenient()));
        }
    }
    match decoder.finish() {
        Err(Error::Codestream(m)) => assert!(m.contains("predictable termination"), "{m}"),
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
        assert_eq!(decoder.refine(), Ok(None));
        match (decoder.finish(), box_layer) {
            (Err(Error::Codestream(m)), false) | (Err(Error::FileFormat(m)), true) => {
                assert!(m.starts_with("the input is truncated: "), "{m}");
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
