//! The walk of the tile-parts that looks for a fault and keeps no tile
//! record: [`tile_parts_fault`] gives the error [`parse_tiles_with`] gives,
//! in time linear in the input and with heap that grows with the tile count
//! and, for the tiles whose first tile-part header of over [`KEPT_OVER`]
//! bytes a later tile-part restates a coding value of, with those headers.
//!
//! It is [`walk_tiles`] step for step, with what a tile record holds cut to
//! what the faults depend on: for each tile, the tile-parts seen, the count
//! they declare, the `POC` volumes they carry, and where its first
//! tile-part's header starts. That header is read again for a later
//! tile-part that restates a value ([`check_restatement`]): for each such
//! tile-part where it is short, and once for the tile where it is long, the
//! coding it gives then kept for the tile's later restatements. The two
//! walks are kept in step
//! by a differential over every committed codestream and header seed in
//! `tests.rs`, and over fuzzed input by the `header` fuzz target
//! ([`crate::fuzz::tile_parts_walks_agree`]).
//!
//! [`parse_tiles_with`]: super::parse_tiles_with
//! [`walk_tiles`]: super::walk_tiles

use std::cell::OnceCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

use super::{
    Cursor, Error, MAX_POC_VOLUMES, Overrides, ParsedHeader, Restated, Result, TileHeader,
    both_packed_headers, check_poc_budget, check_restatement, check_tile_in_full,
    count_is_one_short, decode_sot, marker, read_tile_part_header, read_u16, segment, sot_at,
    tile_overrides_are_valid,
};

/// What the walk keeps of one tile, 6 bytes.
#[derive(Debug, Clone, Copy, Default)]
struct Seen {
    /// The tile-parts read, which `TPsot` must equal.
    parts: u16,
    /// The count of tile-parts declared, or 0 while no `TNsot` has stated
    /// one (a stated count is at least 1).
    declared: u16,
    /// The `POC` volumes of its tile-parts, at most [`MAX_POC_VOLUMES`].
    poc: u8,
    /// [`CODES`], [`POC_FIRST`], [`INVALID`] and [`LONG`].
    flags: u8,
}

/// The first tile-part's header carries a `COD`, `QCD`, `COC` or `QCC`, so
/// [`Firsts`] holds where it starts.
const CODES: u8 = 1;
/// The first tile-part carries a `POC`, which replaces the main header's.
const POC_FIRST: u8 = 2;
/// The first tile-part's markers fail [`tile_overrides_are_valid`].
const INVALID: u8 = 4;
/// The first tile-part's header, flagged [`CODES`], is longer than
/// [`KEPT_OVER`], so the coding read from it again is kept.
const LONG: u8 = 8;

/// The length past which a first tile-part header, read again for a later
/// tile-part that restates a value, is kept rather than read again for
/// each. A kept header costs about a kilobyte whatever its length (its
/// `Overrides` and the state of [`check_restatement`], with their maps),
/// and more for each `COC` or `QCC` it holds: up to about 13 bytes of heap
/// a byte of header, for a header of `QCC`s. Keeping only the headers
/// longer than this holds that heap to at most about 13 times their bytes
/// in the input, where keeping every header would cost a kilobyte for a
/// tile whose header is one 14-byte `COD`. A shorter header is read again
/// for each restating tile-part, which is at least 14 bytes long, so that
/// work stays within about 73 times the input.
const KEPT_OVER: usize = 1024;

/// Where the header of each tile's first tile-part starts, for the tiles
/// flagged [`CODES`]. Nothing is held until a tile is flagged; then it
/// holds an entry for every tile of the grid, 4 bytes each where the input
/// is under 4 GiB and 8 bytes past that.
enum Firsts {
    Narrow(Vec<u32>),
    Wide(Vec<usize>),
}

impl Firsts {
    fn new(len: usize) -> Self {
        if u32::try_from(len).is_ok() {
            Firsts::Narrow(Vec::new())
        } else {
            Firsts::Wide(Vec::new())
        }
    }

