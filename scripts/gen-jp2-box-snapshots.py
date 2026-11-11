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

- `colour`: the first counted `colr` (below). `METH` 1 with `EnumCS` 16, 17
  or 18 is `srgb`, `greyscale` or `sycc`; another `EnumCS` is
  `enumerated:<n>`; `METH` 2 is `icc:<profile bytes>`.
- `channels`: one `<type>/<association>` per channel. With a `cdef` box, its
  entries placed by channel index `Cn`: `Typ` 0, 1, 2, 65535 is `colour`,
  `opacity`, `premultiplied`, `unspecified`, else `other(<Typ>)`; `Asoc` 0 is
  `whole`, 65535 is `none`, else the colour number. Without one, channel `i`
  is `colour/<i+1>` while `i` is below the colour space's colour count
  (greyscale 1, sRGB and sYCC 3, another space `NC`) and `unspecified/none`
  after (ISO/IEC 15444-1 I.5.3.6). `NC` of `ihdr` must equal `Csiz` of the
  codestream's SIZ.
- `palette_applied`: false. A `pclr` or `cmap` box is refused.
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
- `colour_specifications`: every `colr` box before the codestream, in file
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
- `tests/fixtures/jp2_*.expected.json`: the `jp2` section when the snapshot
  has one, else a `jp2_boxes` section.
- `tests/fixtures/conformance/jp2/boxes.json`: file1..file9 (created).

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


REAL_COMMENT = (
    "What each decodable file's header says, in the `jp2` snapshot schema of "
    "tests/support/mod.rs (Jp2Snapshot). Recorded from a box dump of each file (colr EnumCS, "
    "the resc/resd fields in box order VRN VRD HRN HRD VRE HRE, the top-level xml and uuid "
    "boxes; ftyp, ihdr, colr and rreq from scripts/gen-jp2-box-snapshots.py), not from this "
    "decoder. Channels follow each file's cdef entries, in output order."
)


def boxes(data: bytes):
    """Yield (type, payload) for each box in data."""
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
        "data_len": len(data),
        "data_sha256": hashlib.sha256(data).hexdigest(),
    }
    if method == 1 and len(data) == 4:
        record["enum_cs"] = struct.unpack(">I", data)[0]
    return record


def colour_name(spec: dict) -> str:
    if spec["method"] == 1 and "enum_cs" in spec:
        return {16: "srgb", 17: "greyscale", 18: "sycc"}.get(
            spec["enum_cs"], f"enumerated:{spec['enum_cs']}"
        )
    if spec["method"] == 2:
        return f"icc:{spec['data_len']}"
    sys.exit(f"unsupported colr method {spec['method']}")


def channel_label(typ: int, asoc: int) -> str:
    kind = {0: "colour", 1: "opacity", 2: "premultiplied", 0xFFFF: "unspecified"}.get(
        typ, f"other({typ})"
    )
    assoc = "whole" if asoc == 0 else "none" if asoc == 0xFFFF else str(asoc)
    return f"{kind}/{assoc}"


def channels(cdef: bytes | None, colour: str, nc: int) -> list:
    if cdef is not None:
        if len(cdef) < 2 or len(cdef) != 2 + 6 * struct.unpack(">H", cdef[:2])[0]:
            sys.exit("malformed cdef")
        entries = [struct.unpack(">HHH", cdef[i : i + 6]) for i in range(2, len(cdef), 6)]
        if sorted(e[0] for e in entries) != list(range(nc)):
            sys.exit("cdef does not define each channel once")
        return [channel_label(t, a) for _, t, a in sorted(entries)]
    colours = {"greyscale": 1, "srgb": 3, "sycc": 3}.get(colour, nc)
    return [f"colour/{i + 1}" if i < colours else "unspecified/none" for i in range(nc)]


def res(payload: bytes):
    """VRN, VRD, HRN, HRD (u16) then VRE, HRE (i8), or None unless 10 bytes."""
    if len(payload) != 10:
        return None
    return list(struct.unpack(">HHHHbb", payload))


