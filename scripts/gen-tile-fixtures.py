#!/usr/bin/env python3
"""Generate tiled decode fixtures, with their OpenJPEG oracles.

The ISO/IEC 15444-4 corpus tiles four entries this decoder can reach (`p0_03`,
`p0_07`, `p0_10`, `p0_15`), but every one of them tiles on a **power-of-two
grid**: `p0_10`, the only one that decodes without POC, is 256x256 in 128x128
tiles, so its tile-components start at 0 and 32 and every subband origin in the
pyramid is even. That is the easy half of tiling.

The hard half is the **odd tile origin**. A wavelet coefficient is low-pass when
its coordinate on the tile-component grid is even, so a tile whose origin is odd
starts its rows on a *high*-pass sample and every interleave parity in the
inverse DWT flips (OpenJPEG calls this `cas`). Nothing in the corpus produces
one, and a decoder that ignores parity still passes all four corpus entries — it
reconstructs garbage only once a tile lands off an even coordinate.

So these fixtures tile on a **prime** grid. 13x13 tiles over a 40x40 image put
tile-components at 13, 26 and 39: two of the three tile columns and rows start
odd, and 13 stays odd one level up (`ceil(13/2) == 7`), so the parity flips at
more than one resolution. `tiles_odd_origin_lossless` is the fixture that fails
if the parity is assumed rather than computed.

The others cover the structural half:

- `tiles_even_origin_lossless` — a power-of-two grid, the corpus's own shape, as
  the control: it must keep passing whatever the parity handling does.
- `tiles_multipart_lossless` — `-TP R` splits every tile into one tile-part per
  resolution, so a tile's packets arrive in several SOT segments and the decoder
  has to gather them (`Isot`/`TPsot`/`TNsot`, A.4.2) before it can read a packet.
- `tiles_odd_origin_irreversible` — the 9/7 path at odd parity, whose lifting
  steps sweep the opposite parity from the 5/3 path's.

Lossless fixtures round-trip bit-exactly (`tolerance: exact`); a mis-parity'd
interleave does not produce a rounding error, it produces garbage.

Usage: python3 scripts/gen-tile-fixtures.py
Requires `opj_compress` and `opj_decompress` on PATH.
"""

from __future__ import annotations

import json
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

FIXTURES = Path(__file__).resolve().parent.parent / "tests" / "fixtures"

WIDTH, HEIGHT, COMPONENTS = 40, 40, 3


def sample(component: int, x: int, y: int) -> int:
    """Deterministic and non-separable, so a tile placed at the wrong offset — or
    reconstructed on the wrong parity — cannot pass by accident."""
    return (37 * component + 5 * x + 11 * y + ((x * y) % 7) * 13) % 256


def read_pgx(path: Path) -> tuple[int, int, int, bool, list[int]]:
    raw = path.read_bytes()
    end = raw.index(b"\n")
    tokens = raw[:end].split()
    assert tokens[0] == b"PG", f"{path}: not a PGX file"
    rest = tokens[2:]
    if rest[0] in (b"+", b"-"):
        depth = int((rest[0] + rest[1]).decode())
        rest = rest[2:]
    else:
        depth = int(rest[0])
        rest = rest[1:]
    width, height = int(rest[0]), int(rest[1])
    signed = depth < 0
    depth = abs(depth)
    stride = 1 if depth <= 8 else (2 if depth <= 16 else 4)
    fmt = {1: "b" if signed else "B", 2: "h" if signed else "H", 4: "i" if signed else "I"}[stride]
    count = width * height
    values = list(struct.unpack(f">{count}{fmt}", raw[end + 1 : end + 1 + count * stride]))
    return width, height, depth, signed, values


def run(*argv: str) -> None:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}\n{result.stderr}")


def tile_geometry(codestream: Path) -> tuple[int, int, int]:
    """`(XTsiz, YTsiz, tile-part count)` read back out of the codestream, so a
    fixture cannot silently regress to one tile if OpenJPEG ignores `-t`."""
    raw = codestream.read_bytes()
    offset, tile_parts, tiling = 2, 0, None
    while offset < len(raw) - 3:
        marker = struct.unpack(">H", raw[offset : offset + 2])[0]
        if marker == 0xFFD9:  # EOC
            break
        if 0xFF30 <= marker <= 0xFF3F:
            offset += 2
            continue
        length = struct.unpack(">H", raw[offset + 2 : offset + 4])[0]
        if marker == 0xFF51:  # SIZ
            body = raw[offset + 4 : offset + 2 + length]
            # Rsiz(2), Xsiz(4), Ysiz(4), XOsiz(4), YOsiz(4), XTsiz(4), YTsiz(4).
            tiling = struct.unpack(">II", body[18:26])
        if marker == 0xFF90:  # SOT
            tile_parts += 1
            # Psot spans the whole tile-part; step over it to the next marker.
            psot = struct.unpack(">I", raw[offset + 6 : offset + 10])[0]
            offset += psot
            continue
        offset += 2 + length
    if tiling is None:
        sys.exit(f"{codestream}: no SIZ marker")
    return tiling[0], tiling[1], tile_parts


