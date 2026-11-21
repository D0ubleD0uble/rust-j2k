#!/usr/bin/env python3
"""Write tests/fixtures/work_bound/pocmulti_64tiles_4096_2.j2k or
pocmulti_16tiles_4096_2.j2k.

A codestream of TILES tiles (one column, each WIDTH x HEIGHT, one component,
no wavelet levels, 2 x 2 precincts, LAYERS LRCP layers) with VOLUMES
identical POC volumes over all of them, and tile-parts with no packet bytes.
Each tile's volumes reach about half a default packet work bound, so a decode
with `strict` off meets a POC reach refusal on tile after tile, and a retry
that names only what the tiles seen so far need climbs one factor per tile.

    python3 scripts/gen-multipoc-fixture.py OUT TILES WIDTH HEIGHT LAYERS VOLUMES

The committed files are made with `TILES WIDTH HEIGHT LAYERS VOLUMES` =
`64 64 64 4096 2` (64 tiles) and `16 64 64 4096 2` (16 tiles). Add `--data` to give each
tile-part its packet bytes (not committed).
"""
import struct
import sys


def seg(code, body):
    return struct.pack(">HH", code, 2 + len(body)) + body


def main(args):
    with_data = "--data" in args
    args = [a for a in args if a != "--data"]
    out = args[0]
    tiles, width, height, layers, volumes = (int(a) for a in args[1:6])
    lrcp = 0
    siz = seg(
        0xFF51,
        struct.pack(">HIIIIIIIIH", 0, width, height * tiles, 0, 0, width, height, 0, 0, 1)
        + bytes([7, 1, 1]),
    )
    cod = seg(0xFF52, bytes([1, lrcp]) + struct.pack(">H", layers) + bytes([0, 0, 0, 0, 0, 1, 0x11]))
    qcd = seg(0xFF5C, bytes([0x40, 0x40]))
    entry = bytes([0, 0]) + struct.pack(">H", layers) + bytes([1, 1, lrcp])
    poc = seg(0xFF5F, entry * volumes)
    packets = layers * ((width + 1) // 2) * ((height + 1) // 2)
    data = b"\x00" * packets if with_data else b""
    parts = b"".join(
        struct.pack(">HHHIBB", 0xFF90, 10, i, 14 + len(data), 0, 1) + b"\xff\x93" + data
        for i in range(tiles)
    )
    with open(out, "wb") as f:
        f.write(b"\xff\x4f" + siz + cod + qcd + poc + parts + b"\xff\xd9")


if __name__ == "__main__":
    main(sys.argv[1:])
