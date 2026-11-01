#!/usr/bin/env python3
"""Generate the synthetic JP2 fixtures and their OpenJPEG oracles.

Each fixture wraps a lossless codestream in hand-built JP2 boxes (signature,
`ftyp`, `jp2h` with `ihdr` + `colr` + the boxes under test, `jp2c`), so it
isolates one box-layer feature over samples this repo already trusts. The
oracle is `opj_decompress` on the `.jp2`; its PGX output becomes
`<name>.expected.json` with the same schema as the codestream fixtures.

Palette fixtures (`pclr` + `cmap`):

- `jp2_palette_8bit`: 256 entries x 3 eight-bit columns over the one-component
  `multilayer_lossless.j2k`.
- `jp2_palette_12bit`: the same with 12-bit columns, so each entry is two bytes.
- `jp2_palette_mixed`: over the three-component `multicomponent_lossless.j2k`,
  channel 0 is component 2 as coded and channels 1 and 2 look up component 0.
- `jp2_palette_clamp`: a signed 8-bit index plane (generated here with
  `opj_compress`) over a 100-entry palette, so indices fall below 0 and past 99
  and are clamped.
- `jp2_palette_corrected`: a one-component file whose `cmap` makes channel 1
  direct, which leaves column 1 unused; OpenJPEG then reads the mapping as the
  identity ("Component mapping seems wrong. Trying to correct.").
- `jp2_palette_subsampled_big_first` and `jp2_palette_subsampled_small_first`:
  a two-component codestream (generated here; component 0 at full resolution,
  component 1 sub-sampled 2x2) with one palette and one direct channel, in
  both orders. OpenJPEG 2.5.0 sizes every channel by the first one's grid and
  crashes or emits uninitialised memory, so the oracle is OpenJPEG's decode of
  the bare codestream, with the palette applied by table lookup here. Each
  channel keeps its own component's grid (an owner-approved deviation).

Usage: python3 scripts/gen-jp2-fixtures.py
Requires `opj_compress` and `opj_decompress` on PATH (OpenJPEG 2.5.0 was used).
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
SCRIPT = f"scripts/{Path(__file__).name}"

SIGNATURE = bytes.fromhex("0000000c6a5020200d0a870a")


def box(kind: bytes, payload: bytes) -> bytes:
    return struct.pack(">I", 8 + len(payload)) + kind + payload


def ihdr(height: int, width: int, components: int, depth: int, signed: bool) -> bytes:
    bpc = (depth - 1) | (0x80 if signed else 0)
    return box(b"ihdr", struct.pack(">IIHBBBB", height, width, components, bpc, 7, 0, 0))


FTYP = box(b"ftyp", b"jp2 " + b"\0\0\0\0" + b"jp2 ")
COLR_SRGB = box(b"colr", bytes([1, 0, 0]) + struct.pack(">I", 16))


def pclr(entries: int, bi: list[int], value) -> bytes:
    """A Palette box; entry j of column i is `value(j, i)` in ceil(depth/8) bytes."""
    out = struct.pack(">HB", entries, len(bi)) + bytes(bi)
    for j in range(entries):
        for i, b in enumerate(bi):
            width = ((b & 0x7F) + 1 + 7) // 8
            out += value(j, i).to_bytes(width, "big")
    return box(b"pclr", out)


def cmap(channels: list[tuple[int, int, int]]) -> bytes:
    """A Component Mapping box from (CMP, MTYP, PCOL) triples."""
    return box(b"cmap", b"".join(struct.pack(">HBB", *c) for c in channels))


def jp2(codestream: bytes, geometry: dict, children: list[bytes]) -> bytes:
    header = ihdr(
        geometry["height"], geometry["width"], geometry["components"],
        geometry["depth"], geometry["signed"],
    )
    return SIGNATURE + FTYP + box(b"jp2h", header + COLR_SRGB + b"".join(children)) + box(
        b"jp2c", codestream
    )


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


def run(*args: str) -> subprocess.CompletedProcess:
    result = subprocess.run(args, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{' '.join(args)} failed:\n{result.stdout}\n{result.stderr}")
    return result


def opj_decode(data: bytes, suffix: str) -> tuple[list[dict], str]:
    """Decode `data` with opj_decompress; return the PGX planes and its log."""
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        source = tmp / f"in{suffix}"
        source.write_bytes(data)
        result = run("opj_decompress", "-i", str(source), "-o", str(tmp / "out.pgx"))
        planes = sorted(tmp.glob("out*.pgx"), key=lambda p: p.name)
        components = []
        for plane in planes:
            w, h, depth, signed, values = read_pgx(plane)
            check_container(plane.name, depth, signed, values)
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
        return components, result.stdout + result.stderr


def check_container(name: str, depth: int, signed: bool, values: list[int]) -> None:
    """Refuse a reference that may have been clamped. `imagetopgx` clamps each
    sample to the PGX container (1, 2 or 4 bytes), not to the declared depth,
    so a sample sitting on a container bound outside the declared range may be
    a clamp, not the decoder's value."""
    bytes_ = 1 if depth <= 8 else (2 if depth <= 16 else 4)
    lo, hi = (-(1 << (8 * bytes_ - 1)), (1 << (8 * bytes_ - 1)) - 1) if signed else (0, (1 << (8 * bytes_)) - 1)
    in_lo, in_hi = (-(1 << (depth - 1)), (1 << (depth - 1)) - 1) if signed else (0, (1 << depth) - 1)
    for v in values:
        if v in (lo, hi) and not in_lo <= v <= in_hi:
            sys.exit(f"{name}: sample {v} sits on the PGX container bound outside the "
                     f"declared {depth}-bit range; opj_decompress may have clamped it")


