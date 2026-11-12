#!/usr/bin/env python3
"""Generate the POC precedence fixtures in tests/fixtures/poc_precedence/.

Each codestream carries a POC in the main header *and* a different one in the
tile-part headers. A.6.6 gives the precedence `Tile-part POC > Main POC >
Tile-part COD > Main COD`: the tile's POC overrides the main header's, so the
tile's packets are read in the tile-part POC's order alone. The packets were
written in that order, so a decode under the standard equals the source image.

`opj_decompress` 2.5.0 decodes both wrong. `opj_j2k_read_poc` appends a
tile-part's volumes to the ones the tile inherited from the main header, so it
walks the main volume first and reads the first packets in the wrong order. So
the grading reference is the source image, not the OpenJPEG decode. See
docs/correctness.md, "A tile-part POC overrides the main header's".

How each file is made:

1. `opj_compress` encodes the source with a two-volume tile-part POC (CPRL over
   resolution 0, then RLCP over resolutions 1-2). It writes both volumes in the
   first tile-part header and splits the tile into one tile-part per volume.
   `opj_decompress` round-trips this file exactly, which the script checks.
2. A main-header POC is spliced in before the first SOT: one LRCP volume over
   resolution 0, every component and every layer. With three components and
   three layers, LRCP and CPRL read resolution 0's nine packets in different
   orders.
3. `poc_main_and_split_tile.j2k` also moves the second tile volume out of the
   first tile-part header into its own POC in the second tile-part header,
   right before the packets it governs, so the tile's volumes accumulate across
   its tile-parts. Both Psot values are fixed up.

The source is three 24x16 8-bit components, `sample(c, x, y)` below, stored
plane after plane in `source.raw`.

Usage: python3 scripts/gen-poc-precedence-fixtures.py
Requires OpenJPEG 2.5.0 `opj_compress` and `opj_decompress` on PATH.
"""

from __future__ import annotations

import hashlib
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

from oracle_tools import require_openjpeg

DIR = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "poc_precedence"

WIDTH, HEIGHT, COMPONENTS = 24, 16, 3
TILE_POC = "T1=0,0,3,1,3,CPRL/T1=1,0,3,3,3,RLCP"
FLAGS = [
    "-F", f"{WIDTH},{HEIGHT},{COMPONENTS},8,u",
    "-mct", "0",
    "-n", "3",
    "-b", "16,16",
    "-r", "20,10,1",
    "-POC", TILE_POC,
]
# RSpoc, CSpoc, LYEpoc, REpoc, CEpoc, Ppoc: resolution 0, components [0, 3),
# layers [0, 3), LRCP.
MAIN_POC_BODY = bytes([0, 0]) + struct.pack(">H", 3) + bytes([1, 3, 0])

POC, SOT, SOD = 0xFF5F, 0xFF90, 0xFF93
VOLUME = 7  # bytes per POC volume with fewer than 257 components


def sample(component: int, x: int, y: int) -> int:
    """Deterministic and non-separable, so a misordered packet walk cannot pass
    by accident (the same pattern as gen-progression-fixtures.py)."""
    return (37 * component + 5 * x + 11 * y + ((x * y) % 7) * 13) % 256


def source() -> bytes:
    return bytes(
        sample(c, x, y) for c in range(COMPONENTS) for y in range(HEIGHT) for x in range(WIDTH)
    )


def run(*argv: str) -> None:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}\n{result.stderr}")


def tile_parts(cs: bytes) -> list[int]:
    """Offsets of every SOT, walked by Psot."""
    offsets = []
    at = cs.index(struct.pack(">H", SOT))
    while cs[at : at + 2] == struct.pack(">H", SOT):
        offsets.append(at)
        psot = struct.unpack(">I", cs[at + 6 : at + 10])[0]
        assert psot, "Psot = 0 not expected from opj_compress"
        at += psot
    assert cs[at : at + 2] == b"\xff\xd9", "tile-parts do not end at EOC"
    return offsets


def set_psot(cs: bytearray, at: int, delta: int) -> None:
    psot = struct.unpack(">I", cs[at + 6 : at + 10])[0]
    cs[at + 6 : at + 10] = struct.pack(">I", psot + delta)


