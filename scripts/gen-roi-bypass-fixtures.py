#!/usr/bin/env python3
"""Generate the RGN maxshift + bypass fixtures in tests/fixtures/roi_bypass/.

`opj_compress -ROI c=0,U=3` with a bypass code-block style (`-M 1`, `-M 5`,
`-M 33`, `-M 17`) writes a lossless codestream whose decode must equal the source image.
`opj_decompress` 2.5.0 gets these wrong: its decoder switches a pass to raw at
`cblk->numbps - 4`, a plane count that leaves the maxshift out, while its
encoder and the standard (D.6, Table D.9) count from the code-block's first
coded plane. So the grading reference is the source image itself, not the
OpenJPEG decode. See docs/correctness.md, "Bypass under a maxshift".

The script writes `source.pgm` and one `.j2k` per style, then checks that each
`.j2k` carries the RGN and the bypass flag, and that `opj_decompress` does not
reproduce the source (the fixture would not exercise the bug if it did). A
control encode without bypass must round-trip under `opj_decompress`, so the
ROI itself is known good.

Usage: python3 scripts/gen-roi-bypass-fixtures.py
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

DIR = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "roi_bypass"

WIDTH, HEIGHT = 64, 64
ROI = "c=0,U=3"
# `-M` value -> fixture name. 1 = bypass; 5 = bypass + termination on every
# pass; 33 = bypass + segmentation symbols; 17 = bypass + predictable
# termination. Under 5 every pass is its own segment, so a segment opens at
# the significance pass of every plane; under the lazy split one only opens at
# the fifth coded plane and below. Only 5 tells a raw decision that starts one
# plane early (`top - 3`) from the right one.
STYLES = {1: "roi_bypass_m1", 5: "roi_bypass_m5", 33: "roi_bypass_m33",
          17: "roi_bypass_m17"}
BASE_FLAGS = ["-n", "3", "-ROI", ROI]

COD, RGN, SOT = 0xFF52, 0xFF5E, 0xFF90
LAZY = 0x01


def source_pgm() -> bytes:
    """A 64x64 gradient plus noise from a fixed LCG, so the image has enough
    bit-planes to reach the bypass region and does not depend on Python's
    `random` implementation."""
    state = 7
    pixels = bytearray()
    for y in range(HEIGHT):
        for x in range(WIDTH):
            state = (state * 1103515245 + 12345) & 0x7FFFFFFF
            noise = (state >> 16) % 121 - 60
            pixels.append(max(0, min(255, 128 + (60 * x) // WIDTH - (60 * y) // HEIGHT + noise)))
    return b"P5\n%d %d\n255\n" % (WIDTH, HEIGHT) + bytes(pixels)


def main_header_markers(codestream: bytes) -> dict[int, list[bytes]]:
    """Main-header marker segments by marker code, each as its body."""
    found: dict[int, list[bytes]] = {}
    offset = 2  # past SOC
    while offset < len(codestream) - 3:
        marker = struct.unpack(">H", codestream[offset : offset + 2])[0]
        if marker == SOT:
            break
        length = struct.unpack(">H", codestream[offset + 2 : offset + 4])[0]
        found.setdefault(marker, []).append(codestream[offset + 4 : offset + 2 + length])
        offset += 2 + length
    return found


def run(*argv: str) -> None:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}\n{result.stderr}")


def opj_round_trips(codestream: Path, source: bytes, tmp: Path) -> bool:
    out = tmp / "out.pgm"
    out.unlink(missing_ok=True)
    subprocess.run(["opj_decompress", "-i", str(codestream), "-o", str(out)],
                   capture_output=True)
    return out.exists() and out.read_bytes()[-WIDTH * HEIGHT :] == source[-WIDTH * HEIGHT :]


def main() -> None:
    require_openjpeg("opj_compress", "opj_decompress")
    DIR.mkdir(parents=True, exist_ok=True)
    source = source_pgm()
    (DIR / "source.pgm").write_bytes(source)
    print(f"source.pgm sha256 {hashlib.sha256(source).hexdigest()}")

    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        control = tmp / "control.j2k"
        run("opj_compress", "-i", str(DIR / "source.pgm"), "-o", str(control), *BASE_FLAGS)
        if not opj_round_trips(control, source, tmp):
            sys.exit("control encode without bypass does not round-trip under opj_decompress")

        for mode, name in STYLES.items():
            path = DIR / f"{name}.j2k"
            flags = [*BASE_FLAGS, "-M", str(mode)]
            run("opj_compress", "-i", str(DIR / "source.pgm"), "-o", str(path), *flags)
            markers = main_header_markers(path.read_bytes())
            # RGN body: Crgn (1 byte while Csiz < 257), Srgn, SPrgn.
            if [tuple(b) for b in markers.get(RGN, [])] != [(0, 0, 3)]:
                sys.exit(f"{name}: expected one RGN (c=0, Srgn=0, SPrgn=3)")
            # COD body: Scod, SGcod (4), then levels, xcb, ycb, style, transform.
            style = markers[COD][0][8]
            if style != mode or not style & LAZY:
                sys.exit(f"{name}: code-block style {style:#x}, expected {mode:#x}")
            opj_ok = opj_round_trips(path, source, tmp)
            if opj_ok:
                sys.exit(f"{name}: opj_decompress round-trips it, so it does not exercise #258")
            print(f"{name}.j2k  opj_compress -i source.pgm -o {name}.j2k {' '.join(flags)}"
                  f"  (opj_decompress: differs from source)")


if __name__ == "__main__":
    main()
