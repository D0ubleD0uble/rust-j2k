#!/usr/bin/env python3
"""Generate the precinct-partition decode fixtures and their OpenJPEG oracles.

The ISO/IEC 15444-4 corpus grades the partition through `p0_11` alone, and `p0_11`
is the weakest case there is: one component, one layer, one resolution, LRCP. It
proves the precinct *geometry* and nothing about the packet order. Every other
corpus entry with precincts (`p0_04`, `p1_01`, `p1_02`, `p1_05`, `p1_07`) is held
up by something else -- ICT, a code-block style, an image offset -- so the
partition is graded here instead.

What needs grading is the **position axis**. Packets are ordered over four axes,
and with one precinct per resolution the position axis has a single value and
drops out; that is why LRCP through CPRL all reduced to a nesting of three
counters before this. With a real partition the three positional orders --
RPCL, PCRL, CPRL -- stop being counters: a packet's place in the stream depends
on where its precinct sits on the canvas, so the decoder has to sweep the
reference grid and ask each (component, resolution) whether a precinct of its own
starts at that point (ISO B.12.1.3-B.12.1.5). Getting the sweep's step, its
lattice test, or its precinct index wrong reorders the packets, and a reordered
packet does not decode to a slightly wrong image -- it decodes to garbage.

Each fixture carries **three components and three quality layers**, the minimum
that separates the orders (see `gen-progression-fixtures.py`), and a partition
fine enough to put 17 precincts in each component: 12 at the finest resolution,
4 at the middle, 1 at the coarsest.

Two fixtures beyond the five orders:

- `varied_sizes` gives each resolution a partition of its own (2^4, 2^4, 2^5,
  2^6), the shape `p1_02` has. A decoder that reads one precinct size and applies
  it to every resolution passes the five above and fails this.
- `tiled` puts a 48x48 tile grid under a 32x32 precinct lattice. The lattice is
  anchored at the canvas origin, not at the tile, so tiles 1 and 2 begin at x=48
  and x=96 -- off-lattice -- and their **leading precinct is partial**. That is a
  separate branch in the sweep (OpenJPEG's `x == tx0` escape from the lattice
  test) and no untiled fixture reaches it.

`opj_compress -r 20,10,1` makes the last layer lossless, so a full decode is
bit-exact and the snapshot records `tolerance: exact`.

Usage: python3 scripts/gen-precinct-fixtures.py
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

WIDTH, HEIGHT, COMPONENTS = 128, 96, 3
LAYER_RATES = "20,10,1"  # three layers, the last lossless

# `-c` records run highest resolution first, and the last one is right-expanded
# to every remaining lower resolution.
UNIFORM = "[32,32],[32,32],[32,32]"

# name -> (extra opj_compress flags, expected (PPx, PPy) per resolution
#          coarsest-first as opj_dump prints them, note)
VARIANTS: dict[str, tuple[list[str], list[tuple[int, int]], str]] = {
    "precincts_lrcp_lossless": (
        ["-p", "LRCP", "-n", "3", "-c", UNIFORM],
        [(5, 5), (5, 5), (5, 5)],
        "LRCP; position is the innermost axis, so the precincts are simply counted",
    ),
    "precincts_rlcp_lossless": (
        ["-p", "RLCP", "-n", "3", "-c", UNIFORM],
        [(5, 5), (5, 5), (5, 5)],
        "RLCP; position is the innermost axis, so the precincts are simply counted",
    ),
    "precincts_rpcl_lossless": (
        ["-p", "RPCL", "-n", "3", "-c", UNIFORM],
        [(5, 5), (5, 5), (5, 5)],
        "RPCL; the position sweep runs inside the resolution loop",
    ),
    "precincts_pcrl_lossless": (
        ["-p", "PCRL", "-n", "3", "-c", UNIFORM],
        [(5, 5), (5, 5), (5, 5)],
        "PCRL; one position sweep over every component and resolution at once",
    ),
    "precincts_cprl_lossless": (
        ["-p", "CPRL", "-n", "3", "-c", UNIFORM],
        [(5, 5), (5, 5), (5, 5)],
        "CPRL; a position sweep per component. Only a real precinct partition "
        "tells this apart from PCRL",
    ),
    "precincts_varied_sizes_lossless": (
        ["-p", "RPCL", "-n", "4", "-c", "[64,64],[32,32],[16,16],[16,16]"],
        [(4, 4), (4, 4), (5, 5), (6, 6)],
        "a different precinct size at each resolution, the shape p1_02 has",
    ),
    "precincts_tiled_cprl_lossless": (
        ["-p", "CPRL", "-n", "3", "-c", UNIFORM, "-t", "48,48"],
        [(5, 5), (5, 5), (5, 5)],
        "a 3x2 grid of 48x48 tiles under a 32x32 precinct lattice: the lattice is "
        "anchored at the canvas origin, so every tile but the first begins "
        "off-lattice and its leading precinct is partial",
    ),
}


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


def cod_precincts(codestream: Path) -> tuple[int, list[tuple[int, int]]]:
    """`Scod` and COD's `SPcod` precinct sizes, straight out of the marker, so a
    fixture cannot silently regress to a maximal partition if OpenJPEG
    reinterprets a flag. Returns `(PPx, PPy)` per resolution, coarsest first."""
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
            # Scod(1) | SGcod: progression(1), layers(2), mct(1) | SPcod: NL(1),
            # xcb(1), ycb(1), style(1), transform(1), then NL+1 precinct bytes.
            scod, levels = body[0], body[5]
            sizes = [(b & 0x0F, b >> 4) for b in body[10 : 10 + levels + 1]]
            return scod, sizes
        offset += 2 + length
    sys.exit(f"{codestream}: no COD marker")


def build(name: str, flags: list[str], expected_sizes: list[tuple[int, int]], note: str) -> None:
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
            "-b", "16,16",     # code-blocks below the precinct, so neither is capped away
            "-r", LAYER_RATES,
            *flags,
        ]
        run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(codestream), *flags)

        scod, sizes = cod_precincts(codestream)
        if not scod & 0x01:
            sys.exit(f"{name}: COD does not signal an explicit precinct partition")
        if sizes != expected_sizes:
            sys.exit(f"{name}: COD declares precincts {sizes}, expected {expected_sizes}")

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
                "notes": f"{note}; precincts (PPx, PPy) per resolution = {sizes}",
            },
        }
        (FIXTURES / f"{name}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
        print(f"wrote {name}.j2k + {name}.expected.json (precincts {sizes})")


def main() -> None:
    for tool in ("opj_compress", "opj_decompress"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH; install OpenJPEG to regenerate the fixtures")
    for name, (flags, sizes, note) in VARIANTS.items():
        build(name, flags, sizes, note)


if __name__ == "__main__":
    main()
