#!/usr/bin/env python3
"""Generate the 16-bit 9/7 fixture and its OpenJPEG oracle.

At 16 bits this decoder and `opj_decompress` differ by more than 1 on a 9/7
codestream. The cause is the high-pass scaling in the inverse 9/7: this decoder
uses the exact `2/K`, and OpenJPEG uses `two_invK = 1.625732422`, which is
5.4e-5 (3.3e-5 relative) short of it (see "When the oracle and the standard disagree" in
docs/correctness.md). The error that constant adds grows with the sample range,
so it shows at high bit depths.

`irreversible_16bit` pins that result. Both components are a 16-bit pattern of
3x5 blocks, coded with every bit-plane (`-r 1`) and five decomposition levels.
Component 0 swings over the full range, which gives a large difference, but
the output clamp hides any overshoot past 0 or 65535. Component 1 swings from
a quarter to three quarters of the range, so nothing clamps there. The snapshot
is graded `tolerance: absolute` at the largest measured difference.
`tests/irreversible_16bit.rs` also checks that each component differs from
OpenJPEG by more than 1, and that this decoder's output is the closer of the
two to the encoder's input, which `sample` below reproduces.

Usage: python3 scripts/gen-irreversible-16bit-fixture.py
Requires `opj_compress` and `opj_decompress` (OpenJPEG 2.5.0) on PATH.
"""

from __future__ import annotations

import json
import struct
import subprocess
import sys
import tempfile
from pathlib import Path
from oracle_tools import require_openjpeg

FIXTURES = Path(__file__).resolve().parent.parent / "tests" / "fixtures"

NAME = "irreversible_16bit"
WIDTH, HEIGHT, DEPTH = 32, 32, 16

# The largest difference from OpenJPEG this decoder shows on the fixture,
# measured (component 0). It is not a bound on other images: larger images with
# denser edges have reached 7 at 16 bits (docs/correctness.md, "Agreement
# standard").
TOLERANCE = 5.0

COMPONENTS = 2
FLAGS = ["-F", f"{WIDTH},{HEIGHT},{COMPONENTS},{DEPTH},u", "-mct", "0", "-p", "LRCP",
         "-I", "-n", "6", "-r", "1"]


def sample(component: int, x: int, y: int) -> int:
    """Blocks 3 wide and 5 tall: full swing on component 0, a quarter to three
    quarters of the range on component 1. `tests/irreversible_16bit.rs`
    computes the same values."""
    top = (1 << DEPTH) - 1
    low, high = (0, top) if component == 0 else (top // 4, 3 * top // 4)
    return high if (x // 3 + y // 5) % 2 else low


def read_pgx(path: Path) -> tuple[int, int, int, bool, list[int]]:
    raw = path.read_bytes()
    end = raw.index(b"\n")
    tokens = raw[:end].split()
    assert tokens[0] == b"PG" and tokens[1] == b"ML", f"{path}: not a big-endian PGX file"
    rest = list(tokens[2:])
    if rest[0] in (b"+", b"-"):
        depth = int((rest[0] + rest[1]).decode())
        rest = rest[2:]
    else:
        depth = int(rest[0])
        rest = rest[1:]
    width, height = int(rest[0]), int(rest[1])
    signed = depth < 0
    depth = abs(depth)
    assert 8 < depth <= 16, f"{path}: expected a 16-bit PGX"
    count = width * height
    values = list(struct.unpack(f">{count}{'h' if signed else 'H'}", raw[end + 1 : end + 1 + 2 * count]))
    return width, height, depth, signed, values


def run(*argv: str) -> None:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}\n{result.stderr}")


def qmfbid(codestream: Path) -> int:
    """The COD transform byte, so the fixture cannot silently become 5/3."""
    raw = codestream.read_bytes()
    offset = 2  # past SOC
    while offset < len(raw) - 3:
        marker = struct.unpack(">H", raw[offset : offset + 2])[0]
        if marker == 0xFF90:  # SOT
            break
        length = struct.unpack(">H", raw[offset + 2 : offset + 4])[0]
        if marker == 0xFF52:  # COD
            # Scod(1) prog(1) layers(2) mct(1) levels(1) cbw(1) cbh(1) style(1) transform(1)
            return raw[offset + 4 + 9]
        offset += 2 + length
    sys.exit(f"{codestream}: no COD marker")


def main() -> None:
    require_openjpeg("opj_compress", "opj_decompress")
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        values = [
            sample(c, x, y) for c in range(COMPONENTS) for y in range(HEIGHT) for x in range(WIDTH)
        ]
        (tmp / "in.raw").write_bytes(struct.pack(f">{len(values)}H", *values))

        codestream = FIXTURES / f"{NAME}.j2k"
        run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(codestream), *FLAGS)
        if qmfbid(codestream) != 0:
            sys.exit(f"{NAME}: COD says the transform is 5/3; the fixture must be 9/7")

        run("opj_decompress", "-i", str(codestream), "-o", str(tmp / "out.pgx"))
        components = []
        for c in range(COMPONENTS):
            w, h, depth, signed, decoded = read_pgx(tmp / f"out_{c}.pgx")
            if (w, h, depth, signed) != (WIDTH, HEIGHT, DEPTH, False):
                sys.exit(f"{NAME}: component {c} decoded to {w}x{h}, {depth} bits, signed={signed}")
            components.append(
                {
                    "width": WIDTH,
                    "height": HEIGHT,
                    "bit_depth": DEPTH,
                    "signed": False,
                    "x_sampling": 1,
                    "y_sampling": 1,
                    "samples": decoded,
                }
            )

    snapshot = {
        "image": {"width": WIDTH, "height": HEIGHT},
        "tolerance": {"mode": "absolute", "max_abs_error": TOLERANCE},
        "components": components,
        "provenance": {
            "source": f"synthetic; generated by scripts/{Path(__file__).name}",
            "oracle_command": (
                f"opj_compress -i in.raw -o {NAME}.j2k " + " ".join(FLAGS)
                + f" ; opj_decompress -i {NAME}.j2k -o out.pgx"
            ),
            "notes": (
                "irreversible 9/7, 16-bit, every bit-plane coded, five decomposition levels; "
                "3x5 blocks, full swing on component 0 and a quarter to three quarters of the "
                "range on component 1. The tolerance is this decoder's largest measured "
                "difference from OpenJPEG: the exact 2/K high-pass scaling here against "
                "OpenJPEG's two_invK. Component 0's output clamp hides any overshoot past 0 "
                "or 65535, so its check against the encoder's input is weaker; component 1 "
                "never clamps. See docs/correctness.md."
            ),
        },
    }
    (FIXTURES / f"{NAME}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
    print(f"wrote {NAME}.j2k + {NAME}.expected.json")


if __name__ == "__main__":
    main()
