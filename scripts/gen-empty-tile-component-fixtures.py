#!/usr/bin/env python3
"""Generate the fixtures in tests/fixtures/empty_tile_component/.

Each file has a tile narrower or shorter than a component's sub-sampling, so
the tile holds no sample of that component (ISO B.3, Eq. B-12 gives
`tcx0 == tcx1` or `tcy0 == tcy1`). SIZ allows it, and OpenJPEG 2.5.0 decodes
such files: the tile-component has no precinct, so no packet.

Two kinds of file:

* Encodes: `opj_compress` 2.5.0 of raw sources (one byte per sample from a
  fixed LCG, `state * 1103515245 + 12345` mod 2^31, taking
  `(state >> 16) & 0xFF`, seed 595; each component at its sub-sampled size,
  one after another). The script checks that `opj_decompress` gives the
  source back, so each oracle is also the encoder input.
* Probes: small images in 1 x 1 tiles, written byte by byte, with every
  packet empty. In `probe_1pkt` (2 x 1, component 1 sub-sampled 2 across)
  tile 1 holds no sample of component 1 and carries one packet, component
  0's. `probe_2pkt` adds a byte after it, which OpenJPEG ignores.
  `probe_ht_1pkt` is `probe_1pkt` with a `CAP` and HT code-blocks,
  `probe_y_1pkt` the same down, and `probe_xy_l2` 3 x 3 with component 1
  sub-sampled 2 both ways, under two wavelet levels.

For each file the oracle `<name>.opj.raw` is the `opj_decompress` output,
the components' samples one after another. For each encode, the oracle
`<name>.tile<N>.opj.raw` is `opj_decompress -t <N> -c 0` of the tile at the
right edge or bottom, which holds no sample of component 1, and for each
encode under two wavelet levels `<name>.r1.opj.raw` is `opj_decompress -r 1`.

Usage: python3 scripts/gen-empty-tile-component-fixtures.py
Requires OpenJPEG 2.5.0 `opj_compress` and `opj_decompress` on PATH.
"""

from __future__ import annotations

import struct
import subprocess
import sys
import tempfile
from pathlib import Path

from oracle_tools import require_openjpeg

DIR = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "empty_tile_component"

# name -> (width, height, sub-sampling per component, tile size, levels).
# OpenJPEG 2.5.0 does not give the source back where the image width is not a
# multiple of `XRsiz` (the last tile row of the component comes back 0), nor
# for a component one sample wide at an odd origin under wavelet levels, and
# `opj_compress` crashes on a 1 x 1 tile-component under two levels; the
# shapes avoid all three. The both-ways case at two levels is `probe_xy_l2`.
SHAPES = {
    # Component 1 is 3 x 20: tiles 0 to 2 of each row hold one column of it,
    # tile 3 none.
    "x_l0": (24, 20, [(1, 1), (8, 1)], (6, 5), 0),
    # Component 1 is 1 x 20: tile 0 of each row holds it, tiles 1 and 2 none.
    "x_l2": (16, 20, [(1, 1), (16, 1)], (6, 5), 2),
    "y_l0": (20, 24, [(1, 1), (1, 8)], (5, 6), 0),
    "y_l2": (20, 16, [(1, 1), (1, 16)], (5, 6), 2),
    # Both ways: component 1 is 3 x 3.
    "xy_l0": (24, 24, [(1, 1), (8, 8)], (6, 6), 0),
}

# The progressions each shape is encoded with: a layer-first order and a
# position-first one. `x_l0` takes all five.
ORDERS = {"x_l0": ["LRCP", "RLCP", "RPCL", "PCRL", "CPRL"]}
DEFAULT_ORDERS = ["LRCP", "RPCL"]

# A POC in every tile of `x_l2` (`opj_compress` numbers tiles from 1):
# resolutions 0 and 1 in CPRL, then resolution 2 in LRCP.
POC = ("x_l2", "/".join(
    f"T{t}=0,0,1,2,2,CPRL/T{t}=2,0,1,3,2,LRCP" for t in range(1, 13)))


