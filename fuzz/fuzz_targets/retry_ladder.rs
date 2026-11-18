#![no_main]
//! Property target over [`rust_j2k::DecodeOptions::with_retry`]: following
//! [`rust_j2k::LimitDetail::retry_with`] from one limit refusal to the next
//! raises the option the refusal names and no other, steps strictly upward,
//! never turns an image into an error, and ends at the image the default
//! options give, and the one no limits give.
//!
//! A hash of the input picks where each of the three limits starts
//! (`Default`, or a factor of 1 or 2), a resolution reduction of 0 or 1, and
//! `strict`. A decode past a factor of 4 can take longer than the fuzzer's
//! timeout, so the ladder stops when a refusal asks for more, after its
//! options are checked; it also stops after 8 steps, which no input should
//! reach.
//!
//! An input that declares more samples than the fuzz budget allows
//! (`common/budget.rs`) is only read for its header: a sample limit refusal
//! there is checked as one step of the ladder, with no decode.

use libfuzzer_sys::fuzz_target;

#[path = "../common/budget.rs"]
mod budget;

use rust_j2k::{
    decode_with, read_header_with, DecodeOptions, Error, LimitDetail, LimitKind, WorkLimit,
};

/// The highest factor the ladder decodes at.
const MAX_FACTOR: u64 = 4;

/// The most steps the ladder takes.
const MAX_STEPS: u32 = 8;

/// The three limits an option raises.
const KINDS: [LimitKind; 3] = [
    LimitKind::Samples,
    LimitKind::PacketWork,
    LimitKind::CodeBlockWork,
];

/// A low starting limit from two bits: `Default` or a factor of 1 or 2.
/// `WorkLimit` sets nothing below the default bound.
fn low(bits: u64) -> WorkLimit {
    match bits & 3 {
        0 | 1 => WorkLimit::Default,
        2 => WorkLimit::Raised(1),
        _ => WorkLimit::Raised(2),
    }
}

/// A limit as a number to compare: `Default` is `Raised(1)`.
fn rank(limit: &WorkLimit) -> u64 {
    match limit {
        WorkLimit::Default => 1,
        WorkLimit::Raised(factor) => u64::from(*factor),
        WorkLimit::Unlimited => u64::MAX,
        other => unreachable!("a new limit setting: {other:?}"),
    }
}

/// The option of `options` that limit `kind` reads.
fn field(options: &DecodeOptions, kind: LimitKind) -> &WorkLimit {
    match kind {
        LimitKind::Samples => &options.sample_limit,
        LimitKind::PacketWork => &options.packet_work_limit,
        LimitKind::CodeBlockWork => &options.code_block_work_limit,
        other => unreachable!("an adjustable limit with no option: {other:?}"),
    }
}

/// One step of the ladder: the options `with_retry` gives for `limit`,
/// checked against `options`, with the limit it raised. `None` when no
/// option raises it.
fn step(options: &DecodeOptions, limit: &LimitDetail) -> Option<(DecodeOptions, WorkLimit)> {
    let kind = limit.kind();
    let Some(next) = options.with_retry(limit) else {
        assert!(
            limit.retry_with().is_none(),
            "{kind:?}: retry_with gives {:?}, with_retry none",
            limit.retry_with()
        );
        return None;
    };
    assert!(
        kind.is_adjustable(),
        "{kind:?} is fixed but with_retry raises it"
    );
    let raised = limit.retry_with().expect("with_retry raised a limit");
    // The option the kind names is set to `retry_with`, strictly above
    // where it was; the other two are unchanged.
    assert_eq!(
        field(&next, kind),
        &raised,
        "{kind:?} raised the wrong option"
    );
    assert!(
        rank(&raised) > rank(field(options, kind)),
        "{kind:?}: retry_with {raised:?} is not above {:?} ({limit})",
        field(options, kind)
    );
    for other in KINDS.into_iter().filter(|&k| k != kind) {
        assert_eq!(
            field(&next, other),
            field(options, other),
            "{kind:?} retry moved {other:?}"
        );
    }
    assert_eq!(
        (
            next.resolution_reduction,
            next.strict,
            &next.region,
            &next.components
        ),
        (
            options.resolution_reduction,
            options.strict,
            &options.region,
            &options.components
        ),
        "{kind:?} retry moved another option"
    );
    Some((next, raised))
}

fuzz_target!(|data: &[u8]| {
    let hash = fnv1a(data);
    let mut options = DecodeOptions::default()
        .with_sample_limit(low(hash))
        .with_packet_work_limit(low(hash >> 2))
        .with_code_block_work_limit(low(hash >> 4))
        .with_resolution_reduction(((hash >> 6) & 1) as u8)
        .with_strict((hash >> 7) & 1 == 0);
    if budget::over(data) {
        if let Err(Error::Limit(limit)) = read_header_with(data, &options) {
            step(&options, &limit);
        }
        return;
    }
    let mut result = decode_with(data, &options);
    let mut steps = 0;
    while let Err(Error::Limit(limit)) = &result {
        let Some((next, raised)) = step(&options, limit) else {
            break;
        };
        steps += 1;
        assert!(
            steps <= MAX_STEPS,
            "ladder past {MAX_STEPS} steps: {limit:?}"
        );
        if rank(&raised) > MAX_FACTOR {
            return;
        }
        // A refusal by the same limit again must name a larger factor:
        // `step` checks it on the next pass against the raised option.
        options = next;
        result = decode_with(data, &options);
    }
    let unlimited = |options: &DecodeOptions| {
        options
            .clone()
            .with_sample_limit(WorkLimit::Unlimited)
            .with_packet_work_limit(WorkLimit::Unlimited)
            .with_code_block_work_limit(WorkLimit::Unlimited)
    };
    let defaults = DecodeOptions::default()
        .with_resolution_reduction(options.resolution_reduction)
        .with_strict(options.strict);
    let default = decode_with(data, &defaults);
    // Raising a limit never turns an image into an error.
    if default.is_ok() {
        assert!(
            result.is_ok(),
            "the default options decode, the ladder gives {:?}",
            result.as_ref().err()
        );
    }
    let Ok(image) = &result else {
        return;
    };
    // The samples match. The report need not: with `strict` off, a
    // `Psot = 0` JP2 tile whose box-end walk passes the packet bound at the
    // default limit decodes with `LeftoverBytes`, and with no entry once
    // `packet_work_limit` is raised.
    let reference = match default {
        Ok(d) => d,
        Err(_) => match decode_with(data, &unlimited(&options)) {
            Ok(u) => u,
            Err(e) => panic!("the ladder decodes, no limits give {e:?}"),
        },
    };
    assert_eq!(
        (image.width, image.height, &image.components, &image.jp2),
        (
            reference.width,
            reference.height,
            &reference.components,
            &reference.jp2
        ),
        "the ladder image differs from the default or unlimited decode"
    );
});

/// FNV-1a over `bytes`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
