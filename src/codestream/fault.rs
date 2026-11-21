//! The walk of the tile-parts that looks for a fault and keeps no tile
//! record: [`tile_parts_fault`] gives the error [`parse_tiles_with`] gives,
//! in time linear in the input and with heap that grows with the tile count
//! and, for the tiles whose later tile-parts restate their own coding with
//! headers that differ, with those tiles' first tile-part headers.
//!
//! It is [`walk_tiles`] step for step, with what a tile record holds cut to
//! what the faults depend on: for each tile, the tile-parts seen, the count
//! they declare, the `POC` volumes they carry, and where its first
//! tile-part's header starts. That header is read again for a later
//! tile-part that restates a value ([`check_restatement`]), at most twice a
//! tile: at the first such tile-part, after which where that tile-part's
//! header lies is kept, so that a later header of the same bytes is known
//! to restate the same; and at the first whose header differs, after which
//! the coding it gives is kept for the tile's later restatements. The two
//! walks are kept in step
//! by a differential over every committed codestream and header seed in
//! `tests.rs`, and over fuzzed input by the `header` fuzz target
//! ([`crate::fuzz::tile_parts_walks_agree`]). [`tile_parts_fault_kept`]
//! keeps the walk between calls on an input that grows, with `strict` off,
//! and the same differential holds it to a walk from the start on growing
//! prefixes.
//!
//! [`parse_tiles_with`]: super::parse_tiles_with
//! [`walk_tiles`]: super::walk_tiles

use std::cell::OnceCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

use super::{
    Cursor, Error, FaultSite, Overrides, ParsedHeader, Restated, Result, both_packed_headers,
    check_first_tile_part, check_poc_budget, check_restatement, count_is_one_short, decode_sot,
    fault_may_be_cut, marker, part_end, read_tile_part_header, read_tile_part_header_at, read_u16,
    segment, sot_at,
};

impl ParsedHeader {
    /// The length of the main header: the offset of the first `SOT`.
    pub(crate) fn main_header_len(&self) -> usize {
        self.sot_offset
    }
}

/// What the walk keeps of one tile, 6 bytes.
#[derive(Debug, Clone, Copy, Default)]
struct Seen {
    /// The tile-parts read, which `TPsot` must equal.
    parts: u16,
    /// The count of tile-parts declared, or 0 while no `TNsot` has stated
    /// one (a stated count is at least 1).
    declared: u16,
    /// The `POC` volumes of its tile-parts, at most
    /// [`MAX_POC_VOLUMES`](super::MAX_POC_VOLUMES).
    poc: u8,
    /// [`CODES`] and [`POC_FIRST`].
    flags: u8,
}

/// The first tile-part's header carries a `COD`, `QCD`, `COC` or `QCC`, so
/// [`Walk::firsts`] holds where it starts.
const CODES: u8 = 1;
/// The first tile-part carries a `POC`, which replaces the main header's.
const POC_FIRST: u8 = 2;

/// Offsets of the input, a fixed number of slots for each tile of the
/// grid. Nothing is held until a slot is set; then it holds an entry for
/// every slot, 4 bytes each where the input is under 4 GiB and 8 bytes
/// past that. A slot never set reads as 0, which no tile-part header
/// starts at.
enum Offsets {
    Narrow(Vec<u32>),
    Wide(Vec<usize>),
}

impl Offsets {
    fn new(len: usize) -> Self {
        if u32::try_from(len).is_ok() {
            Offsets::Narrow(Vec::new())
        } else {
            Offsets::Wide(Vec::new())
        }
    }

    /// Widen the offsets for an input of `len` bytes, which may have grown
    /// past 4 GiB since [`new`](Self::new).
    fn fit(&mut self, len: usize) {
        if let Offsets::Narrow(offsets) = self {
            if u32::try_from(len).is_err() {
                *self = Offsets::Wide(offsets.iter().map(|&at| at as usize).collect());
            }
        }
    }

    /// Set slot `index` of `slots` to `at`, an offset of the input.
    fn set(&mut self, index: usize, slots: usize, at: usize) {
        match self {
            Offsets::Narrow(offsets) => {
                if offsets.is_empty() {
                    offsets.resize(slots, 0);
                }
                // `new` saw the input fits a `u32`, and `at` is in it.
                offsets[index] = at as u32;
            }
            Offsets::Wide(offsets) => {
                if offsets.is_empty() {
                    offsets.resize(slots, 0);
                }
                offsets[index] = at;
            }
        }
    }

