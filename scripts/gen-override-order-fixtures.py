#!/usr/bin/env python3
"""Generate the override-order fixtures (issue #338) and print what OpenJPEG does.

ISO/IEC 15444-1 A.6.1, A.6.2, A.6.4 and A.6.5 (and Figure A.3, which
fixes only SOC and SIZ in the header order) resolve COD/COC and QCD/QCC by marker class: a COC or QCC
applies to its component whatever its position among the other markers of the
same header. OpenJPEG 2.5.0 resolves by position: `opj_j2k_read_cod` and
`opj_j2k_read_qcd` (`j2k.c:2767`, `j2k.c:3089`) copy component 0's parameters
over every component, so a COD or QCD that follows a COC or QCC wipes it out.

Each stream starts from `multicomponent_lossless.j2k` (three reversible 8-bit
components, one tile). A COD and a QCD that are wrong for every component
(selective arithmetic bypass on; shifted guard bits and exponents) are
corrected by one COC and one QCC per component carrying the original values.
The result decodes to the source image only if the COC and QCC win.

| File               | Where the markers are | Order             |
| main_usual.j2k     | main header           | COD QCD COC* QCC* |
| main_reordered.j2k | main header           | COC* QCC* COD QCD |
| tile_usual.j2k     | tile-part header      | COD QCD COC* QCC* |
| tile_reordered.j2k | tile-part header      | COC* QCC* COD QCD |

The tile-part streams keep the source's own COD and QCD in the main header.

Usage: python3 scripts/gen-override-order-fixtures.py
Requires `opj_decompress` 2.5.0 on PATH.
"""

from __future__ import annotations

import struct
import subprocess
import tempfile
from pathlib import Path

from oracle_tools import require_openjpeg

FIXTURES = Path(__file__).resolve().parent.parent / "tests" / "fixtures"
OUT = FIXTURES / "override_order"
SOURCE = FIXTURES / "multicomponent_lossless.j2k"

COD, QCD, COC, QCC, SOT, SOD = 0xFF52, 0xFF5C, 0xFF53, 0xFF5D, 0xFF90, 0xFF93


def seg(marker: int, body: bytes) -> bytes:
    return struct.pack(">HH", marker, len(body) + 2) + body


def main_segments(raw: bytes) -> dict[int, tuple[int, int]]:
    """marker -> (start of marker, end of segment) for the main header."""
    out = {}
    offset = 2
    while True:
        marker = struct.unpack(">H", raw[offset : offset + 2])[0]
        if marker == SOT:
            out[SOT] = (offset, offset)
            return out
        length = struct.unpack(">H", raw[offset + 2 : offset + 4])[0]
        out[marker] = (offset, offset + 2 + length)
        offset += 2 + length


def build() -> dict[str, bytes]:
    raw = SOURCE.read_bytes()
    segs = main_segments(raw)
    cod_s, cod_e = segs[COD]
    qcd_s, qcd_e = segs[QCD]
    cod_body = raw[cod_s + 4 : cod_e]
    qcd_body = raw[qcd_s + 4 : qcd_e]
    assert cod_body[0] == 0 and qcd_body[0] & 0x1F == 0
    csiz = 3

    # Wrong for every component: selective bypass (code-block style 0x01), and shifted
    # QCD guard bits and exponents.
    bad_cod = bytearray(cod_body)
    bad_cod[8] = 0x01
    bad_qcd = bytes([(((qcd_body[0] >> 5) + 2) & 7) << 5]) + bytes(
        ((((b >> 3) + 4) & 0x1F) << 3) for b in qcd_body[1:]
    )
    assert bytes(bad_cod) != cod_body and bad_qcd != qcd_body

    # One COC and one QCC per component, with the original values. SPcoc is
    # SPcod, and Scoc is the Scod precinct bit, 0 here.
    cocs = b"".join(seg(COC, bytes([c, 0]) + cod_body[5:]) for c in range(csiz))
    qccs = b"".join(seg(QCC, bytes([c]) + qcd_body) for c in range(csiz))
    bad = seg(COD, bytes(bad_cod)) + seg(QCD, bad_qcd)
    usual = bad + cocs + qccs
    reordered = cocs + qccs + bad

    # Main header: the wrong COD and QCD replace the source's. COD, QCD are
    # adjacent in the source; everything after QCD (the COM) is kept.
    assert qcd_s == cod_e

    def main(markers: bytes) -> bytes:
        return raw[:cod_s] + markers + raw[qcd_e:]

    # Tile-part header: markers go before SOD, and Psot grows by their length.
    sot_s = segs[SOT][0]
    sod = raw.index(struct.pack(">H", SOD), sot_s)

    def tile(markers: bytes) -> bytes:
        head = bytearray(raw[:sod])
        psot = struct.unpack(">I", head[sot_s + 6 : sot_s + 10])[0] + len(markers)
        head[sot_s + 6 : sot_s + 10] = struct.pack(">I", psot)
        return bytes(head) + markers + raw[sod:]

    return {
        "main_usual.j2k": main(usual),
        "main_reordered.j2k": main(reordered),
        "tile_usual.j2k": tile(usual),
        "tile_reordered.j2k": tile(reordered),
    }


def opj(path: Path, tmp: Path) -> list[bytes] | str:
    for stale in tmp.glob("out*.pgx"):
        stale.unlink()
    run = subprocess.run(
        ["opj_decompress", "-i", str(path), "-o", str(tmp / "out.pgx")],
        capture_output=True,
        text=True,
    )
    if run.returncode != 0:
        return f"fails (exit {run.returncode})"
    comps = []
    for c in range(3):
        raw = (tmp / f"out_{c}.pgx").read_bytes()
        comps.append(raw[raw.index(b"\n") + 1 :])
    return comps


def main_() -> None:
    require_openjpeg("opj_decompress")
    OUT.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        truth = opj(SOURCE, tmp)
        for name, data in build().items():
            (OUT / name).write_bytes(data)
            got = opj(OUT / name, tmp)
            if isinstance(got, str):
                verdict = got
            elif got == truth:
                verdict = "decodes to the source image"
            else:
                differ = sum(a != b for g, t in zip(got, truth) for a, b in zip(g, t))
                verdict = f"decodes to a different image ({differ} of 1152 samples differ)"
            print(f"{name}: OpenJPEG {verdict}")


if __name__ == "__main__":
    main_()