def build(
    name: str,
    tile: tuple[int, int],
    *,
    irreversible: bool = False,
    tile_parts_by: str | None = None,
    note: str,
) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        planes = bytearray()
        for component in range(COMPONENTS):
            for y in range(HEIGHT):
                for x in range(WIDTH):
                    planes.append(sample(component, x, y))
        (tmp / "in.raw").write_bytes(bytes(planes))

        codestream = FIXTURES / f"{name}.j2k"
        flags = [
            "-F", f"{WIDTH},{HEIGHT},{COMPONENTS},8,u",
            "-mct", "0",       # components stay independent: no RCT/ICT
            "-p", "LRCP",
            "-n", "3",         # 2 decomposition levels: the parity flips twice
            "-b", "16,16",
            "-t", f"{tile[0]},{tile[1]}",
        ]
        if tile_parts_by:
            flags += ["-TP", tile_parts_by]
        if irreversible:
            flags += ["-I", "-r", "8"]
        run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(codestream), *flags)

        xt, yt, parts = tile_geometry(codestream)
        if (xt, yt) != tile:
            sys.exit(f"{name}: SIZ tiles {xt}x{yt}, expected {tile[0]}x{tile[1]}")
        tiles_across = -(-WIDTH // xt)
        tiles_down = -(-HEIGHT // yt)
        if tiles_across * tiles_down < 2:
            sys.exit(f"{name}: {tiles_across}x{tiles_down} tiles is not a tiled image")
        if tile_parts_by and parts <= tiles_across * tiles_down:
            sys.exit(f"{name}: {parts} tile-parts for {tiles_across * tiles_down} tiles; -TP did not split any")

        run("opj_decompress", "-i", str(codestream), "-o", str(tmp / "out.pgx"))

        snapshot_components = []
        for component in range(COMPONENTS):
            w, h, depth, signed, values = read_pgx(tmp / f"out_{component}.pgx")
            if irreversible:
                if (w, h) != (WIDTH, HEIGHT):
                    sys.exit(f"{name}: component {component} decoded {w}x{h}")
            else:
                expected = [sample(component, x, y) for y in range(HEIGHT) for x in range(WIDTH)]
                if (w, h) != (WIDTH, HEIGHT) or values != expected:
                    sys.exit(f"{name}: component {component} did not round-trip losslessly")
            snapshot_components.append(
                {
                    "width": w,
                    "height": h,
                    "bit_depth": depth,
                    "signed": signed,
                    "x_sampling": 1,
                    "y_sampling": 1,
                    "samples": values,
                }
            )

        # The lossy path is graded against OpenJPEG's own reconstruction, not
        # against the source image: the 9/7 inverse is the thing under test, so
        # the oracle's output is the reference and the tolerance covers only the
        # f32 lifting's last-bit spread.
        tolerance = (
            {"mode": "absolute", "max_abs_error": 1.0}
            if irreversible
            else {"mode": "exact"}
        )

        snapshot = {
            "image": {"width": WIDTH, "height": HEIGHT},
            "tolerance": tolerance,
            "components": snapshot_components,
            "provenance": {
                "source": f"synthetic; generated by scripts/{Path(__file__).name}",
                "oracle_command": (
                    f"opj_compress -i in.raw -o {name}.j2k " + " ".join(flags)
                    + f" ; opj_decompress -i {name}.j2k -o out.pgx"
                ),
                "notes": f"{note}; {tiles_across}x{tiles_down} tiles in {parts} tile-parts",
            },
        }
        (FIXTURES / f"{name}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
        print(f"wrote {name}.j2k + {name}.expected.json ({tiles_across}x{tiles_down} tiles, {parts} tile-parts)")


def main() -> None:
    for tool in ("opj_compress", "opj_decompress"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH; install OpenJPEG to regenerate the fixtures")

    build(
        "tiles_even_origin_lossless",
        (16, 16),
        note="power-of-two tile grid, every subband origin even (the corpus's own shape)",
    )
    build(
        "tiles_odd_origin_lossless",
        (13, 13),
        note="prime tile grid: tile-components start at 13, 26 and 39, so the "
        "inverse DWT interleaves on odd parity",
    )
    build(
        "tiles_multipart_lossless",
        (16, 16),
        tile_parts_by="R",
        note="one tile-part per resolution per tile, so each tile's packets arrive "
        "across several SOT segments",
    )
    # 11x11, not 13x13: 40 in 13s leaves a 1x1 corner tile, whose low-pass band
    # is empty at an odd origin, and OpenJPEG's 9/7 *encoder* asserts on it
    # (`opj_dwt_encode_1_real: assertion dn + sn > 1 failed`). Its decoder handles
    # the case and so does this one — `tiles_odd_origin_lossless` keeps that
    # corner tile, since the 5/3 encoder emits it happily — but no oracle can be
    # generated for the 9/7 half of it. 11s tile to 0, 11, 22, 33 with a 7-wide
    # remainder: odd origins, no degenerate tile.
    build(
        "tiles_odd_origin_irreversible",
        (11, 11),
        irreversible=True,
        note="the 9/7 path at odd parity, whose lifting sweeps the opposite parity "
        "from the 5/3 path's",
    )


if __name__ == "__main__":
    main()
