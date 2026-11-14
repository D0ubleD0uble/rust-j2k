#!/usr/bin/env python3
"""Record the box-layer fields of the JP2 fixtures.

A small standalone box walker (standard library only) reads each file's JP2
box layer and writes what it finds into the fixture snapshots, so the tests
check `Jp2Metadata` against a dump that is independent of the decoder.
Nothing here uses the crate or its output. The ftyp, ihdr, colr and rreq
fields were cross-checked by hand on files 5 and 7 against `xxd` of the box
bytes and `sha256sum` of the profile (`opj_dump` prints none of these
fields).

`tests/fixtures/jp2_real/metadata.json` is written from scratch with every
field of the snapshot, the ones below and the four after them. The other
targets keep the fields this script does not own.

What it reads, from the JP2 box layer alone:

- `color`: the first counted `colr` (below). `METH` 1 with `EnumCS` 16, 17
  or 18 is `srgb`, `grayscale` or `sycc`; another `EnumCS` is
  `enumerated:<n>`; `METH` 2 is `icc:<profile bytes>`.
- `channels`: one `<type>/<association>` per channel. With a `cdef` box, its
  entries in box order, which must be channels 0 to `NC`-1 in order: `Typ`
  0, 1, 2, 65535 is `color`, `opacity`, `premultiplied`, `unspecified`, else
  `other(<Typ>)`; `Asoc` 0 is `whole`, 65535 is `none`, else the color
  number. Without one, channel `i` is `color/<i+1>` while `i` is below the
  color count (enumerated sRGB 3, grayscale 1, sYCC 3) and
  `unspecified/none` after (ISO/IEC 15444-1 I.5.3.6). `NC` of `ihdr` must
  equal `Csiz` of the codestream's SIZ.
- `palette_applied`: false.

For `metadata.json` the walker refuses (exits, naming the file) instead of
guessing, wherever its rules could differ from the decoder's: a `pclr` or
`cmap` box; a top-level `bpcc`, `pclr`, `cmap` or `cdef` box; more than one
`cdef`; a `cdef` out of channel order; no `cdef` with a color space other
than enumerated sRGB, grayscale or sYCC; `LBox` 0 inside a superbox; a short
`uuid` box; a `jp2h` without an `ihdr`; a `bpcc` whose length is not `NC`;
a `cdef` that associates a color channel with a color other than its
own number, or any channel with a color above `NC`; an `ihdr` `NC` that
differs from `Csiz`; a `colr` that is not
`METH` 1 with a 4-byte `EnumCS` or `METH` 2. The other targets are not
subject to these checks.

- `resolution`: the first 10-byte `resc` and the first 10-byte `resd` in any
  `res ` box of a header, as `[VRN, VRD, HRN, HRD, VRE, HRE]` (`VRE` and
  `HRE` signed), else `null`.
- `xml`, `uuid`, `ipr`, `uuid_info`: the payload length of every top-level
  `xml ` box; the identifier and the length after it of every top-level
  `uuid` box; the length of every top-level `jp2i` box; and, for every
  top-level `uinf` box that holds exactly one `ulst` (`NU`, then exactly
  `NU` identifiers) and exactly one `url ` (at least 4 bytes), its
  identifiers and the length of the `url ` payload after its 4-byte version
  and flags. All in file order, before or after the codestream. `ipr` and
  `uuid_info` are left out when empty.

- `file_type`: the first top-level `ftyp` box.
- `image_header`: `C`, `UnkC` and `IPR` of the first `ihdr` in the first
  `jp2h` box.
- `color_specifications`: every `colr` box before the codestream, in file
  order: those inside any `jp2h` box, and those misplaced at the top level
  after the first `jp2h`. A `colr` before the first `jp2h` or after the
  first `jp2c` is not counted (docs/correctness.md, "The JP2 box layer").
- `reader_requirements`: the first top-level `rreq`, before or after the
  codestream (ISO/IEC 15444-2 M.11.1); `null` when there is none or it does
  not hold exactly its fields.

Four-byte codes are written as text, with `\\xNN` for a byte outside
printable ASCII.

Targets (all rewritten in place; run after scripts/gen-jp2-fixtures.py,
which rewrites the synthetic snapshots without these fields):

- `tests/fixtures/jp2_real/metadata.json`: every `*.jp2` there, written
  from scratch.
- `tests/fixtures/jp2_*.expected.json`: for a fixture in `FULL_RECORD`, the
  whole `jp2` section, as for `metadata.json` and with its checks; for the
  others, the four box fields in the `jp2` section when the snapshot has
  one, else a `jp2_boxes` section.
- `tests/fixtures/conformance/jp2/boxes.json`: file1..file9 (created).
- `tests/fixtures/conformance/jp2/edge_cases.json` (created): box-layer edge
  cases no committed file has, each made by patching `file5.jp2` with the
  edits `edge_cases` lists. It records each case's edits, which
  tests/jp2_header_fields.rs applies to the same bytes, and the four box
  fields this walker reads from the patched bytes.

Usage: python3 -I scripts/gen-jp2-box-snapshots.py
"""

