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

Channel definition fixtures (`cdef`):

- `jp2_cdef_rgba`: `opj_compress` output for a 16x12 RGBA PNG (generated
  here), which writes a `cdef` of three colours and one opacity channel.
- `jp2_cdef_alpha_first`: the same codestream with a `cdef` that names
  component 0 as opacity and components 1..3 as colours 1..3, so every colour
  entry swaps and the opacity channel ends up last.
- `jp2_cdef_reversed`: file2's layout (components 0, 1, 2 are colours 3, 2,
  1) over `multicomponent_lossless.j2k`, so the channels come out reversed.
- `jp2_cdef_unspecified`: every channel `Typ 65535`, `Asoc 65535` (the ECW
  SDK's layout), so nothing moves.
- `jp2_cdef_palette`: the 8-bit palette over `multilayer_lossless.j2k` with
  the reversed `cdef`, which applies to the palette's channels.

Every `cdef` fixture is three or four components at full resolution with an
sRGB `colr`, so `opj_decompress` does not convert sYCC by its sub-sampling
heuristic, and the oracle is its decode of the `.jp2` itself.

Usage: python3 scripts/gen-jp2-fixtures.py
Requires `opj_compress` and `opj_decompress` on PATH (OpenJPEG 2.5.0 was used).
"""

from __future__ import annotations

import json
import shutil
import zlib
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


def cdef(entries: list[tuple[int, int, int]]) -> bytes:
    """A Channel Definition box from (Cn, Typ, Asoc) triples."""
    return box(b"cdef", struct.pack(">H", len(entries)) + b"".join(struct.pack(">HHH", *e) for e in entries))


def top_boxes(data: bytes) -> list[tuple[bytes, bytes]]:
    """(type, payload) of each top-level box after the signature; 32-bit lengths only."""
    out, at = [], len(SIGNATURE)
    while at < len(data):
        length, kind = struct.unpack(">I4s", data[at : at + 8])
        out.append((kind, data[at + 8 : at + length]))
        at += length
    return out


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


def opj_decode(
    data: bytes, suffix: str, sampling: list[tuple[int, int]] | None = None, verified: bool = False
) -> tuple[list[dict], str]:
    """Decode `data` with opj_decompress; return the PGX planes and its log.

    PGX does not record sub-sampling, so `sampling` gives each plane's
    `(dx, dy)`; without it every plane must be the same size, and is taken to
    be at full resolution. `verified` skips the clamp guard for planes the
    caller compares with known values itself."""
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        source = tmp / f"in{suffix}"
        source.write_bytes(data)
        result = run("opj_decompress", "-i", str(source), "-o", str(tmp / "out.pgx"))
        planes = sorted(tmp.glob("out*.pgx"), key=lambda p: int(p.stem.rsplit("_", 1)[1]))
        if sampling is not None and len(sampling) != len(planes):
            sys.exit(f"{len(planes)} planes decoded but {len(sampling)} samplings given")
        components = []
        for k, plane in enumerate(planes):
            w, h, depth, signed, values = read_pgx(plane)
            if not verified:
                check_container(plane.name, depth, signed, values)
            dx, dy = sampling[k] if sampling is not None else (1, 1)
            components.append(
                {
                    "width": w,
                    "height": h,
                    "bit_depth": depth,
                    "signed": signed,
                    "x_sampling": dx,
                    "y_sampling": dy,
                    "samples": values,
                }
            )
        if sampling is None and len({(c["width"], c["height"]) for c in components}) > 1:
            sys.exit("planes of different sizes: pass their sub-sampling to opj_decode")
        return components, result.stdout + result.stderr


def check_container(name: str, depth: int, signed: bool, values: list[int]) -> None:
    """Refuse a reference that may have been clamped. `imagetopgx` clamps each
    sample to the PGX container (1, 2 or 4 bytes), not to the declared depth,
    so a sample sitting on a container bound outside the declared range may be
    a clamp, not the decoder's value. A signed 8- or 16-bit plane fills its
    container, so its bounds are in range and a clamp onto them cannot be told
    apart: any sample on them is refused too. (A full unsigned 8- or 16-bit
    plane has the same blind spot, but 0 and 255 are ordinary values there;
    the guard does not cover it.)"""
    bytes_ = 1 if depth <= 8 else (2 if depth <= 16 else 4)
    lo, hi = (-(1 << (8 * bytes_ - 1)), (1 << (8 * bytes_ - 1)) - 1) if signed else (0, (1 << (8 * bytes_)) - 1)
    in_lo, in_hi = (-(1 << (depth - 1)), (1 << (depth - 1)) - 1) if signed else (0, (1 << depth) - 1)
    fills = signed and depth == 8 * bytes_
    for v in values:
        if v in (lo, hi) and (fills or not in_lo <= v <= in_hi):
            sys.exit(f"{name}: sample {v} sits on the PGX container bound of a "
                     f"{'signed' if signed else 'unsigned'} {depth}-bit plane; "
                     "opj_decompress may have clamped it")


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
    # Compared with the source values just below, so the clamp guard is moot.
    decoded, _ = opj_decode(codestream, ".j2k", verified=True)
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


def png_rgba(width: int, height: int, pixel) -> bytes:
    """An 8-bit RGBA PNG; `pixel(x, y)` gives the four samples."""
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    rows = b"".join(
        b"\0" + b"".join(bytes(pixel(x, y)) for x in range(width)) for y in range(height)
    )
    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )


def rgba_pixel(x: int, y: int) -> tuple[int, int, int, int]:
    return ((x * 16 + y) & 0xFF, (y * 20 + 3 * x) & 0xFF, (x * y * 5 + 40) & 0xFF, (x * 9 + y * 13 + 7) & 0xFF)


RGBA_CDEF = [(0, 0, 1), (1, 0, 2), (2, 0, 3), (3, 1, 0)]


def cdef_fixtures(one: bytes, one_geometry: dict, three: bytes, three_geometry: dict) -> None:
    """The channel definition fixtures; see the module docstring."""
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        (tmp / "rgba.png").write_bytes(png_rgba(16, 12, rgba_pixel))
        run("opj_compress", "-i", str(tmp / "rgba.png"), "-o", str(tmp / "rgba.jp2"), "-n", "3")
        rgba = (tmp / "rgba.jp2").read_bytes()
    boxes = dict(top_boxes(rgba))
    jp2h_children = top_boxes(SIGNATURE + boxes[b"jp2h"])
    written = [payload for kind, payload in jp2h_children if kind == b"cdef"]
    if written != [cdef(RGBA_CDEF)[8:]]:
        sys.exit(f"opj_compress wrote cdef {written}, expected {RGBA_CDEF}")
    codestream = boxes[b"jp2c"]
    decoded, _ = opj_decode(codestream, ".j2k")
    source = [[rgba_pixel(x, y)[c] for y in range(12) for x in range(16)] for c in range(4)]
    if [c["samples"] for c in decoded] != source:
        sys.exit("opj_compress did not code the RGBA PNG losslessly")
    rgba_geometry = {"width": 16, "height": 12, "components": 4, "depth": 8, "signed": False}
    png = "a 16x12 RGBA PNG generated by png_rgba(), coded with `opj_compress -i rgba.png -o rgba.jp2 -n 3`"

    oracle(
        "jp2_cdef_rgba",
        rgba,
        rgba_geometry,
        png,
        f"opj_compress's own file, unchanged: cdef {RGBA_CDEF} (three colours, then an "
        "opacity channel for the whole image), so nothing moves",
    )
    alpha_first = [(0, 1, 0), (1, 0, 1), (2, 0, 2), (3, 0, 3)]
    oracle(
        "jp2_cdef_alpha_first",
        jp2(codestream, rgba_geometry, [cdef(alpha_first)]),
        rgba_geometry,
        f"the jp2c codestream of {png}",
        f"cdef {alpha_first}: component 0 is opacity, 1..3 are colours 1..3; each colour "
        "entry swaps its channel into place, so the output is components 1, 2, 3, 0",
    )
    reversed_ = [(0, 0, 3), (1, 0, 2), (2, 0, 1)]
    oracle(
        "jp2_cdef_reversed",
        jp2(three, three_geometry, [cdef(reversed_)]),
        three_geometry,
        "tests/fixtures/multicomponent_lossless.j2k",
        f"cdef {reversed_} (the layout of the ISO/IEC 15444-4 file2), so the output is "
        "components 2, 1, 0",
    )
    unspecified = [(0, 65535, 65535), (1, 65535, 65535), (2, 65535, 65535)]
    oracle(
        "jp2_cdef_unspecified",
        jp2(three, three_geometry, [cdef(unspecified)]),
        three_geometry,
        "tests/fixtures/multicomponent_lossless.j2k",
        f"cdef {unspecified}: every type and association unspecified, as the ECW SDK "
        "writes; nothing moves",
    )
    oracle(
        "jp2_cdef_palette",
        jp2(one, one_geometry, [pclr(256, [7, 7, 7], entry8), cmap([(0, 1, 0), (0, 1, 1), (0, 1, 2)]), cdef(reversed_)]),
        one_geometry,
        "tests/fixtures/multilayer_lossless.j2k",
        f"the 8-bit palette with the identity cmap, then cdef {reversed_}, which applies "
        "to the palette's three channels: the output is columns 2, 1, 0",
    )


def subsampled_codestream(tmp: Path) -> tuple[bytes, str]:
    """A 24x16 image: component 0 full resolution, component 1 sub-sampled 2x2."""
    full = bytes((x * 11 + y * 7) % 256 for y in range(16) for x in range(24))
    half = bytes((x * 29 + y * 5 + 100) % 256 for y in range(8) for x in range(12))
    (tmp / "sub.raw").write_bytes(full + half)
    flags = ["-F", "24,16,2,8,u@1x1:2x2", "-n", "2"]
    run("opj_compress", "-i", str(tmp / "sub.raw"), "-o", str(tmp / "sub.j2k"), *flags)
    codestream = (tmp / "sub.j2k").read_bytes()
    decoded, _ = opj_decode(codestream, ".j2k", sampling=[(1, 1), (2, 2)])
    if [c["samples"] for c in decoded] != [list(full), list(half)]:
        sys.exit("the sub-sampled codestream did not round-trip losslessly")
    return codestream, "opj_compress -i sub.raw -o sub.j2k " + " ".join(flags)


def subsampled_fixtures() -> None:
    """Palette and direct channels from components of different sub-sampling."""
    with tempfile.TemporaryDirectory() as tmp:
        codestream, compress = subsampled_codestream(Path(tmp))
    geometry = {"width": 24, "height": 16, "components": 2, "depth": 8, "signed": False}
    planes, _ = opj_decode(codestream, ".j2k", sampling=[(1, 1), (2, 2)])
    bi = [7, 7]
    palette = pclr(256, bi, entry8)
    for name, mapping in [
        ("jp2_palette_subsampled_big_first", [(0, 1, 0), (1, 0, 0)]),
        ("jp2_palette_subsampled_small_first", [(1, 1, 0), (0, 0, 0)]),
    ]:
        data = jp2(codestream, geometry, [palette, cmap(mapping)])
        channels = []
        for i, (component, mtyp, pcol) in enumerate(mapping):
            channel = dict(planes[component])
            if mtyp == 1:
                channel["samples"] = [entry8(min(max(k, 0), 255), pcol) for k in channel["samples"]]
            # OpenJPEG labels a palette channel with its column's Bi, and a
            # direct channel i with Bi[i].
            label = bi[pcol] if mtyp == 1 else bi[i]
            channel["bit_depth"] = (label & 0x7F) + 1
            channel["signed"] = bool(label & 0x80)
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
    cdef_fixtures(one, one_geometry, three, three_geometry)


if __name__ == "__main__":
    main()
