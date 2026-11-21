#![no_main]
//! Property target over [`rust_j2k::DecodeOptions::with_retry`]: following
//! [`rust_j2k::LimitDetail::retry_with`] from one limit refusal to the next
//! raises the option the refusal names and no other, steps strictly upward,
//! and never turns an image into an error. An image it reaches has the
//! samples the default limits give, or, when they refuse, the samples no
//! limits give.
//!
//! The first input byte picks where each of the three limits starts
//! (`Default`, or a factor of 1 or 2, and for the sample limit also a
//! ceiling of `AtMost`), a resolution reduction of 0 or 1, and
//! `strict`, as `incremental_decode`'s first bytes steer its options; the
//! rest is the file. An input that starts with a codestream's `SOC`
//! (`FF 4F`) or the JP2 signature box is instead the whole file with a zero
//! control byte (the default limits, reduction 0, `strict` on), so plain
//! fixtures are valid seeds.
//!
//! A decode past a factor of 4 can take longer than the fuzzer's timeout,
//! so the ladder stops when a refusal asks for more, once that step is
//! checked. A limit that takes more than `MAX_STEPS` steps is a failure: a
//! packet work refusal names twice the factor it ran under, so a ladder ends
//! in about log2 of the factor it needs.
//!
//! An input that declares more samples than the fuzz budget allows
//! (`common/budget.rs`) is only read for its header: a sample limit refusal
//! there is checked as one step of the ladder, with no decode.
//!
//! A sample limit of `AtMost` is a ceiling the caller set, below the default
//! for most inputs, so it is not on the ladder: a refusal by it, or by a
//! per-tile cap that grows with it, gives no retry, and ends the ladder in
//! one step. An image under it has the samples
//! the same options give with the default sample limit.

use libfuzzer_sys::fuzz_target;

#[path = "../../tests/support/inconsistent.rs"]
mod inconsistent;
use inconsistent::checked;

#[path = "../common/budget.rs"]
mod budget;

use rust_j2k::{
    decode_with, read_header_with, DecodeOptions, Error, LimitDetail, LimitKind, WorkLimit,
};

/// The highest factor the ladder decodes at.
const MAX_FACTOR: u64 = 4;
const _: () = assert!(MAX_FACTOR <= rust_j2k::fuzz::CODE_BLOCK_CAP_FACTOR);

/// The most steps the ladder may take on one option. `step` asserts that
/// each step raises the option strictly, and the ladder returns once a step
/// names a factor past [`MAX_FACTOR`] (`Unlimited` included). Factors are
/// whole numbers from 1, so one option's steps name 2, 3, 4, then something
/// past 4: at most `MAX_FACTOR` steps, whatever the hints are. The kinds that
/// share the sample limit's option (see [`slot`]) share the count. This
/// bound therefore catches a ladder that does not end, and not a hint that
/// is too small: that the work hints at least double is held by
/// `tests/work_limit.rs` and the `limit_hint` unit tests, and the tile caps'
/// by `tests/tile_geometry_cap.rs`. The value is
/// log2([`MAX_FACTOR`]) + 2, which is `MAX_FACTOR` while it is 4.
const MAX_STEPS: u32 = MAX_FACTOR.ilog2() + 2;

/// The three options a refusal raises, one for each step count: the sample
/// limit (see [`slot`]), the packet work limit and the code-block work limit.
const KINDS: [LimitKind; 3] = [
    LimitKind::Samples,
    LimitKind::PacketWork,
    LimitKind::CodeBlockWork,
];

/// Whether `kind` is refused by the sample limit, and raised through it: the
/// samples, and the per-tile caps on code-blocks and precincts, which grow
/// with it.
fn is_sample_limit(kind: LimitKind) -> bool {
    matches!(
        kind,
        LimitKind::Samples | LimitKind::TileCodeBlocks | LimitKind::TilePrecincts
    )
}

/// The index in [`KINDS`] of the option that limit `kind` raises: kinds that
/// share an option share a step count.
fn slot(kind: LimitKind) -> usize {
    let key = if is_sample_limit(kind) {
        LimitKind::Samples
    } else {
        kind
    };
    KINDS
        .iter()
        .position(|&k| k == key)
        .expect("an adjustable limit has an option in KINDS")
}

/// A low starting limit from two bits: `Default` or a factor of 1 or 2.
/// No factor sets a bound below the default; `AtMost` does, for the sample
/// limit alone (see [`low_samples`]).
fn low(bits: u64) -> WorkLimit {
    match bits & 3 {
        0 | 1 => WorkLimit::Default,
        2 => WorkLimit::Raised(1),
        _ => WorkLimit::Raised(2),
    }
}

