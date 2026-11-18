#!/usr/bin/env python3
"""Generate the fixtures in tests/fixtures/opj_encoder_bugs/.

Two `opj_compress` 2.5.0 encodes that `opj_decompress` 2.5.0 decodes, with
exit 0, to a wrong image. This decoder refuses them in strict mode:

* `tp_mixed_subsampling.j2k`: `-TP R` with mixed sub-sampling and explicit
  precincts writes tile-parts with no packets, so one component is never
  written.
* `tp_over255.j2k`: 272 tile-parts, so `TPsot` and `TNsot` (8 bits) wrap.

The script writes each source (raw samples from a fixed LCG, each component at
its sub-sampled size, one byte per sample), encodes it, decodes the result with
`opj_decompress`, and keeps that output as the oracle (`*.opj.raw`, the
components' samples one after another). It checks that each oracle differs
from the source, and that each control (the same encode without the option
that triggers the bug) decodes to the source.

Usage: python3 scripts/gen-opj-encoder-bug-fixtures.py
Requires OpenJPEG 2.5.0 `opj_compress` and `opj_decompress` on PATH.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

from oracle_tools import require_openjpeg

DIR = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "opj_encoder_bugs"

# name -> (width, height, sub-sampling per component, opj_compress flags,
#          control flags with the triggering option removed)
CASES = {
    "tp_mixed_subsampling": (
        32,
        32,
        [(1, 2), (2, 1)],
        ["-n", "1", "-p", "CPRL", "-c", "[16,16]", "-TP", "R"],
        ["-n", "1", "-p", "CPRL", "-c", "[16,16]"],
    ),
    "tp_over255": (
        68,
        64,
        [(1, 1)],
        ["-n", "1", "-p", "RPCL", "-c", "[4,4]", "-TP", "C"],
        ["-n", "1", "-p", "RPCL", "-c", "[4,4]"],
    ),
}


def source(width: int, height: int, subs: list[tuple[int, int]]) -> bytes:
    state = 443
    out = bytearray()
    for dx, dy in subs:
        for _ in range(-(-width // dx) * -(-height // dy)):
            state = (state * 1103515245 + 12345) & 0x7FFFFFFF
            out.append((state >> 16) & 0xFF)
    return bytes(out)


def run(*argv: str) -> None:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}{result.stderr}")


def opj_decode(j2k: Path, ncomp: int, tmp: Path) -> bytes:
    """`opj_decompress` of `j2k`, the components' samples one after another."""
    base = tmp / (j2k.stem + ".out")
    run("opj_decompress", "-i", str(j2k), "-o", f"{base}.pgx")
    planes = b""
    for k in range(ncomp):
        data = Path(f"{base}_{k}.pgx").read_bytes()
        planes += data[data.index(b"\n") + 1 :]
    return planes


def main() -> None:
    require_openjpeg("opj_compress", "opj_decompress")
    DIR.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp_name:
        tmp = Path(tmp_name)
        for name, (w, h, subs, flags, control) in CASES.items():
            raw = source(w, h, subs)
            (DIR / f"{name}.source.raw").write_bytes(raw)
            spec = f"{w},{h},{len(subs)},8,u@" + ":".join(f"{x}x{y}" for x, y in subs)
            base = ["opj_compress", "-i", str(DIR / f"{name}.source.raw"), "-F", spec]
            run(*base, "-o", str(DIR / f"{name}.j2k"), *flags)
            run(*base, "-o", str(tmp / f"{name}_control.j2k"), *control)
            oracle = opj_decode(DIR / f"{name}.j2k", len(subs), tmp)
            (DIR / f"{name}.opj.raw").write_bytes(oracle)
            wrong = sum(a != b for a, b in zip(raw, oracle))
            if len(oracle) != len(raw) or wrong == 0:
                sys.exit(f"{name}: oracle is not wrong")
            if opj_decode(tmp / f"{name}_control.j2k", len(subs), tmp) != raw:
                sys.exit(f"{name}: control does not decode to the source")
            print(f"{name}: {wrong} of {len(raw)} samples wrong")


if __name__ == "__main__":
    main()
