#![no_main]
//! Fuzz target over [`rust_j2k::IncrementalDecoder`]: the input is fed in
//! pieces whose ends a hash of it picks, with a refine after each. Each
//! refine that decodes goes on from the packet walks the earlier ones kept,
//! and must give what `decode_with` gives with `strict` off on the bytes
//! fed so far: the same image, or the same error. A refine that does not
//! decode returns `Ok(None)`. After an error, every later refine gives it
//! again, and `finish` is the decode of all the bytes under the decoder's
//! options.

use libfuzzer_sys::fuzz_target;
use rust_j2k::{DecodeOptions, Error, IncrementalDecoder, WorkLimit, decode_with};

fuzz_target!(|data: &[u8]| {
    let mut hash = fnv1a(data);
    // The code-block bound is the one earlier refines cut; without it,
    // every refine that decodes is the one-shot decode of its bytes.
    let mut options = DecodeOptions::default().with_code_block_work_limit(WorkLimit::Unlimited);
    if hash & 1 == 1 {
        options = options.with_max_quality_layers(Some(1 + ((hash >> 1) % 3) as u16));
    }
    if hash & 8 == 8 {
        options = options.with_resolution_reduction(1);
    }
    let lenient = options.clone().with_strict(false);
    let Ok(mut decoder) = IncrementalDecoder::new(options.clone()) else {
        return;
    };
    let mut fed = 0;
    let mut failed = None;
    while fed < data.len() {
        hash = splitmix(hash);
        // Pieces from 1 byte to about a quarter of the input.
        let piece = 1 + (hash % (data.len() as u64 / 4 + 1)) as usize;
        let end = (fed + piece).min(data.len());
        decoder.feed(&data[fed..end]);
        fed = end;
        let refined = decoder.refine();
        if let Some(error) = &failed {
            assert_eq!(refined.as_ref(), Err(error), "poisoned at {fed}");
            continue;
        }
        match refined {
            Ok(None) => {}
            Ok(Some(image)) => {
                let expected = decode_with(&data[..fed], &lenient);
                assert_eq!(Ok(&image), expected.as_ref(), "{fed} bytes");
            }
            Err(error) => {
                match decode_with(&data[..fed], &lenient) {
                    Err(expected) => assert_eq!(error, expected, "{fed} bytes"),
                    Ok(_) => panic!("{fed} bytes: {error:?}, where the decode gives an image"),
                }
                assert!(!matches!(error, Error::Incomplete(_)), "{fed} bytes: {error:?}");
                failed = Some(error);
            }
        }
    }
    let finished = decoder.finish();
    match decode_with(data, &options) {
        // A header cut short is a truncation once the input has ended.
        Err(Error::Incomplete(why)) => match finished {
            Err(Error::Codestream(m) | Error::FileFormat(m)) => assert!(m.contains(&why), "{m}"),
            other => panic!("{other:?}, where the header is cut short"),
        },
        expected => assert_eq!(finished, expected),
    }
});

fn splitmix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}