from __future__ import annotations

import hashlib
import json
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "tests" / "fixtures"


# The synthetic fixtures whose whole `jp2` record this walker writes.
FULL_RECORD = {"jp2_metadata_boxes.jp2"}


def jp2box(kind: bytes, payload: bytes) -> bytes:
    return struct.pack(">I", 8 + len(payload)) + kind + payload


# file5.jp2's top-level boxes: jP (at 0, 12 bytes), ftyp (12, 32), rreq (44,
# 24), jp2h (68, 602), jp2c (670, 585890), then the end of the file (586560).
FILE5_RREQ, FILE5_JP2H, FILE5_JP2C, FILE5_END = 44, 68, 670, 586560
COLR_SRGB = jp2box(b"colr", bytes([1, 0, 0]) + struct.pack(">I", 16))


def file5_second_jp2h(file5: bytes) -> bytes:
    """A second `jp2h`: file5's `ihdr` with `UnkC` 1 and `IPR` 1, which is
    ignored, then an enumerated gray `colr` with `PREC` 2."""
    ihdr = bytearray(file5[84:98])
    ihdr[12:14] = bytes([1, 1])
    colr = jp2box(b"colr", bytes([1, 2, 0]) + struct.pack(">I", 17))
    return jp2box(b"jp2h", jp2box(b"ihdr", bytes(ihdr)) + colr)


def rreq_with_vendor_features() -> bytes:
    """An `rreq` with `ML` 2, one standard feature and two vendor features."""
    payload = bytes([2]) + bytes.fromhex("a000") + bytes.fromhex("c000")
    payload += struct.pack(">HH", 1, 5) + bytes.fromhex("8000")
    payload += struct.pack(">H", 2)
    payload += bytes(range(16)) + bytes.fromhex("2000")
    payload += bytes(range(0xF0, 0x100)) + bytes.fromhex("1000")
    return jp2box(b"rreq", payload)


def edge_cases(file5: bytes) -> list[tuple[str, str, list[tuple[int, int, bytes]]]]:
    """(name, note, edits) per case. Each edit is (at, remove, insert): remove
    `remove` bytes at offset `at` and insert `insert` there, applied in order
    to the bytes the edits before it left.

    The `colr` before the first `jp2h` goes directly after the `ftyp`, the
    one place OpenJPEG 2.5.0 ignores it too ("JPEG2000 Header box not read
    yet"). OpenJPEG ORs its unknown-box state (0x7fffffff), which includes
    the header-read bit, into its state both for a box it has no handler for
    (such as `rreq`) and for a misplaced header box it ignores. So any later
    top-level header box before `jp2h` is read as if `jp2h` had been: a
    `colr` after file5's `rreq`, or the second of two `colr` boxes after the
    `ftyp`, becomes its color space. This decoder follows the standard and
    ignores every header box at the top level before `jp2h` (#274,
    2026-11-11; OpenJPEG's `JP2_STATE_UNKNOWN` sets every state bit). So the
    decoders differ on those cases, and they are not pinned here."""
    return [
        (
            "rreq_vendor_features",
            "file5's rreq replaced by one with ML 2, one standard feature and two "
            "vendor features",
            [(FILE5_RREQ, 24, rreq_with_vendor_features())],
        ),
        (
            "colr_before_jp2h",
            "an sRGB colr at the top level before the first jp2h, just after the ftyp: "
            "not counted",
            [(FILE5_RREQ, 0, COLR_SRGB)],
        ),
        (
            "colr_after_jp2c",
            "an sRGB colr at the top level after the jp2c: not counted",
            [(FILE5_END, 0, COLR_SRGB)],
        ),
        (
            "colr_between_jp2h_and_jp2c",
            "an sRGB colr at the top level after the jp2h and before the jp2c: counted, "
            "after the jp2h's two",
            [(FILE5_JP2C, 0, COLR_SRGB)],
        ),
        (
            "second_jp2h",
            "a second jp2h before the jp2c: its ihdr (UnkC 1, IPR 1) is ignored and its "
            "colr is counted after the first jp2h's two",
            [(FILE5_JP2C, 0, file5_second_jp2h(file5))],
        ),
        (
            "malformed_rreq_first",
            "a cut-short rreq (ML 1, FUAM a0, then nothing) before file5's well-formed "
            "one: the first rreq counts, so there is none",
            [(FILE5_RREQ, 0, jp2box(b"rreq", bytes.fromhex("01a0")))],
        ),
    ]


