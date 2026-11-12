#!/usr/bin/env python3
"""Write the codestreams that test the cost of resolving tile headers.

Each is a raw codestream (SOC ... EOC) of many 1x1 tiles whose cost lies in
the tile headers, not the packets. Before #256, every tile's header was
resolved by copying the main header whole, so the time grew as the tile
count times the component count (or times the TLM entries) before a single
check ran. `--check` prints what OpenJPEG 2.5.0 does with each file.

The shapes come from the review that filed issue #256:

- `tilecomp`: the image `[1, side + 1)^2` in 1x1 tiles, so `side^2` tiles.
  Component 0 is full size; the other `components - 1` are subsampled 255
  times both ways, so they are empty on tile 0 and the decode fails there
  (`Codestream`, "tile 0 has an empty area on component 1"), after the tile
  headers are read. With `levels`, the main header is heavy: that many
  wavelet levels, explicit precincts and 3 * levels + 1 expounded QCD steps.
  With `tile_cod`, every tile also carries its own COD (and QCD, with
  `tile_qcd`) restating the main header's, so each tile overrides every
  component.
- `tlm`: a valid image of `side^2` 1x1 tiles of one component whose main
  header carries `kb` KB of TLM entries (ST = 1, SP = 0: three bytes each).
  Every packet is empty (0x00), so it decodes to a flat 128, as OpenJPEG
  2.5.0 decodes it.

The small files are written to tests/fixtures/tile_header/ (see the README
there). With `--big DIR`, the issue's timing shapes are written to DIR
instead; they are not committed.

Usage: scripts/gen-tile-header-fixtures.py [--check] [--big DIR]
"""

import pathlib
import struct
import subprocess
import sys
import tempfile
import time

DIR = pathlib.Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "tile_header"


def seg(code: int, body: bytes) -> bytes:
    return struct.pack(">HH", code, 2 + len(body)) + body


def siz(side: int, comps: list[tuple[int, int, int]], origin: int) -> bytes:
    """SIZ for the image `[origin, origin + side)^2` in 1x1 tiles whose grid
    starts at the image origin. Each component is `(Ssiz, XRsiz, YRsiz)`."""
    end = origin + side
    return seg(
        0xFF51,
        struct.pack(">HIIIIIIIIH", 0, end, end, origin, origin, 1, 1, origin, origin, len(comps))
        + b"".join(bytes(c) for c in comps),
    )


def cod(levels: int) -> bytes:
    """LRCP, one layer, no MCT, `levels` wavelet levels, 4x4 code-blocks, 5/3.
    With levels, user precincts of 2^15 at every resolution."""
    scod = 1 if levels else 0
    precincts = bytes([0xFF] * (levels + 1)) if levels else b""
    return seg(
        0xFF52, bytes([scod, 0]) + struct.pack(">H", 1) + bytes([0, levels, 0, 0, 0, 1]) + precincts
    )


def qcd(levels: int) -> bytes:
    """With no levels, one reversible step; with levels, 3 * levels + 1
    scalar expounded steps, two guard bits."""
    if not levels:
        return seg(0xFF5C, bytes([0x40, 0x40]))
    return seg(0xFF5C, bytes([0x42]) + struct.pack(">H", 0x4000) * (3 * levels + 1))


def tilepart(index: int, data: bytes, header: bytes = b"") -> bytes:
    """One tile-part of tile `index` (part 0 of 1): SOT, `header`, SOD, `data`."""
    psot = 12 + len(header) + 2 + len(data)
    return struct.pack(">HHHIBB", 0xFF90, 10, index, psot, 0, 1) + header + b"\xff\x93" + data


def tilecomp(
    side: int, components: int, levels: int = 0, tile_cod: bool = False, tile_qcd: bool = False
) -> bytes:
    comps = [(7, 1, 1)] + [(7, 255, 255)] * (components - 1)
    main = siz(side, comps, 1) + cod(levels) + qcd(levels)
    header = (cod(levels) if tile_cod else b"") + (qcd(levels) if tile_qcd else b"")
    tiles = []
    for t in range(side * side):
        x, y = 1 + t % side, 1 + t // side
        # One empty packet for each component the tile holds a sample of:
        # component 0 always, the subsampled ones where both coordinates are
        # multiples of 255. (No decode gets past tile 0, so the count only
        # sets the file's size.)
        held = 1 + (components - 1 if x % 255 == 0 and y % 255 == 0 else 0)
        tiles.append(tilepart(t, b"\x00" * held, header))
    return b"\xff\x4f" + main + b"".join(tiles) + b"\xff\xd9"


