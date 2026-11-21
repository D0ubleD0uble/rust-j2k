#![no_main]
//! Fuzz target over [`rust_j2k::IncrementalDecoder`]: the input is fed in
//! pieces whose ends a hash of it picks, with a refine after each. Each
//! refine that decodes goes on from the packet walks the earlier ones kept,
//! and must give what `decode_with` gives with `strict` off on the bytes
//! fed so far: the same image, or the same error. A refine that does not
//! decode waits with `Waiting::MoreBytes`, naming twice the bytes of the
//! last try. A refine due under the doubling rule must
//! decode unless the header is cut short, the packet or code-block work
//! bound or the sample limit refuses that one-shot decode (more bytes raise
//! each of them), or the bound on the walks of tiles not kept, which
//! all the refines spend together, refuses the refine
//! (`rust_j2k::fuzz::incremental_repeat_work`). That refusal may stand where
//! the one-shot decode gives an image or an error. It must come with the
//! walks' visits past `REPEAT_BOUNDS` times that refine's packet bound, and
//! a decoder with nothing spent then gives the one-shot result, and charges
//! nothing for its walks when it kept every one: a single refine walks each
//! tile once, and the first walk of a tile it keeps is not charged. A due
//! refine that waits names its reason (`check_waiting`): `RepeatLimit`
//! exactly when that bound refused it, the one-shot refusal for a header cut
//! short, the sample limit, the charge for missing tiles and the work
//! bounds. A code-block `WorkLimit` is the one-shot refusal and never
//! stands where the one-shot decode gives another error. `BoundSpent` comes
//! only after work was spent, and where the one-shot decode passes the
//! code-block bound, or is refused by it at the tile and for the work the
//! wait's `refused_at` gives: a refine cut short by what was spent does not
//! decode the tiles it counts, and waits there. The probe below is checked
//! by the same rule. After an
//! error, every later refine gives it again, and `finish` is the decode of
//! all the bytes under the decoder's options.
//!
//! With bit 6 of the first control byte, the input also changes the
//! decoder's options before some refines: the reduction (0 to 2, or 33,
//! past the 32 levels a file may have), the layer cap (none, or 1 to 3), or
//! the region (the whole image, a window, tile 0 or 1), as a hash of the
//! file picks. Each refine after is checked under the options as they are
//! then. A layer cap of `Some(0)` or an empty window must be refused and
//! leave the options as they were. Any real change of an option clears the
//! error, even a change back to the options that gave it, and the refines
//! after it wait until the next one due decodes again.
//!
//! The first six input bytes steer the options, as `tile_body`'s first bytes
//! steer its shape, so a small mutation of the file keeps them; the rest is
//! the file (seed it with six zero bytes in front, which mean the default
//! code-block limit and no other option, or with `40 00 00 00 00 00` to
//! change the options between refines). An input that starts with a
//! codestream's `SOC` (`FF 4F`) or the JP2 signature box is instead the whole
//! file with all-zero control bytes, so plain fixtures are valid seeds. The
//! bytes pick: a layer cap, a resolution reduction, a
//! window (from the last four bytes), a tile, a component subset of one or
//! two distinct indices, and a code-block work limit.
//!
//! The decoder holds all its refines to that limit together, and a one-shot
//! decode of the prefix holds only itself. A due refine with nothing spent
//! gives the one-shot decode, and the code-block work that refine spends
//! and the bound it had say exactly whether a decoder with more work spent
//! refuses it (it waits) or gives the same result. Each due refine is
//! checked by that rule, with a decoder that has nothing spent standing in
//! where the refine itself waits or errs. Under the fuzz cap the bound is
//! 2^22 sample passes (`common/budget.rs`), which few inputs this small
//! come near, so each due
//! refine also starts a probe decoder with work already spent, drawn around
//! the most that leaves the refine room
//! (`rust_j2k::fuzz::incremental_with_code_block_work_spent`), and checks
//! it by the same rule.
//!
//! Only the first [`MAX_CHECKED_TRIES`] refines that are checked against
//! decodes of their own (a due one, or one that gives an image or an error)
//! are checked; the later ones only refine, and an error they give is
//! still held to stick. That bounds the decodes of one execution, whatever
//! the input's length, so the target stays under the fuzz cap on
//! code-block work (`common/budget.rs`).

