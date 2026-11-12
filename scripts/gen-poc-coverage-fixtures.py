#!/usr/bin/env python3
"""Generate the POC coverage fixtures in tests/fixtures/poc_coverage/.

A tile has a packet for every layer, resolution, component and precinct. A
POC marker's volumes that leave some of them out describe a codestream with
packets missing: `opj_compress` writes only the packets its volumes reach,
and `opj_decompress` 2.5.0 decodes the rest as empty, so its image is not
the source. These files are refused with `Codestream`; the covered controls,
the same encodes with a second volume over the rest, decode to the source.

The script writes the two sources (`res_source.raw`, `comp_source.raw`, raw
big-endian samples from a fixed LCG), encodes them with `opj_compress`
2.5.0, and builds `poc_res_tileparts.j2k` from the covered resolution
encode: `opj_compress` writes a tile-part per volume and the whole POC in
the first, and the second volume is moved to the second tile-part. It then
runs each file through `opj_decompress` and checks what the README states:
the covered files and the split decode to the source, the others do not.

Usage: python3 scripts/gen-poc-coverage-fixtures.py
Requires OpenJPEG 2.5.0 `opj_compress` and `opj_decompress` on PATH.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

from oracle_tools import require_openjpeg

DIR = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "poc_coverage"

SOT, POC, EOC = 0xFF90, 0xFF5F, 0xFFD9

# (source, width, height, components, bits)
SOURCES = {
    "res_source.raw": (32, 32, 1, 10),
    "comp_source.raw": (32, 16, 4, 12),
}

RES = ["-F", "32,32,1,10,u@1x1", "-n", "2"]
COMP = ["-F", "32,16,4,12,u@1x1:1x1:1x1:1x1", "-n", "3", "-p", "LRCP", "-r", "10,1", "-M", "33"]

# name -> (source, flags). The first two are the shapes of #260: resolution 1
# left out, and components 0 to 2 left out.
FIXTURES = {
    "poc_res_missing.j2k": ("res_source.raw", RES + ["-POC", "T1=0,0,1,1,1,LRCP"]),
    "poc_res_covered.j2k": (
        "res_source.raw",
        RES + ["-POC", "T1=0,0,1,1,1,LRCP/T1=1,0,1,2,1,LRCP"],
    ),
    "poc_comp_missing.j2k": ("comp_source.raw", COMP + ["-POC", "T1=0,3,20,3,4,CPRL"]),
    "poc_comp_covered.j2k": (
        "comp_source.raw",
        COMP + ["-POC", "T1=0,0,20,3,3,LRCP/T1=0,3,20,3,4,CPRL"],
    ),
}
# `poc_res_covered.j2k` with its second volume moved to the second tile-part.
SPLIT_FROM = "poc_res_covered.j2k"


def source(width: int, height: int, comps: int, bits: int) -> bytes:
    """A gradient plus noise from a fixed LCG, big-endian 16-bit samples."""
    state = 260
    out = bytearray()
    top = (1 << bits) - 1
    for c in range(comps):
        for y in range(height):
            for x in range(width):
                state = (state * 1103515245 + 12345) & 0x7FFFFFFF
                noise = (state >> 16) % 257 - 128
                v = (top * (x + y + 8 * c)) // (width + height + 8 * comps) + noise
                out += max(0, min(top, v)).to_bytes(2, "big")
    return bytes(out)


def u16(data: bytes, at: int) -> int:
    return int.from_bytes(data[at : at + 2], "big")


def split_poc(cs: bytes) -> bytes:
    """Move the second of two POC volumes to the second tile-part.

    `opj_compress` writes a tile-part per POC volume, with the whole POC in
    the first tile-part's header. The packets stay where they are; only the
    second volume moves, to the tile-part whose packets it orders."""
    sot0 = cs.index(SOT.to_bytes(2, "big"))
    at = sot0 + 12
    assert u16(cs, at) == POC, "expected the tile-part POC right after SOT"
    lpoc = u16(cs, at + 2)
    entries = cs[at + 4 : at + 2 + lpoc]
    assert len(entries) == 14, "expected two 7-byte POC entries"
    psot0 = int.from_bytes(cs[sot0 + 6 : sot0 + 10], "big")
    sot1 = sot0 + psot0
    assert u16(cs, sot1) == SOT and cs[sot1 + 10 : sot1 + 12] == bytes([1, 2])
    psot1 = int.from_bytes(cs[sot1 + 6 : sot1 + 10], "big")
    assert sot1 + psot1 == len(cs) - 2 and u16(cs, len(cs) - 2) == EOC

    def part(sot: int, psot: int, entry: bytes) -> bytes:
        poc = POC.to_bytes(2, "big") + (2 + len(entry)).to_bytes(2, "big") + entry
        rest = cs[sot + 12 : sot + psot]
        if rest[:2] == POC.to_bytes(2, "big"):
            rest = rest[2 + u16(rest, 2) :]
        size = 12 + len(poc) + len(rest)
        head = cs[sot : sot + 6] + size.to_bytes(4, "big") + cs[sot + 10 : sot + 12]
        return head + poc + rest

    return cs[:sot0] + part(sot0, psot0, entries[:7]) + part(sot1, psot1, entries[7:]) + cs[-2:]


def run(*argv: str) -> None:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}{result.stderr}")


def opj_matches_source(path: Path, raw: bytes, tmp: Path) -> bool:
    """Whether `opj_decompress` decodes `path` to the raw source."""
    out = tmp / (path.stem + ".out.raw")
    run("opj_decompress", "-i", str(path), "-o", str(out))
    # The raw input is big-endian; `opj_decompress` writes it little-endian.
    out = out.read_bytes()
    return out[1::2] + out[0::2] == raw[0::2] + raw[1::2]


def main() -> None:
    require_openjpeg("opj_compress", "opj_decompress")
    DIR.mkdir(parents=True, exist_ok=True)
    raws = {}
    for name, shape in SOURCES.items():
        raws[name] = source(*shape)
        (DIR / name).write_bytes(raws[name])
    with tempfile.TemporaryDirectory() as tmp_name:
        tmp = Path(tmp_name)
        for name, (src, flags) in FIXTURES.items():
            run("opj_compress", "-i", str(DIR / src), "-o", str(DIR / name), *flags)
        split = split_poc((DIR / SPLIT_FROM).read_bytes())
        (DIR / "poc_res_tileparts.j2k").write_bytes(split)

        for name in [*FIXTURES, "poc_res_tileparts.j2k"]:
            src = FIXTURES.get(name, FIXTURES[SPLIT_FROM])[0]
            matches = opj_matches_source(DIR / name, raws[src], tmp)
            expected = "missing" not in name
            size = (DIR / name).stat().st_size
            print(f"{name}: {size} B, opj_decompress {'matches' if matches else 'differs from'} the source")
            if matches != expected:
                sys.exit(f"{name}: expected opj_decompress to {'match' if expected else 'differ'}")


if __name__ == "__main__":
    main()
