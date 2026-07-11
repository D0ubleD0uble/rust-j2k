#!/usr/bin/env python3
"""Generate the image/tile-offset decode fixtures and their OpenJPEG oracles.

The reference grid lets the image and the tile grid start at a non-zero origin
(`XOsiz`/`YOsiz`, `XTOsiz`/`YTOsiz`). The ISO/IEC 15444-4 corpus grades this
through `p1_01` (offset 5, 128 — odd in x) and `p1_07` (offset 4, 0), but both
are single-tile. The combination that is *not* graded in class is a non-zero
origin under a **tile grid** with its own non-zero offset: that is `p1_05`'s
shape, and `p1_05` is blocked on code-block styles the decoder does not have yet.
So it is graded here instead.

Two fixtures:

- `offset_odd_lossless` — a single component at an odd image origin (5, 3),
  isolating the inverse DWT's interleave parity (F.3.8). A sample is low-pass
  when its *absolute* reference-grid coordinate is even, so an odd origin flips
  which lifting branch index 0 of every band takes. A decoder that reads the
  parity off the band length rather than its origin reconstructs a shifted,
  wrong image. `p1_01` covers this too, but carries five layers and precincts on
  top; this isolates the origin.
- `offset_tiled_lossless` — an image origin (5, 3) *and* a tile origin (3, 2)
  under a 3×3 tile grid. The tile grid is anchored at `XTOsiz`, not at the image
  origin, and each tile clips to `[XOsiz, …)`, so the first tile row and column
  are partial in a way a canvas-origin grid never is. Every tile-component rect
  the reconstruction places is at an absolute origin of its own.

`opj_compress -r 1` keeps every bit-plane, so the decode is bit-exact and the
snapshot records `tolerance: exact`.

Usage: python3 scripts/gen-offset-fixtures.py
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

WIDTH, HEIGHT = 48, 40

# name -> (components, extra opj_compress flags, expected (XOsiz, YOsiz), note)
VARIANTS: dict[str, tuple[int, list[str], tuple[int, int], str]] = {
    "offset_odd_lossless": (
        1,
        ["-d", "5,3", "-n", "3"],
        (5, 3),
        "single component at an odd image origin; isolates the inverse-DWT parity",
    ),
    "offset_tiled_lossless": (
        1,
        ["-d", "5,3", "-T", "3,2", "-t", "20,20", "-n", "3"],
        (5, 3),
        "image origin (5,3) and tile origin (3,2) under a 3×3 tile grid; the "
        "leading tile row and column are partial",
    ),
}


def sample(component: int, x: int, y: int) -> int:
    """Deterministic and non-separable, so a shifted or mis-parited decode cannot
    pass by accident."""
    return (7 * x + 13 * y + 29 * component + ((x * y) % 5) * 17) % 256


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


def siz_origin(codestream: Path) -> tuple[int, int]:
    """`(XOsiz, YOsiz)` straight out of SIZ, so a fixture cannot silently regress
    to a canvas-origin image if OpenJPEG reinterprets a flag."""
    raw = codestream.read_bytes()
    # SOC(2) SIZ marker(2) Lsiz(2) Rsiz(2) Xsiz(4) Ysiz(4) XOsiz(4) YOsiz(4)
    assert raw[0:2] == b"\xff\x4f" and raw[2:4] == b"\xff\x51", "not a raw codestream"
    xosiz = struct.unpack(">I", raw[16:20])[0]
    yosiz = struct.unpack(">I", raw[20:24])[0]
    return xosiz, yosiz


def build(name: str, components: int, flags: list[str], expected_origin: tuple[int, int], note: str) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        planes = bytearray()
        for component in range(components):
            for y in range(HEIGHT):
                for x in range(WIDTH):
                    planes.append(sample(component, x, y))
        (tmp / "in.raw").write_bytes(bytes(planes))

        codestream = FIXTURES / f"{name}.j2k"
        flags = [
            "-F", f"{WIDTH},{HEIGHT},{components},8,u",
            "-mct", "0",
            "-b", "16,16",
            "-r", "1",
            *flags,
        ]
        run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(codestream), *flags)

        origin = siz_origin(codestream)
        if origin != expected_origin:
            sys.exit(f"{name}: SIZ origin {origin}, expected {expected_origin}")

        run("opj_decompress", "-i", str(codestream), "-o", str(tmp / "out.pgx"))

        snapshot_components = []
        for component in range(components):
            suffix = f"_{component}" if components > 1 else "_0"
            w, h, depth, signed, values = read_pgx(tmp / f"out{suffix}.pgx")
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

        snapshot = {
            "image": {"width": WIDTH, "height": HEIGHT},
            "tolerance": {"mode": "exact"},
            "components": snapshot_components,
            "provenance": {
                "source": f"synthetic; generated by scripts/{Path(__file__).name}",
                "oracle_command": (
                    f"opj_compress -i in.raw -o {name}.j2k " + " ".join(flags)
                    + f" ; opj_decompress -i {name}.j2k -o out.pgx"
                ),
                "notes": f"{note}; image origin (XOsiz, YOsiz) = {origin}",
            },
        }
        (FIXTURES / f"{name}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
        print(f"wrote {name}.j2k + {name}.expected.json (origin {origin})")


def main() -> None:
    for tool in ("opj_compress", "opj_decompress"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH; install OpenJPEG to regenerate the fixtures")
    for name, (components, flags, origin, note) in VARIANTS.items():
        build(name, components, flags, origin, note)


if __name__ == "__main__":
    main()
