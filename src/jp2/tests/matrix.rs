//! The robustness table in `docs/roadmap-phase3.md` ("Fuzz and robustness
//! plan"), one or more inputs per row, each pinned to its outcome: it decodes,
//! or which [`Error`] variant it is. A row that changes variant fails the
//! matrix. The messages are pinned by the tests nearer each check; this test
//! is about the variant. Every input is built in code or is a committed
//! synthetic fixture; the table records what OpenJPEG 2.5.0 does with each
//! shape, measured with `opj_decompress` (and, for the raw-channel rows, a
//! probe linked to `libopenjp2` 2.5.0 with its ignore flag set).

use super::after::{
    ONE_COMPONENT, after, decode_raw, identity_cdef, no_colour_then, one_component,
    psot_zero_codestream, with_jp2c_length,
};
use super::channels::cdef_of as cdef;
use super::palette::{cmap_box, pclr_box};
use super::*;

/// What a decode gives, by variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Decodes,
    Codestream,
    FileFormat,
    Unsupported,
    Limit,
    Other,
}

fn outcome(result: crate::Result<Image>) -> Outcome {
    match result {
        Ok(_) => Outcome::Decodes,
        Err(Error::Codestream(_)) => Outcome::Codestream,
        Err(Error::FileFormat(_)) => Outcome::FileFormat,
        Err(Error::Unsupported(_)) => Outcome::Unsupported,
        Err(Error::Limit(_)) => Outcome::Limit,
        Err(_) => Outcome::Other,
    }
}

/// How a row is decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Default,
    Raw,
}

const SUBSAMPLED_PALETTE: &[u8] =
    include_bytes!("../../../tests/fixtures/jp2_palette_subsampled_small_first.jp2");
const CLAMPED_PALETTE: &[u8] = include_bytes!("../../../tests/fixtures/jp2_palette_clamp.jp2");

/// A three-component file with these `jp2h` children after `ihdr` and `colr`.
fn children(extra: &[Vec<u8>]) -> Vec<u8> {
    let mut all = vec![good_ihdr(), colr()];
    all.extend_from_slice(extra);
    file(&[jp2_ftyp(), jp2h(&all), jp2c()])
}

/// A `cmap` box from `(CMP, MTYP, PCOL)` triples.
fn cmap(entries: &[(u16, u8, u8)]) -> Vec<u8> {
    let payload: Vec<u8> = entries
        .iter()
        .flat_map(|&(cmp, mtyp, pcol)| {
            let [c0, c1] = cmp.to_be_bytes();
            [c0, c1, mtyp, pcol]
        })
        .collect();
    bx(b"cmap", &payload)
}

/// A one-entry `pclr` box over columns with these `Bi` bytes, every entry
/// zero.
fn one_entry_pclr(bi: &[u8]) -> Vec<u8> {
    let mut payload = vec![0, 1, u8::try_from(bi.len()).unwrap()];
    payload.extend_from_slice(bi);
    for &b in bi {
        let bytes = (usize::from(b & 0x7F) + 1).div_ceil(8).min(4);
        payload.extend(std::iter::repeat_n(0, bytes));
    }
    bx(b"pclr", &payload)
}

/// A one-component 1024x1024 codestream header: the single-component fixture
/// with its image and tile grown, so its main header parses. Its tile data
/// is never reached.
fn big_one_component() -> Vec<u8> {
    let mut codestream = ONE_COMPONENT.to_vec();
    for at in [8, 12, 24, 28] {
        codestream[at..at + 4].copy_from_slice(&1024u32.to_be_bytes());
    }
    codestream
}

