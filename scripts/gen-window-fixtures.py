#!/usr/bin/env python3
"""Generate the window, tile and component fixtures in tests/fixtures/window/.

`tests/decode_window.rs` decodes these with `DecodeOptions::window`,
`DecodeOptions::tile` and `DecodeOptions::components` set, and compares the
samples with `opj_decompress -d`, `-t` and `-c` 2.5.0 on the same bytes.

The source, `rgb.ppm`, is 70 x 54, 8-bit, three components from a fixed LCG
(`state * 1103515245 + 12345` mod 2^31, taking `state >> 16`, seed 216):
`(x(3 + c) + y(5 - c) + 40c) mod 256` plus noise from -8 to 8, clamped.
`opj_compress` codes it with the image at (5, 4) on the reference grid and
24 x 20 tiles from (3, 2), so the image has 3 x 3 tiles and the ones on its
edges are partial. See CODESTREAMS for the three codestreams.

For each codestream it writes:

- `<name>.sha256`: one line per case of CASES, `<case> <width>x<height>
  <hash>`, where the hash is the SHA-256 of the `opj_decompress` samples,
  every component in order, each a little-endian `i32`. The case is the
  `opj_decompress` arguments with the spaces taken out.
- `<name>_r<R>_<c>.pgx`: component c of the whole decode at `-r R`, for R
  from 0 to 2, for the 9/7 codestream only, whose samples the test grades
  within a tolerance against the crop of these.

It checks that every `-d` and `-t` output is the matching crop of the whole
decode, so the window is graded against the crop as well. For the
codestreams under the color transform it checks that OpenJPEG's `-c` output
differs from those components of the whole decode (OpenJPEG skips the color
transform under `-c`), and records the whole decode's components for those
cases instead: see docs/openjpeg-departures.md.

`python3 scripts/gen-window-fixtures.py --bench` writes the benchmark's
`tests/fixtures/bench/tiled_2048_gray.j2k` instead (see BENCH).

Requires OpenJPEG 2.5.0 `opj_compress` and `opj_decompress` on PATH.
"""

from __future__ import annotations

import hashlib
import math
import re
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

from oracle_tools import require_openjpeg

FIXTURES_ROOT = Path(__file__).resolve().parent.parent / "tests" / "fixtures"
DIR = FIXTURES_ROOT / "window"
BENCH = FIXTURES_ROOT / "bench" / "tiled_2048_gray.j2k"

WIDTH, HEIGHT = 70, 54
ORIGIN = (5, 4)
GRID = ["-t", "24,20", "-T", "3,2", "-d", f"{ORIGIN[0]},{ORIGIN[1]}", "-n", "3"]

# name -> (opj_compress arguments, color transform)
CODESTREAMS = {
    # 5/3 with the reversible color transform.
    "tiled_53": (GRID, True),
    # 9/7 with the irreversible color transform.
    "tiled_97": (GRID + ["-I"], True),
    # 5/3 with no color transform.
    "tiled_53_nomct": (GRID + ["-mct", "0"], False),
}

# Each case is a list of `opj_decompress` arguments.
CASES = [
    # Off the tile grid, across four tiles.
    ["-d", "20,10,41,31"],
    ["-d", "20,10,41,31", "-r", "1"],
    # To the image's right and bottom edges.
    ["-d", "50,40,75,58"],
    # From before the image's origin, which OpenJPEG clips to it.
    ["-d", "0,0,30,25"],
    ["-d", "0,0,30,25", "-r", "2"],
    # Past the far edges, clipped.
    ["-d", "60,45,200,200"],
    # One tile's rectangle exactly.
    ["-d", "27,22,51,42"],
    # One sample.
    ["-d", "33,17,34,18"],
    # Odd bounds at a reduction.
    ["-d", "13,9,46,37", "-r", "1"],
    # Tiles: the top-left and bottom-right are partial.
    ["-t", "0"],
    ["-t", "4"],
    ["-t", "8"],
    ["-t", "4", "-r", "1"],
    # Components.
    ["-c", "1"],
    ["-c", "2,0"],
    ["-c", "0,1,2"],
    ["-c", "1", "-d", "20,10,41,31"],
    ["-c", "2", "-t", "5", "-r", "1"],
]


def lcg(seed: int):
    state = seed
    while True:
        state = (state * 1103515245 + 12345) & 0x7FFFFFFF
        yield state >> 16


def rgb_ppm() -> bytes:
    noise = lcg(216)
    px = bytearray()
    for y in range(HEIGHT):
        for x in range(WIDTH):
            for c in range(3):
                v = (x * (3 + c) + y * (5 - c) + 40 * c) % 256 + next(noise) % 17 - 8
                px.append(max(0, min(255, v)))
    return b"P6\n%d %d\n255\n" % (WIDTH, HEIGHT) + bytes(px)


def run(*argv: str) -> None:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}{result.stderr}")


def read_pgx(data: bytes) -> tuple[int, int, list[int]]:
    line = data.index(b"\n")
    m = re.match(rb"PG\s+(ML|LM)\s*([+-]?)\s*(\d+)\s+(\d+)\s+(\d+)", data[:line])
    if m is None:
        sys.exit(f"not a PGX header: {data[:line]!r}")
    order, sign, depth, w, h = m.groups()
    depth, w, h = int(depth), int(w), int(h)
    size = 1 if depth <= 8 else 2 if depth <= 16 else 4
    code = {1: "b", 2: "h", 4: "i"}[size]
    if sign != b"-":
        code = code.upper()
    endian = ">" if order == b"ML" else "<"
    return w, h, list(struct.unpack(f"{endian}{w * h}{code}", data[line + 1:line + 1 + w * h * size]))


