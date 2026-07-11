#!/usr/bin/env python3
"""Generate the code-block style decode fixtures and their OpenJPEG oracles.

`p0_12` is the corpus oracle for `restart` (termination on each coding pass): it
is bit-exact, single-tile, single-layer, one component, and `restart` is its only
style bit. What it does *not* exercise is a layer contributing several coding
passes at once, where one packet must carry several codeword segments for the
same block. That is the interesting case, so it is graded here.

`opj_compress -M <mask>` sets the code-block style byte: 1 bypass, 2 reset,
4 restart, 8 vertically causal, 16 predictable termination, 32 segmentation
symbols. `restart`, `predictable termination` and `segmentation symbols` are
decoded today.

The corpus cannot grade the latter two. `p0_02` sets both (plus `restart`) and
`p0_13` sets `predictable termination` alone, but each also carries a COC marker
(#58), so both stop on COC before Tier-1 ever sees the style byte. Hence these
synthetic fixtures.

`segmentation symbols` appends four uniform-context decisions to every cleanup
pass, so it is part of the codeword: a decoder that ignores the flag falls out
of step with the MQ coder and reconstructs garbage. `predictable termination`
constrains only the encoder, so `pterm` decodes identically to the default
style -- the fixture exists to prove the flag is accepted and changes nothing,
which is a claim worth a regression test rather than a comment.

Two quality layers and two components; the last layer is lossless, so a full
decode is bit-exact. A decoder that ran the segments as one continuous MQ
codeword does not produce a rounding error -- it produces garbage.

Usage: python3 scripts/gen-cblksty-fixtures.py
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

# name -> (opj_compress -M mask, expected code-block style byte)
VARIANTS = {
    "restart": (4, 0x04),
    "segsym": (32, 0x20),
    "pterm": (16, 0x10),
    # All three at once: each cleanup pass is its own terminated segment *and*
    # ends with a segmentation symbol. This is `p0_02`'s style byte (0x34).
    "restart_pterm_segsym": (4 | 16 | 32, 0x34),
    # Vertically causal context (bit 3) and reset context probabilities (bit 1),
    # each alone. Both change the MQ context every affected decision is coded
    # under, so a decoder that ignores the flag reconstructs garbage — the golden
    # vector is bit-exact against OpenJPEG. The conformance corpus's own vcausal
    # and reset entries (`p1_02`, `p1_06`) are blocked on PPT, so these are the
    # only bit-exact grade of the two modes.
    "vcausal": (8, 0x08),
    "reset": (2, 0x02),
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


def code_block_style(codestream: Path) -> int:
    """`SPcod`'s code-block style byte straight out of COD, so a fixture cannot
    silently lose its style if OpenJPEG reinterprets a switch."""
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
            body = raw[offset + 4 : offset + 2 + length]
            # Scod(1), SGcod = prog(1) layers(2) mct(1), then SPcod:
            # levels(1), xcb(1), ycb(1), style(1), transform(1).
            return body[8]
        offset += 2 + length
    sys.exit(f"{codestream}: no COD marker")


def build(variant: str) -> None:
    mask, want_style = VARIANTS[variant]
    name = f"cblksty_{variant}_lossless"
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
            "-M", str(mask),
        ]
        run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(codestream), *flags)

        got = code_block_style(codestream)
        if got != want_style:
            sys.exit(f"{name}: COD code-block style is {got:#04x}, expected {want_style:#04x}")

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
                "notes": f"code-block style {got:#04x} ({variant}); two quality layers, "
                f"{COMPONENTS} components, LRCP -- a layer contributes several coding "
                f"passes, so one packet carries several codeword segments per block",
            },
        }
        (FIXTURES / f"{name}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
        print(f"wrote {name}.j2k + {name}.expected.json (style {got:#04x})")


def main() -> None:
    for tool in ("opj_compress", "opj_decompress"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH; install OpenJPEG to regenerate the fixtures")
    for variant in VARIANTS:
        build(variant)


if __name__ == "__main__":
    main()