/// A file whose palette expands a 1024x1024 index plane into 255 channels:
/// 255 * 2^20 samples, over the 2^26 sample budget.
fn palette_over_budget() -> Vec<u8> {
    let columns: Vec<u8> = (0..=254).collect();
    let mapping: Vec<(u16, u8, u8)> = columns.iter().map(|&i| (0, 1, i)).collect();
    file(&[
        jp2_ftyp(),
        jp2h(&[
            ihdr(1024, 1024, 1),
            colr(),
            one_entry_pclr(&[7; 255]),
            cmap(&mapping),
        ]),
        bx(b"jp2c", &big_one_component()),
    ])
}

/// `count` empty boxes of an unknown type.
fn unknown_boxes(count: usize) -> Vec<u8> {
    header_only(8, b"zzzz").repeat(count)
}

/// `depth` `asoc` superboxes nested in one another.
fn nested_asoc(depth: usize) -> Vec<u8> {
    (0..depth).fold(bx(b"xml ", b"<deep/>"), |inner, _| bx(b"asoc", &inner))
}

#[test]
fn every_robustness_table_row_has_its_outcome() {
    use Mode::{Default, Raw};
    use Outcome::*;

    let ihdr_payload = good_ihdr()[8..].to_vec();
    let truncated = &CODESTREAM[..CODESTREAM.len() - 40];
    let rows: Vec<(&str, Vec<u8>, Mode, Outcome)> = vec![
        (
            "neither magic",
            b"not a codestream".to_vec(),
            Default,
            Codestream,
        ),
        (
            "signature box wrong payload",
            {
                let mut bytes = minimal();
                bytes[11] = 0x0B;
                bytes
            },
            Default,
            FileFormat,
        ),
        (
            "signature box wrong length",
            [
                bx(b"jP  ", &[0x0D, 0x0A, 0x87, 0x0A, 0]),
                jp2_ftyp(),
                good_jp2h(),
                jp2c(),
            ]
            .concat(),
            Default,
            FileFormat,
        ),
        (
            "ftyp missing",
            file(&[good_jp2h(), jp2c()]),
            Default,
            FileFormat,
        ),
        (
            "ftyp not second",
            file(&[good_jp2h(), jp2_ftyp(), jp2c()]),
            Default,
            FileFormat,
        ),
        (
            "no jp2 in brand or compatibility list",
            file(&[ftyp(b"jpx ", &[b"jpx "]), good_jp2h(), jp2c()]),
            Default,
            Unsupported,
        ),
        (
            "box length < 8",
            file(&[jp2_ftyp(), header_only(5, b"zzzz"), good_jp2h(), jp2c()]),
            Default,
            FileFormat,
        ),
        (
            "XLBox < 16",
            file(&[jp2_ftyp(), xl_header_only(15, b"zzzz"), good_jp2h(), jp2c()]),
            Default,
            FileFormat,
        ),
        (
            "box runs past its parent",
            file(&[
                jp2_ftyp(),
                bx(b"jp2h", &[good_ihdr(), header_only(100, b"colr")].concat()),
                jp2c(),
            ]),
            Default,
            FileFormat,
        ),
        (
            "box runs past the file",
            [file(&[jp2_ftyp(), good_jp2h()]), header_only(100, b"xml ")].concat(),
            Default,
            FileFormat,
        ),
        (
            "jp2c runs past the end of the file",
            [
                file(&[jp2_ftyp(), good_jp2h()]),
                header_only(u32::try_from(CODESTREAM.len() + 100).unwrap(), b"jp2c"),
                CODESTREAM.to_vec(),
            ]
            .concat(),
            Default,
            Decodes,
        ),
        (
            "jp2c with LBox 2..7",
            with_jp2c_length(3, &[]),
            Default,
            Decodes,
        ),
        (
            "jp2c with XLBox < 16",
            [
                file(&[jp2_ftyp(), good_jp2h()]),
                xl_header_only(8, b"jp2c"),
                CODESTREAM.to_vec(),
            ]
            .concat(),
            Default,
            Decodes,
        ),
        (
            "jp2c with LBox >= 8 but shorter than its codestream",
            with_jp2c_length(108, &[]),
            Default,
            Decodes,
        ),
        (
            "LBox = 0 on a box that is not last",
            file(&[jp2_ftyp(), open_box(b"xml ", b""), good_jp2h(), jp2c()]),
            Default,
            FileFormat,
        ),
        (
            "LBox = 0 on the last box, jp2c",
            file(&[jp2_ftyp(), good_jp2h(), open_box(b"jp2c", CODESTREAM)]),
            Default,
            Decodes,
        ),
        (
            "XLBox >= 2^32 on a box other than jp2c",
            [
                file(&[jp2_ftyp(), good_jp2h()]),
                xl_header_only(1 << 40, b"xml "),
            ]
            .concat(),
            Default,
            FileFormat,
        ),
        (
            "XLBox >= 2^32 on jp2c",
            [
                file(&[jp2_ftyp(), good_jp2h()]),
                xl_header_only(1 << 40, b"jp2c"),
                CODESTREAM.to_vec(),
            ]
            .concat(),
            Default,
            Decodes,
        ),
        (
            "jp2h missing",
            file(&[jp2_ftyp(), jp2c()]),
            Default,
            FileFormat,
        ),
        (
            "second jp2h before jp2c",
            file(&[jp2_ftyp(), good_jp2h(), jp2h(&[good_ihdr()]), jp2c()]),
            Default,
            Decodes,
        ),
        (
            "jp2c before jp2h",
            file(&[jp2_ftyp(), jp2c(), good_jp2h()]),
            Default,
            FileFormat,
        ),
        (
            "ihdr missing",
            file(&[jp2_ftyp(), jp2h(&[colr()]), jp2c()]),
            Default,
            FileFormat,
        ),
        (
            "ihdr not 14 bytes",
            file(&[
                jp2_ftyp(),
                jp2h(&[bx(b"ihdr", &ihdr_payload[..13]), colr()]),
                jp2c(),
            ]),
            Default,
            FileFormat,
        ),
        (
            "ihdr not first in jp2h",
            file(&[jp2_ftyp(), jp2h(&[colr(), good_ihdr()]), jp2c()]),
            Default,
            Decodes,
        ),
        (
            "ihdr W/H != SIZ image area",
            file(&[
                jp2_ftyp(),
                jp2h(&[ihdr(HEIGHT, WIDTH + 1, COMPONENTS), colr()]),
                jp2c(),
            ]),
            Default,
            FileFormat,
        ),
        (
            "ihdr.NC != Csiz",
            file(&[jp2_ftyp(), jp2h(&[ihdr(HEIGHT, WIDTH, 2), colr()]), jp2c()]),
            Default,
            Decodes,
        ),
        (
            "ihdr width of 0",
            file(&[
                jp2_ftyp(),
                jp2h(&[ihdr(HEIGHT, 0, COMPONENTS), colr()]),
                jp2c(),
            ]),
            Default,
            FileFormat,
        ),
        (
            "ihdr NC > 16384",
            file(&[
                jp2_ftyp(),
                jp2h(&[ihdr(HEIGHT, WIDTH, 16385), colr()]),
                jp2c(),
            ]),
            Default,
            FileFormat,
        ),
        (
            "ihdr.BPC or bpcc disagree with SIZ depth",
            children(&[bx(b"bpcc", &[15, 15, 15])]),
            Default,
            Decodes,
        ),
        (
            "second ihdr",
            children(&[bx(b"ihdr", &[0])]),
            Default,
            Decodes,
        ),
        (
            "second colr",
            children(&[bx(b"colr", &[1, 0, 0, 0, 0, 0, 17])]),
            Default,
            Decodes,
        ),
        (
            "colr METH other than 1 or 2",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), bx(b"colr", &[3, 0, 0]), colr()]),
                jp2c(),
            ]),
            Default,
            Decodes,
        ),
        (
            "colr under 3 bytes",
            children(&[bx(b"colr", &[1, 0])]),
            Default,
            FileFormat,
        ),
        (
            "colr METH 1 under 7 bytes",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), bx(b"colr", &[1, 0, 0, 0])]),
                jp2c(),
            ]),
            Default,
            FileFormat,
        ),
        (
            "colr METH 2 with no profile",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), bx(b"colr", &[2, 0, 0])]),
                jp2c(),
            ]),
            Default,
            FileFormat,
        ),
        (
            "colr METH 1 with bytes past EnumCS",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), bx(b"colr", &[1, 0, 0, 0, 0, 0, 16, 9])]),
                jp2c(),
            ]),
            Default,
            Decodes,
        ),
        (
            "bpcc not NC bytes",
            children(&[bx(b"bpcc", &[7, 7])]),
            Default,
            FileFormat,
        ),
        (
            "ihdr.UnkC = 1 with a colr",
            file(&[
                jp2_ftyp(),
                jp2h(&[
                    bx(b"ihdr", &[ihdr_payload[..12].to_vec(), vec![1, 0]].concat()),
                    colr(),
                ]),
                jp2c(),
            ]),
            Default,
            Decodes,
        ),
        (
            "no colr at all",
            file(&[jp2_ftyp(), jp2h(&[good_ihdr()]), jp2c()]),
            Default,
            Decodes,
        ),
        (
            "colr METH 2 with too few bytes for a profile header",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), bx(b"colr", &[2, 0, 0, 1, 2, 3])]),
                jp2c(),
            ]),
            Default,
            Decodes,
        ),
        (
            "pclr NE = 0",
            children(&[bx(b"pclr", &[0, 0, 3, 7, 7, 7]), cmap_box()]),
            Default,
            FileFormat,
        ),
        (
            "pclr claiming 1024 x 255 entries in a 40-byte box",
            children(&[
                bx(b"pclr", &[[4, 0, 255].as_slice(), &[7; 37]].concat()),
                cmap_box(),
            ]),
            Default,
            FileFormat,
        ),
        (
            "cmap without pclr",
            children(&[cmap_box()]),
            Default,
            FileFormat,
        ),
        (
            "cmap before pclr",
            children(&[cmap_box(), pclr_box()]),
            Default,
            FileFormat,
        ),
        (
            "cmap shorter than 4 * NPC bytes",
            children(&[pclr_box(), cmap(&[(0, 1, 0), (0, 1, 1)])]),
            Default,
            FileFormat,
        ),
        (
            "cmap longer than 4 * NPC bytes",
            children(&[pclr_box(), [cmap_box(), vec![0; 4]].concat()]),
            Default,
            FileFormat,
        ),
        (
            "pclr without cmap",
            children(&[pclr_box()]),
            Default,
            Decodes,
        ),
        (
            "cmap CMP >= Csiz",
            children(&[pclr_box(), cmap(&[(0, 1, 0), (0, 1, 1), (3, 1, 2)])]),
            Default,
            FileFormat,
        ),
        (
            "cmap palette column mapped twice",
            children(&[pclr_box(), cmap(&[(0, 1, 0), (0, 1, 0), (0, 1, 2)])]),
            Default,
            FileFormat,
        ),
        (
            "cmap with MTYP 1 && PCOL != i",
            children(&[pclr_box(), cmap(&[(0, 1, 1), (0, 1, 0), (0, 1, 2)])]),
            Default,
            Unsupported,
        ),
        (
            "one component, cmap leaves a column unused",
            one_component(&[pclr_box(), cmap(&[(0, 1, 0), (0, 0, 0), (0, 1, 2)])], &[]),
            Default,
            Decodes,
        ),
        (
            "direct channel i",
            children(&[pclr_box(), cmap(&[(0, 1, 0), (1, 0, 0), (0, 1, 2)])]),
            Default,
            Decodes,
        ),
        (
            "palette column deeper than the i32 container",
            children(&[one_entry_pclr(&[31, 7, 7]), cmap_box()]),
            Default,
            Unsupported,
        ),
        (
            "palette column deeper than the i32 container, raw channels",
            children(&[one_entry_pclr(&[31, 7, 7]), cmap_box()]),
            Raw,
            Decodes,
        ),
        (
            "palette and direct channels of different sub-sampling",
            SUBSAMPLED_PALETTE.to_vec(),
            Default,
            Decodes,
        ),
        (
            "signed palette column",
            children(&[one_entry_pclr(&[0x87, 7, 7]), cmap_box()]),
            Default,
            Decodes,
        ),
        (
            "palette index out of range",
            CLAMPED_PALETTE.to_vec(),
            Default,
            Decodes,
        ),
        (
            "second pclr",
            children(&[pclr_box(), pclr_box(), cmap_box()]),
            Default,
            FileFormat,
        ),
        (
            "second cdef",
            children(&[identity_cdef(), identity_cdef()]),
            Default,
            FileFormat,
        ),
        (
            "cdef N = 0",
            children(&[bx(b"cdef", &[0, 0])]),
            Default,
            FileFormat,
        ),
        (
            "cdef indices at 65535",
            children(&[cdef(&[(65535, 0, 65535), (1, 0, 2), (2, 0, 3)])]),
            Default,
            FileFormat,
        ),
        (
            "cdef indices at 65535, raw channels (layout only)",
            children(&[cdef(&[(65535, 0, 65535), (1, 0, 2), (2, 0, 3)])]),
            Raw,
            Decodes,
        ),
        (
            "cdef incomplete list",
            children(&[cdef(&[(0, 0, 1), (1, 0, 2)])]),
            Default,
            FileFormat,
        ),
        (
            "cdef names a channel twice, every channel named",
            children(&[cdef(&[(0, 0, 1), (1, 0, 2), (2, 0, 3), (0, 1, 0)])]),
            Default,
            Decodes,
        ),
        (
            "cdef colour entry not at Asoc - 1",
            children(&[cdef(&[(0, 0, 3), (1, 0, 2), (2, 0, 1)])]),
            Default,
            Decodes,
        ),
        (
            "cdef Typ not 0/1/2/65535",
            children(&[cdef(&[(0, 0, 1), (1, 0, 2), (2, 7, 0)])]),
            Default,
            Decodes,
        ),
        (
            "unknown box anywhere",
            file(&[
                jp2_ftyp(),
                bx(b"zzzz", b"?"),
                jp2h(&[good_ihdr(), bx(b"zzzz", b"?"), colr()]),
                jp2c(),
                bx(b"zzzz", b"?"),
            ]),
            Default,
            Decodes,
        ),
        (
            "10 000 unknown boxes before jp2h and after jp2c",
            file(&[
                jp2_ftyp(),
                unknown_boxes(10_000),
                good_jp2h(),
                jp2c(),
                unknown_boxes(10_000),
            ]),
            Default,
            Decodes,
        ),
        (
            "res malformed",
            children(&[bx(
                b"res ",
                &[header_only(3, b"resc"), vec![0; 10]].concat(),
            )]),
            Default,
            Decodes,
        ),
        (
            "resc with a zero denominator",
            children(&[bx(b"res ", &bx(b"resc", &[0, 1, 0, 0, 0, 1, 0, 0, 0, 0]))]),
            Default,
            Decodes,
        ),
        (
            "uuid shorter than its identifier",
            file(&[jp2_ftyp(), bx(b"uuid", b"short"), good_jp2h(), jp2c()]),
            Default,
            Decodes,
        ),
        (
            "xml/uuid after jp2c",
            after(&[bx(b"xml ", b"<a/>"), bx(b"uuid", &[0; 20])]),
            Default,
            Decodes,
        ),
        (
            "ihdr W/H wrong and the codestream truncated",
            file(&[
                jp2_ftyp(),
                jp2h(&[ihdr(HEIGHT, WIDTH + 1, COMPONENTS), colr()]),
                bx(b"jp2c", truncated),
            ]),
            Default,
            FileFormat,
        ),
        (
            "header boxes at the top level after jp2h",
            file(&[jp2_ftyp(), good_jp2h(), identity_cdef(), jp2c()]),
            Default,
            Decodes,
        ),
        (
            "the same boxes at the top level before jp2h",
            file(&[jp2_ftyp(), bx(b"cdef", &[0, 0]), good_jp2h(), jp2c()]),
            Default,
            Decodes,
        ),
        (
            "boxes after jp2c: LBox 2..7",
            after(&[header_only(5, b"junk")]),
            Default,
            FileFormat,
        ),
        (
            "boxes after jp2c: XLBox < 16",
            after(&[xl_header_only(15, b"junk")]),
            Default,
            FileFormat,
        ),
        (
            "boxes after jp2c: fewer than 8 bytes, LBox 0, a box past the end",
            after(&[open_box(b"junk", b"?")]),
            Default,
            Decodes,
        ),
        (
            "boxes after jp2c: a known header box past the end of the file",
            after(&[header_only(100, b"cdef")]),
            Default,
            FileFormat,
        ),
        (
            "boxes after jp2c: a second signature",
            after(&[SIGNATURE.to_vec()]),
            Default,
            FileFormat,
        ),
        (
            "boxes after jp2c: an empty jp2h",
            after(&[jp2h(&[])]),
            Default,
            FileFormat,
        ),
        (
            "boxes after jp2c: a short colr",
            after(&[bx(b"colr", &[1])]),
            Default,
            FileFormat,
        ),
        (
            "boxes after jp2c: a wrong-length bpcc",
            after(&[bx(b"bpcc", &[7])]),
            Default,
            FileFormat,
        ),
        (
            "boxes after jp2c: a short METH 1 with no usable colr before it",
            no_colour_then(&[bx(b"colr", &[1, 0, 0])]),
            Default,
            FileFormat,
        ),
        (
            "boxes after jp2c: a cmap without pclr",
            after(&[cmap_box()]),
            Default,
            FileFormat,
        ),
        (
            "boxes after jp2c: a second cdef, raw channels",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), colr(), identity_cdef()]),
                jp2c(),
                identity_cdef(),
            ]),
            Raw,
            FileFormat,
        ),
        (
            "boxes after jp2c: a second cdef after an applied one",
            file(&[
                jp2_ftyp(),
                jp2h(&[good_ihdr(), colr(), identity_cdef()]),
                jp2c(),
                identity_cdef(),
            ]),
            Default,
            Decodes,
        ),
        (
            "boxes after jp2c: anything after a Psot = 0 codestream",
            [
                file(&[
                    jp2_ftyp(),
                    good_jp2h(),
                    bx(b"jp2c", &psot_zero_codestream()),
                ]),
                header_only(3, b"junk"),
            ]
            .concat(),
            Default,
            Decodes,
        ),
        (
            "second jp2c",
            after(&[bx(b"jp2c", b"not a codestream")]),
            Default,
            Decodes,
        ),
        (
            "deeply nested superboxes",
            file(&[jp2_ftyp(), nested_asoc(2_000), good_jp2h(), jp2c()]),
            Default,
            Decodes,
        ),
        (
            "palette expansion over the sample budget",
            palette_over_budget(),
            Default,
            Limit,
        ),
    ];
    let mut wrong = Vec::new();
    for (what, bytes, mode, want) in &rows {
        let got = outcome(match mode {
            Mode::Default => decode(bytes),
            Mode::Raw => decode_raw(bytes),
        });
        if got != *want {
            wrong.push(format!("{what} ({mode:?}): expected {want:?}, got {got:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "rows off the table:\n{}",
        wrong.join("\n")
    );
}
