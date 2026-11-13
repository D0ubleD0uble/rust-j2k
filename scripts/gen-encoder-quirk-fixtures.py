#!/usr/bin/env python3
"""Generate the encoder-quirk fixtures (issue #282), with their OpenJPEG oracles.

Three quirks that real encoders write and OpenJPEG 2.5.0 corrects on purpose.
Each fixture is an `opj_compress` encode, rewritten afterwards to carry the
quirk, so the packets are OpenJPEG's own and the oracle is `opj_decompress`.
All three are lossless 5/3, and the script checks that the oracle equals the
source image, so `tolerance: exact` also grades against the source.

- `quirk_tpsot_tnsot_lossless`: every tile's parts are numbered 0 to N and
  N declared (`TPsot == TNsot` on the last), interleaved across the tiles, as
  Adobe Photoshop's plug-in writes them (openjpeg-data `issue254.jp2`). OpenJPEG
  raises every count by one (`opj_j2k_need_nb_tile_parts_correction`).
- `quirk_restated_cod_qcd_lossless`: the main header's COD and QCD repeated in
  every later tile-part, as BAP Image Systems writes them (openjpeg-data
  `db11217111510058.jp2`). OpenJPEG reads a COD or QCD in any tile-part header.
- `quirk_mct_one_component_lossless`: one component with COD's color transform
  flag set (openjpeg-data `123.j2c`). OpenJPEG skips the transform
  (`opj_tcd_mct_decode`).

The openjpeg-data files are not committed (their licenses vary); these are.

Usage: python3 scripts/gen-encoder-quirk-fixtures.py
Requires `opj_compress` and `opj_decompress` on PATH.
"""

from __future__ import annotations

import json
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

from oracle_tools import require_openjpeg

FIXTURES = Path(__file__).resolve().parent.parent / "tests" / "fixtures"
SOT, SOD, COD, QCD, EOC = 0xFF90, 0xFF93, 0xFF52, 0xFF5C, 0xFFD9


def sample(component: int, x: int, y: int) -> int:
    return (29 * component + 7 * x + 13 * y + ((x * y) % 5) * 17) % 256


def run(*argv: str) -> str:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}\n{result.stderr}")
    return result.stdout + result.stderr


def read_pgx(path: Path) -> tuple[int, int, int, list[int]]:
    raw = path.read_bytes()
    end = raw.index(b"\n")
    tokens = raw[:end].split()
    # `PG ML + 8 W H`, or `PG ML +8 W H`.
    rest = tokens[2:]
    if rest[0] in (b"+", b"-"):
        rest = [rest[0] + rest[1]] + rest[2:]
    depth, width, height = abs(int(rest[0])), int(rest[1]), int(rest[2])
    assert depth <= 8, "8-bit fixtures only"
    return width, height, depth, list(raw[end + 1 : end + 1 + width * height])


def main_header_end(cs: bytes) -> int:
    """Offset of the first SOT."""
    at = 2
    while True:
        marker, length = struct.unpack(">HH", cs[at : at + 4])
        if marker == SOT:
            return at
        at += 2 + length


def main_segment(cs: bytes, code: int) -> bytes:
    """The whole main-header segment with marker `code`."""
    at = 2
    while True:
        marker, length = struct.unpack(">HH", cs[at : at + 4])
        if marker == SOT:
            sys.exit(f"no main-header segment {code:#06x}")
        if marker == code:
            return cs[at : at + 2 + length]
        at += 2 + length


def tile_parts(cs: bytes) -> list[dict]:
    """Each tile-part: Isot, TPsot, TNsot, header markers (bytes), data."""
    parts = []
    at = main_header_end(cs)
    while struct.unpack(">H", cs[at : at + 2])[0] == SOT:
        _, _, isot, psot, tpsot, tnsot = struct.unpack(">HHHIBB", cs[at : at + 12])
        assert psot != 0, "opj_compress states every Psot"
        end = at + psot
        sod = at + 12
        while struct.unpack(">H", cs[sod : sod + 2])[0] != SOD:
            sod += 2 + struct.unpack(">H", cs[sod + 2 : sod + 4])[0]
        parts.append(
            {
                "isot": isot,
                "tpsot": tpsot,
                "tnsot": tnsot,
                "markers": cs[at + 12 : sod],
                "data": cs[sod + 2 : end],
            }
        )
        at = end
    assert struct.unpack(">H", cs[at : at + 2])[0] == EOC
    return parts


