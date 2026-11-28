//! Where the codestream ends, and the boxes after it.
//!
//! OpenJPEG 2.5.0 does not use the `jp2c` box's length. It decodes the
//! codestream from the start of the box, and when the decode is done it walks
//! the boxes that follow from wherever the decoder stopped: just after `EOC`
//! when every tile-part has a length (`Psot`), the end of the file when the
//! last one runs to the end (`Psot = 0`). On that walk it runs its header box
//! readers again (`opj_jp2_end_decompress`), so a malformed header box after
//! the codestream fails the decode. [`codestream_end`] finds the same place
//! from the tile-part lengths, and [`check`] runs the same readers, in the
//! same state:
//!
//! - `ihdr` is ignored, whatever its length.
//! - `jP  ` and `ftyp` are errors: each must come first or second.
//! - A `jp2h` must hold an `ihdr`, and its children are read like the boxes
//!   below.
//! - A `colr` under 3 bytes is an error. Once a `colr` with `METH` 1 or 2 has
//!   been read (in the header or after the codestream), later ones are
//!   ignored; before that, a `METH` 1 box under 7 bytes or a `METH` 2 box
//!   with no profile is an error. A `colr` read here never changes the
//!   decoded image's color space: OpenJPEG has set it already.
//! - A `bpcc` must hold `ihdr.NC` bytes.
//! - A `pclr` is an error if a palette is still held, and is otherwise
//!   checked like one in the header. OpenJPEG drops a header palette with no
//!   `cmap` when it decodes, and keeps one that was applied or skipped.
//! - A `cmap` needs a held palette with no `cmap` yet, and `4 * NPC` bytes.
//! - A `cdef` is an error if one is still held, and is otherwise checked like
//!   one in the header (its layout only). OpenJPEG drops the header's `cdef`
//!   once it is applied, and keeps it when it skips it (the raw channels).
//!
//! Nothing read here is applied to the image.

use super::boxes::{AfterCodestream, Boxes, Jp2Box};
use super::{BoxFault, Jp2File};
use super::{channels, color, palette};
use crate::codestream::markers::marker::{EOC, SOC, SOD, SOT, has_segment, is_marker};
use crate::{Error, Result};

/// The fewest bytes a tile-part can span: `SOT` and its 10-byte segment,
/// then `SOD`.
const MIN_TILE_PART: u32 = 14;

/// Where a codestream ends, as OpenJPEG's decoder leaves its stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    /// Just after the `EOC` marker at this offset minus 2: every tile-part
    /// has a length, and the last is followed by `EOC`.
    At(usize),
    /// A tile-part with `Psot = 0`, of the tile with this index (`Isot`),
    /// runs to the end of the file.
    OpenLastPart(u16),
    /// The main header or the tile-part chain is cut short: the codestream
    /// parser rejects it, or `read_header` reads only its header. More bytes
    /// can take the walk on.
    Unknown,
    /// The main header or the tile-part chain is broken on bytes the input
    /// holds: a marker or length no codestream has there, or a `Psot` that
    /// steps to a marker that is neither `SOT` nor `EOC`, or to the input's
    /// last byte where that byte is not `0xFF`. The walk reads
    /// those bytes the same whatever follows, so no byte appended to the
    /// input finds the end.
    Broken,
}

/// Where [`codestream_end`] stopped on input that cuts its walk short
/// ([`End::Unknown`]), to go on from on a longer input that starts with the
/// same bytes ([`codestream_end_from`]). Each step reads only bytes before
/// the end of the input it was taken on, so on such an input the walk goes
/// through the same steps to the same place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct EndWalk {
    /// The offset of the next marker to read: `SOC` at 0, then a marker of
    /// the main header, or an `SOT` once [`Self::tile_parts`] is set.
    at: usize,
    /// Whether the walk is past the main header, among the tile-parts.
    tile_parts: bool,
}

impl EndWalk {
    /// Whether the walk, gone on with on `codestream`, which extends the
    /// input it stopped on, still finds no end or fault there
    /// ([`End::Unknown`]). It is left where it stops.
    pub(crate) fn still_cut(&mut self, codestream: &[u8]) -> bool {
        codestream_end_from(codestream, self) == End::Unknown
    }
}

/// Find where the codestream at the start of `bytes` ends by stepping over the
/// main header's marker segments and then the tile-parts by their `Psot`. This
/// reads only marker and segment lengths; the codestream parser checks the
/// rest. Each step moves at least 2 bytes, so the walk is linear in the input.
pub(crate) fn codestream_end(bytes: &[u8]) -> End {
    codestream_end_from(bytes, &mut EndWalk::default())
}

