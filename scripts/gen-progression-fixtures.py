#!/usr/bin/env python3
"""Generate one decode fixture per progression order, with its OpenJPEG oracle.

The ISO/IEC 15444-4 corpus only reaches RLCP: `p0_16` is RLCP with three quality
layers and one component, which does distinguish `r → l` from LRCP's `l → r`.
Every RPCL, PCRL and CPRL entry (`p0_05`, `p0_06`, `p0_08`, `p1_03`) also needs
COC/QCC, tiles, or SOP/EPH, so none of them decode yet. Those three orders are
therefore graded against synthetic codestreams encoded by OpenJPEG.

Each fixture carries **three components and three quality layers**, which is the
minimum that separates the orders. With one component or one layer several of
them collapse onto each other — `p0_01` is exactly that trap, and passing it
proves nothing (see `docs/correctness.md` §A passing entry is not proof the
feature works). With three of each:

    LRCP  l → r → c        RPCL  r → c → l
    RLCP  r → l → c        PCRL  c → r → l
                           CPRL  c → r → l

**PCRL and CPRL still coincide here.** With one precinct per resolution their
position loops both degenerate, and the two codestreams OpenJPEG emits for them
differ in exactly one byte: the progression code in COD. That is not a shortcut in
this script — the orders genuinely agree under a maximal partition. Separating
them needs more than one precinct to position, which is what
`gen-precinct-fixtures.py` adds.

`opj_compress -r 20,10,1` makes the last layer lossless, so a full decode is
bit-exact and the snapshot records `tolerance: exact`. A packet enumerated in the
wrong order does not produce a rounding error; it produces garbage.

Usage: python3 scripts/gen-progression-fixtures.py
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

ORDERS = ["RLCP", "RPCL", "PCRL", "CPRL"]
WIDTH, HEIGHT, COMPONENTS = 24, 16, 3
LAYER_RATES = "20,10,1"  # three layers, the last lossless


def sample(component: int, x: int, y: int) -> int:
    """Deterministic and non-separable, so a mis-ordered packet walk cannot pass
    by accident."""
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


def cod_fields(codestream: Path) -> tuple[int, int]:
    """`(progression, layers)` straight out of COD, so a fixture cannot silently
    regress to LRCP or one layer if OpenJPEG reinterprets a flag."""
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
            # Scod(1), then SGcod = progression(1), layers(2), mct(1).
            return body[1], struct.unpack(">H", body[2:4])[0]
        offset += 2 + length
    sys.exit(f"{codestream}: no COD marker")


PROGRESSION_CODE = {"LRCP": 0, "RLCP": 1, "RPCL": 2, "PCRL": 3, "CPRL": 4}


def build(order: str) -> None:
    name = f"progression_{order.lower()}_lossless"
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
            "-p", order,
            "-n", "3",         # 2 decomposition levels
            "-b", "16,16",
            "-r", LAYER_RATES,
        ]
        run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(codestream), *flags)

        progression, layers = cod_fields(codestream)
        if progression != PROGRESSION_CODE[order]:
            sys.exit(f"{name}: COD says progression {progression}, expected {order}")
        if layers < 3:
            sys.exit(f"{name}: COD declares {layers} layer(s); the orders need three to separate")

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

        note = f"{order} progression, {layers} quality layers, {COMPONENTS} components"
        if order in ("PCRL", "CPRL"):
            note += "; PCRL and CPRL coincide with one precinct, so this cannot tell them apart"

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
                "notes": note,
            },
        }
        (FIXTURES / f"{name}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
        print(f"wrote {name}.j2k + {name}.expected.json ({order}, {layers} layers)")


def main() -> None:
    for tool in ("opj_compress", "opj_decompress"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH; install OpenJPEG to regenerate the fixtures")
    for order in ORDERS:
        build(order)


if __name__ == "__main__":
    main()
