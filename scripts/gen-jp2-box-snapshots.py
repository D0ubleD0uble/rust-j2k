#!/usr/bin/env python3
"""Record the ftyp, ihdr, colr and rreq fields of the JP2 fixtures.

A small standalone box walker (standard library only) reads each file's JP2
box layer and writes what it finds into the fixture snapshots, so the tests
check `Jp2Metadata::{file_type, image_header, colour_specifications,
reader_requirements}` against a dump that is independent of the decoder.
Nothing here uses the crate or its output. The walker was cross-checked
by hand on files 5 and 7 against `xxd` of the box bytes and `sha256sum` of
the profile (`opj_dump` prints none of these fields).

What it reads, from the JP2 box layer alone:

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

- `tests/fixtures/jp2_real/metadata.json`: each file's record.
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


def walk(data: bytes) -> dict:
    file_type = None
    image_header = None
    colours = []
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
        elif kind == b"jp2c":
            seen_jp2c = True
        elif kind == b"jp2h" and not seen_jp2c:
            seen_jp2h = True
            for child, body in boxes(payload):
                if child == b"ihdr" and image_header is None:
                    if len(body) != 14:
                        sys.exit("malformed ihdr")
                    image_header = {
                        "compression": body[11],
                        "unknown_colour_space": body[12],
                        "ipr": body[13],
                    }
                elif child == b"colr":
                    colours.append(colr(body))
        elif kind == b"colr" and seen_jp2h and not seen_jp2c:
            colours.append(colr(payload))
    if file_type is None or image_header is None:
        sys.exit("no ftyp or ihdr")
    return {
        "file_type": file_type,
        "image_header": image_header,
        "colour_specifications": colours,
        "reader_requirements": reader,
    }


def dump(path: Path) -> dict:
    return walk(path.read_bytes())


def rewrite(path: Path, edit) -> None:
    text = path.read_text()
    value = json.loads(text)
    if json.dumps(value, indent=2) + "\n" != text:
        sys.exit(f"{path} is not in json.dumps(indent=2) form; refusing to reformat it")
    edit(value)
    path.write_text(json.dumps(value, indent=2) + "\n")


def main() -> None:
    real = FIXTURES / "jp2_real"

    def real_edit(value: dict) -> None:
        for name, record in value["files"].items():
            record.update(dump(real / name))

    rewrite(real / "metadata.json", real_edit)

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
