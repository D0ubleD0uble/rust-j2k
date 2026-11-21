#!/usr/bin/env python3
"""Write seeds for the `header_split` fuzz target into fuzz/corpus/header_split/.

Run from the repository root:

    python3 scripts/gen-header-split-seeds.py [OUT_DIR]

OUT_DIR defaults to fuzz/corpus/header_split. The output is the same on every
run: no randomness, no dates.

The target takes 12 control bytes and then the file (see
fuzz/fuzz_targets/header_split.rs). `IncrementalDecoder::header` keeps what
its read found of tiles that are over a cap under the main header, and random
bytes seldom reach one, so this writes codestreams built to have them. They
are the files `tests/incremental_header.rs` builds (`rescued`,
`small_row_over_tall_row` and `in_jp2`), written out again here:

- `rescued_*`: 64 tiles of 1040 x 512 in a row, one gray component, one
  wavelet level, and precincts of 1x1 and 2x2 in the main header, so a tile
  with no `COD` of its own has 266,240 precincts, over the 2^18 cap. Each
  tile's first tile-part says what the tile is:
  - `all_rescued`: a `COD` of maximal precincts, which brings the tile within
    the caps (16 tile-parts a tile, a tile's together, or with
    `_interleaved` all the first tile-parts first);
  - `tile40_over`: tile 40 has no `COD`, so the file is refused for it;
  - `tile30_nolevels`: tile 30 has a `COD` and `QCD` of no levels, which a
    reduction of 1 refuses;
  - `tile20_missing`: tile 20 has no tile-part.
- `many_components`: 1024 gray components over a row of 24 tiles of 1 x 1
  and a row of 24 tiles of 1 x 4096, which a reduction of 1 refuses.
- `jp2_*`: `tile40_over` inside a JP2 file, and the same with two `cdef`
  boxes (a box layout fault).

Each file comes behind four sets of control bytes, so the options differ:
the defaults; a reduction of 1; tile 25 under `Unlimited`; and a window over
tiles 10 to 12 with `strict` on, a raised sample limit and a reduction of 1.
The last six bytes of each set steer the sizes of the pieces. Four more
seeds of `tile30_nolevels` (`_cut0` to `_cut3`), at a reduction of 1, have
pieces that end inside tile 30's `COD`.
"""

import os
import struct
import sys

SOC = b"\xff\x4f"
EOC = b"\xff\xd9"


def seg(code, body):
    """A marker segment: the code, `Lxxx` (body length plus 2), the body."""
    return struct.pack(">HH", code, len(body) + 2) + body


def box(kind, payload):
    """A JP2 box: `LBox` (payload length plus 8), the type, the payload."""
    return struct.pack(">I", 8 + len(payload)) + kind + payload


def siz_body(grid, components):
    """`Rsiz` 0, the eight grid fields, `Csiz` and (`Ssiz`, `XRsiz`, `YRsiz`)."""
    out = struct.pack(">H8IH", 0, *grid, len(components))
    return out + b"".join(bytes(c) for c in components)


def tile_part(isot, tpsot, tnsot, extra, data):
    """`SOT`, `extra` (header marker segments), `SOD` and `data`."""
    psot = 12 + len(extra) + 2 + len(data)
    sot = seg(0xFF90, struct.pack(">HIBB", isot, psot, tpsot, tnsot))
    return sot + extra + b"\xff\x93" + data


def cod_body(layers, levels, block):
    """LRCP, no color transform, 5/3, no precinct sizes, no block style."""
    return bytes([0, 0]) + struct.pack(">H", layers) + bytes(
        [0, levels, block, block, 0, 1]
    )


def with_precincts(cod):
    """`cod` for one level with precincts of 1x1, then 2x2."""
    return bytes([1]) + cod[1:] + bytes([0x00, 0x11])


def qcd_none_body(guard, exponents):
    return bytes([guard << 5]) + bytes(e << 3 for e in exponents)


def rescued(tiles, parts, own, interleaved):
    """`tiles` tiles of `parts` tile-parts. `own(tile)` is what the first
    tile-part carries: 'rescued', 'nothing', 'nolevels' or 'missing'."""
    siz = siz_body([1040 * tiles, 512, 0, 0, 1040, 512, 0, 0], [[7, 1, 1]])
    out = (
        SOC
        + seg(0xFF51, siz)
        + seg(0xFF52, with_precincts(cod_body(1, 1, 4)))
        + seg(0xFF5C, qcd_none_body(2, [8] * 4))
    )
    rescue = seg(0xFF52, cod_body(1, 1, 4))
    no_levels = seg(0xFF52, cod_body(1, 0, 4)) + seg(0xFF5C, qcd_none_body(2, [8]))

    def part(tile, p):
        extra = b""
        if p == 0 and own(tile) == "rescued":
            extra = rescue
        elif p == 0 and own(tile) == "nolevels":
            extra = no_levels
        return tile_part(tile, p, parts, extra, b"\x00" if p == 0 else b"")

    present = [t for t in range(tiles) if own(t) != "missing"]
    if interleaved:
        for p in range(parts):
            for t in present:
                out += part(t, p)
    else:
        for t in present:
            for p in range(parts):
                out += part(t, p)
    return out + EOC


