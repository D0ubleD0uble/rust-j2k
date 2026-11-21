#!/usr/bin/env python3
"""Write tests/fixtures/tier1_work_bound/lower_tile_t1bomb_over_cap.j2k (#572).

Two 1024 x 1024 tiles side by side, one 8-bit component, no wavelet levels.
Tile 0 is the tier-1 bomb of `gen-tier1-work-bound-fixtures.py`
(`t1bomb(1024, 6, 88)`, 64x64 code-blocks, 88 passes of one byte each: 9.2e7
sample-passes, past the default code-block work bound). Tile 1 has
no tile-part `COD` and no data, and the main header gives 1x1 precincts, so it
holds 2^20 of them, over the precinct cap. Tile 0 has a tile-part `COD` of the
maximal precincts, which brings it within the caps.

`read_header` never decodes tile 0, so it refuses tile 1 for its precincts
(`Limit`), while `decode` reaches tile 0 first and is refused for its
code-block work (`Limit`, `LimitKind::CodeBlockWork`).

Usage: scripts/gen-lower-tile-work-fixture.py [--check]
With --check, compare the committed file with what this writes and run
`opj_decompress` on it.
"""

import importlib.util
import pathlib
import struct
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "tests" / "fixtures" / "tier1_work_bound" / "lower_tile_t1bomb_over_cap.j2k"

spec = importlib.util.spec_from_file_location(
    "t1", ROOT / "scripts" / "gen-tier1-work-bound-fixtures.py"
)
t1 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(t1)


def cod(scod: int, precincts: bytes) -> bytes:
    """LRCP, one layer, no MCT, no levels, 64x64 code-blocks, 5/3."""
    return t1.seg(
        0xFF52, bytes([scod, 0]) + struct.pack(">H", 1) + bytes([0, 0, 4, 4, 0, 1]) + precincts
    )


def build() -> bytes:
    one = t1.t1bomb(1024, 6, 88)
    sot = one.index(b"\xff\x90")
    siz = bytearray(one[2:sot])
    qcd_at = siz.index(b"\xff\x5c")
    qcd = bytes(siz[qcd_at:])
    siz = siz[: siz.index(b"\xff\x52")]
    assert siz[:2] == b"\xff\x51"
    # SIZ: Xsiz (offset 6 in the segment after the marker + Lsiz + Rsiz)
    struct.pack_into(">I", siz, 6, 2048)
    # The main COD gives 1x1 precincts: a 1024 x 1024 tile holds 2^20, over
    # the 2^18 cap.
    main = bytes(siz) + cod(1, b"\x00") + qcd
    # Tile 0's tile-part header has a COD of the maximal precincts, which
    # brings it within the caps; its data is the bomb's.
    data = one[sot + 12 + 2 : -2]
    body = cod(0, b"") + b"\xff\x93" + data
    tile0 = struct.pack(">HHHIBB", 0xFF90, 10, 0, 12 + len(body), 0, 1) + body
    # Tile 1: a tile-part with no COD and no data, so it has the main
    # header's 1x1 precincts.
    tile1 = struct.pack(">HHHIBB", 0xFF90, 10, 1, 14, 0, 1) + b"\xff\x93"
    return b"\xff\x4f" + main + tile0 + tile1 + b"\xff\xd9"


def main() -> None:
    data = build()
    if "--check" in sys.argv:
        assert OUT.read_bytes() == data, "committed file differs"
        with tempfile.TemporaryDirectory() as tmp:
            subprocess.run(
                ["opj_decompress", "-i", str(OUT), "-o", f"{tmp}/out.pgm"], check=False
            )
    else:
        OUT.write_bytes(data)
    print(len(data), "bytes")


if __name__ == "__main__":
    main()
