#!/usr/bin/env python3
"""Write the files that measure the decoder's peak memory at the sample budget.

Each file declares images totaling 2^26 samples, the `MAX_IMAGE_SAMPLES`
budget in src/codestream/mod.rs, but carries no coded data: every packet is
empty (the byte 0x00), so the decode reconstructs a flat image and its cost is
the buffers alone. The shapes are the ones issue #257 measured:

- `peak_97.j2k`: one 8192x8192 8-bit component, one tile, 9/7 wavelet with 5
  levels, one layer, expounded quantization; one empty packet per resolution.
  It is 118 bytes.
- `peak_53.j2k`: the same with the 5/3 wavelet and no quantization.
- `peak_mct.j2k`: three 8192x2730 components, 9/7 with the color transform
  (MCT), one tile: 16384 samples short of 2^26.
- `peak_palette.jp2`: one 8192x2730 8-bit index component in a JP2 file whose
  palette (`pclr` + `cmap`) maps it to three 8-bit columns, so the expanded
  output is three such planes.
- `peak_sycc.jp2`: a JP2 file with `colr` 18 (sYCC): an 8192x2730 luma plane
  and two 4096x1365 chroma planes (2x2 sub-sampling), all 5/3. Decoded with
  sRGB output, the chroma is replicated to full size, so the output is three
  luma-sized planes.

None is committed; they are written to the directory given (default
`scratch/peak/`). Peak RSS is read with GNU time on a release build:

    /usr/bin/time -v <decoder> <file>        # "Maximum resident set size"
    /usr/bin/time -v opj_decompress -i peak_97.j2k -o out.pgm

where `<decoder>` is any program that calls `rust_j2k::decode_with` on the
file (with `ColorOutput::Srgb` for `peak_sycc.jp2`). The measured peaks are
in the `MAX_IMAGE_SAMPLES` rustdoc; the figures before the single-tile fix are
in issue #257.

Provenance: written from scratch by this script; no encoder is involved.

Usage: scripts/gen-peak-memory-shapes.py [DIR]
"""

import pathlib
import struct
import sys

LRCP = 0
SIGNATURE = bytes.fromhex("0000000c6a5020200d0a870a")


def seg(code: int, body: bytes) -> bytes:
    return struct.pack(">HH", code, 2 + len(body)) + body


def box(kind: bytes, payload: bytes) -> bytes:
    return struct.pack(">I", 8 + len(payload)) + kind + payload


def siz(width: int, height: int, comps: list[tuple[int, int, int]]) -> bytes:
    """SIZ for a `width` x `height` image at the origin, as one tile; each
    component is `(Ssiz, XRsiz, YRsiz)`."""
    return seg(
        0xFF51,
        struct.pack(">HIIIIIIIIH", 0, width, height, 0, 0, width, height, 0, 0, len(comps))
        + b"".join(bytes(c) for c in comps),
    )


def cod(levels: int, irreversible: bool, mct: bool) -> bytes:
    """Default precincts, LRCP, one layer; `levels` levels, 64x64
    code-blocks, no code-block style."""
    return seg(
        0xFF52,
        bytes([0, LRCP]) + struct.pack(">H", 1) + bytes([int(mct), levels, 4, 4, 0, int(not irreversible)]),
    )


def qcd(levels: int, irreversible: bool) -> bytes:
    """Two guard bits; for 9/7 expounded steps of exponent 8 and mantissa 0,
    for 5/3 no quantization with exponent 8."""
    bands = 1 + 3 * levels
    if irreversible:
        return seg(0xFF5C, bytes([0x42]) + struct.pack(">H", 8 << 11) * bands)
    return seg(0xFF5C, bytes([0x40]) + bytes([8 << 3]) * bands)


def codestream(main: bytes, packets: int) -> bytes:
    """SOC, the main header, one tile-part of `packets` empty packets, EOC."""
    data = b"\x00" * packets
    psot = 12 + 2 + len(data)
    return (
        b"\xff\x4f"
        + main
        + struct.pack(">HHHIBB", 0xFF90, 10, 0, psot, 0, 1)
        + b"\xff\x93"
        + data
        + b"\xff\xd9"
    )


def single(irreversible: bool) -> bytes:
    levels = 5
    main = siz(8192, 8192, [(7, 1, 1)]) + cod(levels, irreversible, False) + qcd(levels, irreversible)
    return codestream(main, levels + 1)


def mct() -> bytes:
    levels = 5
    main = siz(8192, 2730, [(7, 1, 1)] * 3) + cod(levels, True, True) + qcd(levels, True)
    return codestream(main, 3 * (levels + 1))


def jp2(code: bytes, width: int, height: int, components: int, colr: int, children: bytes) -> bytes:
    ihdr = box(b"ihdr", struct.pack(">IIHBBBB", height, width, components, 7, 7, 0, 0))
    colr_box = box(b"colr", bytes([1, 0, 0]) + struct.pack(">I", colr))
    return (
        SIGNATURE
        + box(b"ftyp", b"jp2 " + b"\0\0\0\0" + b"jp2 ")
        + box(b"jp2h", ihdr + colr_box + children)
        + box(b"jp2c", code)
    )


def palette() -> bytes:
    levels = 5
    main = siz(8192, 2730, [(7, 1, 1)]) + cod(levels, False, False) + qcd(levels, False)
    entries = 256
    pclr = struct.pack(">HB", entries, 3) + bytes([7, 7, 7])
    pclr += b"".join(bytes([j, 255 - j, j // 2]) for j in range(entries))
    cmap = b"".join(struct.pack(">HBB", 0, 1, column) for column in range(3))
    return jp2(
        codestream(main, levels + 1), 8192, 2730, 1, 16, box(b"pclr", pclr) + box(b"cmap", cmap)
    )


def sycc() -> bytes:
    levels = 5
    main = siz(8192, 2730, [(7, 1, 1), (7, 2, 2), (7, 2, 2)]) + cod(levels, False, False) + qcd(levels, False)
    return jp2(codestream(main, 3 * (levels + 1)), 8192, 2730, 3, 18, b"")


def main() -> None:
    root = pathlib.Path(__file__).resolve().parent.parent
    out = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else root / "scratch" / "peak"
    out.mkdir(parents=True, exist_ok=True)
    files = {
        "peak_97.j2k": single(True),
        "peak_53.j2k": single(False),
        "peak_mct.j2k": mct(),
        "peak_palette.jp2": palette(),
        "peak_sycc.jp2": sycc(),
    }
    for name, data in files.items():
        (out / name).write_bytes(data)
        print(f"{out / name}: {len(data)} bytes")


if __name__ == "__main__":
    main()