/// [`codestream_end`], going on from `walk`, where a walk of a shorter input
/// that `bytes` extend stopped. Where it gives [`End::Unknown`], `walk` is
/// left where this walk stopped; otherwise it is left anywhere.
pub(crate) fn codestream_end_from(bytes: &[u8], walk: &mut EndWalk) -> End {
    let marker = |at: usize| {
        bytes
            .get(at..at.checked_add(2)?)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
    };
    if walk.at == 0 {
        match marker(0) {
            Some(SOC) => walk.at = 2,
            Some(_) => return End::Broken,
            None => return End::Unknown,
        }
    }
    // The main header is stepped through as the codestream parser walks it:
    // `SOC`, `SOD`, `EOC` and a code that is no marker are refused there on
    // their two bytes, and a marker with no segment (`EPH` and the reserved
    // `0xFF30..=0xFF3F`) is two bytes long.
    while !walk.tile_parts {
        let at = walk.at;
        match marker(at) {
            Some(SOT) => walk.tile_parts = true,
            Some(SOC | SOD | EOC) => return End::Broken,
            Some(code) if !is_marker(code) => return End::Broken,
            Some(code) if !has_segment(code) => walk.at = stepped(at, at + 2, bytes),
            Some(_) => match marker(at + 2) {
                Some(length) if length >= 2 => {
                    walk.at = stepped(at, at + 2 + usize::from(length), bytes);
                }
                Some(_) => return End::Broken,
                None => return End::Unknown,
            },
            None => return End::Unknown,
        }
    }
    loop {
        let at = walk.at;
        let Some(&[p0, p1, p2, p3]) = at.checked_add(6).and_then(|from| bytes.get(from..from + 4))
        else {
            return End::Unknown;
        };
        let psot = u32::from_be_bytes([p0, p1, p2, p3]);
        if psot == 0 {
            let isot = u16::from_be_bytes([bytes[at + 4], bytes[at + 5]]);
            return End::OpenLastPart(isot);
        }
        if psot < MIN_TILE_PART {
            return End::Broken;
        }
        // A step past the address space reaches no byte of any input.
        let Some(end) = usize::try_from(psot)
            .ok()
            .and_then(|psot| at.checked_add(psot))
        else {
            return End::Broken;
        };
        match marker(end) {
            Some(EOC) => return End::At(end + 2),
            Some(SOT) => walk.at = stepped(at, end, bytes),
            Some(_) => return End::Broken,
            // One byte in that is not `0xFF` starts no marker, whatever
            // byte follows it.
            None if bytes.get(end).is_some_and(|&byte| byte != 0xFF) => return End::Broken,
            None => return End::Unknown,
        }
    }
}

/// A step of [`codestream_end`] from `at` to `to`, which tests count, as
/// far as `bytes` go.
fn stepped(at: usize, to: usize, bytes: &[u8]) -> usize {
    #[cfg(test)]
    super::tests::AFTER_BYTES.with(|read| {
        let span = to.min(bytes.len()).saturating_sub(at) as u64;
        read.set(read.get() + span);
    });
    #[cfg(not(test))]
    let _ = (at, bytes);
    to
}

/// What OpenJPEG still holds when it walks the boxes after the codestream,
/// updated as the walk reads them.
#[cfg_attr(test, derive(Debug, Clone, PartialEq))]
struct State {
    /// `ihdr.NC`, which a `bpcc` must match.
    components: usize,
    /// Whether a `colr` with `METH` 1 or 2 has been read.
    has_color: bool,
    /// The held palette's column count (`NPC`), if one is held.
    palette: Option<usize>,
    /// Whether the held palette has its `cmap`.
    mapped: bool,
    /// Whether a `cdef` is held.
    channel_definitions: bool,
}

impl State {
    /// The state after the decode, from the header boxes and whether the
    /// palette and channel definitions were skipped (`raw_channels`).
    fn after_decode(file: &Jp2File<'_>, raw_channels: bool) -> Self {
        let header = &file.header;
        let mapped = !header.cmap().is_empty();
        // The header's boxes have passed their checks, so the first `pclr`
        // parses; a palette with no `cmap` is dropped unless it was skipped.
        let palette = header
            .pclr()
            .first()
            .and_then(|pclr| pclr.get(2))
            .filter(|_| mapped || raw_channels)
            .map(|&npc| usize::from(npc));
        State {
            components: usize::from(header.ihdr.components),
            has_color: header
                .colr()
                .iter()
                .any(|colr| matches!(colr.first(), Some(1 | 2))),
            mapped: palette.is_some() && mapped,
            palette,
            channel_definitions: raw_channels && !header.cdef().is_empty(),
        }
    }

