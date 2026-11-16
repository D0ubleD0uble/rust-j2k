#!/usr/bin/env python3
"""Generate the many-small-blocks fixture.

`tests/tier1_alloc.rs` counts the allocations of a decode whose time goes
to many tiny code-blocks, the shape of a file written with 4x4 code-blocks.
This writes one: a 128x128 8-bit gray image, one tile, the 5/3 wavelet, 5
decomposition levels, 1 quality layer, 4x4 code-blocks, LRCP, lossless.

The samples are a smooth ramp plus noise from a fixed seed, so nearly every
code-block codes some passes. Nothing external is read, so the output is
reproducible. OpenJPEG decodes the file, and the test checks our decode
against that output through an FNV-1a digest in the expected file.

Usage: python3 scripts/gen-small-block-fixture.py
Requires `opj_compress` and `opj_decompress` on PATH (OpenJPEG 2.5.0).
"""

from __future__ import annotations

import json
import random
import subprocess
import sys
import tempfile
from pathlib import Path

from oracle_tools import require_openjpeg

OUT = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "bench"
NAME = "small_blocks_128_b4"
WIDTH, HEIGHT = 128, 128
SEED = 20261115

FLAGS = ["-n", "6", "-b", "4,4", "-p", "LRCP"]


def image() -> bytes:
    rng = random.Random(SEED)
    return bytes(
        min(255, max(0, (x + y) // 2 + rng.randint(-24, 24)))
        for y in range(HEIGHT)
        for x in range(WIDTH)
    )


def fnv1a64(values: bytes) -> str:
    """FNV-1a over the samples as bytes, in raster order."""
    h = 0xCBF29CE484222325
    for b in values:
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
        pgm = tmp / "in.pgm"
        pgm.write_bytes(f"P5\n{WIDTH} {HEIGHT}\n255\n".encode() + image())
        codestream = OUT / f"{NAME}.j2k"
        run("opj_compress", "-i", str(pgm), "-o", str(codestream), *FLAGS)
        run("opj_decompress", "-i", str(codestream), "-o", str(tmp / "out.pgm"))
        raw = (tmp / "out.pgm").read_bytes()
        decoded = raw[len(raw) - WIDTH * HEIGHT:]
        (OUT / f"{NAME}.expected.json").write_text(json.dumps({
            "width": WIDTH, "height": HEIGHT, "depth": 8,
            "oracle": f"opj_decompress {version}", "seed": SEED,
            "fnv1a64": fnv1a64(decoded),
        }, indent=2) + "\n")
        print(f"{codestream}: {codestream.stat().st_size} bytes")


if __name__ == "__main__":
    main()
