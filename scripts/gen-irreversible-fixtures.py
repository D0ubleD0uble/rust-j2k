#!/usr/bin/env python3
"""Generate bit-exact 9/7 decode fixtures and their OpenJPEG oracles.

The irreversible path had no fixture that could catch an error of half a
quantization step. `p0_09` is 9/7 and bit-exact, but its coefficients never
exercise the case; `eta_lambert_irreversible` is graded with a tolerance wide
enough to absorb it. So a double mid-point reconstruction bias -- Tier-1's
`oneplushalf` plus a second `+ ½` in dequantization -- lived in the decoder,
cancelled for odd coefficients by an integer halving that should not have been
there, and wrong for even ones (issue #101).

These two fixtures close that. Both are snapshotted `tolerance: exact` against
`opj_decompress`, which is only achievable if the arithmetic matches OpenJPEG's:

- `irreversible_flat_lossless` — **one resolution**, so the inverse DWT is the
  identity and the fixture grades dequantization and the double-scale halving in
  isolation. Nothing else can be blamed for a mismatch. Coded at `-r 1` so every
  bit-plane is present, which is what makes the double-scale magnitudes odd and
  the half bit observable at all.
- `irreversible_lossy` — two decomposition levels and a truncated rate, the
  ordinary shape of a 9/7 codestream.

A lossless 9/7 with decomposition levels is *not* bit-exact against OpenJPEG
today: 2 of 1536 samples differ by 1. That is a separate rounding difference in
the inverse 9/7 lifting, tracked on its own issue, and it is why the lossless
fixture here uses one resolution and the multi-level one is lossy.

Usage: python3 scripts/gen-irreversible-fixtures.py
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

WIDTH, HEIGHT, COMPONENTS = 32, 24, 2

# name -> (extra opj_compress flags, note)
VARIANTS = {
    "irreversible_flat_lossless": (
        ["-I", "-n", "1", "-r", "1"],
        "one resolution, so the inverse DWT is the identity: this grades "
        "dequantization and the double-scale halving on their own. Every bit-plane "
        "is coded, so the double-scale magnitudes are odd and the half bit matters.",
    ),
    "irreversible_lossy": (
        ["-I", "-n", "3", "-r", "8"],
        "two decomposition levels, rate-truncated: the ordinary shape of a 9/7 "
        "codestream, decoded bit-exactly.",
    ),
}


def sample(component: int, x: int, y: int) -> int:
    """Deterministic and non-separable, so a transposed or component-swapped
    decode cannot pass by accident."""
    return (37 * component + 5 * x + 11 * y + ((x * y) % 7) * 13) % 256


def read_pgx(path: Path) -> tuple[int, int, int, bool, list[int]]:
    raw = path.read_bytes()
    end = raw.index(b"\n")
    tokens = raw[:end].split()
    assert tokens[0] == b"PG", f"{path}: not a PGX file"
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
    stride = 1 if depth <= 8 else (2 if depth <= 16 else 4)
    fmt = {1: "b" if signed else "B", 2: "h" if signed else "H", 4: "i" if signed else "I"}[stride]
    count = width * height
    values = list(struct.unpack(f">{count}{fmt}", raw[end + 1 : end + 1 + count * stride]))
    return width, height, depth, signed, values


def run(*argv: str) -> None:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}\n{result.stderr}")


def qmfbid(codestream: Path) -> int:
    """The COD transform byte, so a fixture cannot silently regress to 5/3 —
    which would defeat its whole purpose."""
    raw = codestream.read_bytes()
    offset = 2  # past SOC
    while offset < len(raw) - 3:
        marker = struct.unpack(">H", raw[offset : offset + 2])[0]
        if marker == 0xFF90:  # SOT
            break
        if 0xFF30 <= marker <= 0xFF3F:
            offset += 2
            continue
        length = struct.unpack(">H", raw[offset + 2 : offset + 4])[0]
        if marker == 0xFF52:  # COD
            body = raw[offset + 4 : offset + 2 + length]
            # Scod(1) prog(1) layers(2) mct(1) levels(1) cbw(1) cbh(1) style(1) transform(1)
            return body[9]
        offset += 2 + length
    sys.exit(f"{codestream}: no COD marker")


def build(name: str, flags: list[str], note: str) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        planes = bytearray()
        for component in range(COMPONENTS):
            for y in range(HEIGHT):
                for x in range(WIDTH):
                    planes.append(sample(component, x, y))
        (tmp / "in.raw").write_bytes(bytes(planes))

        codestream = FIXTURES / f"{name}.j2k"
        argv = [
            "-F", f"{WIDTH},{HEIGHT},{COMPONENTS},8,u",
            "-mct", "0",  # components stay independent: no RCT/ICT
            "-p", "LRCP",
            "-b", "16,16",
            *flags,
        ]
        run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(codestream), *argv)

        if qmfbid(codestream) != 0:
            sys.exit(f"{name}: COD says the transform is 5/3; the fixture must be 9/7")

        run("opj_decompress", "-i", str(codestream), "-o", str(tmp / "out.pgx"))

        components = []
        for component in range(COMPONENTS):
            w, h, depth, signed, values = read_pgx(tmp / f"out_{component}.pgx")
            if (w, h) != (WIDTH, HEIGHT):
                sys.exit(f"{name}: component {component} decoded to {w}x{h}")
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
                    f"opj_compress -i in.raw -o {name}.j2k " + " ".join(argv)
                    + f" ; opj_decompress -i {name}.j2k -o out.pgx"
                ),
                "notes": f"irreversible 9/7; {note}",
            },
        }
        (FIXTURES / f"{name}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
        print(f"wrote {name}.j2k + {name}.expected.json")


def main() -> None:
    for tool in ("opj_compress", "opj_decompress"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH; install OpenJPEG to regenerate the fixtures")
    for name, (flags, note) in VARIANTS.items():
        build(name, flags, note)


if __name__ == "__main__":
    main()
