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
//! The first six input bytes steer the options, as `tile_body`'s first bytes
//! steer its shape, so a small mutation of the file keeps them; the rest is
//! the file (seed it with six zero bytes in front, which mean the default
//! code-block limit and no other option). An input that starts with a
//! codestream's `SOC` (`FF 4F`) or the JP2 signature box is instead the whole
//! file with all-zero control bytes, so plain fixtures are valid seeds. The
//! bytes pick: a layer cap, a resolution reduction, a
//! window (from the last four bytes), a tile, a component subset of one or
//! two distinct indices, and a code-block work limit. The decoder holds all
//! its refines to that limit together, and a one-shot decode of the prefix
//! holds only itself, so once earlier refines have spent some of it, a refine
//! it refuses (`Ok(None)`) is checked against a fresh decoder instead of the
//! one-shot decode; before that, and for every refine that decodes, the
//! result equals the one-shot decode. The limit is at least 2^26 sample
//! passes, so inputs this small do not reach the cut (see #425).

use libfuzzer_sys::fuzz_target;
use rust_j2k::{DecodeOptions, Error, IncrementalDecoder, Window, WorkLimit, decode_with};

fuzz_target!(|input: &[u8]| {
    const JP2_SIGNATURE: [u8; 8] = [0, 0, 0, 12, b'j', b'P', b' ', b' '];
    let (control, data) = if input.starts_with(&[0xFF, 0x4F]) || input.starts_with(&JP2_SIGNATURE)
    {
        (&[0u8; 6], input)
    } else {
        let Some((control, data)) = input.split_first_chunk::<6>() else {
            return;
        };
        (control, data)
    };
    let mut hash = fnv1a(data);
    // The code-block bound is the one earlier refines cut: the decoder
    // holds all its refines to it together. Without a bound, every refine
    // that decodes is the one-shot decode of its bytes.
    let (limit, cut) = match control[0] & 3 {
        0 => (WorkLimit::Default, true),
        1 => (WorkLimit::Unlimited, false),
        2 => (WorkLimit::Raised(2), true),
        _ => (WorkLimit::Raised(4), true),
    };
    let mut options = DecodeOptions::default().with_code_block_work_limit(limit);
    if control[0] & 4 != 0 {
        options = options.with_max_quality_layers(Some(1 + u16::from((control[0] >> 3) % 3)));
    }
    if control[0] & 0x20 != 0 {
        options = options.with_resolution_reduction(1);
    }
    // The pieces of the file one refine decodes: a window, or a tile, or
    // neither; and a subset of one or two distinct components. A choice
    // outside the file gives the same error from the decoder and the
    // one-shot decode.
    match control[1] & 3 {
        1 => {
            let (x, y) = (u32::from(control[2] % 24), u32::from(control[3] % 24));
            let (w, h) = (1 + u32::from(control[4] % 40), 1 + u32::from(control[5] % 40));
            options = options.with_window(Some(Window::new(x, y, x + w, y + h)));
        }
        2 => {
            // Tile 0 is the common one.
            let tile = [0, 0, 0, 0, 1, 1, 2, 3][usize::from((control[1] >> 2) & 7)];
            options = options.with_tile(Some(tile));
        }
        _ => {}
    }
    let subset: Option<Vec<u16>> = match control[1] >> 5 {
        1 => Some(vec![0]),
        2 => Some(vec![1]),
        3 => Some(vec![2]),
        4 => Some(vec![0, 1]),
        5 => Some(vec![1, 0]),
        6 => Some(vec![0, 2]),
        7 => Some(vec![2, 1]),
        _ => None,
    };
    options = options.with_components(subset);
    let lenient = options.clone().with_strict(false);
    let Ok(mut decoder) = IncrementalDecoder::new(options.clone()) else {
        return;
    };
    let mut fed = 0;
    let mut failed = None;
    // The bytes fed at the last refine that was due.
    let mut tried = 0;
    // No one-shot decode of a prefix has yet got past the main header, so
    // no code-block work is spent.
    let mut fresh = true;
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
            let one_shot = decode_with(&data[..fed], &lenient);
            let was_fresh = fresh;
            if !matches!(one_shot, Err(Error::Incomplete(_))) {
                fresh = false;
            }
            let expected = match one_shot {
                Ok(image) => Ok(Some(image)),
                Err(Error::Incomplete(_)) => Ok(None),
                Err(Error::Limit(m))
                    if m.contains("packet_work_limit") || m.contains("code_block_work_limit") =>
                {
                    Ok(None)
                }
                Err(error) => Err(error),
            };
            // The bound may refuse a refine its one-shot decode passes, but
            // only once earlier refines have spent some of it. A decoder
            // with nothing spent must give what this one does.
            if cut && refined == Ok(None) && !was_fresh {
                let mut zero = IncrementalDecoder::new(options.clone()).unwrap();
                zero.feed(&data[..fed]);
                assert_eq!(zero.refine(), expected, "{fed} bytes, nothing spent");
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