    /// Read one box as OpenJPEG's handler for its type does; other types are
    /// ignored.
    fn read(&mut self, item: &Jp2Box<'_>) -> Result<()> {
        let payload = item.payload;
        match &item.kind {
            b"colr" => {
                if payload.len() < 3 {
                    return Err(fault(format!(
                        "a `colr` box holds {} bytes; it needs at least 3",
                        payload.len()
                    )));
                }
                if !self.has_color {
                    self.has_color = color::is_usable(payload).map_err(context)?;
                }
            }
            b"bpcc" => {
                if payload.len() != self.components {
                    return Err(fault(format!(
                        "a `bpcc` box holds {} bytes; it needs one per component ({})",
                        payload.len(),
                        self.components
                    )));
                }
            }
            b"pclr" => {
                if self.palette.is_some() {
                    return Err(fault("the file has a second `pclr` box"));
                }
                let columns = palette::Palette::parse(payload)
                    .map_err(context)?
                    .columns()
                    .len();
                self.palette = Some(columns);
                self.mapped = false;
            }
            b"cmap" => {
                let Some(columns) = self.palette else {
                    return Err(fault(
                        "the file has a `cmap` box with no `pclr` box before it",
                    ));
                };
                if self.mapped {
                    return Err(fault("the file has a second `cmap` box"));
                }
                palette::parse_cmap(payload, columns).map_err(context)?;
                self.mapped = true;
            }
            b"cdef" => {
                if self.channel_definitions {
                    return Err(fault("the file has a second `cdef` box"));
                }
                channels::check_layout(payload).map_err(context)?;
                self.channel_definitions = true;
            }
            _ => {}
        }
        Ok(())
    }
}

/// A box fault after the codestream.
fn fault(message: impl core::fmt::Display) -> Error {
    Error::file_format(format!("after the codestream, {message}"))
}

/// Mark a box reader's error as found after the codestream.
fn context(error: Error) -> Error {
    match error {
        Error::FileFormat(message) => fault(message),
        other => other,
    }
}

/// Read the boxes after the codestream as OpenJPEG does once it has decoded
/// it, if its walk reaches them ([`Jp2File::after_read`]). `raw_channels` is
/// whether the palette and channel definitions were skipped, which decides
/// what OpenJPEG still holds. The walk's length fault, if any, comes after
/// every box it walked.
///
/// A fault is open ([`BoxFault::open`]) where the input ends inside the box
/// at fault: the walk's fault of a box that runs past the end, or a fault in
/// the last box when it has length 0. The boxes before it are whole and read
/// the same whatever follows, so every other fault is final.
pub(crate) fn check(file: &Jp2File<'_>, raw_channels: bool) -> core::result::Result<(), BoxFault> {
    if !file.after_read {
        return Ok(());
    }
    let mut state = State::after_decode(file, raw_channels);
    let last = file.after_codestream.len().checked_sub(1);
    for (index, item) in file.after_codestream.iter().enumerate() {
        let open = file.after_open && file.after_fault.is_none() && Some(index) == last;
        check_box(&mut state, item).map_err(|error| BoxFault { error, open })?;
    }
    match &file.after_fault {
        Some(error) => Err(BoxFault {
            error: error.clone(),
            open: file.after_open,
        }),
        None => Ok(()),
    }
}

/// The top-level types [`check_box`] reads after the codestream: those it
/// judges on their type, `jp2h`, and the `jp2h` children it reads at the
/// top level ([`State::read`]). It takes every other type, `ihdr` included,
/// whatever the box holds and whatever came before it.
const CHECKED: [&[u8; 4]; 8] = [
    b"jP  ", b"ftyp", b"jp2h", b"colr", b"bpcc", b"pclr", b"cmap", b"cdef",
];