def small_row_over_tall_row(components, width, tall):
    """A row of `width` tiles of 1x1 over a row of 1 x `tall`, each with a
    tile-part of no `COD` and no data."""
    siz = siz_body(
        [width, 2 * tall, 0, tall - 1, 1, tall, 0, 0], [[7, 1, 1]] * components
    )
    out = (
        SOC
        + seg(0xFF51, siz)
        + seg(0xFF52, with_precincts(cod_body(1, 1, 0)))
        + seg(0xFF5C, qcd_none_body(2, [8] * 4))
    )
    for tile in range(2 * width):
        out += tile_part(tile, 0, 1, b"", b"")
    return out + EOC


def ihdr_for(codestream):
    """An `ihdr` matching the codestream's `SIZ`, 8-bit unsigned samples."""
    xsiz, ysiz, x0, y0 = struct.unpack(">4I", codestream[8:24])
    csiz = codestream[40:42]
    tail = bytes([7, 7, 0, 0])
    return box(b"ihdr", struct.pack(">II", ysiz - y0, xsiz - x0) + csiz + tail)


def in_jp2(codestream, boxes):
    """`codestream` in a JP2 file with a gray `colr` and the `jp2h` `boxes`."""
    colr = box(b"colr", bytes([1, 0, 0]) + struct.pack(">I", 17))
    header = ihdr_for(codestream) + colr + b"".join(boxes)
    return (
        box(b"jP  ", b"\x0d\x0a\x87\x0a")
        + box(b"ftyp", b"jp2 \0\0\0\0jp2 ")
        + box(b"jp2h", header)
        + box(b"jp2c", codestream)
    )


def cdef(entries):
    payload = struct.pack(">H", len(entries))
    for entry in entries:
        payload += struct.pack(">3H", *entry)
    return box(b"cdef", payload)


# The 12 control bytes (fuzz/fuzz_targets/header_split.rs): reduction,
# strict and sample limit, region code and argument, the limit's size and the
# option flags, then six bytes for the sizes of the pieces.
CONTROLS = [
    bytes([0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6]),
    # a reduction of 1
    bytes([1, 0, 0, 0, 0, 0, 7, 6, 5, 4, 3, 2]),
    # Unlimited (bits 1-2 of byte 1 are 2), tile 25
    bytes([0, 4, 1, 25, 0, 0, 11, 12, 13, 14, 15, 16]),
    # strict on, Raised(3) (bits 1-2 are 1, byte 4 is 2), a window from tile 10
    # (code 2, argument 80: x = 80 * 1040 / 8) three tiles wide, a reduction of 1
    bytes([1, 3, 2, 80, 2, 0, 21, 22, 23, 24, 25, 26]),
]

# `tile30_nolevels` at a reduction of 1 is refused once tile 30's `COD` is
# whole, so a feed that ends inside that `COD` must not settle the tile. These
# piece-size seeds (bytes 6 to 8) end a feed there. A search over the
# 24-bit values found them, against a `header()` that took a tile-part cut
# short for whole; they are not special otherwise.
CUT_IN_COD = [bytes([1, 0, 0, 0, 0, 0, k, 0, 9, 0, 0, 0]) for k in (1, 10, 25, 28)]


def main():
    out_dir = sys.argv[1] if len(sys.argv) > 1 else "fuzz/corpus/header_split"
    os.makedirs(out_dir, exist_ok=True)

    def only(index, kind):
        return lambda tile: kind if tile == index else "rescued"

    files = {
        "rescued_all_rescued": rescued(64, 16, lambda t: "rescued", False),
        "rescued_all_rescued_interleaved": rescued(64, 16, lambda t: "rescued", True),
        "rescued_tile40_over": rescued(64, 4, only(40, "nothing"), False),
        "rescued_tile30_nolevels": rescued(64, 4, only(30, "nolevels"), False),
        "rescued_tile20_missing": rescued(64, 4, only(20, "missing"), True),
        "many_components": small_row_over_tall_row(1024, 24, 4096),
    }
    over = files["rescued_tile40_over"]
    files["jp2_tile40_over"] = in_jp2(over, [])
    files["jp2_two_cdefs"] = in_jp2(over, [cdef([(0, 0, 1)])] * 2)

    for name, body in sorted(files.items()):
        for i, control in enumerate(CONTROLS):
            with open(os.path.join(out_dir, f"{name}_c{i}"), "wb") as f:
                f.write(control + body)
    nolevels = files["rescued_tile30_nolevels"]
    for i, control in enumerate(CUT_IN_COD):
        with open(os.path.join(out_dir, f"rescued_tile30_nolevels_cut{i}"), "wb") as f:
            f.write(control + nolevels)
    print(f"wrote {len(files) * len(CONTROLS) + len(CUT_IN_COD)} seeds to {out_dir}")


if __name__ == "__main__":
    main()