def apply_edits(data: bytes, edits: list[tuple[int, int, bytes]]) -> bytes:
    out = bytearray(data)
    for at, remove, insert in edits:
        if at + remove > len(out):
            sys.exit(f"an edit at {at} runs past the end of the file")
        out[at : at + remove] = insert
    return bytes(out)


EDGE_COMMENT = (
    "Box-layer edge cases made by patching ISO/IEC 15444-4 file5.jp2. Each case lists its "
    "edits (remove `remove` bytes at offset `at` and insert the `insert` hex there, in "
    "order) and the ftyp, ihdr, colr and rreq fields that the box walker in "
    "scripts/gen-jp2-box-snapshots.py reads from the patched bytes (not the decoder). "
    "tests/jp2_header_fields.rs applies the same edits; see tests/fixtures/README.md."
)


REAL_COMMENT = (
    "What each decodable file's header says, in the `jp2` snapshot schema of "
    "tests/support/mod.rs (Jp2Snapshot). Every field is written from the file's box layer by "
    "scripts/gen-jp2-box-snapshots.py, not from this decoder. Channels are the cdef entries "
    "in box order (which must be channels 0 to NC-1), or color/n for an enumerated "
    "sRGB, grayscale or sYCC file without one."
)


def boxes(data: bytes, no_zero_length: bool = False):
    """Yield (type, payload) for each box in data.

    `no_zero_length` refuses an `LBox` of 0 (to the end of the data), which is
    only legal for a top-level box; it is set for the children of a superbox.
    """
    at = 0
    end = len(data)
    while at + 8 <= end:
        (lbox,) = struct.unpack(">I", data[at : at + 4])
        kind = data[at + 4 : at + 8]
        header = 8
        if lbox == 1:
            (lbox,) = struct.unpack(">Q", data[at + 8 : at + 16])
            header = 16
        elif lbox == 0:
            if no_zero_length:
                sys.exit(f"a box at {at} inside a superbox has LBox 0")
            lbox = end - at
        if lbox < header or at + lbox > end:
            sys.exit(f"a box at {at} runs outside its parent")
        yield kind, data[at + header : at + lbox]
        at += lbox


def code(four: bytes) -> str:
    return "".join(chr(b) if 0x20 <= b <= 0x7E else f"\\x{b:02x}" for b in four)


def rreq(payload: bytes):
    """The ISO/IEC 15444-2 M.11.1 layout, or None when it does not fit exactly."""
    at = 0

    def take(n: int) -> bytes | None:
        nonlocal at
        if at + n > len(payload):
            return None
        chunk = payload[at : at + n]
        at += n
        return chunk

    first = take(1)
    if first is None:
        return None
    ml = first[0]
    fuam, dcm, nsf = take(ml), take(ml), take(2)
    if fuam is None or dcm is None or nsf is None:
        return None
    standard = []
    for _ in range(struct.unpack(">H", nsf)[0]):
        sf, sm = take(2), take(ml)
        if sf is None or sm is None:
            return None
        standard.append([struct.unpack(">H", sf)[0], sm.hex()])
    nvf = take(2)
    if nvf is None:
        return None
    vendor = []
    for _ in range(struct.unpack(">H", nvf)[0]):
        vf, vm = take(16), take(ml)
        if vf is None or vm is None:
            return None
        vendor.append([vf.hex(), vm.hex()])
    if at != len(payload):
        return None
    return {
        "mask_length": ml,
        "fully_understand": fuam.hex(),
        "decode_completely": dcm.hex(),
        "standard_features": standard,
        "vendor_features": vendor,
    }