/// Whether no byte appended to the input changes what [`check`] gives on
/// the walk after the codestream `after`, once OpenJPEG reads it:
///
/// - the walk stopped for good on bytes in ([`AfterCodestream::ended`]),
///   a box of length 0 with 2^32 bytes or more from its start among them:
///   the walk ends before it with no fault, and more bytes keep it so;
/// - or it ends on a box of length 0, under 2^32 bytes from its start, of
///   a type [`check_box`] does not read ([`CHECKED`]), such as `xml `. That
///   box runs to the end of the input, so no box follows it whatever is
///   appended. While it stays under 2^32 bytes, the walk takes it and
///   [`check`] passes it; from 2^32 on, the walk ends before it as above.
///   Either way [`check`] reads the same whole boxes before it, in the same
///   state.
///
/// Under 2^32 bytes, a box of length 0 of a type [`check_box`] reads can
/// still change the answer: its contents grow (a `colr`, a `jp2h`), and
/// from 2^32 bytes on it is skipped, which takes away a fault of its type
/// alone (an `ftyp`, a second `pclr`).
pub(crate) fn settled(after: &AfterCodestream<'_>) -> bool {
    let open_last = after.open && after.fault.is_none();
    after.ended
        || (open_last
            && after
                .boxes
                .last()
                .is_some_and(|last| !CHECKED.contains(&&last.kind)))
}

/// Whether, on a walk after the codestream `after` that is not [`settled`],
/// [`check`] gives what it gives now whatever bytes are appended, where it
/// passes now: the walk ends on a `colr` of length 0, under 2^32 bytes from
/// its start. That box runs to the end of the input, so no box follows it.
/// While it stays under 2^32 bytes, [`check`] reads it, and a `colr` it
/// passes it passes with more bytes: it needs 3, and whether it passes
/// depends only on its first bytes, the method and, for `METH` 2, a
/// profile that is not empty. From 2^32 on, the walk ends before it,
/// with no fault.
/// Either way [`check`] reads the same whole boxes before it.
///
/// That a passing `colr` keeps passing depends on [`check`] reading nothing
/// of it but the method, the length of an enumerated color space and
/// whether a profile is empty ([`color::is_usable`]), and not copying the
/// profile. A rule on the profile's size would break it. OpenJPEG 2.5's
/// `opj_jp2_read_colr` casts `icc_len` to `OPJ_INT32`, so it fails a
/// `METH` 2 profile of 2^31 + 3 bytes or more; this decoder does not
/// copy that, and a `colr` of length 0 reaches that size only past 2^31
/// bytes of input, as the walk stops short of it from 2^32 on.
///
/// The other types [`check`] reads can turn at fault as they grow: a
/// `bpcc` holds one byte for each component, and a `jp2h` can take a child
/// at fault.
pub(crate) fn passes_on(after: &AfterCodestream<'_>) -> bool {
    after.open
        && after.fault.is_none()
        && after.boxes.last().is_some_and(|last| &last.kind == b"colr")
}

