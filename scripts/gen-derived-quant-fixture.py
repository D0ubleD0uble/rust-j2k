#!/usr/bin/env python3
"""Generate the scalar-derived quantization fixture and its OpenJPEG oracle.

The derived style (Sqcd = 1) signals one (exponent, mantissa) pair and the
decoder derives every finer subband's exponent as eps_b = max(eps_0 -
floor((b-1)/3), 0) (E-5). That per-level drop had unit tests but no
oracle-backed end-to-end fixture: `opj_compress` cannot emit the derived style
(its 9/7 default is expounded), so none of the committed 9/7 fixtures reach
the derived arm of `Qcd::subband_step`.

This script closes the gap by patching an expounded codestream into a derived
one, then snapshotting `opj_decompress` of the *patched* file:

  1. Encode 9/7, three resolutions, rate-truncated — the same shape as the
     bit-exactly graded `irreversible_lossy` fixture (a *lossless-rate* 9/7
     with decomposition levels is not bit-exact against OpenJPEG; issue #104).
  2. Rewrite QCD: style expounded -> derived, keeping the guard bits and only
     the first (LL) step entry, with Lqcd fixed up.
  3. Decode the patched codestream with OpenJPEG for the oracle. Encoder steps
     and decoder steps now disagree per subband, so the image is distorted —
     harmlessly: both decoders see the same derived steps, and agreement on
     the reconstruction is exactly what the fixture grades.

The patch is self-checked: the QCD must have been expounded before and must
parse as derived (one entry) after.

Usage: python3 scripts/gen-derived-quant-fixture.py
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

NAME = "derived_quant_lossy"
WIDTH, HEIGHT = 32, 24
FLAGS = ["-I", "-n", "3", "-r", "8"]


def sample(x: int, y: int) -> int:
    """Deterministic and non-separable, as in gen-irreversible-fixtures.py."""
    return (5 * x + 11 * y + ((x * y) % 7) * 13) % 256


def run(*argv: str) -> None:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}\n{result.stderr}")


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


def find_qcd(raw: bytes) -> tuple[int, int]:
    """Offset and Lqcd of the main-header QCD segment."""
    offset = 2  # past SOC
    while offset < len(raw) - 3:
        marker = struct.unpack(">H", raw[offset : offset + 2])[0]
        if marker == 0xFF90:  # SOT: main header ended without a QCD
            break
        if 0xFF30 <= marker <= 0xFF3F:
            offset += 2
            continue
        length = struct.unpack(">H", raw[offset + 2 : offset + 4])[0]
        if marker == 0xFF5C:  # QCD
            return offset, length
        offset += 2 + length
    sys.exit(f"{NAME}: no QCD marker in the main header")


def patch_qcd_to_derived(raw: bytes) -> bytes:
    offset, length = find_qcd(raw)
    body = raw[offset + 4 : offset + 2 + length]
    sqcd, steps = body[0], body[1:]
    if sqcd & 0x1F != 2:
        sys.exit(f"{NAME}: encoder QCD style is {sqcd & 0x1F}, expected expounded (2)")
    if len(steps) < 2 or len(steps) % 2 != 0:
        sys.exit(f"{NAME}: malformed expounded step table")
    derived_body = bytes([(sqcd & 0xE0) | 1]) + steps[:2]  # guard bits + LL entry
    patched = (
        raw[: offset + 2]
        + struct.pack(">H", 2 + len(derived_body))
        + derived_body
        + raw[offset + 2 + length :]
    )
    # Self-check: the patched QCD must parse as derived with a single entry.
    check_off, check_len = find_qcd(patched)
    check = patched[check_off + 4 : check_off + 2 + check_len]
    if check[0] & 0x1F != 1 or len(check) != 3:
        sys.exit(f"{NAME}: patched QCD did not come out derived")
    return patched


def main() -> None:
    for tool in ("opj_compress", "opj_decompress"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH; install OpenJPEG to regenerate the fixture")

    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        plane = bytes(sample(x, y) for y in range(HEIGHT) for x in range(WIDTH))
        (tmp / "in.raw").write_bytes(plane)

        argv = ["-F", f"{WIDTH},{HEIGHT},1,8,u", "-p", "LRCP", "-b", "16,16", *FLAGS]
        run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(tmp / "expounded.j2k"), *argv)

        codestream = FIXTURES / f"{NAME}.j2k"
        codestream.write_bytes(patch_qcd_to_derived((tmp / "expounded.j2k").read_bytes()))

        run("opj_decompress", "-i", str(codestream), "-o", str(tmp / "out.pgx"))
        # opj_decompress names the plane out.pgx or out_0.pgx depending on version.
        out = next(p for p in (tmp / "out.pgx", tmp / "out_0.pgx") if p.exists())
        w, h, depth, signed, values = read_pgx(out)
        if (w, h) != (WIDTH, HEIGHT):
            sys.exit(f"{NAME}: decoded to {w}x{h}, expected {WIDTH}x{HEIGHT}")

        snapshot = {
            "image": {"width": WIDTH, "height": HEIGHT},
            "tolerance": {"mode": "exact"},
            "components": [
                {
                    "width": w,
                    "height": h,
                    "bit_depth": depth,
                    "signed": signed,
                    "x_sampling": 1,
                    "y_sampling": 1,
                    "samples": values,
                }
            ],
            "provenance": {
                "source": f"synthetic; generated by scripts/{Path(__file__).name}",
                "oracle_command": (
                    f"opj_compress -i in.raw -o expounded.j2k {' '.join(argv)} ; "
                    "QCD patched expounded->derived (style 1, LL entry only) by the "
                    f"generator ; opj_decompress -i {NAME}.j2k -o out.pgx"
                ),
                "notes": (
                    "scalar-derived quantization (Sqcd style 1): grades the per-level "
                    "exponent drop of E-5 end to end. opj_compress cannot emit the "
                    "derived style, so the QCD is patched from the expounded encode and "
                    "the oracle is OpenJPEG's decode of the patched codestream."
                ),
            },
        }
        (FIXTURES / f"{NAME}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
        print(f"wrote {NAME}.j2k + {NAME}.expected.json")


if __name__ == "__main__":
    main()