def colr(payload: bytes) -> dict:
    if len(payload) < 3:
        sys.exit("malformed colr")
    method, precedence, approximation = payload[0], payload[1], payload[2]
    data = payload[3:]
    record = {
        "method": method,
        "precedence": precedence,
        "approximation": approximation,
        "payload_len": len(data),
        "payload_sha256": hashlib.sha256(data).hexdigest(),
    }
    if method == 1 and len(data) == 4:
        record["enum_cs"] = struct.unpack(">I", data)[0]
    return record


def color_name(spec: dict) -> str:
    if spec["method"] == 1 and "enum_cs" in spec:
        return {16: "srgb", 17: "grayscale", 18: "sycc"}.get(
            spec["enum_cs"], f"enumerated:{spec['enum_cs']}"
        )
    if spec["method"] == 2:
        return f"icc:{spec['payload_len']}"
    if spec["method"] == 1:
        sys.exit(f"colr METH 1 with {spec['payload_len']} bytes of data instead of a 4-byte EnumCS")
    sys.exit(f"colr METH {spec['method']} is not recorded")


def channel_label(typ: int, asoc: int) -> str:
    kind = {0: "color", 1: "opacity", 2: "premultiplied", 0xFFFF: "unspecified"}.get(
        typ, f"other({typ})"
    )
    assoc = "whole" if asoc == 0 else "none" if asoc == 0xFFFF else str(asoc)
    return f"{kind}/{assoc}"


def channels(cdef: bytes | None, spec: dict, nc: int) -> list:
    if cdef is not None:
        if len(cdef) < 2 or len(cdef) != 2 + 6 * struct.unpack(">H", cdef[:2])[0]:
            sys.exit("malformed cdef")
        entries = [struct.unpack(">HHH", cdef[i : i + 6]) for i in range(2, len(cdef), 6)]
        if [e[0] for e in entries] != list(range(nc)):
            sys.exit("cdef entries are not channels 0 to NC-1 in order")
        for cn, typ, asoc in entries:
            if asoc not in (0, 0xFFFF) and asoc > nc:
                sys.exit(f"cdef channel {cn} is associated with color {asoc} in a {nc}-channel file")
            if typ == 0 and asoc not in (0, 0xFFFF) and asoc != cn + 1:
                sys.exit(f"cdef color channel {cn} is associated with color {asoc}, not {cn + 1}")
        return [channel_label(t, a) for _, t, a in entries]
    colors = {16: 3, 17: 1, 18: 3}.get(spec.get("enum_cs")) if spec["method"] == 1 else None
    if colors is None:
        sys.exit("no cdef, and the color space is not enumerated sRGB, grayscale or sYCC")
    return [f"color/{i + 1}" if i < colors else "unspecified/none" for i in range(nc)]


def res(payload: bytes):
    """VRN, VRD, HRN, HRD (u16) then VRE, HRE (i8), or None unless 10 bytes."""
    if len(payload) != 10:
        return None
    return list(struct.unpack(">HHHHbb", payload))


def uinf(payload: bytes):
    ulst = url = None
    for kind, body in boxes(payload, no_zero_length=True):
        if kind == b"ulst":
            if ulst is not None:
                return None
            ulst = body
        elif kind == b"url ":
            if url is not None:
                return None
            url = body
    if ulst is None or url is None or len(ulst) < 2 or len(url) < 4:
        return None
    if len(ulst) != 2 + 16 * struct.unpack(">H", ulst[:2])[0]:
        return None
    return [[ulst[i : i + 16].hex() for i in range(2, len(ulst), 16)], len(url) - 4]


