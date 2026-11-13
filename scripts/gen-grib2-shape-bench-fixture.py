#!/usr/bin/env python3
"""Generate the GRIB2-shaped decode benchmark fixture.

`benches/decode.rs` needs a GRIB2-sized codestream to time, and the only
committed GRIB2 codestream is 2 KiB. This writes a synthetic one in the shape
operational template 5.40 messages have: 1440x721 (a 0.25 degree global grid),
one component of 16-bit samples, one tile, the 5/3 wavelet, 5 decomposition
levels, 1 quality layer, 64x64 code-blocks, LRCP, maximum precincts, and no
SOP, EPH, TLM or PLT.

The samples are a smooth field (a few low-order waves, like a temperature
map) plus small noise from a fixed seed. Nothing external is read, so the
output is reproducible. The encode stops at a fixed compression ratio, as
GRIB2 writers do; the decode is still on the reversible 5/3 path. OpenJPEG
decodes the file, and `tests/bench_fixture.rs` checks our decode against that
output, sample for sample, through an FNV-1a digest in the expected file.

Usage: python3 scripts/gen-grib2-shape-bench-fixture.py
Requires `opj_compress` and `opj_decompress` on PATH (OpenJPEG 2.5.0).
"""

from __future__ import annotations

import json
import math
import random
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

from oracle_tools import require_openjpeg

OUT = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "bench"
NAME = "grib2_shape_1440x721_u16"
WIDTH, HEIGHT, DEPTH = 1440, 721, 16
SEED = 20261113
RATIO = 3  # opj_compress -r: stop at about 1/3 of the raw size

FLAGS = ["-F", f"{WIDTH},{HEIGHT},1,{DEPTH},u", "-n", "6", "-b", "64,64",
         "-p", "LRCP", "-r", str(RATIO)]


def field() -> list[int]:
    rng = random.Random(SEED)
    values = []
    for y in range(HEIGHT):
        lat = math.pi * (y / (HEIGHT - 1) - 0.5)
        for x in range(WIDTH):
            lon = 2 * math.pi * x / WIDTH
            smooth = (0.55 * math.cos(lat) ** 2 + 0.12 * math.sin(3 * lon) * math.cos(lat)
                      + 0.06 * math.sin(7 * lat + 2 * lon) + 0.03 * math.sin(19 * lon))
            noise = rng.gauss(0.0, 1.0) * 0.002
            values.append(min(65535, max(0, round((smooth + noise + 0.1) * 60000))))
    return values


def fnv1a64(values: tuple[int, ...]) -> str:
    """FNV-1a over the samples as little-endian u16, in raster order."""
    h = 0xCBF29CE484222325
    for v in values:
        for b in (v & 0xFF, v >> 8):
            h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return f"{h:016x}"


def run(*argv: str) -> None:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}\n{result.stderr}")


def main() -> None:
    version = require_openjpeg("opj_compress", "opj_decompress")
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        values = field()
        (tmp / "in.raw").write_bytes(struct.pack(f">{len(values)}H", *values))
        codestream = OUT / f"{NAME}.j2k"
        run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(codestream), *FLAGS)
        run("opj_decompress", "-i", str(codestream), "-o", str(tmp / "out.pgx"))
        raw = (tmp / "out_0.pgx").read_bytes()
        decoded = struct.unpack(f">{WIDTH * HEIGHT}H", raw[raw.index(b"\n") + 1:])
        (OUT / f"{NAME}.expected.json").write_text(json.dumps({
            "width": WIDTH, "height": HEIGHT, "depth": DEPTH,
            "oracle": f"opj_decompress {version}", "seed": SEED,
            "fnv1a64": fnv1a64(decoded),
        }, indent=2) + "\n")
        print(f"{codestream}: {codestream.stat().st_size} bytes")


if __name__ == "__main__":
    main()
