#!/usr/bin/env python3
"""Generate the layer-cap fixtures in tests/fixtures/layers/.

`tests/max_quality_layers.rs` decodes these with
`DecodeOptions::max_quality_layers` set, and compares the samples with
`opj_decompress -l N` 2.5.0 on the same bytes.

The script writes two sources from a fixed LCG (`state * 1103515245 + 12345`
mod 2^31, taking `state >> 16`): `gray.pgm`, 64 x 64 8-bit, a gradient plus
noise from -8 to 8 (seed 215), and `rgb.ppm`, 40 x 32 8-bit, three such
gradients (seed 2150). It encodes them with `opj_compress` into the codestreams
in FIXTURES, then for each one and each layer count N from 1 to its layer
count writes `<name>_l<N>_<c>.pgx`, the `opj_decompress -l N` output of
component c, and `<name>_l<N>_r<R>_<c>.pgx` for the reductions in REDUCED.
`zero_length_contribution.j2k` and `zero_length_contribution_bypass.j2k`, in
tests/fixtures/ (see `gen-zero-length-fixtures.py`), are graded the same way;
their oracles are written here.

It checks that each N decodes to other samples than N + 1, so every cap is
graded on samples it changes.

`pterm_cut.j2k` is written by hand, with the code-block writer of
`gen-zero-length-fixtures.py`: two 8 x 8 code-blocks under `predictable
termination`, each coded alone by `opj_compress -n 1 -b 8,8 -M 16`, in two
layers. Layer 0 brings 3 passes of each block with every byte of the block's
one codeword segment, and layer 1 brings the rest of the passes with a length
of zero. A cap of 1 then ends the segment 3 passes in, with bytes the decoder
of those passes does not reach, which `predictable termination` forbids of a
whole segment: the check cannot apply to a segment the cap cuts short, and
OpenJPEG does not make it under `-l`. The script checks that the full decode
is the source.

For the ISO/IEC 15444-4 codestreams in PART4 (5/3, several layers), the
decoded samples are not committed: `layers/` ships in the crate, and the
conformance corpus is excluded from it. The script writes `part4.sha256` instead: for each codestream, N and reduction, the
SHA-256 of the `opj_decompress -l N -r R` samples, every component in order,
each sample as a little-endian i32.

Usage: python3 scripts/gen-layer-fixtures.py
Requires OpenJPEG 2.5.0 `opj_compress` and `opj_decompress` on PATH.
"""

from __future__ import annotations

import hashlib
import importlib.util
import re
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

from oracle_tools import require_openjpeg

FIXTURES_ROOT = Path(__file__).resolve().parent.parent / "tests" / "fixtures"
DIR = FIXTURES_ROOT / "layers"
CORPUS = FIXTURES_ROOT / "conformance" / "codestreams"

# name -> (source, opj_compress arguments, layer count)
FIXTURES = {
    # 5/3, LRCP, four layers, the last lossless.
    "lrcp": ("gray.pgm", ["-n", "4", "-p", "LRCP", "-r", "80,20,5,1"], 4),
    # 5/3, RPCL, precincts and 16 x 16 code-blocks, three layers.
    "rpcl": ("gray.pgm", ["-n", "3", "-p", "RPCL", "-c", "[32,32],[16,16]", "-b", "16,16",
                          "-r", "40,10,1"], 3),
    # 5/3, LRCP, `bypass` and `predictable termination`: a cap ends inside
    # a segment the encoder terminated later.
    "bypass_pterm": ("gray.pgm", ["-n", "3", "-p", "LRCP", "-M", "17", "-r", "20,6,2,1"], 4),
    # 9/7 and the irreversible color transform, CPRL, three layers.
    "cprl_irreversible": ("rgb.ppm", ["-n", "3", "-p", "CPRL", "-I", "-r", "60,20,6"], 3),
    # Written by `pterm_cut` rather than encoded: see the module docs.
    "pterm_cut": (None, [], 2),
    # The fixtures of `gen-zero-length-fixtures.py`, read from
    # tests/fixtures/: a block's layer 0 passes have no bytes, and layer 1
    # brings them, so a cap of 1 decodes those passes from the fill.
    "zero_length_contribution": ("..", [], 2),
    "zero_length_contribution_bypass": ("..", [], 2),
}

# name -> [(reduction, [layer counts])]
REDUCED = {
    "lrcp": [(1, [1, 2, 3, 4])],
    "rpcl": [(2, [2])],
    "cprl_irreversible": [(1, [1])],
}

# Part 4 codestream -> (layer count, [reductions])
PART4 = {
    "p0_02": (6, [0]),  # LRCP, restart + pterm + segsym
    "p0_03": (8, [0, 1]),  # PCRL
    "p0_10": (2, [0]),  # LRCP, three components, two tiles
    "p0_16": (3, [0]),  # RLCP
    "p1_01": (5, [0]),  # LRCP, restart + pterm + segsym, precincts
}


def lcg(seed: int):
    state = seed
    while True:
        state = (state * 1103515245 + 12345) & 0x7FFFFFFF
        yield state >> 16


def gray_pgm() -> bytes:
    noise = lcg(215)
    px = bytearray()
    for y in range(64):
        for x in range(64):
            v = x * 3 + y * 2 + next(noise) % 17 - 8
            px.append(max(0, min(255, v)))
    return b"P5\n64 64\n255\n" + bytes(px)


def rgb_ppm() -> bytes:
    noise = lcg(2150)
    px = bytearray()
    for y in range(32):
        for x in range(40):
            for c in range(3):
                v = (x * (3 + c) + y * (5 - c) + 40 * c) % 256 + next(noise) % 17 - 8
                px.append(max(0, min(255, v)))
    return b"P6\n40 32\n255\n" + bytes(px)