def assemble(cs: bytes, parts: list[dict]) -> bytes:
    out = bytearray(cs[: main_header_end(cs)])
    for p in parts:
        psot = 12 + len(p["markers"]) + 2 + len(p["data"])
        out += struct.pack(">HHHIBB", SOT, 10, p["isot"], psot, p["tpsot"], p["tnsot"])
        out += p["markers"] + struct.pack(">H", SOD) + p["data"]
    out += struct.pack(">H", EOC)
    return bytes(out)


def tpsot_tnsot(cs: bytes) -> tuple[bytes, str]:
    parts = tile_parts(cs)
    for p in parts:
        assert p["tnsot"] == 3, p
        p["tnsot"] = 2
    parts.sort(key=lambda p: (p["tpsot"], p["isot"]))
    return assemble(cs, parts), (
        "every TNsot lowered from 3 to 2, so each tile's last part has TPsot == TNsot; "
        "the parts interleaved across the tiles in (TPsot, Isot) order"
    )


def restated_cod_qcd(cs: bytes) -> tuple[bytes, str]:
    restated = main_segment(cs, COD) + main_segment(cs, QCD)
    parts = tile_parts(cs)
    for p in parts:
        if p["tpsot"] > 0:
            assert not p["markers"], p
            p["markers"] = restated
    return assemble(cs, parts), (
        "the main header's COD and QCD segments copied, byte for byte, into the header "
        "of every tile-part after the first"
    )


def mct_one_component(cs: bytes) -> tuple[bytes, str]:
    cod = cs.index(main_segment(cs, COD))
    # COD: marker (2), Lcod (2), Scod, then SGcod: progression, layers (2), MCT.
    mct = cod + 8
    assert cs[mct] == 0, "opj_compress sets no color transform on one component"
    return cs[:mct] + b"\x01" + cs[mct + 1 :], "COD's SGcod MCT byte set from 0 to 1"


VARIANTS = {
    # name: (width, height, components, encoder flags, rewrite)
    "quirk_tpsot_tnsot_lossless": (
        32, 32, 3, ["-t", "16,16", "-n", "3", "-TP", "R"], tpsot_tnsot,
    ),
    "quirk_restated_cod_qcd_lossless": (
        24, 16, 3, ["-n", "3", "-TP", "R"], restated_cod_qcd,
    ),
    "quirk_mct_one_component_lossless": (24, 16, 1, ["-n", "3"], mct_one_component),
}


def build(name: str) -> None:
    width, height, components, switches, rewrite = VARIANTS[name]
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        planes = bytearray(
            sample(c, x, y) for c in range(components) for y in range(height) for x in range(width)
        )
        (tmp / "in.raw").write_bytes(bytes(planes))
        flags = ["-F", f"{width},{height},{components},8,u", "-b", "8,8", *switches]
        run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(tmp / "plain.j2k"), *flags)
        quirk, how = rewrite((tmp / "plain.j2k").read_bytes())
        codestream = FIXTURES / f"{name}.j2k"
        codestream.write_bytes(quirk)

        log = run("opj_decompress", "-i", str(codestream), "-o", str(tmp / "out.pgx"))
        warnings = sorted({l.strip() for l in log.splitlines() if "WARNING" in l or "ERROR" in l})

        snapshot_components = []
        for c in range(components):
            w, h, depth, values = read_pgx(tmp / f"out_{c}.pgx")
            expected = [sample(c, x, y) for y in range(height) for x in range(width)]
            if (w, h) != (width, height) or values != expected:
                sys.exit(f"{name}: component {c} does not match the source")
            snapshot_components.append(
                {
                    "width": w,
                    "height": h,
                    "bit_depth": depth,
                    "signed": False,
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
                    f"opj_compress -i in.raw -o plain.j2k {' '.join(flags)} ; "
                    f"rewrite: {how} ; opj_decompress -i {name}.j2k -o out.pgx"
                ),
                "notes": "opj_decompress 2.5.0 says: " + (" / ".join(warnings) or "nothing")
                + "; its output equals the source image",
            },
        }
        (FIXTURES / f"{name}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
        print(f"wrote {name}.j2k + .expected.json; opj_decompress: {warnings or 'no warning'}")


def main() -> None:
    require_openjpeg("opj_compress", "opj_decompress")
    for name in VARIANTS:
        build(name)


if __name__ == "__main__":
    main()