    /// Note that tile `index` of `tiles` has its first tile-part's header
    /// at `at`, an offset of the input.
    fn set(&mut self, index: usize, tiles: usize, at: usize) {
        match self {
            Firsts::Narrow(firsts) => {
                if firsts.is_empty() {
                    firsts.resize(tiles, 0);
                }
                // `new` saw the input fits a `u32`, and `at` is in it.
                firsts[index] = at as u32;
            }
            Firsts::Wide(firsts) => {
                if firsts.is_empty() {
                    firsts.resize(tiles, 0);
                }
                firsts[index] = at;
            }
        }
    }

    /// What [`set`](Self::set) noted for tile `index`.
    fn get(&self, index: usize) -> usize {
        match self {
            Firsts::Narrow(firsts) => firsts[index] as usize,
            Firsts::Wide(firsts) => firsts[index],
        }
    }
}

/// The error [`parse_tiles_with`](super::parse_tiles_with) gives, `strict`
/// or not, for the tile-parts after `parsed`'s main header, or `None` if it
/// gives none. It reads `parsed` in place and keeps no tile record: its
/// heap is 6 bytes a tile of the grid, and 4 more a tile (8 on an input of
/// 4 GiB or more) once any tile's first tile-part carries a `COD`, `QCD`,
/// `COC` or `QCC`, plus one tile-part header at a time, plus, for each tile
/// whose first tile-part header is over [`KEPT_OVER`] bytes and has a value
/// a later tile-part restates, the coding it gives: up to about 13 times
/// the bytes of those headers. The walk again for a tile whose joined
/// header is at fault
/// starts once the first walk's record is let go, so the two are not held
/// at once.
pub(crate) fn tile_parts_fault(bytes: &[u8], parsed: &ParsedHeader, strict: bool) -> Option<Error> {
    walk(bytes, parsed, strict, None)
        .and_then(|(seen, _)| check_headers(bytes, parsed, strict, seen))
        .err()
}