use libfuzzer_sys::fuzz_target;

#[path = "../../tests/support/inconsistent.rs"]
mod inconsistent;
use inconsistent::checked;

#[path = "../common/budget.rs"]
mod budget;

use rust_j2k::fuzz::{
    incremental_code_block_work, incremental_repeat_work, incremental_with_code_block_work_spent,
    REPEAT_BOUNDS,
};
use rust_j2k::{
    decode_with, DecodeOptions, Error, Image, IncrementalDecoder, LimitKind, Refined, Region,
    SampleLimit, Waiting, Window, WorkLimit,
};

/// `result` with its error as its `Debug` text, so that two results compare
/// by variant, data and message: `Error` has no `PartialEq`.
fn keyed<T>(result: Result<T, Error>) -> Result<T, String> {
    result.map_err(|error| format!("{error:?}"))
}

/// The most refines one execution checks against decodes of its own.
/// `tests/fuzz_seed_outcomes.rs` reads this to check the work weight.
const MAX_CHECKED_TRIES: u32 = 4;

fuzz_target!(|input: &[u8]| {
    const JP2_SIGNATURE: [u8; 8] = [0, 0, 0, 12, b'j', b'P', b' ', b' '];
    let (control, data) = if input.starts_with(&[0xFF, 0x4F]) || input.starts_with(&JP2_SIGNATURE) {
        (&[0u8; 6], input)
    } else {
        let Some((control, data)) = input.split_first_chunk::<6>() else {
            return;
        };
        (control, data)
    };
    // The decoder, its kept walks and a one-shot decode of each prefix are
    // alive together, and the prefixes are decoded again and again.
    if budget::over(data, 8) {
        return;
    }
    // The schedule below (fnv1a, splitmix, the piece formula) is copied by
    // `incremental_target_repeat_check_holds_on_a_restarted_walk` in
    // `src/fuzz.rs`. Change both together.
    let mut hash = fnv1a(data);
    // The code-block bound is the one earlier refines cut: the decoder
    // holds all its refines to it together. Without a bound, every refine
    // that decodes is the one-shot decode of its bytes.
    let limit = match control[0] & 3 {
        0 => WorkLimit::Default,
        1 => WorkLimit::Unlimited,
        2 => WorkLimit::Raised(2),
        _ => WorkLimit::Raised(4),
    };
    // Each checked try runs up to three decodes besides the refine (the
    // one-shot, `zero` and the probe), each at up to 4 bounds; the refines
    // share 4 bounds, and `finish` and the last `decode_with` take 8:
    // 12 * MAX_CHECKED_TRIES + 12 bounds, under 64.
    let mut options = budget::options(64).with_code_block_work_limit(limit);
    if control[0] & 4 != 0 {
        options =
            options.with_max_quality_layers(Some(1 + u16::from(((control[0] & 0x7F) >> 3) % 3)));
    }
    if control[0] & 0x20 != 0 {
        options = options.with_resolution_reduction(1);
    }
    // A ceiling the bytes do not raise: a refusal by the sample limit, or by
    // the code-blocks or precincts it allows a tile, is the one-shot decode's
    // sticky error, not a wait. It is sized from the samples the file
    // declares: all of them for three draws in four, else half. The first
    // are admitted by the header's check, unless the decoder counts more
    // than the header declares, so the refines go on to decode; the others
    // may be refused by it, but a reduction, a region or a component subset
    // count fewer samples and can pass. One exception is accepted: a refine
    // cut short by what earlier refines spent waits on `BoundSpent` where a
    // later tile's cap refusal ends the one-shot decode, ceiling or not,
    // because the count's check of that tile's cap ends it, and the
    // code-blocks it skipped may hold an error the one-shot decode meets
    // first (`pipeline/decode.rs`, the comment on `counting`).
    let ceiling = control[0] & 0x80 != 0;
    if ceiling {
        let declared = u64::try_from(budget::declared(data)).unwrap_or(u64::MAX);
        let shift = u64::from(hash % 4 == 3);
        options = options.with_sample_limit(SampleLimit::AtMost((declared >> shift).max(1)));
    }
    // The pieces of the file one refine decodes: a window, or a tile, or
    // neither; and a subset of one or two distinct components. A choice
    // outside the file gives the same error from the decoder and the
    // one-shot decode.
    match control[1] & 3 {
        1 => {
            let (x, y) = (u32::from(control[2] % 24), u32::from(control[3] % 24));
            let (w, h) = (
                1 + u32::from(control[4] % 40),
                1 + u32::from(control[5] % 40),
            );
            options = options.with_region(Region::Window(Window::new(x, y, x + w, y + h)));
        }
        2 => {
            // Tile 0 is the common one.
            let tile = [0, 0, 0, 0, 1, 1, 2, 3][usize::from((control[1] >> 2) & 7)];
            options = options.with_region(Region::Tile(tile));
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
    let changes = control[0] & 0x40 != 0;
    let mut lenient = options.clone().with_strict(false);
    let Ok(mut decoder) = checked(IncrementalDecoder::new(options.clone())) else {
        return;
    };
    let mut fed = 0;
    let mut failed = None;
    // The bytes fed at the last refine that was due.
    let mut tried = 0;
    let mut checks_left = MAX_CHECKED_TRIES;
    while fed < data.len() {
        hash = splitmix(hash);
        // Pieces from 1 byte to about a quarter of the input.
        let piece = 1 + (hash % (data.len() as u64 / 4 + 1)) as usize;
        let end = (fed + piece).min(data.len());
        decoder.feed(&data[fed..end]);
        fed = end;
        if changes {
            hash = splitmix(hash);
            if change(&mut decoder, &mut options, hash) {
                lenient = options.clone().with_strict(false);
                failed = None;
            }
        }
        let (spent, _) = incremental_code_block_work(&decoder);
        let refine = checked(decoder.refine());
        let waiting = match &refine {
            Ok(Refined::Waiting(why)) => Some(why.clone()),
            _ => None,
        };
        let refined = keyed(refine.map(Refined::image));
        if let Some(error) = &failed {
            assert_eq!(refined.as_ref(), Err(error), "poisoned at {fed}");
            continue;
        }
        let due = fed >= tried * 2;
        if due {
            tried = fed;
        }
        if checks_left == 0 || (!due && refined == Ok(None)) {
            // A refine that is not due waits for twice the bytes of the
            // last try. This decodes nothing, so it is checked on every
            // refine, past the checked tries too.
            if !due && refined == Ok(None) {
                match &waiting {
                    Some(Waiting::MoreBytes { until, .. }) if *until == tried * 2 => {}
                    other => panic!("{fed} bytes, not due after {tried}: {other:?}"),
                }
            }
            if let Err(error) = refined {
                failed = Some(error);
            }
            continue;
        }
        checks_left -= 1;
        if due {
            let one_shot = checked(decode_with(&data[..fed], &lenient));
            let shape = one_shot.as_ref().map(|_| ()).map_err(Error::clone);
            let repeat_work = incremental_repeat_work(&decoder);
            if let Some(why) = &waiting {
                assert!(
                    !(ceiling && matches!(why, Waiting::SampleLimit { .. })),
                    "{fed} bytes: waits on a ceiling: {why:?}"
                );
                check_waiting(why, &shape, repeat_work.refused.is_some(), spent, fed);
            }
            let expected = keyed(match one_shot {
                Ok(image) => Ok(Some(image)),
                Err(Error::Incomplete(_)) => Ok(None),
                // The work bounds grow with the bytes under any sample limit.
                Err(Error::Limit(m))
                    if matches!(m.kind(), LimitKind::PacketWork | LimitKind::CodeBlockWork) =>
                {
                    Ok(None)
                }
                // The sample limit's kinds do too, unless the limit is a
                // ceiling.
                Err(Error::Limit(m))
                    if !ceiling
                        && matches!(
                            m.kind(),
                            LimitKind::Samples
                                | LimitKind::TileCodeBlocks
                                | LimitKind::TilePrecincts
                        ) =>
                {
                    Ok(None)
                }
                Err(error) => Err(error),
            });
            // The work this try spends, and what it gives with nothing spent:
            // the one-shot decode. A refine that decodes, or errs, has passed
            // every cut a decoder with nothing spent would pass, so it is
            // what that decoder gives and spends. One that waits
            // once the bound is known may be refused for what earlier
            // refines spent, so a decoder with nothing spent is asked. So
            // is one when the bound on the walks of tiles not kept refused
            // the refine: the code-block rule below does not hold for it.
            let (after, bound) = incremental_code_block_work(&decoder);
            let repeats = refined == Ok(None) && repeat_work.refused.is_some();
            if let Some(packet_bound) = repeat_work.refused {
                // The refusal is the bound's: the walks have spent past it.
                assert!(
                    repeat_work.visits > packet_bound.saturating_mul(REPEAT_BOUNDS),
                    "{fed} bytes: refused at {} visits, of {packet_bound}",
                    repeat_work.visits
                );
            }
            let zeroed;
            let (fresh, work) = if refined == Ok(None) && (bound.is_some() || repeats) {
                let mut zero = checked(IncrementalDecoder::new(options.clone())).unwrap();
                zero.feed(&data[..fed]);
                zeroed = keyed(checked(zero.refine()).map(Refined::image));
                assert_eq!(zeroed, expected, "{fed} bytes, nothing spent");
                let (work, zero_bound) = incremental_code_block_work(&zero);
                assert_eq!(zero_bound, bound, "{fed} bytes, nothing spent");
                // One refine walks each tile once, so it charges only the
                // walks of the tiles it does not keep.
                let zero_repeats = incremental_repeat_work(&zero);
                assert_eq!(zero_repeats.refused, None, "{fed} bytes, nothing spent");
                if zero_repeats.not_kept == 0 {
                    assert_eq!(
                        zero_repeats.visits, 0,
                        "{fed} bytes, nothing spent: a kept walk was charged"
                    );
                }
                (&zeroed, work)
            } else {
                assert_eq!(refined, expected, "{fed} bytes, due");
                let work = after
                    .checked_sub(spent)
                    .expect("the work spent never falls");
                (&refined, work)
            };
            let cut = Cut { fresh, work, bound };
            if !repeats {
                cut.check(&decoder, spent, &refined, "due");
            }
            // A decoder that starts with work spent, drawn around the most
            // that leaves this refine's work room.
            if let Some(bound) = bound {
                hash = splitmix(hash);
                let room = bound.saturating_sub(work);
                let spare = (hash >> 8) % work.saturating_add(1);
                let spent = match hash % 8 {
                    0 => room.saturating_sub(1),
                    1 | 2 => room,
                    3 | 4 => room.saturating_add(1),
                    5 => room.saturating_sub(spare),
                    6 => room.saturating_add(1).saturating_add(spare),
                    _ => u64::MAX - (hash >> 8) % 2,
                };
                let mut probe =
                    incremental_with_code_block_work_spent(options.clone(), spent).unwrap();
                probe.feed(&data[..fed]);
                let probe_refine = checked(probe.refine());
                // The probe is the path that reaches the count past the
                // cut, so its reason is checked too.
                if let Ok(Refined::Waiting(why)) = &probe_refine {
                    let repeats = incremental_repeat_work(&probe).refused.is_some();
                    check_waiting(why, &shape, repeats, spent, fed);
                }
                let probed = keyed(probe_refine.map(Refined::image));
                cut.check(&probe, spent, &probed, "probe");
            }
            if let Err(error) = refined {
                failed = Some(error);
            }
            continue;
        }
        match refined {
            Ok(None) => unreachable!("checked only when it decodes"),
            Ok(Some(image)) => {
                let expected = keyed(checked(decode_with(&data[..fed], &lenient)));
                assert_eq!(Ok(&image), expected.as_ref(), "{fed} bytes");
            }
            Err(error) => {
                match checked(decode_with(&data[..fed], &lenient)) {
                    Err(expected) => {
                        assert_eq!(error, format!("{expected:?}"), "{fed} bytes");
                        assert!(
                            !matches!(expected, Error::Incomplete(_)),
                            "{fed} bytes: {error}"
                        );
                    }
                    Ok(_) => panic!("{fed} bytes: {error}, where the decode gives an image"),
                }
                failed = Some(error);
            }
        }
    }
    let finished = checked(decoder.finish());
    match checked(decode_with(data, &options)) {
        // A header cut short is a truncation once the input has ended.
        Err(Error::Incomplete(why)) => match finished {
            Err(Error::Codestream(m) | Error::FileFormat(m)) => {
                assert!(m.message().contains(why.message()), "{m}")
            }
            other => panic!("{other:?}, where the header is cut short"),
        },
        expected => assert_eq!(keyed(finished), keyed(expected)),
    }
});

/// Check `why`, the reason a due refine of `fed` bytes waited, against
/// `one_shot`, the decode of those bytes on their own (its error, or `()`
/// for an image); `repeats`, whether the bound on the walks done again
/// refused the refine; and `spent`, the code-block work spent before it.
fn check_waiting(
    why: &Waiting,
    one_shot: &Result<(), Error>,
    repeats: bool,
    spent: u64,
    fed: usize,
) {
    if repeats {
        assert!(
            matches!(why, Waiting::RepeatLimit { .. }),
            "{fed} bytes: {why:?}"
        );
        return;
    }
    match (why, one_shot) {
        (Waiting::HeaderIncomplete { .. }, Err(Error::Incomplete(_))) => {}
        // The sample limit, and the code-blocks and precincts it allows a
        // tile, grow with the bytes.
        (Waiting::SampleLimit { limit, .. }, Err(Error::Limit(expected)))
            if matches!(
                expected.kind(),
                LimitKind::Samples | LimitKind::TileCodeBlocks | LimitKind::TilePrecincts
            ) =>
        {
            assert_eq!(format!("{limit:?}"), format!("{expected:?}"), "{fed} bytes");
        }
        (
            Waiting::MissingTiles { limit, .. } | Waiting::WorkLimit { limit, .. },
            Err(Error::Limit(expected)),
        ) if limit.kind() == LimitKind::PacketWork => {
            assert_eq!(format!("{limit:?}"), format!("{expected:?}"), "{fed} bytes");
        }
        // A code-block `WorkLimit` is the decode's own refusal, and never
        // stands in for an error it gives.
        (Waiting::WorkLimit { limit, .. }, Err(Error::Limit(expected)))
            if limit.kind() == LimitKind::CodeBlockWork
                && expected.kind() == LimitKind::CodeBlockWork =>
        {
            assert_eq!(format!("{limit:?}"), format!("{expected:?}"), "{fed} bytes");
        }
        // What earlier refines spent cut it short, so work was spent. The
        // decode on its own passes the sample limit, which the header's
        // check refuses before any tile. It may stand in for any other
        // error, a limit included, that the decode meets in the tiles the
        // refine did not decode. That includes `TileCodeBlocks` and
        // `TilePrecincts`, unlike the arm above. A tile-part `COD` can take
        // a later tile over those caps. The decode checks a tile's caps as
        // it reaches the tile, after the tiles before it
        // (`bound_spent_stands_in_for_a_tile_cap_refusal.seed`). Where that
        // decode is refused by the code-block bound, the refine's count was
        // too, at the same tile and for the same work, and the wait names
        // them.
        (Waiting::BoundSpent { limit, .. }, _)
            if spent > 0
                && !matches!(one_shot, Err(Error::Incomplete(_)))
                && !matches!(one_shot, Err(Error::Limit(m)) if m.kind() == LimitKind::Samples) =>
        {
            assert_eq!(limit.kind(), LimitKind::CodeBlockWork, "{fed} bytes");
            assert_eq!(limit.retry_with(), None, "{fed} bytes");
            if let Err(Error::Limit(m)) = one_shot {
                if m.kind() == LimitKind::CodeBlockWork {
                    assert!(m.refused_at().is_some(), "{fed} bytes: {m:?}");
                    assert_eq!(
                        limit.refused_at(),
                        m.refused_at(),
                        "{fed} bytes: {limit}, where the decode gives {m}"
                    );
                }
            }
        }
        _ => panic!("{fed} bytes: waited for {why:?}, where the decode gives {one_shot:?}"),
    }
}

/// Change one of `decoder`'s options as `hash` picks, or none, through the
/// setters, and `options` with it. Whether an option changed.
fn change(decoder: &mut IncrementalDecoder, options: &mut DecodeOptions, hash: u64) -> bool {
    let pick = hash >> 8;
    let wanted = match hash % 8 {
        0 => options
            .clone()
            .with_resolution_reduction([0, 1, 2, 33][(pick % 4) as usize]),
        1 => options.clone().with_max_quality_layers(
            [None, Some(0), Some(1), Some(2), Some(3)][(pick % 5) as usize],
        ),
        2 => {
            let (x, y) = ((pick >> 3) as u32 % 24, (pick >> 8) as u32 % 24);
            let (w, h) = (1 + (pick >> 13) as u32 % 40, 1 + (pick >> 19) as u32 % 40);
            let region = match pick % 6 {
                0 => Region::Whole,
                1 | 2 => Region::Window(Window::new(x, y, x + w, y + h)),
                3 => Region::Tile(0),
                4 => Region::Tile(1),
                _ => Region::Window(Window::new(x, y, x, y + h)),
            };
            options.clone().with_region(region)
        }
        _ => return false,
    };
    let set = match hash % 8 {
        0 => checked(decoder.set_resolution_reduction(wanted.resolution_reduction)),
        1 => checked(decoder.set_max_quality_layers(wanted.max_quality_layers)),
        _ => checked(decoder.set_region(wanted.region.clone())),
    };
    let empty = matches!(wanted.region, Region::Window(w) if w.x0 >= w.x1 || w.y0 >= w.y1);
    if wanted.max_quality_layers == Some(0) || empty {
        assert!(matches!(set, Err(Error::InvalidOptions(_))), "{set:?}");
        return false;
    }
    set.expect("valid options are set");
    let changed = wanted.resolution_reduction != options.resolution_reduction
        || wanted.max_quality_layers != options.max_quality_layers
        || wanted.region != options.region;
    *options = wanted;
    changed
}

/// What a decoder with nothing spent gives on some bytes: its result, the
/// code-block work its refine spent and the bound that refine had.
struct Cut<'a> {
    fresh: &'a Result<Option<Image>, String>,
    work: u64,
    bound: Option<u64>,
}

impl Cut<'_> {
    /// Check `refined`, what `decoder` gave on the same bytes after `spent`
    /// of the code-block work, by the ledger's rule: the decode may spend
    /// what is left of the bound after `spent` (all of it, when the bound
    /// is `u64::MAX`), and a tile whose work passes what is left refuses the
    /// refine. The work comes in tile by tile, so the refine refuses if and
    /// only if this try's work, all of it, passes what is left; then it
    /// spends at most that work, and otherwise all of it. A refused refine
    /// waits, or gives the error the decoder with nothing spent gives. A
    /// refine on which a decoder with nothing spent waits waits too.
    fn check(
        &self,
        decoder: &IncrementalDecoder,
        spent: u64,
        refined: &Result<Option<Image>, String>,
        what: &str,
    ) {
        let (after, bound) = incremental_code_block_work(decoder);
        assert_eq!(bound, self.bound, "{what}");
        let left = match self.bound {
            None => u64::MAX,
            Some(u64::MAX) => u64::MAX,
            Some(bound) => bound.saturating_sub(spent),
        };
        let total = spent.saturating_add(self.work);
        // Whether the decoder with nothing spent gave an image or an error:
        // it passed every cut, so this try's work is all of `work`.
        let answered = *self.fresh != Ok(None);
        if answered && self.work > left {
            // Refused for what was spent, the refine counts on through the
            // later tiles without decoding them, and an error it meets there
            // ends the count: it waits, and never gives an error.
            assert!(
                refined == &Ok(None),
                "{what}: {} past {left}: {refined:?}, where nothing spent gives {:?}",
                self.work,
                self.fresh
            );
        } else {
            assert_eq!(refined, self.fresh, "{what}: {} within {left}", self.work);
        }
        if answered && self.work <= left {
            assert_eq!(after, total, "{what}: {spent} and {}", self.work);
        } else {
            assert!(
                spent <= after && after <= total,
                "{what}: {spent} to {after} of {total}"
            );
        }
    }
}

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
