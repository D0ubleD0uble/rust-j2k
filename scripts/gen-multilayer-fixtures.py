#!/usr/bin/env python3
"""Generate the multi-layer decode fixtures and their OpenJPEG oracles.

No ISO/IEC 15444-4 corpus entry is unblocked by quality layers alone. Fourteen of
the twenty-one not-yet-decoded entries carry more than one layer, but every one
of them also needs a progression order, a tile grid, an image origin, precincts,
SOP/EPH, or a code-block style. So the layer path is graded against synthetic
codestreams encoded by OpenJPEG, the way the multi-component path is (see
`gen-multicomponent-fixtures.py`).

`opj_compress -r 20,10,1` writes three quality layers at decreasing compression
ratios. The last ratio of 1 is lossless, so decoding *every* layer reconstructs
the input exactly and the snapshot records `tolerance: exact`. That matters: a
decoder that dropped a layer, or that mis-accumulated the coding passes of one,
would not be off by a rounding error -- it would be visibly wrong.

The second fixture crosses layers with components, because LRCP nests them
(`for layer { for resolution { for component }}`) and a decoder that carried
per-precinct state across the wrong axis would still pass the single-component
one.

Usage: python3 scripts/gen-multilayer-fixtures.py
Requires `opj_compress` and `opj_decompress` on PATH. Rewrites the `.j2k`
fixtures and their `.expected.json` snapshots under `tests/fixtures/`.
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

# Three layers; the final lossless ratio makes a full decode bit-exact.
LAYER_RATES = "20,10,1"


def sample(component: int, x: int, y: int) -> int:
    """Deterministic and non-separable, so a transposed, component-swapped, or
    layer-dropping decode cannot pass by accident."""
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


def layer_count(codestream: Path) -> int:
    """Read `SGcod`'s layer count straight out of COD, so the fixture cannot
    silently regress to one layer if OpenJPEG changes how `-r` is interpreted."""
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
            # Lcod body: Scod(1), then SGcod = progression(1), layers(2), mct(1).
            return struct.unpack(">H", body[2:4])[0]
        offset += 2 + length
    sys.exit(f"{codestream}: no COD marker")


def build(name: str, width: int, height: int, components: int, note: str) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        planes = bytearray()
        for component in range(components):
            for y in range(height):
                for x in range(width):
                    planes.append(sample(component, x, y))
        (tmp / "in.raw").write_bytes(bytes(planes))

        codestream = FIXTURES / f"{name}.j2k"
        flags = [
            "-F", f"{width},{height},{components},8,u",
            "-mct", "0",          # components stay independent: no RCT/ICT
            "-p", "LRCP",
            "-n", "3",            # 2 decomposition levels
            "-b", "16,16",
            "-r", LAYER_RATES,    # three layers, the last lossless
        ]
        run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(codestream), *flags)

        layers = layer_count(codestream)
        if layers < 2:
            sys.exit(f"{name}: COD declares {layers} layer(s); the fixture must be multi-layer")

        run("opj_decompress", "-i", str(codestream), "-o", str(tmp / "out.pgx"))

        snapshot_components = []
        for component in range(components):
            w, h, depth, signed, values = read_pgx(tmp / f"out_{component}.pgx")
            expected = [sample(component, x, y) for y in range(height) for x in range(width)]
            if (w, h) != (width, height) or values != expected:
                sys.exit(
                    f"{name}: component {component} did not round-trip losslessly; "
                    f"the last layer's rate must be 1"
                )
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
            "image": {"width": width, "height": height},
            "tolerance": {"mode": "exact"},
            "components": snapshot_components,
            "provenance": {
                "source": f"synthetic; generated by scripts/{Path(__file__).name}",
                "oracle_command": (
                    f"opj_compress -i in.raw -o {name}.j2k " + " ".join(flags)
                    + f" ; opj_decompress -i {name}.j2k -o out.pgx"
                ),
                "notes": f"{layers} quality layers; {note}",
            },
        }
        (FIXTURES / f"{name}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
        print(f"wrote {name}.j2k + {name}.expected.json ({layers} layers, {components} components)")


def main() -> None:
    for tool in ("opj_compress", "opj_decompress"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH; install OpenJPEG to regenerate the fixtures")

    build(
        "multilayer_lossless",
        width=32,
        height=24,
        components=1,
        note="one 8-bit component, reversible 5/3, LRCP, one tile, maximal precincts",
    )
    build(
        "multilayer_multicomponent_lossless",
        width=32,
        height=24,
        components=3,
        note="three 8-bit components, reversible 5/3, LRCP -- layers nest outside "
        "resolutions, which nest outside components, so this crosses both axes",
    )


if __name__ == "__main__":
    main()