def walk(data: bytes, strict: bool = False) -> tuple[dict, dict]:
    """The ftyp, ihdr, colr and rreq fields, and what `snapshot` adds to them.

    Without `strict` only the four box fields are gathered, with the checks
    they always had. `strict` (used by `snapshot`) also reads the other fields
    and refuses a file in any case where this walker's rules could differ from
    the decoder's.
    """
    file_type = None
    image_header = None
    nc = csiz = None
    cdef = None
    capture = display = None
    xml, uuid, ipr, uuid_info = [], [], [], []
    colors = []
    palette = False
    bpcc = []
    reader = None
    seen_rreq = False
    seen_jp2h = False
    seen_jp2c = False
    for kind, payload in boxes(data):
        if kind == b"ftyp" and file_type is None:
            if len(payload) < 8 or len(payload) % 4:
                sys.exit("malformed ftyp")
            file_type = {
                "brand": code(payload[0:4]),
                "minor_version": struct.unpack(">I", payload[4:8])[0],
                "compatibility": [code(payload[i : i + 4]) for i in range(8, len(payload), 4)],
            }
        elif kind == b"rreq" and not seen_rreq:
            seen_rreq = True
            reader = rreq(payload)
        elif strict and kind == b"xml ":
            xml.append(len(payload))
        elif strict and kind == b"uuid":
            if len(payload) < 16:
                sys.exit("malformed uuid")
            uuid.append([payload[:16].hex(), len(payload) - 16])
        elif strict and kind == b"jp2i":
            ipr.append(len(payload))
        elif strict and kind == b"uinf":
            info = uinf(payload)
            if info is not None:
                uuid_info.append(info)
        elif strict and kind in (b"bpcc", b"pclr", b"cmap", b"cdef"):
            sys.exit(f"a top-level {code(kind)} box (a misplaced header box)")
        elif kind == b"jp2c":
            if not seen_jp2c and payload[:4] == b"\xff\x4f\xff\x51" and len(payload) >= 42:
                # SOC, SIZ marker, Lsiz, Rsiz, eight 4-byte fields, then Csiz.
                (csiz,) = struct.unpack(">H", payload[40:42])
            seen_jp2c = True
        elif kind == b"jp2h" and not seen_jp2c:
            seen_jp2h = True
            if strict and not any(k == b"ihdr" for k, _ in boxes(payload, no_zero_length=True)):
                sys.exit("a jp2h box without an ihdr box")
            for child, body in boxes(payload, no_zero_length=strict):
                if child == b"ihdr" and image_header is None:
                    if len(body) != 14:
                        sys.exit("malformed ihdr")
                    (nc,) = struct.unpack(">H", body[8:10])
                    image_header = {
                        "compression": body[11],
                        "unknown_color_space": body[12],
                        "ipr_flag": body[13],
                    }
                elif child == b"colr":
                    colors.append(colr(body))
                elif strict and child == b"cdef":
                    if cdef is not None:
                        sys.exit("more than one cdef box")
                    cdef = body
                elif strict and child == b"bpcc":
                    bpcc.append(len(body))
                elif strict and child in (b"pclr", b"cmap"):
                    palette = True
                elif strict and child == b"res ":
                    for grand, rbody in boxes(body, no_zero_length=True):
                        if grand == b"resc" and capture is None:
                            capture = res(rbody)
                        elif grand == b"resd" and display is None:
                            display = res(rbody)
        elif kind == b"colr" and seen_jp2h and not seen_jp2c:
            colors.append(colr(payload))
    if file_type is None or image_header is None:
        sys.exit("no ftyp or ihdr")
    boxes_found = {
        "file_type": file_type,
        "image_header": image_header,
        "color_specifications": colors,
        "reader_requirements": reader,
    }
    extra = {
        "nc": nc,
        "csiz": csiz,
        "cdef": cdef,
        "palette": palette,
        "bpcc": bpcc,
        "capture": capture,
        "display": display,
        "xml": xml,
        "uuid": uuid,
        "ipr": ipr,
        "uuid_info": uuid_info,
    }
    return boxes_found, extra