def run(*argv: str) -> None:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}{result.stderr}")


def pgx_samples(data: bytes) -> list[int]:
    line = data.index(b"\n")
    m = re.match(rb"PG\s+(ML|LM)\s*([+-]?)\s*(\d+)\s+(\d+)\s+(\d+)", data[:line])
    if m is None:
        sys.exit(f"not a PGX header: {data[:line]!r}")
    order, sign, depth, w, h = m.groups()
    depth, w, h = int(depth), int(w), int(h)
    size = 1 if depth <= 8 else 2 if depth <= 16 else 4
    code = {1: "b", 2: "h", 4: "i"}[size]
    if sign != b"-":
        code = code.upper()
    endian = ">" if order == b"ML" else "<"
    return list(struct.unpack(f"{endian}{w * h}{code}", data[line + 1:line + 1 + w * h * size]))


def decode(path: Path, stem: Path, layers: int, reduction: int) -> list[Path]:
    """`opj_decompress -l layers -r reduction` of `path` to `<stem>_<c>.pgx`."""
    for old in stem.parent.glob(stem.name + "_*.pgx"):
        old.unlink()
    run("opj_decompress", "-i", str(path), "-o", str(stem) + ".pgx", "-l", str(layers),
        "-r", str(reduction))
    outs = sorted(stem.parent.glob(stem.name + "_*.pgx"),
                  key=lambda p: int(p.stem.rsplit("_", 1)[1]))
    if not outs:
        sys.exit(f"opj_decompress wrote no PGX for {path}")
    return outs


def samples_of(outs: list[Path]) -> bytes:
    return b"".join(struct.pack(f"<{len(v)}i", *v)
                    for v in (pgx_samples(p.read_bytes()) for p in outs))


def pterm_cut(tmp: Path) -> None:
    """Write `pterm_cut.j2k`, built by hand (see the module docs)."""
    spec = importlib.util.spec_from_file_location(
        "zero_length", Path(__file__).resolve().parent / "gen-zero-length-fixtures.py")
    zl = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(zl)
    pterm = 0x10
    plans = [[(3, "a"), (zl.REST, "z")], [(3, "a"), (zl.REST, "z")]]
    width = zl.BLOCK * len(plans)
    image = zl.source(width)
    blocks, qcd = [], b""
    for b in range(len(plans)):
        pixels = bytes(image[y * width + b * zl.BLOCK + x]
                       for y in range(zl.BLOCK) for x in range(zl.BLOCK))
        pgm = tmp / f"block{b}.pgm"
        pgm.write_bytes(b"P5\n8 8\n255\n" + pixels)
        j2k = tmp / f"block{b}.j2k"
        run("opj_compress", "-i", str(pgm), "-o", str(j2k), "-n", "1", "-b", "8,8", "-M", "16")
        raw = j2k.read_bytes()
        if zl.marker_segment(raw, 0xFF52)[4 + 8] != pterm:
            sys.exit("pterm_cut: opj_compress -M 16 did not set predictable termination")
        qcd = zl.marker_segment(raw, 0xFF5C)
        blocks.append(zl.parse_block(raw, pterm))
    codestream, complete = zl.build(pterm, qcd, width, blocks, plans)
    if not all(complete):
        sys.exit("pterm_cut: a block lacks bytes")
    out = DIR / "pterm_cut.j2k"
    out.write_bytes(codestream)
    run("opj_decompress", "-i", str(out), "-o", str(tmp / "full.pgx"))
    if bytes(zl.read_pgx(tmp / "full_0.pgx", width)) != image:
        sys.exit("pterm_cut: the full decode is not the source")


def main() -> None:
    require_openjpeg("opj_compress", "opj_decompress")
    DIR.mkdir(exist_ok=True)
    for old in DIR.glob("*.pgx"):
        old.unlink()
    (DIR / "gray.pgm").write_bytes(gray_pgm())
    (DIR / "rgb.ppm").write_bytes(rgb_ppm())
    with tempfile.TemporaryDirectory() as tmp:
        pterm_cut(Path(tmp))
    for name, (source, args, layers) in FIXTURES.items():
        j2k = DIR / f"{name}.j2k"
        if source == "..":
            j2k = FIXTURES_ROOT / f"{name}.j2k"
        elif source is not None:
            run("opj_compress", "-i", str(DIR / source), "-o", str(j2k), *args)
        plans = [(0, list(range(1, layers + 1)))] + REDUCED.get(name, [])
        for reduction, counts in plans:
            previous = None
            for n in counts:
                tag = f"{name}_l{n}" + (f"_r{reduction}" if reduction else "")
                got = samples_of(decode(j2k, DIR / tag, n, reduction))
                if reduction == 0 and previous == got:
                    sys.exit(f"{name}: -l {n} decodes as -l {n - 1}")
                previous = got
        print(f"{name}.j2k: {j2k.stat().st_size} bytes, {layers} layers")

    lines = []
    with tempfile.TemporaryDirectory() as tmp:
        for name, (layers, reductions) in PART4.items():
            path = CORPUS / f"{name}.j2k"
            for reduction in reductions:
                for n in range(1, layers + 1):
                    got = samples_of(decode(path, Path(tmp) / "out", n, reduction))
                    lines.append(f"{name} {n} {reduction} {hashlib.sha256(got).hexdigest()}")
    (DIR / "part4.sha256").write_text("\n".join(lines) + "\n")
    print(f"part4.sha256: {len(lines)} oracles")


if __name__ == "__main__":
    main()