def split_tile_poc(cs: bytes) -> bytes:
    """Move the second volume of tile-part 0's POC into its own POC at the head
    of tile-part 1's header, and fix both Psot values."""
    tp0, tp1 = tile_parts(cs)
    out = bytearray(cs)
    start = tp0 + 12 + 4 + VOLUME
    second_volume = bytes(out[start : start + VOLUME])
    own_poc = struct.pack(">HH", POC, 2 + VOLUME) + second_volume
    # Tile-part 1 first, so tile-part 0's offsets stay valid.
    out[tp1 + 12 : tp1 + 12] = own_poc
    set_psot(out, tp1, len(own_poc))
    del out[start : start + VOLUME]
    out[tp0 + 14 : tp0 + 16] = struct.pack(">H", 2 + VOLUME)
    set_psot(out, tp0, -VOLUME)
    return bytes(out)


def opj_decode(path: Path, tmp: Path) -> bytes:
    for old in tmp.glob("out_*.pgx"):
        old.unlink()
    run("opj_decompress", "-i", str(path), "-o", str(tmp / "out.pgx"))
    planes = b""
    for c in range(COMPONENTS):
        raw = (tmp / f"out_{c}.pgx").read_bytes()
        planes += raw[raw.index(b"\n") + 1 :]
    return planes


def mismatch(decoded: bytes, src: bytes) -> str:
    plane = WIDTH * HEIGHT
    parts = []
    for c in range(COMPONENTS):
        d, s = decoded[c * plane : (c + 1) * plane], src[c * plane : (c + 1) * plane]
        wrong = [abs(a - b) for a, b in zip(d, s) if a != b]
        parts.append(f"component {c}: {len(wrong)} of {plane} wrong, max error {max(wrong, default=0)}")
    return "; ".join(parts)


def main() -> None:
    require_openjpeg("opj_compress", "opj_decompress")
    DIR.mkdir(parents=True, exist_ok=True)
    src = source()
    (DIR / "source.raw").write_bytes(src)
    print(f"source.raw sha256 {hashlib.sha256(src).hexdigest()}")

    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        (tmp / "in.raw").write_bytes(src)
        tile_only = tmp / "tile_only.j2k"
        run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(tile_only), *FLAGS)
        if opj_decode(tile_only, tmp) != src:
            sys.exit("control: the tile-POC-only encode does not round-trip under opj_decompress")

        cs = tile_only.read_bytes()
        parts = tile_parts(cs)
        if len(parts) != 2:
            sys.exit(f"expected two tile-parts, found {len(parts)}")
        # The first tile-part header must be exactly SOT, POC(2 volumes), SOD.
        first = parts[0] + 12
        poc_len = struct.unpack(">H", cs[first + 2 : first + 4])[0]
        if cs[first : first + 2] != struct.pack(">H", POC) or poc_len != 2 + 2 * VOLUME:
            sys.exit("first tile-part header does not open with a two-volume POC")
        if cs[first + 2 + poc_len : first + 4 + poc_len] != struct.pack(">H", SOD):
            sys.exit("first tile-part header carries more than its POC")
        if cs[parts[1] + 12 : parts[1] + 14] != struct.pack(">H", SOD):
            sys.exit("second tile-part header is not empty")

        main_poc = struct.pack(">HH", POC, 2 + len(MAIN_POC_BODY)) + MAIN_POC_BODY
        both = cs[: parts[0]] + main_poc + cs[parts[0] :]
        fixtures = {"poc_main_and_tile.j2k": both}

        fixtures["poc_main_and_split_tile.j2k"] = split_tile_poc(both)

        # Control: the same split without the main POC round-trips, so the
        # split itself is a valid codestream.
        control = tmp / "split_control.j2k"
        control.write_bytes(split_tile_poc(cs))
        if opj_decode(control, tmp) != src:
            sys.exit("control: the split tile POC alone does not round-trip under opj_decompress")

        for name, data in fixtures.items():
            path = DIR / name
            path.write_bytes(data)
            tile_parts(data)
            decoded = opj_decode(path, tmp)
            if decoded == src:
                sys.exit(f"{name}: opj_decompress reproduces the source, so the fixture shows nothing")
            print(f"wrote {name} ({len(data)} bytes); opj_decompress 2.5.0: {mismatch(decoded, src)}")


if __name__ == "__main__":
    main()