def snapshot(data: bytes) -> dict:
    """The whole `jp2` snapshot record, in the field order of `Jp2Snapshot`."""
    found, extra = walk(data, strict=True)
    colors = found["color_specifications"]
    if not colors:
        sys.exit("no colr before the codestream")
    if extra["palette"]:
        sys.exit("a palette box is not supported")
    if extra["csiz"] is None:
        sys.exit("no codestream, or one that does not start with SOC and SIZ")
    if any(n != extra["nc"] for n in extra["bpcc"]):
        sys.exit(f"a bpcc box whose length is not NC ({extra['nc']})")
    if extra["nc"] != extra["csiz"]:
        sys.exit(f"ihdr NC {extra['nc']} differs from SIZ Csiz {extra['csiz']}")
    color = color_name(colors[0])
    record = {
        "color": color,
        "channels": channels(extra["cdef"], colors[0], extra["nc"]),
        "palette_applied": False,
        "resolution": {"capture": extra["capture"], "display": extra["display"]},
        "xml": extra["xml"],
        "uuid": extra["uuid"],
    }
    if extra["ipr"]:
        record["ipr"] = extra["ipr"]
    if extra["uuid_info"]:
        record["uuid_info"] = extra["uuid_info"]
    return {**record, **found}


def dump(path: Path) -> dict:
    return walk(path.read_bytes())[0]


def record(path: Path) -> dict:
    """`snapshot` of a file, with the file's name in any refusal."""
    try:
        return snapshot(path.read_bytes())
    except SystemExit as refusal:
        sys.exit(f"{path.name}: {refusal}")


def rewrite(path: Path, edit) -> None:
    text = path.read_text()
    value = json.loads(text)
    if json.dumps(value, indent=2) + "\n" != text:
        sys.exit(f"{path} is not in json.dumps(indent=2) form; refusing to reformat it")
    edit(value)
    path.write_text(json.dumps(value, indent=2) + "\n")


def main() -> None:
    for jp2 in sorted(FIXTURES.glob("jp2_*.jp2")):
        full = jp2.name in FULL_RECORD
        found = record(jp2) if full else dump(jp2)

        def edit(value: dict, found=found, full=full) -> None:
            if full:
                value["jp2"] = found
            elif "jp2" in value:
                value["jp2"].update(found)
            else:
                value["jp2_boxes"] = found

        rewrite(jp2.with_suffix(".expected.json"), edit)

    conformance = FIXTURES / "conformance" / "jp2"
    files = {f"file{n}.jp2": dump(conformance / "files" / f"file{n}.jp2") for n in range(1, 10)}
    comment = (
        "The ftyp, ihdr, colr and rreq fields of ISO/IEC 15444-4 files 1 to 9, from the "
        "box walker in scripts/gen-jp2-box-snapshots.py (not from the decoder); "
        "see tests/fixtures/README.md."
    )
    (conformance / "boxes.json").write_text(
        json.dumps({"_comment": comment, "files": files}, indent=2) + "\n"
    )

    # After the targets above, so a refusal here does not stop them. The
    # record is built whole before the file is touched.
    real = FIXTURES / "jp2_real"
    metadata = {
        "_comment": REAL_COMMENT,
        "files": {f.name: record(f) for f in sorted(real.glob("*.jp2"))},
    }
    (real / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")

    # Last, for the same reason: file5's guard and a refused case exit.
    file5 = (conformance / "files" / "file5.jp2").read_bytes()
    # The box at every offset the edits use, and the 22-byte `ihdr` box at 76
    # whose payload (84..98) file5_second_jp2h copies.
    tags = {FILE5_RREQ: b"rreq", FILE5_JP2H: b"jp2h", 76: b"ihdr", FILE5_JP2C: b"jp2c"}
    if (
        len(file5) != FILE5_END
        or any(file5[at + 4 : at + 8] != tag for at, tag in tags.items())
        or file5[76:80] != struct.pack(">I", 22)
    ):
        sys.exit("file5.jp2 is not the file the edge cases were written for")
    cases = {}
    for name, note, edits in edge_cases(file5):
        try:
            found = walk(apply_edits(file5, edits))[0]
        except SystemExit as refusal:
            sys.exit(f"edge case {name}: {refusal}")
        cases[name] = {
            "note": note,
            "edits": [{"at": at, "remove": n, "insert": data.hex()} for at, n, data in edits],
            "boxes": found,
        }
    (conformance / "edge_cases.json").write_text(
        json.dumps({"_comment": EDGE_COMMENT, "cases": cases}, indent=2) + "\n"
    )


if __name__ == "__main__":
    main()