def decode(path: Path, tmp: Path, args: list[str]) -> list[tuple[int, int, list[int]]]:
    """The `opj_decompress` output of `path` under `args`, one entry per component."""
    for old in tmp.glob("out_*.pgx"):
        old.unlink()
    run("opj_decompress", "-i", str(path), "-o", str(tmp / "out.pgx"), *args)
    outs = sorted(tmp.glob("out_*.pgx"), key=lambda p: int(p.stem.rsplit("_", 1)[1]))
    if not outs:
        sys.exit(f"opj_decompress wrote no PGX for {path} {args}")
    return [read_pgx(p.read_bytes()) for p in outs]


def sample_hash(components: list[tuple[int, int, list[int]]]) -> str:
    data = b"".join(struct.pack(f"<{len(s)}i", *s) for _, _, s in components)
    return hashlib.sha256(data).hexdigest()


def option(args: list[str], flag: str) -> str | None:
    return args[args.index(flag) + 1] if flag in args else None


def area(args: list[str]) -> tuple[int, int, int, int]:
    """The reference-grid area a case returns, clipped to the image."""
    x0, y0 = ORIGIN
    x1, y1 = x0 + WIDTH, y0 + HEIGHT
    if (d := option(args, "-d")) is not None:
        a, b, c, e = map(int, d.split(","))
        return max(a, x0), max(b, y0), min(c, x1), min(e, y1)
    if (t := option(args, "-t")) is not None:
        t = int(t)
        p, q = t % 3, t // 3
        return (max(3 + 24 * p, x0), max(2 + 20 * q, y0),
                min(3 + 24 * (p + 1), x1), min(2 + 20 * (q + 1), y1))
    return x0, y0, x1, y1


def crop(whole, args: list[str]) -> list[tuple[int, int, list[int]]]:
    """The part of the whole decode at the case's reduction that the case returns."""
    r = int(option(args, "-r") or 0)
    s = 1 << r
    ax0, ay0, ax1, ay1 = (math.ceil(v / s) for v in area(args))
    ox, oy = (math.ceil(v / s) for v in ORIGIN)
    out = []
    for w, _, samples in whole:
        rows = [samples[(y - oy) * w + (x - ox)] for y in range(ay0, ay1) for x in range(ax0, ax1)]
        out.append((ax1 - ax0, ay1 - ay0, rows))
    return out


def main_fixtures() -> None:
    DIR.mkdir(exist_ok=True)
    for old in DIR.glob("*.pgx"):
        old.unlink()
    (DIR / "rgb.ppm").write_bytes(rgb_ppm())
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        for name, (args, mct) in CODESTREAMS.items():
            j2k = DIR / f"{name}.j2k"
            run("opj_compress", "-i", str(DIR / "rgb.ppm"), "-o", str(j2k), *args)
            wholes = {r: decode(j2k, tmp, ["-r", str(r)]) for r in range(3)}
            lines = []
            for case in CASES:
                got = decode(j2k, tmp, case)
                r = int(option(case, "-r") or 0)
                comps = option(case, "-c")
                picked = [int(c) for c in comps.split(",")] if comps else [0, 1, 2]
                expected = [crop(wholes[r], case)[c] for c in picked]
                if comps and mct:
                    if got == expected:
                        sys.exit(f"{name} {case}: -c gives the whole decode's components; "
                                 "the departure note is out of date")
                    got = expected
                elif got != expected:
                    sys.exit(f"{name} {case}: not the crop of the whole decode")
                w, h, _ = got[0]
                lines.append(f"{''.join(case)} {w}x{h} {sample_hash(got)}")
            (DIR / f"{name}.sha256").write_text("\n".join(lines) + "\n")
            if name == "tiled_97":
                for r in range(3):
                    run("opj_decompress", "-i", str(j2k), "-o", str(tmp / "whole.pgx"),
                        "-r", str(r))
                    for c in range(3):
                        (DIR / f"{name}_r{r}_{c}.pgx").write_bytes(
                            (tmp / f"whole_{c}.pgx").read_bytes())
            print(f"{name}.j2k: {j2k.stat().st_size} bytes, {len(lines)} cases")


def gray_2048() -> bytes:
    noise = lcg(2160)
    px = bytearray()
    for y in range(2048):
        for x in range(2048):
            v = ((x * 3 + y * 2) >> 3) % 256 + next(noise) % 9 - 4
            px.append(max(0, min(255, v)))
    return b"P5\n2048 2048\n255\n" + bytes(px)


def main_bench() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        pgm = Path(tmp) / "gray.pgm"
        pgm.write_bytes(gray_2048())
        run("opj_compress", "-i", str(pgm), "-o", str(BENCH), "-t", "256,256", "-n", "5",
            "-r", "20")
    print(f"{BENCH.name}: {BENCH.stat().st_size} bytes")


def main() -> None:
    require_openjpeg("opj_compress", "opj_decompress")
    if sys.argv[1:] == ["--bench"]:
        main_bench()
    else:
        main_fixtures()


if __name__ == "__main__":
    main()
