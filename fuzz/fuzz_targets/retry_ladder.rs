#![no_main]
//! Property target over [`rust_j2k::DecodeOptions::with_retry`]: following
//! [`rust_j2k::LimitDetail::retry_with`] from one limit refusal to the next
//! raises the option the refusal names and no other, steps strictly upward,
//! and never turns an image into an error. An image it reaches has the
//! samples the default limits give, or, when they refuse, the samples no
//! limits give.
//!
//! The first input byte picks where each of the three limits starts
//! (`Default`, or a factor of 1 or 2), a resolution reduction of 0 or 1, and
//! `strict`, as `incremental_decode`'s first bytes steer its options; the
//! rest is the file. An input that starts with a codestream's `SOC`
//! (`FF 4F`) or the JP2 signature box is instead the whole file with a zero
//! control byte (the default limits, reduction 0, `strict` on), so plain
//! fixtures are valid seeds.
//!
//! A decode past a factor of 4 can take longer than the fuzzer's timeout,
//! so the ladder stops when a refusal asks for more, once that step is
//! checked. A ladder of more than 16 steps is a failure: it should end in a
//! few.
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

/// The most steps the ladder may take. Each step raises one of three limits
/// to at most a factor of [`MAX_FACTOR`] before the ladder stops, so a few
/// steps are enough; this bound only catches a ladder that does not end.
const MAX_STEPS: u32 = 16;

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

/// `options` with the limit `kind` reads set to `limit`.
fn with_field(options: &DecodeOptions, kind: LimitKind, limit: WorkLimit) -> DecodeOptions {
    let options = options.clone();
    match kind {
        LimitKind::Samples => options.with_sample_limit(limit),
        LimitKind::PacketWork => options.with_packet_work_limit(limit),
        LimitKind::CodeBlockWork => options.with_code_block_work_limit(limit),
        other => unreachable!("an adjustable limit with no option: {other:?}"),
    }
}

/// One step of the ladder: the options `with_retry` gives for `limit`,
/// checked against `options`, with the limit it raised. `None` when no
/// option raises it.
fn step(options: &DecodeOptions, limit: &LimitDetail) -> Option<(DecodeOptions, WorkLimit)> {
    let kind = limit.kind();
    // A limit no option raises names no factor to raise it to.
    if !kind.is_adjustable() {
        assert!(
            limit.retry_with().is_none(),
            "{kind:?} is fixed but gives {:?}",
            limit.retry_with()
        );
    }
    let Some(next) = options.with_retry(limit) else {
        // An adjustable limit gives no retry only when its refusal shows no
        // factor, such as a packet work refusal while reading the header.
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
    assert!(
        rank(&raised) > rank(field(options, kind)),
        "{kind:?}: retry_with {raised:?} is not above {:?} ({limit})",
        field(options, kind)
    );
    // The option the kind names is set to `retry_with` and every other
    // option is as it was: compared by `Debug`, so the check covers every
    // field, a new one included (`DecodeOptions` has no `PartialEq`).
    let expected = with_field(options, kind, raised.clone());
    assert_eq!(
        format!("{next:?}"),
        format!("{expected:?}"),
        "{kind:?}: with_retry changed more than the option it names"
    );
    Some((next, raised))
}

fuzz_target!(|input: &[u8]| {
    const JP2_SIGNATURE: [u8; 8] = [0, 0, 0, 12, b'j', b'P', b' ', b' '];
    let (control, data) = if input.starts_with(&[0xFF, 0x4F]) || input.starts_with(&JP2_SIGNATURE) {
        (0, input)
    } else {
        let Some((&control, data)) = input.split_first() else {
            return;
        };
        (control, data)
    };
    let control = u64::from(control);
    let mut options = DecodeOptions::default()
        .with_sample_limit(low(control))
        .with_packet_work_limit(low(control >> 2))
        .with_code_block_work_limit(low(control >> 4))
        .with_resolution_reduction(((control >> 6) & 1) as u8)
        .with_strict(control >> 7 == 0);
    if budget::over(data) {
        if let Err(Error::Limit(limit)) = read_header_with(data, &options) {
            step(&options, &limit);
        }
        return;
    }
    let defaults = DecodeOptions::default()
        .with_resolution_reduction(options.resolution_reduction)
        .with_strict(options.strict);
    let at_defaults = KINDS
        .into_iter()
        .all(|kind| *field(&options, kind) == WorkLimit::Default);
    let mut result = decode_with(data, &options);
    // The decode under the default limits, when the options start above
    // them. At the defaults it is `result` before any step.
    let default = (!at_defaults).then(|| decode_with(data, &defaults));
    let default_ok = default.as_ref().map_or(result.is_ok(), Result::is_ok);
    // Raising a limit never turns an image into an error. The options start
    // at or above the default limits, so when those decode, the first decode
    // must too, and the ladder takes no step.
    if default_ok {
        assert!(
            result.is_ok(),
            "the default limits decode, the options above them give {:?}",
            result.as_ref().err()
        );
    }
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
    let Ok(image) = &result else {
        return;
    };
    // The samples match. The report need not: with `strict` off, a
    // `Psot = 0` JP2 tile whose box-end walk passes the packet bound at the
    // default limit decodes with `LeftoverBytes`, and with no entry once
    // `packet_work_limit` is raised.
    let reference = match default {
        Some(Ok(image)) => image,
        // At the defaults with no step, `result` is the default decode.
        None if steps == 0 => return,
        // The defaults refuse: compare with no limits.
        _ => {
            let unlimited = options
                .clone()
                .with_sample_limit(WorkLimit::Unlimited)
                .with_packet_work_limit(WorkLimit::Unlimited)
                .with_code_block_work_limit(WorkLimit::Unlimited);
            match decode_with(data, &unlimited) {
                Ok(image) => image,
                Err(e) => panic!("the ladder decodes, no limits give {e:?}"),
            }
        }
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