def tlm(side: int, kb: int) -> bytes:
    """`side^2` 1x1 tiles of one component, after `kb` KB of TLM entries,
    each naming tile 0 with a length of 14 (never checked against the
    tile-parts, which TLM does not govern)."""
    per_marker = 65530 // 3
    entries = kb * 1024 // 3
    markers = b""
    z = 0
    while entries > 0:
        k = min(per_marker, entries)
        entries -= k
        markers += seg(0xFF55, bytes([z & 0xFF, 0x10]) + bytes([0, 0, 14]) * k)
        z += 1
    main = siz(side, [(7, 1, 1)], 0) + cod(0) + qcd(0) + markers
    tiles = b"".join(tilepart(t, b"\x00") for t in range(side * side))
    return b"\xff\x4f" + main + tiles + b"\xff\xd9"


# Committed: name -> bytes.
FILES = {
    # 256 tiles x 256 components, refused at tile 0.
    "tilecomp_16_256.j2k": tilecomp(16, 256),
    # The same under the heavy header: 32 levels, 97 steps.
    "tilecomp_heavy_16_256.j2k": tilecomp(16, 256, 32),
    # The same with a COD and a QCD in every tile, overriding every component.
    "tilecomp_override_16_256.j2k": tilecomp(16, 256, 0, True, True),
    # 1024 tiles after 16 KB of TLM entries: decodes, flat 128.
    "tlm_32_16k.j2k": tlm(32, 16),
}

# Not committed (`--big DIR`): the issue's timing shapes, and the same with
# per-tile overrides.
BIG = {
    "tilecomp_64_16384.j2k": lambda: tilecomp(64, 16384),
    "tilecomp_heavy_128_16384.j2k": lambda: tilecomp(128, 16384, 32),
    "tilecomp_255_16384.j2k": lambda: tilecomp(255, 16384),
    "tilecomp_cod_255_16384.j2k": lambda: tilecomp(255, 16384, 0, True),
    "tilecomp_codqcd_255_16384.j2k": lambda: tilecomp(255, 16384, 0, True, True),
    "tlm_255_1m.j2k": lambda: tlm(255, 1024),
    "tlm_255_4m.j2k": lambda: tlm(255, 4096),
}


def check(path: pathlib.Path) -> str:
    with tempfile.TemporaryDirectory() as tmp:
        start = time.monotonic()
        run = subprocess.run(
            ["opj_decompress", "-i", str(path), "-o", str(pathlib.Path(tmp) / "out.pgx")],
            capture_output=True,
            text=True,
        )
        took = time.monotonic() - start
        outs = sorted(pathlib.Path(tmp).glob("out*.pgx"))
        if not outs:
            last = [l for l in (run.stdout + run.stderr).splitlines() if "ERROR" in l]
            return f"no output ({last[-1] if last else run.returncode}), {took:.2f} s"
        samples = set()
        for out in outs:
            head, data = out.read_bytes().split(b"\n", 1)
            w, h = map(int, head.split()[-2:])
            samples |= set(data[: w * h])
        return f"decodes, {len(outs)} components, sample values {sorted(samples)}, {took:.2f} s"


def main() -> None:
    written = []
    if "--big" in sys.argv:
        out = pathlib.Path(sys.argv[sys.argv.index("--big") + 1])
        out.mkdir(parents=True, exist_ok=True)
        for name, make in BIG.items():
            (out / name).write_bytes(make())
            written.append(out / name)
    else:
        DIR.mkdir(exist_ok=True)
        for name, data in FILES.items():
            (DIR / name).write_bytes(data)
            written.append(DIR / name)
    if "--check" in sys.argv:
        from oracle_tools import require_openjpeg

        require_openjpeg("opj_decompress")
        for path in written:
            print(f"{path.name} ({path.stat().st_size} B): {check(path)}")


if __name__ == "__main__":
    main()
