#!/usr/bin/env python3
"""Generate a QCC decode fixture, and its OpenJPEG oracle, from an existing one.

No ISO/IEC 15444-4 corpus entry is unblocked by QCC alone. `p0_13` and `p0_06`
carry one, but each also carries an RGN marker (#77), so neither reaches Tier-1.
`opj_compress` has no flag that emits a QCC either. So the fixture is built by
rewriting a codestream this repo already trusts.

The trick rests on what QCD is: a *default*. A codestream must carry one, but a
QCC may override it for every component, and then QCD's own values are never
read. So take `multicomponent_lossless.j2k` (three reversible components, no
COC/QCC), corrupt QCD's guard bits and exponents, and give every component a QCC
holding the values QCD used to have. The codestream stays valid and decodes to
exactly the same image -- but only for a decoder that honours QCC. One that
ignores it reads the corrupted defaults, gets the wrong `Mb` for every subband,
and reconstructs garbage.

That is what makes this fixture grade the feature rather than merely exercise it:
the mutation "ignore QCC" is precisely the mutation "read the corrupted QCD".

`opj_decompress` is the oracle for both claims -- that the rewritten codestream
is legal, and that it decodes to the original samples.

Usage: python3 scripts/gen-qcc-fixture.py
Requires `opj_decompress` on PATH.
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

SOURCE = "multicomponent_lossless"
TARGET = "qcc_lossless"

SOC, SIZ, COD, QCD, QCC, SOT = 0xFF4F, 0xFF51, 0xFF52, 0xFF5C, 0xFF5D, 0xFF90


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


def segments(raw: bytes) -> list[tuple[int, int, int]]:
    """`(marker, body_start, body_end)` for each main-header segment, up to SOT."""
    out = []
    offset = 2  # past SOC
    while offset < len(raw) - 3:
        marker = struct.unpack(">H", raw[offset : offset + 2])[0]
        if marker == SOT:
            out.append((marker, offset, offset))
            break
        if 0xFF30 <= marker <= 0xFF3F:  # reserved, no segment
            offset += 2
            continue
        length = struct.unpack(">H", raw[offset + 2 : offset + 4])[0]
        out.append((marker, offset + 4, offset + 2 + length))
        offset += 2 + length
    return out


def seg_bytes(marker: int, body: bytes) -> bytes:
    return struct.pack(">HH", marker, len(body) + 2) + body


def build() -> None:
    raw = (FIXTURES / f"{SOURCE}.j2k").read_bytes()
    segs = segments(raw)
    by_marker = {m: (s, e) for m, s, e in segs}
    if QCC in by_marker or 0xFF53 in by_marker:
        sys.exit(f"{SOURCE}: already carries a COC or QCC; pick a plain source")

    csiz = struct.unpack(">H", raw[by_marker[SIZ][0] + 34 : by_marker[SIZ][0] + 36])[0]
    if csiz >= 257:
        sys.exit(f"{SOURCE}: {csiz} components would need a two-byte Cqcc")

    qcd_start, qcd_end = by_marker[QCD]
    qcd_body = raw[qcd_start:qcd_end]
    sqcd = qcd_body[0]
    style = sqcd & 0x1F
    if style != 0:
        sys.exit(f"{SOURCE}: QCD style {style}; this script only rewrites the reversible one")

    # Corrupt the default: different guard bits, and every exponent shifted. A
    # decoder that reads this instead of the QCC places every coefficient bit at
    # the wrong weight.
    guard = sqcd >> 5
    bad_guard = (guard + 2) & 0x07
    bad = bytes([(bad_guard << 5) | style]) + bytes(
        ((((b >> 3) + 4) & 0x1F) << 3) for b in qcd_body[1:]
    )
    if bad == qcd_body:
        sys.exit("the corrupted QCD is identical to the original; nothing would be graded")

    # One QCC per component, carrying QCD's original body verbatim.
    qccs = b"".join(seg_bytes(QCC, bytes([comp]) + qcd_body) for comp in range(csiz))

    rewritten = (
        raw[: qcd_start - 4] + seg_bytes(QCD, bad) + qccs + raw[qcd_end:]
    )
    out = FIXTURES / f"{TARGET}.j2k"
    out.write_bytes(rewritten)

    # The oracle: OpenJPEG must accept the rewrite and reproduce the source's
    # samples. If it does not, the rewrite is wrong, not the decoder under test.
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        run = subprocess.run(
            ["opj_decompress", "-i", str(out), "-o", str(tmp / "out.pgx")],
            capture_output=True,
            text=True,
        )
        if run.returncode != 0:
            sys.exit(f"opj_decompress rejected the rewritten codestream:\n{run.stdout}\n{run.stderr}")

        source_snapshot = json.loads((FIXTURES / f"{SOURCE}.expected.json").read_text())
        components = []
        for comp in range(csiz):
            w, h, depth, signed, values = read_pgx(tmp / f"out_{comp}.pgx")
            expected = source_snapshot["components"][comp]["samples"]
            if values != expected:
                sys.exit(
                    f"component {comp}: the rewritten codestream does not decode to "
                    f"{SOURCE}'s samples; the QCC bodies do not restore QCD's values"
                )
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
        "image": source_snapshot["image"],
        "tolerance": {"mode": "exact"},
        "components": components,
        "provenance": {
            "source": f"synthetic; generated by scripts/{Path(__file__).name} from {SOURCE}.j2k",
            "oracle_command": f"opj_decompress -i {TARGET}.j2k -o out.pgx",
            "notes": (
                f"{SOURCE}.j2k with QCD's guard bits and exponents corrupted and a QCC "
                f"restoring the original values for each of the {csiz} components. Decodes to "
                "the same image only if QCC overrides QCD; a decoder that ignores QCC reads the "
                "corrupted defaults and reconstructs garbage."
            ),
        },
    }
    (FIXTURES / f"{TARGET}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
    print(f"wrote {TARGET}.j2k + {TARGET}.expected.json ({csiz} components, {len(qccs)} QCC bytes)")


def main() -> None:
    if shutil.which("opj_decompress") is None:
        sys.exit("opj_decompress not found on PATH; install OpenJPEG to regenerate the fixture")
    build()


if __name__ == "__main__":
    main()