/// The sample limit's start: as [`low`], or for the value 1 a ceiling of
/// [`CEILING`] samples, which the fuzzer's small images pass and miss.
fn low_samples(bits: u64) -> WorkLimit {
    match bits & 3 {
        1 => WorkLimit::AtMost(CEILING),
        _ => low(bits),
    }
}

/// The `AtMost` start of the sample limit: 2^12 samples, a 64 x 64 image.
const CEILING: u64 = 1 << 12;

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
        _ if is_sample_limit(kind) => &options.sample_limit,
        LimitKind::PacketWork => &options.packet_work_limit,
        LimitKind::CodeBlockWork => &options.code_block_work_limit,
        other => unreachable!("an adjustable limit with no option: {other:?}"),
    }
}

/// `options` with the limit `kind` reads set to `limit`.
fn with_field(options: &DecodeOptions, kind: LimitKind, limit: WorkLimit) -> DecodeOptions {
    let options = options.clone();
    match kind {
        _ if is_sample_limit(kind) => options.with_sample_limit(limit),
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
    // A refusal by a ceiling the caller set names nothing to retry with, so
    // the ladder ends in this one step. The tile caps are refused by the same
    // ceiling. Not reached for a tile cap at `CEILING`: the area check
    // refuses first. Kept for a higher ceiling.
    if is_sample_limit(kind) && matches!(options.sample_limit, WorkLimit::AtMost(_)) {
        assert!(
            limit.retry_with().is_none() && options.with_retry(limit).is_none(),
            "a refusal under {:?} gives a retry: {limit:?}",
            options.sample_limit
        );
        return None;
    }
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
    // Every decode below, the one with no limits too, is held to the fuzz
    // cap on code-block work, which lets the factors up to `MAX_FACTOR` go
    // as high as they ask. In bounds: the first decode at up to 2, the
    // default 1, the steps 2 and 4, and the decode with no limits 4.
    let base = budget::options(16);
    let mut options = base
        .clone()
        .with_sample_limit(low_samples(control))
        .with_packet_work_limit(low(control >> 2))
        .with_code_block_work_limit(low(control >> 4))
        .with_resolution_reduction(((control >> 6) & 1) as u8)
        .with_strict(control >> 7 == 0);
    // `result` is held while the default decode runs.
    if budget::over(data, 2) {
        if let Err(Error::Limit(limit)) = checked(read_header_with(data, &options)) {
            step(&options, &limit);
        }
        return;
    }
    let defaults = base
        .with_resolution_reduction(options.resolution_reduction)
        .with_strict(options.strict);
    let at_defaults = KINDS
        .into_iter()
        .all(|kind| *field(&options, kind) == WorkLimit::Default);
    let mut result = checked(decode_with(data, &options));
    if matches!(options.sample_limit, WorkLimit::AtMost(_)) {
        // Below the default: a refusal by it is one step, and an image under
        // it is the image of the same options with the default sample limit.
        // In bounds: the two decodes at up to 2 each.
        if let Err(Error::Limit(limit)) = &result {
            // A tile cap's refusal is not reached at `CEILING`: the area
            // check refuses first. Kept for a higher ceiling.
            let next = step(&options, limit);
            assert!(
                !is_sample_limit(limit.kind()) || next.is_none(),
                "a ceiling's refusal did not end the ladder"
            );
        }
        if let Ok(image) = &result {
            let uncapped = options.clone().with_sample_limit(WorkLimit::Default);
            let reference = checked(decode_with(data, &uncapped)).unwrap_or_else(|e| {
                panic!("an image under a ceiling, the default sample limit gives {e:?}")
            });
            assert_eq!(
                (image.width, image.height, &image.components, &image.jp2),
                (
                    reference.width,
                    reference.height,
                    &reference.components,
                    &reference.jp2
                ),
                "the image under a ceiling differs from the decode without it"
            );
        }
        return;
    }
    // The decode under the default limits, when the options start above
    // them. At the defaults it is `result` before any step.
    let default = (!at_defaults).then(|| checked(decode_with(data, &defaults)));
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
    let mut by_kind = [0u32; KINDS.len()];
    while let Err(Error::Limit(limit)) = &result {
        let Some((next, raised)) = step(&options, limit) else {
            break;
        };
        steps += 1;
        let slot = slot(limit.kind());
        by_kind[slot] += 1;
        assert!(
            by_kind[slot] <= MAX_STEPS,
            "{:?}: past {MAX_STEPS} steps: {limit:?}",
            limit.kind()
        );
        if rank(&raised) > MAX_FACTOR {
            return;
        }
        // A refusal by the same limit again must name a larger factor:
        // `step` checks it on the next pass against the raised option.
        options = next;
        result = checked(decode_with(data, &options));
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
            match checked(decode_with(data, &unlimited)) {
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
