#!/usr/bin/env python3
"""Generate the multi-component decode fixtures and their OpenJPEG oracles.

The ISO/IEC 15444-4 corpus cannot grade multi-component decoding on its own:
every one of its multi-component entries also needs a feature that is not
decoded yet (a progression other than LRCP, several quality layers, a tile grid,
an image origin, or the reversible colour transform). `p0_14` comes closest and
still needs RCT. So the multi-component path is graded against synthetic
codestreams encoded by OpenJPEG, with `opj_decompress` supplying the per-component
oracle -- the same arrangement Phase 1 uses for its GRIB2 fixtures.

Both fixtures are reversible (5/3), so the decode is bit-exact and the snapshot
records `tolerance: exact`.

A trap worth recording. OpenJPEG's raw reader takes per-component sub-sampling as
`-F w,h,n,depth,u@1x1:2x2:2x2`, which is how a 4:2:0 image would be described.
That path is broken in the OpenJPEG this was generated with: the encoder writes a
SIZ whose sub-sampled components are correctly sized, but encodes them at full
size, and `opj_decompress` then emits every component at the image size. Feeding
it constant planes (100 / 150 / 200) returns 200 / 42 / 138. Do not use it.
`-s <dx>,<dy>` applies one sub-sampling factor to every component and round-trips
bit-exactly, so the sub-sampled fixture uses that instead.

The consequence is that *mixed* per-component sub-sampling has no end-to-end
oracle here. Its geometry is covered by `Siz::component_extent` unit tests, and
the corpus grades it once the progression and layer milestones land (`p0_05`,
`p0_06`, `p1_07`).

Usage: python3 scripts/gen-multicomponent-fixtures.py
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


def sample(component: int, x: int, y: int) -> int:
    """A deterministic, non-separable pattern, so a transposed or component-swapped
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


def build(name: str, width: int, height: int, components: int, subsampling: int, note: str) -> None:
    """Encode a `components`-component image and snapshot OpenJPEG's decode of it.

    `subsampling` is the factor applied to every component via `-s`. With `-s d,d`
    the reference grid becomes `(width - 1) * d + 1` wide, so the image extent and
    the component extent differ and the ceil in the reference-grid equations bites.
    """
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
            # No `-c`: passing precinct sizes sets the Scod "user-defined
            # precincts" bit even at the maximal size, which the decoder rejects.
            # Omitting it leaves the default maximal precincts unsignalled.
        ]
        if subsampling != 1:
            flags += ["-s", f"{subsampling},{subsampling}"]

        encode = ["opj_compress", "-i", str(tmp / "in.raw"), "-o", str(codestream)] + flags
        run(*encode)

        # Reproducible without the temporary paths: `in.raw` is the planar
        # component data this script writes, one plane per component.
        oracle_command = (
            f"opj_compress -i in.raw -o {name}.j2k " + " ".join(flags)
            + f" ; opj_decompress -i {name}.j2k -o out.pgx"
        )

        run("opj_decompress", "-i", str(codestream), "-o", str(tmp / "out.pgx"))

        snapshot_components = []
        for component in range(components):
            w, h, depth, signed, values = read_pgx(tmp / f"out_{component}.pgx")
            expected = [sample(component, x, y) for y in range(height) for x in range(width)]
            if (w, h) != (width, height) or values != expected:
                sys.exit(
                    f"{name}: component {component} did not round-trip losslessly "
                    f"({w}x{h}); the encoder or the raw layout is wrong"
                )
            snapshot_components.append(
                {
                    "width": w,
                    "height": h,
                    "bit_depth": depth,
                    "signed": signed,
                    "x_sampling": subsampling,
                    "y_sampling": subsampling,
                    "samples": values,
                }
            )

        # The reference grid: `-s d,d` places the last sample at (width-1)*d.
        grid_width = (width - 1) * subsampling + 1
        grid_height = (height - 1) * subsampling + 1

        snapshot = {
            "image": {"width": grid_width, "height": grid_height},
            "tolerance": {"mode": "exact"},
            "components": snapshot_components,
            "provenance": {
                "source": f"synthetic; generated by scripts/{Path(__file__).name}",
                "oracle_command": oracle_command,
                "notes": note,
            },
        }
        (FIXTURES / f"{name}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
        print(f"wrote {name}.j2k + {name}.expected.json ({components} components)")


def main() -> None:
    for tool in ("opj_compress", "opj_decompress"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH; install OpenJPEG to regenerate the fixtures")

    build(
        "multicomponent_lossless",
        width=24,
        height=16,
        components=3,
        subsampling=1,
        note="three independent 8-bit components at unit sub-sampling, reversible 5/3, "
        "LRCP, one layer, one tile, maximal precincts, no MCT",
    )
    build(
        "multicomponent_subsampled_lossless",
        width=12,
        height=8,
        components=3,
        subsampling=2,
        note="three 8-bit components sub-sampled 2x2, so the 23x15 reference grid "
        "differs from each 12x8 component grid; reversible 5/3, LRCP, one layer, "
        "one tile, maximal precincts, no MCT",
    )


if __name__ == "__main__":
    main()
