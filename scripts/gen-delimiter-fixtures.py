#!/usr/bin/env python3
"""Generate the packet-delimiter (SOP/EPH) decode fixtures and their oracles.

No ISO/IEC 15444-4 corpus entry is unblocked by SOP/EPH alone. Eleven of the
nineteen not-yet-decoded entries use them, but every one also needs a code-block
style, a tile grid, COC/QCC, precincts, or an image origin. So the delimiters are
graded against synthetic codestreams encoded by OpenJPEG, the way the layer and
progression paths are.

`opj_compress -SOP` and `-EPH` are independent switches, and the standard treats
them differently: SOP *may* precede each packet even when `Scod` signals it
(A.8.1), while EPH *shall* follow every packet header (A.8.2), an empty packet's
included. Both flags are exercised alone and together, because the corpus does
too -- `p0_12` signals SOP only, `p0_11` EPH only.

Two layers and two components, so the packets are numerous enough that a
delimiter mis-consumed by even one byte desynchronises the walk. The last layer
is lossless, so a full decode is bit-exact.

Usage: python3 scripts/gen-delimiter-fixtures.py
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

WIDTH, HEIGHT, COMPONENTS = 24, 16, 2
LAYER_RATES = "20,1"  # two layers, the last lossless

VARIANTS = {
    "sop": (["-SOP"], 0x02),
    "eph": (["-EPH"], 0x04),
    "sop_eph": (["-SOP", "-EPH"], 0x06),
}


def sample(component: int, x: int, y: int) -> int:
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


def scod(codestream: Path) -> int:
    """`Scod` straight out of COD, so a fixture cannot silently lose its
    delimiter flags if OpenJPEG reinterprets a switch."""
    raw = codestream.read_bytes()
    offset = 2
    while offset < len(raw) - 3:
        marker = struct.unpack(">H", raw[offset : offset + 2])[0]
        if marker == 0xFF90:
            break
        if 0xFF30 <= marker <= 0xFF3F:
            offset += 2
            continue
        length = struct.unpack(">H", raw[offset + 2 : offset + 4])[0]
        if marker == 0xFF52:  # COD
            return raw[offset + 4]
        offset += 2 + length
    sys.exit(f"{codestream}: no COD marker")


def build(variant: str) -> None:
    switches, want_scod = VARIANTS[variant]
    name = f"delimiters_{variant}_lossless"
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
            "-mct", "0",
            "-p", "LRCP",
            "-n", "3",
            "-b", "16,16",
            "-r", LAYER_RATES,
            *switches,
        ]
        run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(codestream), *flags)

        got = scod(codestream)
        if got & 0x06 != want_scod:
            sys.exit(f"{name}: COD Scod is {got:#04x}, expected delimiter bits {want_scod:#04x}")

        run("opj_decompress", "-i", str(codestream), "-o", str(tmp / "out.pgx"))

        snapshot_components = []
        for component in range(COMPONENTS):
            w, h, depth, signed, values = read_pgx(tmp / f"out_{component}.pgx")
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
                "notes": f"Scod {got:#04x}: {' and '.join(s.lstrip('-') for s in switches)}; "
                f"two quality layers, {COMPONENTS} components, LRCP",
            },
        }
        (FIXTURES / f"{name}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
        print(f"wrote {name}.j2k + {name}.expected.json (Scod {got:#04x})")


def main() -> None:
    for tool in ("opj_compress", "opj_decompress"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH; install OpenJPEG to regenerate the fixtures")
    for variant in VARIANTS:
        build(variant)


if __name__ == "__main__":
    main()