/// [`walk_tiles`](super::walk_tiles) without the tile records: each step it
/// takes, and each error it gives, in the same order. With `keep`, it also
/// gives that tile's header as the tile record would hold it, its `POC`
/// volumes joined over its tile-parts.
fn walk(
    bytes: &[u8],
    parsed: &ParsedHeader,
    strict: bool,
    keep: Option<usize>,
) -> Result<(Vec<Seen>, Option<TileHeader>)> {
    let main = &parsed.header;
    let ppm = &parsed.ppm;
    let tile_count = main.siz.num_tiles() as usize;
    let mut ppm_seq = 0usize;
    let mut seen = vec![Seen::default(); tile_count];
    let mut firsts = Firsts::new(bytes.len());
    let mut kept: Option<TileHeader> = None;
    let mut count_checked = false;
    let mut count_correction = 0u16;
    let uniform = OnceCell::new();
    let summary = OnceCell::new();
    // For each tile flagged [`LONG`] a later tile-part of which restates a
    // coding value: its first tile-part's coding, read again once, and what
    // [`check_restatement`] has found so far, as the tile record holds them.
    let mut restating: HashMap<usize, (Overrides, Option<Restated>)> = HashMap::new();
    // Whether the walk ended where the input did, with `strict` off: where
    // `walk_tiles` records `ReportEntry::MissingEoc`.
    let mut ended_early = false;

    let mut part_offset = parsed.sot_offset;
    loop {
        if !strict && bytes.len().saturating_sub(part_offset) < 12 {
            ended_early = true;
            break;
        }
        // The fixed `SOT` layout is read directly; anything else is read
        // as `walk_tiles` reads it, for its error.
        let (sot, mut cur) = match sot_at(bytes, part_offset) {
            Some((sot, pos)) => (sot, Cursor::at(bytes, pos)),
            None => {
                let mut cur = Cursor::at(bytes, part_offset);
                if cur.u16()? != marker::SOT {
                    return Err(Error::codestream("tile-part does not start with `SOT`"));
                }
                let sot = decode_sot(segment(&mut cur)?)?;
                (sot, cur)
            }
        };

        let index = usize::from(sot.tile_index);
        // `walk_tiles` refuses a tile record past 65534 here, which the grid
        // of at most 65535 tiles never reaches.
        let tile = seen.get_mut(index).ok_or_else(|| {
            Error::codestream(format!(
                "tile-part names tile {index}, but the grid holds {tile_count} tiles"
            ))
        })?;

        if usize::from(sot.tile_part_index) != usize::from(tile.parts) {
            return Err(Error::codestream(format!(
                "tile {index} tile-part index is {}, but {} of its tile-parts have been read",
                sot.tile_part_index, tile.parts,
            )));
        }
        if sot.num_tile_parts != 0 {
            let stated = u16::from(sot.num_tile_parts) + count_correction;
            if strict && u16::from(sot.tile_part_index) >= stated {
                return Err(Error::codestream(format!(
                    "tile {index} tile-part {} of a declared {stated} tile-parts",
                    sot.tile_part_index,
                )));
            }
            match tile.declared {
                0 => tile.declared = stated,
                declared if declared != stated => {
                    return Err(Error::codestream(format!(
                        "tile {index} declares {stated} tile-parts in one `SOT` and {declared} in another",
                    )));
                }
                _ => {}
            }
        }

        let first_part = sot.tile_part_index == 0;
        let cut = !strict
            && (sot.psot == 0
                || part_offset
                    .checked_add(sot.psot as usize)
                    .is_none_or(|end| end > bytes.len()));
        let header_at = cur.pos;
        // A header of `SOD` alone, as most later tile-parts have, reads as
        // nothing: no marker of its own, no `POC` volume and no packed
        // header. It is stepped over rather than read.
        let read = if read_u16(bytes, header_at) == Some(marker::SOD) {
            cur.pos += 2;
            Ok(None)
        } else {
            read_tile_part_header(&mut cur, main, first_part).map(Some)
        };
        let read = match read {
            Ok(read) => read,
            Err(_) if cut => {
                ended_early = true;
                break;
            }
            Err(error) => return Err(error),
        };
        let mut packed_headers = false;
        if let Some((tile_header, packed)) = read {
            packed_headers = !packed.is_empty();
            if first_part {
                let own = &tile_header.overrides;
                if own.codes_or_quantizes() {
                    tile.flags |= CODES;
                    if !tile_overrides_are_valid(main, &summary, own) {
                        tile.flags |= INVALID;
                    }
                    firsts.set(index, tile_count, header_at);
                    if cur.pos - header_at > KEPT_OVER {
                        tile.flags |= LONG;
                    }
                }
                if !tile_header.poc.is_empty() {
                    tile.flags |= POC_FIRST;
                }
                // `decode_poc` holds a header to `MAX_POC_VOLUMES`.
                tile.poc = tile_header.poc.len() as u8;
                if keep == Some(index) {
                    kept = Some(tile_header);
                }
            } else {
                if tile_header.overrides.codes_or_quantizes() {
                    // The first tile-part's header read again: it read the
                    // same when the walk passed it. What `check_restatement`
                    // keeps between tile-parts only saves it comparing
                    // again, so starting it afresh gives the same result.
                    let first_again = || -> Result<Overrides> {
                        #[cfg(test)]
                        super::tests::FIRST_REREADS.with(|reads| reads.set(reads.get() + 1));
                        let mut at = Cursor::at(bytes, firsts.get(index));
                        Ok(read_tile_part_header(&mut at, main, true)?.0.overrides)
                    };
                    if tile.flags & CODES == 0 {
                        let own = Overrides::default();
                        check_restatement(main, &uniform, &own, &mut None, &tile_header, index)?;
                    } else if tile.flags & LONG == 0 {
                        let own = first_again()?;
                        check_restatement(main, &uniform, &own, &mut None, &tile_header, index)?;
                    } else {
                        let (own, restated) = match restating.entry(index) {
                            Entry::Occupied(entry) => entry.into_mut(),
                            Entry::Vacant(entry) => entry.insert((first_again()?, None)),
                        };
                        check_restatement(main, &uniform, own, restated, &tile_header, index)?;
                    }
                }
                let volumes = usize::from(tile.poc) + tile_header.poc.len();
                check_poc_budget(volumes)?;
                tile.poc = volumes as u8;
                if let Some(kept) = kept.as_mut().filter(|_| keep == Some(index)) {
                    kept.poc.extend(tile_header.poc);
                }
            }
        } else if first_part && keep == Some(index) {
            kept = Some(TileHeader::default());
        }
        // A tile-part with no `POC` volume adds none to the tile's, which
        // passed the budget, so the budget is not checked again.
        if !ppm.is_empty() {
            if packed_headers {
                return Err(both_packed_headers());
            }
            if ppm_seq >= ppm.len() {
                return Err(Error::codestream(
                    "`PPM` carries fewer packet-header chunks than the codestream has tile-parts",
                ));
            }
            ppm_seq += 1;
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
                        bytes.len() - part_offset
                    )));
                }
            };
            if end < data_start {
                return Err(Error::codestream(format!(
                    "tile {} tile-part {} declares `Psot` {}, shorter than its {}-byte header",
                    sot.tile_index,
                    sot.tile_part_index,
                    sot.psot,
                    data_start - part_offset
                )));
            }
            end
        };
        let tile = &mut seen[index];
        tile.parts += 1;

        if !count_checked && tile.declared == u16::from(sot.tile_part_index) + 1 {
            count_checked = true;
            if sot.psot != 0 && count_is_one_short(bytes, data_end, sot.tile_index) {
                count_correction = 1;
                for tile in &mut seen {
                    if tile.declared != 0 {
                        tile.declared += 1;
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
            Some(marker::SOT) => part_offset = data_end,
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

    if ppm_seq != ppm.len() && !(ended_early && ppm_seq < ppm.len()) {
        return Err(Error::codestream(format!(
            "`PPM` carries {} packet-header chunk(s) but the codestream has {ppm_seq} tile-part(s)",
            ppm.len(),
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
    Ok((seen, kept))
}

/// [`check_tile_headers`](super::check_tile_headers) over the tiles `seen`
/// found: each tile's markers were checked as its first tile-part was read,
/// and its `POC` volumes are counted here. The first tile at fault is walked
/// for again, to gather its header as the tile record holds it, and checked
/// in full, so that the fault is reported in the full check's words. Like
/// that check, it numbers a tile by its place among the tiles found.
fn check_headers(bytes: &[u8], parsed: &ParsedHeader, strict: bool, seen: Vec<Seen>) -> Result<()> {
    let main = &parsed.header;
    let mut seen = Some(seen);
    // The tiles found before `after`, by position, passed.
    let mut after = 0;
    loop {
        let tiles = match seen.take() {
            Some(tiles) => tiles,
            None => walk(bytes, parsed, strict, None)?.0,
        };
        let found = tiles.iter().enumerate().filter(|(_, tile)| tile.parts != 0);
        let flagged = found.enumerate().skip(after).find(|(_, (_, tile))| {
            let poc = usize::from(tile.poc);
            let volumes = if tile.flags & POC_FIRST != 0 {
                poc
            } else {
                main.poc.len() + poc
            };
            tile.flags & INVALID != 0 || volumes > MAX_POC_VOLUMES
        });
        let Some((position, (index, _))) = flagged else {
            return Ok(());
        };
        // The walk for the tile's header holds its own record of each tile,
        // so this one is let go first.
        drop(tiles);
        let header = walk(bytes, parsed, strict, Some(index))?
            .1
            .ok_or_else(|| Error::inconsistent(format!("tile {index} was not found again")))?;
        let full = check_tile_in_full(main, &header, position);
        debug_assert!(
            full.is_err(),
            "tile {index}: the quick check refused a valid tile"
        );
        full?;
        // Not reached while the quick check is exact: the record is walked
        // again to go on past the tile.
        after = position + 1;
    }
}