def uinf(payload: bytes):
    ulst = url = None
    for kind, body in boxes(payload):
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


def walk(data: bytes) -> tuple[dict, dict]:
    """The ftyp, ihdr, colr and rreq fields, and what `snapshot` adds to them."""
    file_type = None
    image_header = None
    nc = csiz = None
    cdef = None
    capture = display = None
    xml, uuid, ipr, uuid_info = [], [], [], []
    colours = []
    palette = False
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
        elif kind == b"xml ":
            xml.append(len(payload))
        elif kind == b"uuid":
            if len(payload) < 16:
                sys.exit("malformed uuid")
            uuid.append([payload[:16].hex(), len(payload) - 16])
        elif kind == b"jp2i":
            ipr.append(len(payload))
        elif kind == b"uinf":
            info = uinf(payload)
            if info is not None:
                uuid_info.append(info)
        elif kind == b"jp2c":
            if not seen_jp2c and payload[:4] == b"\xff\x4f\xff\x51" and len(payload) >= 42:
                # SOC, SIZ marker, Lsiz, Rsiz, eight 4-byte fields, then Csiz.
                (csiz,) = struct.unpack(">H", payload[40:42])
            seen_jp2c = True
        elif kind == b"jp2h" and not seen_jp2c:
            seen_jp2h = True
            for child, body in boxes(payload):
                if child == b"ihdr" and image_header is None:
                    if len(body) != 14:
                        sys.exit("malformed ihdr")
                    (nc,) = struct.unpack(">H", body[8:10])
                    image_header = {
                        "compression": body[11],
                        "unknown_colour_space": body[12],
                        "ipr": body[13],
                    }
                elif child == b"colr":
                    colours.append(colr(body))
                elif child == b"cdef" and cdef is None:
                    cdef = body
                elif child in (b"pclr", b"cmap"):
                    palette = True
                elif child == b"res ":
                    for grand, rbody in boxes(body):
                        if grand == b"resc" and capture is None:
                            capture = res(rbody)
                        elif grand == b"resd" and display is None:
                            display = res(rbody)
        elif kind == b"colr" and seen_jp2h and not seen_jp2c:
            colours.append(colr(payload))
    if file_type is None or image_header is None:
        sys.exit("no ftyp or ihdr")
    boxes_found = {
        "file_type": file_type,
        "image_header": image_header,
        "colour_specifications": colours,
        "reader_requirements": reader,
    }
    extra = {
        "nc": nc,
        "csiz": csiz,
        "cdef": cdef,
        "palette": palette,
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
    found, extra = walk(data)
    colours = found["colour_specifications"]
    if not colours:
        sys.exit("no colr before the codestream")
    if extra["palette"]:
        sys.exit("a palette box is not supported")
    if extra["csiz"] is None:
        sys.exit("no codestream, or one that does not start with SOC and SIZ")
    if extra["nc"] != extra["csiz"]:
        sys.exit(f"ihdr NC {extra['nc']} differs from SIZ Csiz {extra['csiz']}")
    colour = colour_name(colours[0])
    record = {
        "colour": colour,
        "channels": channels(extra["cdef"], colour, extra["nc"]),
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


def rewrite(path: Path, edit) -> None:
    text = path.read_text()
    value = json.loads(text)
    if json.dumps(value, indent=2) + "\n" != text:
        sys.exit(f"{path} is not in json.dumps(indent=2) form; refusing to reformat it")
    edit(value)
    path.write_text(json.dumps(value, indent=2) + "\n")


def main() -> None:
    real = FIXTURES / "jp2_real"
    metadata = {
        "_comment": REAL_COMMENT,
        "files": {f.name: snapshot(f.read_bytes()) for f in sorted(real.glob("*.jp2"))},
    }
    (real / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")

    for jp2 in sorted(FIXTURES.glob("jp2_*.jp2")):
        found = dump(jp2)

        def edit(value: dict, found=found) -> None:
            if "jp2" in value:
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


if __name__ == "__main__":
    main()