/// Read one top-level box after the codestream.
fn check_box(state: &mut State, item: &Jp2Box<'_>) -> Result<()> {
    match &item.kind {
        b"jP  " => Err(fault(
            "the file has a signature box, which must be the first box",
        )),
        b"ftyp" => Err(fault(
            "the file has an `ftyp` box, which must be the second box",
        )),
        b"ihdr" => Ok(()),
        b"jp2h" => {
            let mut has_ihdr = false;
            for child in Boxes::children(item.payload, "a `jp2h` box after the codestream") {
                let child = child?;
                if &child.kind == b"ihdr" {
                    has_ihdr = true;
                } else {
                    state.read(&child)?;
                }
            }
            if has_ihdr {
                Ok(())
            } else {
                Err(fault("a `jp2h` box has no `ihdr` box"))
            }
        }
        _ => state.read(item),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jp2::boxes::READ_BY_OPENJPEG;

    /// The states [`check_box`] can meet: nothing held, and a palette with
    /// its `cmap`, a `colr` and a `cdef` all held.
    fn states() -> [State; 2] {
        let empty = State {
            components: 1,
            has_color: false,
            palette: None,
            mapped: false,
            channel_definitions: false,
        };
        let full = State {
            components: 3,
            has_color: true,
            palette: Some(1),
            mapped: true,
            channel_definitions: true,
        };
        [empty, full]
    }

    /// Payloads of each shape a reader tells apart: empty, too short for
    /// any, a `colr` of `METH` 1, a `pclr`, a `cmap`, a `cdef`, and junk.
    const PAYLOADS: [&[u8]; 7] = [
        &[],
        &[1, 0],
        &[1, 0, 0, 0, 0, 0, 17],
        &[0, 1, 1, 7, 0],
        &[0, 0, 1, 0],
        &[0, 1, 0, 0, 0, 0, 0, 1],
        &[0xFF; 24],
    ];

    /// [`check_box`] passes a box of each type outside [`CHECKED`], whatever
    /// it holds and in either state, and leaves the state as it was, so
    /// [`settled`] is right to take a box of length 0 of such a type as
    /// final. Each type in [`CHECKED`] is at fault on some payload in some
    /// state, so none is there for nothing, and [`CHECKED`] is every type
    /// OpenJPEG reads but `ihdr`.
    #[test]
    fn check_box_reads_the_checked_types_alone() {
        let unread = [
            *b"ihdr", *b"xml ", *b"uuid", *b"jp2i", *b"res ", *b"uinf", *b"rreq", *b"free",
            *b"jp2c", *b"res\0",
        ];
        for kind in CHECKED.iter().map(|kind| **kind).chain(unread) {
            let checked = CHECKED.contains(&&kind);
            let mut faults = false;
            for start in states() {
                for payload in PAYLOADS {
                    let mut state = start.clone();
                    let read = check_box(&mut state, &Jp2Box { kind, payload });
                    faults |= read.is_err();
                    if !checked {
                        assert!(read.is_ok(), "{kind:?}, {payload:?}");
                        assert_eq!(state, start, "{kind:?}, {payload:?}");
                    }
                }
            }
            assert_eq!(faults, checked, "{kind:?}");
        }
        let read: Vec<_> = READ_BY_OPENJPEG
            .iter()
            .filter(|kind| **kind != b"ihdr")
            .collect();
        assert_eq!(read, CHECKED.iter().collect::<Vec<_>>());
    }

    /// The `colr` read as it was before it stopped copying the profile:
    /// the original `parse_usable` for a box with nothing held yet. Returns
    /// the fault as text, or whether a `colr` is held after the box.
    fn old_colr(has_color: bool, payload: &[u8]) -> core::result::Result<bool, String> {
        fn old_parse(payload: &[u8]) -> Result<Option<Vec<u8>>> {
            let rest = &payload[3..];
            match payload[0] {
                1 => {
                    let Some(&[_, _, _, _]) = rest.first_chunk::<4>() else {
                        return Err(Error::file_format(format!(
                            "a `colr` box with `METH` 1 holds {} bytes; it needs 7 for the \
                             enumerated color space",
                            payload.len()
                        )));
                    };
                    Ok(Some(Vec::new()))
                }
                2 => {
                    if rest.is_empty() {
                        return Err(Error::file_format(
                            "a `colr` box with `METH` 2 holds no ICC profile",
                        ));
                    }
                    Ok(Some(rest.to_vec()))
                }
                _ => Ok(None),
            }
        }
        if payload.len() < 3 {
            return Err(format!(
                "{:?}",
                fault(format!(
                    "a `colr` box holds {} bytes; it needs at least 3",
                    payload.len()
                ))
            ));
        }
        if has_color {
            return Ok(true);
        }
        old_parse(payload)
            .map(|parsed| parsed.is_some())
            .map_err(|error| format!("{:?}", context(error)))
    }

    /// For every `colr` body of each `METH` and of 0 to 11 bytes, with and
    /// without an earlier `colr` taken, the walk after the codestream
    /// accepts and refuses as the copying read did, with the same fault and
    /// the same state after.
    #[test]
    fn colr_after_the_codestream_reads_as_it_did_with_the_profile_copied() {
        let [empty, _] = states();
        let mut shapes = 0;
        for method in [0u8, 1, 2, 3, 4, 255] {
            for length in 0..12usize {
                for fill in [0u8, 16, 0xFF] {
                    let mut payload = vec![fill; length];
                    if let Some(first) = payload.first_mut() {
                        *first = method;
                    }
                    for has_color in [false, true] {
                        let mut state = State {
                            has_color,
                            ..empty.clone()
                        };
                        let item = Jp2Box {
                            kind: *b"colr",
                            payload: &payload,
                        };
                        let new = state
                            .read(&item)
                            .map(|()| state.has_color)
                            .map_err(|error| format!("{error:?}"));
                        let old = old_colr(has_color, &payload);
                        assert_eq!(new, old, "METH {method}, {payload:?}, held {has_color}");
                        shapes += 1;
                    }
                }
            }
        }
        assert_eq!(shapes, 6 * 12 * 3 * 2);
    }
}