def entry8(j: int, i: int) -> int:
    """No column is the identity, so a direct channel never agrees by accident."""
    return (j * (i + 1) + 40 * i + 17) & 0xFF


def entry12(j: int, i: int) -> int:
    return (j * 16 * (i + 1) + 300 * i + 5) & 0xFFF


def signed_index_codestream(tmp: Path) -> tuple[bytes, str]:
    """A 32x24 signed 8-bit plane running over -128..127, coded losslessly."""
    width, height = 32, 24
    values = [((x * 7 + y * 13) % 256) - 128 for y in range(height) for x in range(width)]
    (tmp / "signed.raw").write_bytes(bytes(v & 0xFF for v in values))
    flags = ["-F", f"{width},{height},1,8,s", "-n", "3"]
    run("opj_compress", "-i", str(tmp / "signed.raw"), "-o", str(tmp / "signed.j2k"), *flags)
    codestream = (tmp / "signed.j2k").read_bytes()
    decoded, _ = opj_decode(codestream, ".j2k")
    if decoded[0]["samples"] != values:
        sys.exit("the signed index plane did not round-trip losslessly")
    command = "opj_compress -i signed.raw -o signed.j2k " + " ".join(flags)
    return codestream, command


def write(name: str, data: bytes, geometry: dict, components: list[dict], provenance: dict) -> None:
    (FIXTURES / f"{name}.jp2").write_bytes(data)
    snapshot = {
        "image": {"width": geometry["width"], "height": geometry["height"]},
        "tolerance": {"mode": "exact"},
        "components": components,
        "provenance": provenance,
        # Every fixture carries COLR_SRGB and a palette that applies, with no
        # `res `, `xml ` or `uuid` box: one colour channel per palette column.
        "jp2": {
            "colour": "srgb",
            "channels": [f"colour/{i + 1}" for i in range(len(components))],
            "palette_applied": True,
            "resolution": {"capture": None, "display": None},
            "xml": [],
            "uuid": [],
        },
    }
    (FIXTURES / f"{name}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
    print(f"wrote {name}.jp2 + {name}.expected.json ({len(components)} channels)")


def oracle(name: str, data: bytes, geometry: dict, source: str, notes: str, expect_log: str = "") -> None:
    components, log = opj_decode(data, ".jp2")
    if expect_log and expect_log not in log:
        sys.exit(f"{name}: expected opj_decompress to say {expect_log!r}; it said:\n{log}")
    write(
        name,
        data,
        geometry,
        components,
        {
            "source": f"{source}, wrapped by {SCRIPT}",
            "oracle_command": f"opj_decompress -i {name}.jp2 -o out.pgx",
            "notes": notes,
        },
    )


def subsampled_codestream(tmp: Path) -> tuple[bytes, str]:
    """A 24x16 image: component 0 full resolution, component 1 sub-sampled 2x2."""
    full = bytes((x * 11 + y * 7) % 256 for y in range(16) for x in range(24))
    half = bytes((x * 29 + y * 5 + 100) % 256 for y in range(8) for x in range(12))
    (tmp / "sub.raw").write_bytes(full + half)
    flags = ["-F", "24,16,2,8,u@1x1:2x2", "-n", "2"]
    run("opj_compress", "-i", str(tmp / "sub.raw"), "-o", str(tmp / "sub.j2k"), *flags)
    codestream = (tmp / "sub.j2k").read_bytes()
    decoded, _ = opj_decode(codestream, ".j2k")
    if [c["samples"] for c in decoded] != [list(full), list(half)]:
        sys.exit("the sub-sampled codestream did not round-trip losslessly")
    return codestream, "opj_compress -i sub.raw -o sub.j2k " + " ".join(flags)


def subsampled_fixtures() -> None:
    """Palette and direct channels from components of different sub-sampling."""
    with tempfile.TemporaryDirectory() as tmp:
        codestream, compress = subsampled_codestream(Path(tmp))
    geometry = {"width": 24, "height": 16, "components": 2, "depth": 8, "signed": False}
    planes, _ = opj_decode(codestream, ".j2k")
    planes[1]["x_sampling"] = planes[1]["y_sampling"] = 2
    palette = pclr(256, [7, 7], entry8)
    for name, mapping in [
        ("jp2_palette_subsampled_big_first", [(0, 1, 0), (1, 0, 0)]),
        ("jp2_palette_subsampled_small_first", [(1, 1, 0), (0, 0, 0)]),
    ]:
        data = jp2(codestream, geometry, [palette, cmap(mapping)])
        channels = []
        for i, (component, mtyp, _) in enumerate(mapping):
            channel = dict(planes[component])
            if mtyp == 1:
                channel["samples"] = [entry8(min(max(k, 0), 255), i) for k in channel["samples"]]
            channels.append(channel)
        write(
            name,
            data,
            geometry,
            channels,
            {
                "source": f"synthetic two-component plane pair, coded with `{compress}`, "
                f"wrapped by {SCRIPT}",
                "oracle_command": f"opj_decompress -i sub.j2k -o out.pgx, then cmap "
                f"{mapping} applied by table lookup in Python",
                "notes": "component 0 is full resolution, component 1 sub-sampled 2x2. "
                "OpenJPEG 2.5.0 sizes every palette channel by the first channel's grid and "
                "crashes or emits uninitialised memory on this file, so it is graded by "
                "table lookup over OpenJPEG's decode of the bare codestream; each channel "
                "keeps its source component's grid (owner-approved deviation, 2026-11-01)",
            },
        )


def main() -> None:
    for tool in ("opj_compress", "opj_decompress"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH; install OpenJPEG to regenerate the fixtures")

    one = (FIXTURES / "multilayer_lossless.j2k").read_bytes()
    one_geometry = {"width": 32, "height": 24, "components": 1, "depth": 8, "signed": False}
    three = (FIXTURES / "multicomponent_lossless.j2k").read_bytes()
    three_geometry = {"width": 24, "height": 16, "components": 3, "depth": 8, "signed": False}
    identity = cmap([(0, 1, 0), (0, 1, 1), (0, 1, 2)])
    palette8 = pclr(256, [7, 7, 7], entry8)

    oracle(
        "jp2_palette_8bit",
        jp2(one, one_geometry, [palette8, identity]),
        one_geometry,
        "tests/fixtures/multilayer_lossless.j2k",
        "pclr 256 entries x 3 columns, 8-bit; cmap channel i = component 0 through column i",
    )
    oracle(
        "jp2_palette_12bit",
        jp2(one, one_geometry, [pclr(256, [11, 11, 11], entry12), identity]),
        one_geometry,
        "tests/fixtures/multilayer_lossless.j2k",
        "pclr 256 entries x 3 columns, 12-bit (two bytes per entry); identity cmap",
    )
    oracle(
        "jp2_palette_mixed",
        jp2(three, three_geometry, [palette8, cmap([(2, 0, 0), (0, 1, 1), (0, 1, 2)])]),
        three_geometry,
        "tests/fixtures/multicomponent_lossless.j2k",
        "cmap mixes direct and palette channels: channel 0 = component 2 as coded, "
        "channels 1 and 2 = component 0 through columns 1 and 2; column 0 is 8-bit like "
        "component 2, since OpenJPEG labels a direct channel i with column i's depth",
    )
    oracle(
        "jp2_palette_corrected",
        jp2(one, one_geometry, [palette8, cmap([(0, 1, 0), (0, 0, 0), (0, 1, 2)])]),
        one_geometry,
        "tests/fixtures/multilayer_lossless.j2k",
        "one component, cmap channel 1 direct, so column 1 is unused: OpenJPEG warns "
        "'Component mapping seems wrong. Trying to correct.' and applies the identity mapping",
        expect_log="Trying to correct",
    )

    with tempfile.TemporaryDirectory() as tmp:
        signed, compress = signed_index_codestream(Path(tmp))
    signed_geometry = {"width": 32, "height": 24, "components": 1, "depth": 8, "signed": True}
    clamp = jp2(signed, signed_geometry, [pclr(100, [7, 7, 7], entry8), identity])
    components, _ = opj_decode(clamp, ".jp2")
    write(
        "jp2_palette_clamp",
        clamp,
        signed_geometry,
        components,
        {
            "source": f"synthetic signed 8-bit plane, coded with `{compress}`, wrapped by {SCRIPT}",
            "oracle_command": "opj_decompress -i jp2_palette_clamp.jp2 -o out.pgx",
            "notes": "pclr of 100 entries over indices -128..127: below 0 clamps to entry 0, "
            "past 99 to entry 99",
        },
    )

    subsampled_fixtures()


if __name__ == "__main__":
    main()