    /// What [`set`](Self::set) noted for slot `index`, or 0.
    fn get(&self, index: usize) -> usize {
        match self {
            Offsets::Narrow(offsets) => offsets.get(index).map_or(0, |&at| at as usize),
            Offsets::Wide(offsets) => offsets.get(index).copied().unwrap_or(0),
        }
    }
}

/// The error [`parse_tiles_with`](super::parse_tiles_with) gives, `strict`
/// or not, for the tile-parts after `parsed`'s main header, or `None` if it
/// gives none. It reads `parsed` in place and keeps no tile record: its
/// heap is 6 bytes a tile of the grid, 4 more a tile (8 on an input of
/// 4 GiB or more) once any tile's first tile-part carries a `COD`, `QCD`,
/// `COC` or `QCC`, and 4 more a tile (8) once a later tile-part restates
/// one of those, plus one tile-part header at a time, plus, for each tile
/// whose later tile-parts restate them with headers that differ, the coding
/// its first tile-part gives ([`Walk::restating`]). A tile's header is
/// checked as its first tile-part is read, as the tile walk checks it, so a
/// tile at fault is resolved in full from the header in hand.
pub(crate) fn tile_parts_fault(bytes: &[u8], parsed: &ParsedHeader, strict: bool) -> Option<Error> {
    Walk::new(bytes, parsed)
        .run(bytes, parsed, strict, &mut None)
        .err()
}

/// The walk of [`tile_parts_fault_kept`], kept for its next call on an
/// input that extends this one: where the walk stood after the last
/// tile-part it read whose end, by a `Psot` other than 0, is followed by an
/// `SOT` in the input. Every step before there read only bytes before
/// there, which a longer input holds the same.
pub(crate) struct KeptWalk {
    /// The bytes of the input it was kept for.
    len: usize,
    walk: Walk,
}

/// [`tile_parts_fault`], going on from the walk `kept` holds, and keeping
/// its walk there for a later call, with `strict` off. `kept` must have
/// been filled by a call on the same main header, at the start of an input
/// that `bytes` extend; a walk kept for another tile count or more bytes is
/// dropped.
///
/// The calls on an input that grows walk each tile-part about once in all:
/// each reads the tile-parts past where the last one was kept, and the one
/// it was kept before. A step that stops on a fault is taken back, so the
/// next call reads that tile-part again and finds the same fault. With
/// `strict` on it walks from the start and leaves `kept` as it is: the
/// `TNsot` look-ahead's answer then decides the walk's steps, and it reads
/// the bytes past them.
pub(crate) fn tile_parts_fault_kept(
    bytes: &[u8],
    parsed: &ParsedHeader,
    strict: bool,
    kept: &mut Option<KeptWalk>,
) -> Option<Error> {
    if strict {
        return tile_parts_fault(bytes, parsed, strict);
    }
    let tiles = parsed.header.siz.num_tiles() as usize;
    let mut walk = match kept.take() {
        Some(kept) if kept.len <= bytes.len() && kept.walk.seen.len() == tiles => {
            let mut walk = kept.walk;
            walk.firsts.fit(bytes.len());
            walk.restates.fit(bytes.len());
            walk
        }
        _ => Walk::new(bytes, parsed),
    };
    let mut undo = None;
    let fault = walk.run(bytes, parsed, strict, &mut undo).err();
    if let Some(undo) = undo {
        undo.restore(&mut walk);
    }
    *kept = Some(KeptWalk {
        len: bytes.len(),
        walk,
    });
    fault
}

/// The state of a walk of the tile-parts between two of its steps.
struct Walk {
    seen: Vec<Seen>,
    /// Where the header of each tile's first tile-part starts, for the
    /// tiles flagged [`CODES`].
    firsts: Offsets,
    /// For each tile flagged [`CODES`], where the header of its first later
    /// tile-part that restates a coding value starts. That header was found
    /// to restate the first tile-part's values, and [`check_restatement`]
    /// reads nothing of a later header but the values it carries, so a later
    /// header whose bytes the input holds from there too restates them and
    /// is not checked again. A header is read from its start to the first
    /// `SOD` its markers reach, so those bytes are that header whole. Like
    /// [`firsts`](Self::firsts), it notes bytes the walk has passed, which a
    /// longer input holds the same.
    restates: Offsets,
    /// For each tile flagged [`CODES`] a later tile-part of which restates a
    /// coding value with a header of other bytes than the one
    /// [`restates`](Self::restates) notes: its first tile-part's coding,
    /// read again once, and what [`check_restatement`] has found so far, as
    /// the tile record holds them. An entry costs about a kilobyte for a
    /// header of one `COD`, and more for each `COC` or `QCC`: up to about 14
    /// times the bytes of the tile's tile-parts, for a header of `QCC`s and
    /// three tile-parts, the fewest such a tile has.
    restating: HashMap<usize, (Overrides, Option<Restated>)>,
    /// Whether the `TNsot` look-ahead ([`count_is_one_short`]) has been
    /// taken: once, when a tile first has every tile-part it declares.
    count_checked: bool,
    /// With `strict` on, 1 where the look-ahead raised every tile's count.
    count_correction: u16,
    /// With `strict` off, where the look-ahead starts and for which tile,
    /// once it is taken. Its answer only raises both counts in the error for
    /// a tile that declares two, so it is looked for only there.
    look_ahead: Option<(usize, u16)>,
    /// The `PPM` chunks the tile-parts read so far take.
    ppm_seq: usize,
    /// Where the next tile-part starts.
    part_offset: usize,
}

