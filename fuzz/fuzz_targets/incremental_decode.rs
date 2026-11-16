#![no_main]
//! Fuzz target over [`rust_j2k::IncrementalDecoder`]: the input is fed in
//! pieces whose ends a hash of it picks, with a refine after each. Each
//! refine that decodes goes on from the packet walks the earlier ones kept,
//! and must give what `decode_with` gives with `strict` off on the bytes
//! fed so far: the same image, or the same error. A refine that does not
//! decode returns `Ok(None)`. A refine due under the doubling rule must
//! decode unless the header is cut short or the packet work bound refuses
//! that one-shot decode. After an error, every later refine gives it again,
//! and `finish` is the decode of all the bytes under the decoder's options.
//!
//! The options come from a hash of the input: layers, a resolution
//! reduction, a window or a tile or a component subset, and a code-block
//! work limit that often refuses. The decoder holds all its refines to that
//! bound together, and a one-shot decode of the prefix holds only itself, so
//! a refine the bound refuses (`Ok(None)`) can be checked against nothing,
//! while a refine that decodes still equals the one-shot decode.

use libfuzzer_sys::fuzz_target;
use rust_j2k::{DecodeOptions, Error, IncrementalDecoder, Window, WorkLimit, decode_with};

fuzz_target!(|data: &[u8]| {
    let mut hash = fnv1a(data);
    // The code-block bound is the one earlier refines cut: the decoder
    // holds all its refines to it together. Without a bound, every refine
    // that decodes is the one-shot decode of its bytes.
    let pick = splitmix(hash ^ 0x5eed);
    let (limit, cut) = match pick & 3 {
        0 => (WorkLimit::Unlimited, false),
        1 => (WorkLimit::Default, true),
        2 => (WorkLimit::Raised(2), true),
        _ => (WorkLimit::Raised(4), true),
    };
    let mut options = DecodeOptions::default().with_code_block_work_limit(limit);
    if hash & 1 == 1 {
        options = options.with_max_quality_layers(Some(1 + ((hash >> 1) % 3) as u16));
    }
    if hash & 8 == 8 {
        options = options.with_resolution_reduction(1);
    }
    // The pieces of the file one refine decodes: a window, or a tile, or
    // neither; and a subset of the components. Out-of-range choices give
    // the same error from the decoder and the one-shot decode.
    match (pick >> 2) % 4 {
        0 => {
            let (x, y) = (((pick >> 8) % 24) as u32, ((pick >> 14) % 24) as u32);
            let (w, h) = (1 + ((pick >> 20) % 40) as u32, 1 + ((pick >> 26) % 40) as u32);
            options = options.with_window(Some(Window::new(x, y, x + w, y + h)));
        }
        1 => options = options.with_tile(Some(((pick >> 8) % 5) as u32)),
        _ => {}
    }
    if (pick >> 4) & 3 == 0 {
        let subset = vec![((pick >> 32) % 3) as u16, ((pick >> 40) % 3) as u16];
        options = options.with_components(Some(subset));
    }
    let lenient = options.clone().with_strict(false);
    let Ok(mut decoder) = IncrementalDecoder::new(options.clone()) else {
        return;
    };
    let mut fed = 0;
    let mut failed = None;
    // The bytes fed at the last refine that was due.
    let mut tried = 0;
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
        let due = fed >= tried * 2;
        if due {
            tried = fed;
            let expected = match decode_with(&data[..fed], &lenient) {
                Ok(image) => Ok(Some(image)),
                Err(Error::Incomplete(_)) => Ok(None),
                Err(Error::Limit(m)) if m.contains("packet_work_limit") => Ok(None),
                Err(error) => Err(error),
            };
            // The bound may refuse a refine its one-shot decode passes.
            if cut && refined == Ok(None) {
                continue;
            }
            assert_eq!(refined, expected, "{fed} bytes, due");
            if let Err(error) = refined {
                failed = Some(error);
            }
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
