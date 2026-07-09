#!/usr/bin/env python3
"""Generate a region-of-interest (RGN maxshift) decode fixture and its oracle.

`opj_compress -ROI c=<comp>,U=<shift>` upshifts every quantization index of one
component and writes the RGN marker that records the shift. With a reversible
5/3 transform and a lossless final layer the decode is bit-exact, so a decoder
that ignores RGN -- or that starts the block at the wrong bit-plane -- does not
produce a rounding error, it produces garbage.

Two components, only the first upshifted, so the fixture also pins that the
shift is *per component*: a decoder that applied it to every component would
wreck the second one.

**What this fixture does not grade.** OpenJPEG's `-ROI` upshifts the whole
component, not a region of it, so every coefficient sits above the maxshift
threshold and none below. The `mag >= 1 << roi_shift` test in `decode_block` is
therefore always true here, and a decoder that dropped the test and shifted
unconditionally would still pass. Grading the threshold needs a codestream with
both foreground and background coefficients; `p0_06` is that codestream, and it
is blocked on an unrelated 9/7 divergence. Recorded in the fixture README.

Usage: python3 scripts/gen-roi-fixture.py
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

NAME = "roi_maxshift_lossless"
WIDTH, HEIGHT, COMPONENTS = 24, 16, 2
ROI_COMPONENT, ROI_SHIFT = 0, 6

RGN = 0xFF5E
SOT = 0xFF90


def sample(component: int, x: int, y: int) -> int:
    """Deterministic and non-separable, so a transposed or component-swapped
    decode cannot pass by accident."""
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


def rgn_segments(codestream: Path) -> list[tuple[int, int, int]]:
    """Every main-header RGN as `(Crgn, Srgn, SPrgn)`, so the fixture cannot
    silently lose its ROI if OpenJPEG reinterprets `-ROI`."""
    raw = codestream.read_bytes()
    found = []
    offset = 2  # past SOC
    while offset < len(raw) - 3:
        marker = struct.unpack(">H", raw[offset : offset + 2])[0]
        if marker == SOT:
            break
        if 0xFF30 <= marker <= 0xFF3F:
            offset += 2
            continue
        length = struct.unpack(">H", raw[offset + 2 : offset + 4])[0]
        if marker == RGN:
            body = raw[offset + 4 : offset + 2 + length]
            # Crgn is one byte while Csiz < 257.
            found.append((body[0], body[1], body[2]))
        offset += 2 + length
    return found


def main() -> None:
    for tool in ("opj_compress", "opj_decompress"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH; install OpenJPEG to regenerate the fixture")

    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        planes = bytearray()
        for component in range(COMPONENTS):
            for y in range(HEIGHT):
                for x in range(WIDTH):
                    planes.append(sample(component, x, y))
        (tmp / "in.raw").write_bytes(bytes(planes))

        codestream = FIXTURES / f"{NAME}.j2k"
        flags = [
            "-F", f"{WIDTH},{HEIGHT},{COMPONENTS},8,u",
            "-mct", "0",   # components stay independent: no RCT/ICT
            "-p", "LRCP",
            "-n", "3",     # 2 decomposition levels
            "-b", "16,16",
            "-r", "1",     # one lossless layer, so the decode is bit-exact
            "-ROI", f"c={ROI_COMPONENT},U={ROI_SHIFT}",
        ]
        run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(codestream), *flags)

        rgns = rgn_segments(codestream)
        if rgns != [(ROI_COMPONENT, 0, ROI_SHIFT)]:
            sys.exit(
                f"{NAME}: expected one RGN (c={ROI_COMPONENT}, Srgn=0, SPrgn={ROI_SHIFT}), "
                f"got {rgns}"
            )

        run("opj_decompress", "-i", str(codestream), "-o", str(tmp / "out.pgx"))

        components = []
        for component in range(COMPONENTS):
            w, h, depth, signed, values = read_pgx(tmp / f"out_{component}.pgx")
            expected = [sample(component, x, y) for y in range(HEIGHT) for x in range(WIDTH)]
            if (w, h) != (WIDTH, HEIGHT) or values != expected:
                sys.exit(f"{NAME}: component {component} did not round-trip losslessly")
            components.append(
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
            "components": components,
            "provenance": {
                "source": f"synthetic; generated by scripts/{Path(__file__).name}",
                "oracle_command": (
                    f"opj_compress -i in.raw -o {NAME}.j2k " + " ".join(flags)
                    + f" ; opj_decompress -i {NAME}.j2k -o out.pgx"
                ),
                "notes": (
                    f"RGN maxshift {ROI_SHIFT} on component {ROI_COMPONENT} of {COMPONENTS}, "
                    "reversible 5/3, one lossless layer. `-ROI` upshifts the whole component, so "
                    "every coefficient is above the maxshift threshold and the threshold test "
                    "itself is not graded here."
                ),
            },
        }
        (FIXTURES / f"{NAME}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
        print(f"wrote {NAME}.j2k + {NAME}.expected.json (RGN {rgns[0]})")


if __name__ == "__main__":
    main()