/// What a step of [`Walk::run`] changed, to take back where the walk stops
/// in that step, so that a call on a longer input reads that tile-part
/// again. With `strict` off a step changes only these, what it notes of a
/// first tile-part's header, which reads the same again, and what it notes
/// of a later tile-part's header found to restate that one's values
/// ([`Walk::restates`], [`Walk::restating`]), which restates them again. So
/// the step taken again finds the same, without reading the first header
/// again. It does not
/// move where the next tile-part starts, which moves only once the step is
/// done. Where it takes the `TNsot` look-ahead it also notes where that
/// starts, which the step taken again notes afresh, as `count_checked` is
/// taken back. The checks of a tile's header and its `POC` volumes keep
/// nothing past the step but the tile's count of volumes.
struct Undo {
    index: usize,
    tile: Seen,
    count_checked: bool,
    ppm_seq: usize,
}

impl Undo {
    fn restore(self, walk: &mut Walk) {
        walk.seen[self.index] = self.tile;
        walk.count_checked = self.count_checked;
        walk.ppm_seq = self.ppm_seq;
    }
}

impl Walk {
    /// A walk from the first `SOT` of `bytes`.
    fn new(bytes: &[u8], parsed: &ParsedHeader) -> Self {
        Walk {
            seen: vec![Seen::default(); parsed.header.siz.num_tiles() as usize],
            firsts: Offsets::new(bytes.len()),
            restates: Offsets::new(bytes.len()),
            restating: HashMap::new(),
            count_checked: false,
            count_correction: 0,
            look_ahead: None,
            ppm_seq: 0,
            part_offset: parsed.sot_offset,
        }
    }