def source(width: int, height: int, subs: list[tuple[int, int]]) -> bytes:
    state = 595
    out = bytearray()
    for dx, dy in subs:
        for _ in range(-(-width // dx) * -(-height // dy)):
            state = (state * 1103515245 + 12345) & 0x7FFFFFFF
            out.append((state >> 16) & 0xFF)
    return bytes(out)


def run(*argv: str) -> None:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}{result.stderr}")


def opj_decode(j2k: Path, ncomp: int, tmp: Path, *flags: str) -> bytes:
    """`opj_decompress` of `j2k` under `flags`, the components' samples one
    after another."""
    base = tmp / (j2k.stem + ".out")
    for old in tmp.glob(f"{base.name}*"):
        old.unlink()
    run("opj_decompress", "-i", str(j2k), "-o", f"{base}.pgx", *flags)
    planes = b""
    for k in range(ncomp):
        data = Path(f"{base}_{k}.pgx").read_bytes()
        planes += data[data.index(b"\n") + 1 :]
    return planes


def seg(code: int, body: bytes) -> bytes:
    return struct.pack(">HH", code, len(body) + 2) + body


def ceil_div(a: int, b: int) -> int:
    return -(-a // b)


def probe(width: int, height: int, subs: list[tuple[int, int]], levels: int,
          extra: bytes = b"", ht: bool = False) -> bytes:
    """An image of `width` x `height` in 1 x 1 tiles, 8-bit components
    sub-sampled `subs`, LRCP, one layer, `levels` wavelet levels, 64 x 64
    code-blocks, 5/3, the default precincts. Every packet is empty (0x00):
    one for each component and resolution whose rectangle is not empty (ISO
    B-14), which holds one precinct. The last tile has `extra` after its
    packets."""
    rsiz = 0x4000 if ht else 0
    siz = struct.pack(">HIIIIIIIIH", rsiz, width, height, 0, 0, 1, 1, 0, 0, len(subs))
    siz += b"".join(bytes([7, dx, dy]) for dx, dy in subs)
    out = b"\xff\x4f" + seg(0xFF51, siz)
    if ht:
        # CAP: Part 15 only; Ccap15 with no RGN and magnitude bound 74.
        out += seg(0xFF50, struct.pack(">IH", 0x00020000, 0x001F))
    style = 0x40 if ht else 0
    out += seg(0xFF52, bytes([0, 0, 0, 1, 0, levels, 4, 4, style, 1]))
    out += seg(0xFF5C, bytes([0x00, 0x40] + [0x48] * (3 * levels)))
    tiles = width * height
    for tile in range(tiles):
        x, y = tile % width, tile // width
        packets = 0
        for dx, dy in subs:
            # The tile-component (B-12), then each resolution (B-14).
            tx0, tx1 = ceil_div(x, dx), ceil_div(x + 1, dx)
            ty0, ty1 = ceil_div(y, dy), ceil_div(y + 1, dy)
            for r in range(levels + 1):
                d = 1 << (levels - r)
                if ceil_div(tx0, d) < ceil_div(tx1, d) and ceil_div(ty0, d) < ceil_div(ty1, d):
                    packets += 1
        data = b"\x00" * packets + (extra if tile == tiles - 1 else b"")
        out += seg(0xFF90, struct.pack(">HIBB", tile, 12 + 2 + len(data), 0, 1))
        out += b"\xff\x93" + data
    return out + b"\xff\xd9"


# Tile 1 of each 2-tile probe holds no sample of component 1.
PROBES = {
    "probe_1pkt": probe(2, 1, [(1, 1), (2, 1)], 0),
    "probe_2pkt": probe(2, 1, [(1, 1), (2, 1)], 0, extra=b"\x00"),
    "probe_ht_1pkt": probe(2, 1, [(1, 1), (2, 1)], 0, ht=True),
    "probe_y_1pkt": probe(1, 2, [(1, 1), (1, 2)], 0),
    # 3 x 3 tiles; component 1 (2 x 2 sub-sampled) is empty on every tile in
    # column 1 or row 1, and component 0 on some resolutions of most tiles.
    "probe_xy_l2": probe(3, 3, [(1, 1), (2, 2)], 2),
}


def main() -> None:
    require_openjpeg("opj_compress", "opj_decompress")
    DIR.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp_name:
        tmp = Path(tmp_name)
        jobs = []
        for shape, (w, h, subs, (tw, th), levels) in SHAPES.items():
            for order in ORDERS.get(shape, DEFAULT_ORDERS):
                jobs.append((f"{shape}_{order.lower()}", shape, ["-p", order]))
        jobs.append((f"{POC[0]}_poc", POC[0], ["-POC", POC[1]]))
        for name, shape, flags in jobs:
            w, h, subs, (tw, th), levels = SHAPES[shape]
            raw = source(w, h, subs)
            src = tmp / f"{shape}.raw"
            src.write_bytes(raw)
            spec = f"{w},{h},{len(subs)},8,u@" + ":".join(f"{x}x{y}" for x, y in subs)
            j2k = DIR / f"{name}.j2k"
            run("opj_compress", "-i", str(src), "-o", str(j2k), "-F", spec,
                "-t", f"{tw},{th}", "-n", str(levels + 1), *flags)
            oracle = opj_decode(j2k, len(subs), tmp)
            if oracle != raw:
                sys.exit(f"{name}: opj_decompress does not give the source back")
            (DIR / f"{name}.opj.raw").write_bytes(oracle)
            # The last tile of the first row (`x`) or column (`y`): the one
            # that holds no sample of component 1.
            across = -(-w // tw)
            tile = across - 1 if subs[1][0] > 1 else (-(-h // th) - 1) * across
            part = opj_decode(j2k, 1, tmp, "-t", str(tile), "-c", "0")
            (DIR / f"{name}.tile{tile}.opj.raw").write_bytes(part)
            line = f"{name}: {len(raw)} samples, tile {tile}: {len(part)}"
            if levels:
                reduced = opj_decode(j2k, len(subs), tmp, "-r", "1")
                (DIR / f"{name}.r1.opj.raw").write_bytes(reduced)
                line += f", -r 1: {len(reduced)}"
            print(line)
        for name, data in PROBES.items():
            j2k = DIR / f"{name}.j2k"
            j2k.write_bytes(data)
            oracle = opj_decode(j2k, 2, tmp)
            (DIR / f"{name}.opj.raw").write_bytes(oracle)
            print(f"{name}: {list(oracle)}")


if __name__ == "__main__":
    main()
