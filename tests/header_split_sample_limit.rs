//! The `header_split` target's [`replaceable::replaceable`] where the
//! suffix that changes a held error is a box of length 0 after the
//! codestream run past 2^32 bytes, which OpenJPEG skips unread, and those
//! bytes also raise the default sample limit.
//!
//! On `header_split_colr_lbox0_after_codestream_cap_lifts_tile_part_fault.seed`,
//! at a feed of 2999 bytes, `read_header_with` refuses a tile over a cap and
//! so meets a tile-part fault, which it gives. A raised limit counts no tile
//! over a cap, and the fault is not met. The input ends inside a `colr` of
//! length 0 after the codestream, so `header` holds the fault, and 2^32
//! more bytes take it away. Under [`SampleLimit::AtMost`] no byte raises the
//! limit, so the same fault is final, and `replaceable` must find no suffix
//! there.

#[path = "support/inconsistent.rs"]
mod inconsistent;
#[path = "support/pieces.rs"]
mod pieces;
#[path = "support/replaceable.rs"]
mod replaceable;
#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{DecodeOptions, Error, IncrementalDecoder, SampleLimit, read_header_with};

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
/// under a raised one, and `replaceable` finds that the 2^32 bytes that
/// skip the `colr` of length 0 change it. Before it read that stand-in
/// under the limit those bytes give, it found no suffix, and the target
/// failed.
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