    /// [`walk_tiles`](super::walk_tiles) without the tile records, from
    /// where the walk stands: each step it takes, and each error it gives,
    /// in the same order. `undo` is left holding what to take back of the
    /// step the walk stopped in, or `None` if it stopped before one.
    fn run(
        &mut self,
        bytes: &[u8],
        parsed: &ParsedHeader,
        strict: bool,
        undo: &mut Option<Undo>,
    ) -> Result<()> {
        let main = &parsed.header;
        let ppm = &parsed.ppm;
        let Walk {
            seen,
            firsts,
            restates,
            restating,
            count_checked,
            count_correction,
            look_ahead,
            ppm_seq,
            part_offset,
        } = self;
        let tile_count = seen.len();
        let uniform = OnceCell::new();
        let summary = OnceCell::new();
        // Whether the walk ended where the input did, with `strict` off:
        // where `walk_tiles` records `ReportEntry::MissingEoc`.
        let mut ended_early = false;

        loop {
            *undo = None;
            if !strict && bytes.len().saturating_sub(*part_offset) < 12 {
                ended_early = true;
                break;
            }
            // The fixed `SOT` layout is read directly; anything else is read
            // as `walk_tiles` reads it, for its error.
            let (sot, mut cur) = match sot_at(bytes, *part_offset) {
                Some((sot, pos)) => (sot, Cursor::at(bytes, pos)),
                None => {
                    let mut cur = Cursor::at(bytes, *part_offset);
                    if cur.u16()? != marker::SOT {
                        return Err(Error::codestream("tile-part does not start with `SOT`"));
                    }
                    let sot = decode_sot(segment(&mut cur)?)?;
                    (sot, cur)
                }
            };

            let index = usize::from(sot.tile_index);
            // `walk_tiles` refuses a tile record past 65534 here, which the
            // grid of at most 65535 tiles never reaches.
            let tile = seen.get_mut(index).ok_or_else(|| {
                Error::codestream(format!(
                    "tile-part names tile {index}, but the grid holds {tile_count} tiles"
                ))
            })?;
            *undo = Some(Undo {
                index,
                tile: *tile,
                count_checked: *count_checked,
                ppm_seq: *ppm_seq,
            });

            if usize::from(sot.tile_part_index) != usize::from(tile.parts) {
                return Err(Error::codestream(format!(
                    "tile {index} tile-part index is {}, but {} of its tile-parts have been read",
                    sot.tile_part_index, tile.parts,
                )));
            }
            if sot.num_tile_parts != 0 {
                let stated = u16::from(sot.num_tile_parts) + *count_correction;
                if strict && u16::from(sot.tile_part_index) >= stated {
                    return Err(Error::codestream(format!(
                        "tile {index} tile-part {} of a declared {stated} tile-parts",
                        sot.tile_part_index,
                    )));
                }
                match tile.declared {
                    0 => tile.declared = stated,
                    declared if declared != stated => {
                        // With `strict` off the look-ahead is taken here,
                        // on the same bytes, and raises both counts or
                        // neither.
                        let raised = u16::from(
                            look_ahead
                                .is_some_and(|(from, tile)| count_is_one_short(bytes, from, tile)),
                        );
                        return Err(Error::codestream(format!(
                            "tile {index} declares {} tile-parts in one `SOT` and {} in another",
                            stated + raised,
                            declared + raised,
                        )));
                    }
                    _ => {}
                }
            }

            let first_part = sot.tile_part_index == 0;
            let header_at = cur.pos;
            let mut site = FaultSite::default();
            // A header of `SOD` alone, as most later tile-parts have, reads as
            // nothing: no marker of its own, no `POC` volume and no packed
            // header. It is stepped over rather than read.
            let read = if read_u16(bytes, header_at) == Some(marker::SOD) {
                cur.pos += 2;
                Ok(None)
            } else {
                let end = part_end(bytes.len(), *part_offset, sot.psot);
                read_tile_part_header_at(&mut cur, end, main, first_part, &mut site).map(Some)
            };
            let read = match read {
                Ok(read) => read,
                Err(_) if !strict && fault_may_be_cut(bytes, *part_offset, sot.psot, site) => {
                    ended_early = true;
                    break;
                }
                Err(error) => return Err(error),
            };
            let mut packed_headers = false;
            if let Some((tile_header, packed)) = read {
                packed_headers = packed.is_some();
                if first_part {
                    check_first_tile_part(main, &summary, &tile_header, index)?;
                    if tile_header.overrides.codes_or_quantizes() {
                        tile.flags |= CODES;
                        firsts.set(index, tile_count, header_at);
                    }
                    if !tile_header.poc.is_empty() {
                        tile.flags |= POC_FIRST;
                    }
                    // `decode_poc` holds a header to `MAX_POC_VOLUMES`.
                    tile.poc = tile_header.poc.len() as u8;
                } else {
                    if tile_header.overrides.codes_or_quantizes() {
                        // The first tile-part's header read again: it read
                        // the same when the walk passed it. What
                        // `check_restatement` keeps between tile-parts only
                        // saves it comparing again, so starting it afresh
                        // gives the same result.
                        let first_again = || -> Result<Overrides> {
                            #[cfg(test)]
                            super::tests::FIRST_REREADS.with(|reads| reads.set(reads.get() + 1));
                            // It read in full before, so no arm that `end`
                            // decides is reached, and the input's end serves.
                            let mut at = Cursor::at(bytes, firsts.get(index));
                            Ok(read_tile_part_header(&mut at, bytes.len(), main, true)?
                                .0
                                .overrides)
                        };
                        if tile.flags & CODES == 0 {
                            let own = Overrides::default();
                            check_restatement(
                                main,
                                &uniform,
                                &own,
                                &mut None,
                                &tile_header,
                                index,
                            )?;
                        } else {
                            let from = restates.get(index);
                            let header = &bytes[header_at..cur.pos];
                            if from == 0 {
                                let own = first_again()?;
                                check_restatement(
                                    main,
                                    &uniform,
                                    &own,
                                    &mut None,
                                    &tile_header,
                                    index,
                                )?;
                                restates.set(index, tile_count, header_at);
                            } else if !bytes[from..].starts_with(header) {
                                let (own, restated) = match restating.entry(index) {
                                    Entry::Occupied(entry) => entry.into_mut(),
                                    Entry::Vacant(entry) => {
                                        let mut own = first_again()?;
                                        // Kept, so held to its length.
                                        own.coc.shrink_to_fit();
                                        own.qcc.shrink_to_fit();
                                        own.rgn.shrink_to_fit();
                                        entry.insert((own, None))
                                    }
                                };
                                check_restatement(
                                    main,
                                    &uniform,
                                    own,
                                    restated,
                                    &tile_header,
                                    index,
                                )?;
                            }
                        }
                    }
                    let volumes = usize::from(tile.poc) + tile_header.poc.len();
                    check_poc_budget(volumes)?;
                    tile.poc = volumes as u8;
                    // The volumes the tile resolves to, as the tile walk
                    // counts them.
                    if tile.flags & POC_FIRST == 0 {
                        check_poc_budget(main.poc.len() + volumes)?;
                    }
                }
            }
            // A tile-part with no `POC` volume adds none to the tile's, which
            // passed the budget, so the budget is not checked again.
            if !ppm.is_empty() {
                if packed_headers {
                    return Err(both_packed_headers());
                }
                if *ppm_seq >= ppm.len() {
                    return Err(Error::codestream(
                        "`PPM` carries fewer packet-header chunks than the codestream has tile-parts",
                    ));
                }
                *ppm_seq += 1;
            }
            let data_start = cur.pos;

            let data_end = if sot.psot == 0 {
                match bytes.len().checked_sub(2).filter(|&end| end >= data_start) {
                    Some(end) => end,
                    None if !strict => data_start,
                    None => {
                        return Err(Error::codestream(format!(
                            "tile {} tile-part {} has `Psot` 0 and fewer than two bytes after its \
                             header",
                            sot.tile_index, sot.tile_part_index
                        )));
                    }
                }
            } else {
                let end = match part_offset
                    .checked_add(sot.psot as usize)
                    .filter(|&end| end <= bytes.len())
                {
                    Some(end) => end,
                    None if !strict => bytes.len(),
                    None => {
                        return Err(Error::codestream(format!(
                            "tile {} tile-part {} declares `Psot` {}, but only {} bytes are left in \
                             the codestream",
                            sot.tile_index,
                            sot.tile_part_index,
                            sot.psot,
                            bytes.len() - *part_offset
                        )));
                    }
                };
                if end < data_start {
                    return Err(Error::codestream(format!(
                        "tile {} tile-part {} declares `Psot` {}, shorter than its {}-byte header",
                        sot.tile_index,
                        sot.tile_part_index,
                        sot.psot,
                        data_start - *part_offset
                    )));
                }
                end
            };
            let tile = &mut seen[index];
            tile.parts += 1;

            if !*count_checked && tile.declared == u16::from(sot.tile_part_index) + 1 {
                *count_checked = true;
                if sot.psot != 0 {
                    if !strict {
                        *look_ahead = Some((data_end, sot.tile_index));
                    } else if count_is_one_short(bytes, data_end, sot.tile_index) {
                        *count_correction = 1;
                        for tile in seen.iter_mut() {
                            if tile.declared != 0 {
                                tile.declared += 1;
                            }
                        }
                    }
                }
            }

            if sot.psot == 0 {
                break;
            }
            if data_end == bytes.len() && !strict {
                ended_early = true;
                break;
            }
            match read_u16(bytes, data_end) {
                Some(marker::EOC) => break,
                // The step read nothing past this `SOT`'s code, so it
                // stands in any longer input.
                Some(marker::SOT) => *part_offset = data_end,
                Some(other) => {
                    return Err(Error::codestream(format!(
                        "expected `SOT` or `EOC` after a tile-part, found {other:#06X}"
                    )));
                }
                None if !strict => {
                    ended_early = true;
                    break;
                }
                None => {
                    return Err(Error::codestream("missing `EOC` after the tile-part"));
                }
            }
        }

        if *ppm_seq != ppm.len() && !(ended_early && *ppm_seq < ppm.len()) {
            return Err(Error::codestream(format!(
                "`PPM` carries {} packet-header chunk(s) but the codestream has {} tile-part(s)",
                ppm.len(),
                ppm_seq,
            )));
        }
        for (index, tile) in seen.iter().enumerate() {
            if tile.parts == 0 {
                if strict {
                    return Err(Error::codestream(format!(
                        "the codestream carries no tile-part for tile {index}"
                    )));
                }
                continue;
            }
            if strict && tile.declared != 0 && tile.declared != tile.parts {
                return Err(Error::codestream(format!(
                    "tile {index} declares {} tile-parts but carries {}",
                    tile.declared, tile.parts
                )));
            }
        }
        Ok(())
    }
}
