#!/usr/bin/env python3
"""Write the codestreams that test the decode's tier-2 work bound.

Each is a raw codestream (SOC ... EOC) whose packets are a byte or less of
header each, yet could make tier-2 walk far more than its bytes: a packet
header of one byte names every code-block of its precinct, and a positional
progression order sweeps the reference grid at every step, whether a packet
sits there or not. Every packet here includes nothing (it is "present" with
inclusion bits of 0, or empty, 0x00), so a decoder that walks them all reads
a flat image. OpenJPEG 2.5.0 has no work bound and walks them; `--check`
prints what it does.

The shapes come from the reviews of PR #227, issue #235, PR #249 and
issue #247:

- `t2bomb`: one tile, one component, no wavelet levels, 4x4 code-blocks in
  one precinct, `layers` layers, LRCP. Each packet is 0x80: present, then an
  inclusion tag-tree root bit of 0, so no code-block of the precinct is
  included. The decoder steps over them all at once, so these decode.
- `t2skip`: the same, but the first packet's root bit is 1, so each layer
  reads one bit for each of the root's children instead. With the precinct
  two children wide, every row of code-blocks takes the decoder two steps,
  so each one-byte packet costs as many steps as the precinct has rows of
  code-blocks, twice over.
- `sweep`: one tile in RPCL or PCRL order, `components` components, one
  layer, one wavelet level. Two thin components set the sweep's step to 2 on
  both axes; the rest, subsampled 255 times both ways, hold two packets each
  but are checked at every position.
- `cprl_steps`: one component in CPRL order whose sweep step on each axis
  comes from a different resolution, so the sweep visits far more positions
  than there are precincts; a POC marker can repeat the whole order up to 31
  times.
- `pocdup`: one tile, no wavelet levels, 2 x 2 precincts, LRCP, empty
  packets, and a POC marker of identical volumes over every layer,
  resolution and component, so each volume after the first reaches only
  packets the first one read. A decoder must emit each packet once and skip
  the rest cheaply; these decode.

The small files are written to tests/fixtures/work_bound/ (see the README
there). With `--big DIR`, the large timing shapes from the issue are written
to DIR instead; they are not committed.

`--opj` writes, instead, the two committed `opj_compress` files: a nearly
flat image in 4x4 code-blocks and 64 or 100 quality layers, which must
decode.

Usage: scripts/gen-work-bound-fixtures.py [--check] [--big DIR | --opj]
"""

import pathlib
import struct
import subprocess
import sys
import tempfile
import time

DIR = pathlib.Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "work_bound"

LRCP, RPCL, PCRL, CPRL = 0, 2, 3, 4


def seg(code: int, body: bytes) -> bytes:
    return struct.pack(">HH", code, 2 + len(body)) + body


def ceil(a: int, b: int) -> int:
    return -(-a // b)


def siz(
    x0: int, x1: int, height: int, comps: list[tuple[int, int, int]], tile: int = 0
) -> bytes:
    """SIZ for the image area `[x0, x1) x [0, height)`, as one tile or, when
    `tile` is not 0, in tiles of `tile` rows."""
    return seg(
        0xFF51,
        struct.pack(">HIIIIIIIIH", 0, x1, height, x0, 0, x1, tile or height, 0, 0, len(comps))
        + b"".join(bytes(c) for c in comps),
    )


def codestream(main: bytes, data: bytes, tiles: int = 1) -> bytes:
    """SOC, the main header segments, `tiles` tile-parts each holding `data`,
    EOC."""
    psot = 12 + 2 + len(data)
    parts = b"".join(
        struct.pack(">HHHIBB", 0xFF90, 10, index, psot, 0, 1) + b"\xff\x93" + data
        for index in range(tiles)
    )
    return b"\xff\x4f" + main + parts + b"\xff\xd9"


def t2bomb(width: int, height: int, layers: int, pad: int = 0) -> bytes:
    """`layers` packets of the byte 0x80, each visiting all
    `ceil(width / 4) * ceil(height / 4)` code-blocks, then `pad` zero bytes
    left after the last packet."""
    cod = seg(0xFF52, bytes([0, LRCP]) + struct.pack(">H", layers) + bytes([0, 0, 0, 0, 0, 1]))
    qcd = seg(0xFF5C, bytes([0x40, 0x40]))
    main = siz(0, width, height, [(7, 1, 1)]) + cod + qcd
    return codestream(main, b"\x80" * layers + b"\x00" * pad)


def t2skip(width: int, height: int, layers: int, tiles: int = 1) -> bytes:
    """`t2bomb`, with the first packet 0xC0: present, an inclusion root of
    0, then one bit of 0 for each child of the root met in raster order, so
    each child's value is past the layer. Every later packet is 0x80: present,
    then a 0 for each child again. For a precinct of 512 x 256 or 128 x 128
    code-blocks, each packet's bits fit in its byte. With `tiles`, the
    image is that many such tiles stacked."""
    cod = seg(0xFF52, bytes([0, LRCP]) + struct.pack(">H", layers) + bytes([0, 0, 0, 0, 0, 1]))
    qcd = seg(0xFF5C, bytes([0x40, 0x40]))
    main = siz(0, width, height * tiles, [(7, 1, 1)], height if tiles > 1 else 0) + cod + qcd
    return codestream(main, b"\xc0" + b"\x80" * (layers - 1), tiles)


def sweep(size: int, components: int, order: int) -> bytes:
    """A `size` x `size` tile in RPCL or PCRL order whose sweep checks every
    component at each of `(size / 2)^2` positions, for few empty packets."""
    comps = [(7, 1, 255), (7, 255, 1)] + [(7, 255, 255)] * (components - 2)
    # (PPx, PPy) at resolution 0, then 1.
    sizes = [((0, 15), (1, 15)), ((15, 0), (15, 1))] + [((15, 15), (15, 15))] * (components - 2)
    # User precincts, `order`, one layer, no MCT; one level, 64x64
    # code-blocks, 5/3, 2^15 precincts; the two thin components' COCs.
    cod = seg(
        0xFF52, bytes([1, order]) + struct.pack(">H", 1) + bytes([0, 1, 4, 4, 0, 1, 0xFF, 0xFF])
    )
    coc = seg(0xFF53, bytes([0, 1, 1, 4, 4, 0, 1, 0xF0, 0xF1])) + seg(
        0xFF53, bytes([1, 1, 1, 4, 4, 0, 1, 0x0F, 0x1F])
    )
    if components > 256:
        # Csiz above 256 widens the COC component index to two bytes.
        coc = seg(0xFF53, bytes([0, 0, 1, 1, 4, 4, 0, 1, 0xF0, 0xF1])) + seg(
            0xFF53, bytes([0, 1, 1, 1, 4, 4, 0, 1, 0x0F, 0x1F])
        )
    qcd = seg(0xFF5C, bytes([0x40, 0x40, 0x48, 0x48, 0x50]))

    def precincts(w: int, h: int, ppx: int, ppy: int) -> int:
        return (ceil(w, 1 << ppx) if w else 0) * (ceil(h, 1 << ppy) if h else 0)

    packets = 0
    for (_, dx, dy), (low, high) in zip(comps, sizes):
        w, h = ceil(size, dx), ceil(size, dy)
        packets += precincts(ceil(w, 2), ceil(h, 2), *low) + precincts(w, h, *high)
    main = siz(0, size, size, comps) + cod + coc + qcd
    return codestream(main, b"\x00" * packets)


def cprl_steps(
    x0: int, width: int, height: int, levels: int, precincts: list[int], cb: bytes, volumes: int
) -> bytes:
    """One component on `[x0, x0 + width) x [0, height)` in CPRL order, one
    layer, `levels` wavelet levels, the user precinct byte `precincts[r]`
    (PPx low nibble, PPy high) for each resolution `r` and the SPcod
    code-block byte pair `cb`. When `volumes` is not 0, a POC marker repeats
    the whole CPRL order that many times."""
    x1 = x0 + width
    cod = seg(
        0xFF52,
        bytes([1, CPRL]) + struct.pack(">H", 1) + bytes([0, levels]) + cb + bytes([0, 1])
        + bytes(precincts),
    )
    qcd = seg(0xFF5C, bytes([0x20]) + bytes([0x48] * (3 * levels + 1)))
    # RSpoc, CSpoc, LYEpoc, REpoc, CEpoc, Ppoc: every resolution, layer and
    # component, in CPRL order.
    entry = bytes([0, 0]) + struct.pack(">H", 1) + bytes([levels + 1, 1, CPRL])
    poc = seg(0xFF5F, entry * volumes) if volumes else b""
    # One packet per non-empty precinct (B.6).
    packets = 0
    for r, pp in enumerate(precincts):
        ppx, ppy = pp & 15, pp >> 4
        scale = 1 << (levels - r)
        trx0, trx1 = ceil(x0, scale), ceil(x1, scale)
        try0, try1 = 0, ceil(height, scale)
        if trx0 == trx1 or try0 == try1:
            continue
        packets += (ceil(trx1, 1 << ppx) - (trx0 >> ppx)) * (ceil(try1, 1 << ppy) - (try0 >> ppy))
    main = siz(x0, x1, height, [(7, 1, 1)]) + cod + qcd + poc
    return codestream(main, b"\x00" * packets)


def cprl_tall(volumes: int) -> bytes:
    """One column of 2^24 rows on x in [1, 2), one level, 4 x 1024
    code-blocks. Resolution 0 is empty in x but its PPy = 0 sets the y step
    to 2; resolution 1's PPx = 1 sets the x step to 2. 512 precincts and
    empty packets, and a sweep of 2^23 positions per volume."""
    return cprl_steps(1, 1, 1 << 24, 1, [0x0F, 0xF1], bytes([0, 8]), volumes)


def pocdup(
    width: int,
    height: int,
    components: int,
    layers: int,
    volumes: int,
    data: int = 0,
    order: int = LRCP,
) -> bytes:
    """`components` components on a `width` x `height` tile, no wavelet
    levels, 2 x 2 precincts (4 x 4 code-blocks clipped to them), `layers`
    layers, and a POC marker of `volumes` identical volumes over every
    layer, resolution and component. Every packet is empty (0x00), one for
    each precinct of each component in each layer; with `data`, only the
    first `data` of them, so the walk runs out of packets. `order` is the
    progression of COD and of every volume."""
    # User precincts, `order`, `layers`, no MCT; no levels, 4x4 code-blocks,
    # 5/3; PPx = PPy = 1.
    cod = seg(0xFF52, bytes([1, order]) + struct.pack(">H", layers) + bytes([0, 0, 0, 0, 0, 1, 0x11]))
    qcd = seg(0xFF5C, bytes([0x40, 0x40]))
    # RSpoc, CSpoc, LYEpoc, REpoc, CEpoc, Ppoc; past 256 components the two
    # component fields take two bytes.
    wide = components > 256
    comp = (lambda v: struct.pack(">H", v)) if wide else (lambda v: bytes([v & 0xFF]))
    entry = bytes([0]) + comp(0) + struct.pack(">H", layers) + bytes([1]) + comp(components) + bytes([order])
    poc = seg(0xFF5F, entry * volumes)
    packets = data or components * ceil(width, 2) * ceil(height, 2) * layers
    main = siz(0, width, height, [(7, 1, 1)] * components) + cod + qcd + poc
    return codestream(main, b"\x00" * packets)


def setskip(layers: int, volumes: int, thin: int = 64) -> bytes:
    """One 38608 x 1 tile in RPCL, no wavelet levels, 1 x 1 precincts,
    `layers` layers and `volumes` identical POC volumes. One component at
    XRsiz 254 sets the sweep's step to 254; `thin` more at XRsiz 255 have 152
    precincts each, but the sweep meets only their first. So the volumes
    reach (152 + 152 * thin) packets a layer and read (152 + thin), each
    empty (0x00). From the review of PR #261."""
    comps = [(7, 254, 1)] + [(7, 255, 1)] * thin
    cod = seg(0xFF52, bytes([1, RPCL]) + struct.pack(">H", layers) + bytes([0, 0, 0, 0, 0, 1, 0]))
    qcd = seg(0xFF5C, bytes([0x40, 0x40]))
    entry = bytes([0, 0]) + struct.pack(">H", layers) + bytes([1, len(comps), RPCL])
    poc = seg(0xFF5F, entry * volumes)
    main = siz(0, 152 * 254, 1, comps) + cod + qcd + poc
    return codestream(main, b"\x00" * ((152 + thin) * layers))


# Committed: name -> bytes. Each but the first and the last walks far past
# the decode's work bound for its size and is refused with `Error::Limit`.
FILES = {
    # 2^17 code-blocks, 1000 layers, each skipped in one step: decodes.
    "t2bomb_2048x1024_1000.j2k": t2bomb(2048, 1024, 1000),
    # 512 x 256 code-blocks, 40000 layers: 512 steps a layer, 2.0e7 steps
    # for 40000 bytes of packets.
    "t2skip_2048x1024_40000.j2k": t2skip(2048, 1024, 40000),
    # 64 components, 512^2 positions at 129 visits each: 3.4e7 visits.
    "sweep_rpcl_1024_64.j2k": sweep(1024, 64, RPCL),
    "sweep_pcrl_1024_64.j2k": sweep(1024, 64, PCRL),
    # 2^23 positions, 3 visits each, once and in 31 POC volumes.
    "cprl_tall.j2k": cprl_tall(0),
    "cprl_tall_poc31.j2k": cprl_tall(31),
    # 4 precincts, 1024 layers, 31 identical POC volumes: 4096 packets read
    # once and 126976 skipped. Decodes.
    "pocdup_4x4_1024_31.j2k": pocdup(4, 4, 1, 1024, 31),
}

# Not committed (`--big DIR`): the issue's timing shapes.
BIG = {
    # The PR #227 PCRL sweep: 2048^2, 1024 components, 6140 empty packets.
    "sweep_pcrl_2048_1024.j2k": lambda: sweep(2048, 1024, PCRL),
    # 2^14 code-blocks, 65535 layers.
    "t2bomb_512_65535.j2k": lambda: t2bomb(512, 512, 65535),
    # 64 code-blocks, 65535 layers.
    "t2bomb_32_65535.j2k": lambda: t2bomb(32, 32, 65535),
    # 2^19 code-blocks, the most the geometry admits, and 65535 layers.
    "t2bomb_2896_65535.j2k": lambda: t2bomb(2896, 2896, 65535),
    # The 65535-layer 512^2 bomb padded to 10 MB of tile data.
    "t2bomb_512_65535_10mb.j2k": lambda: t2bomb(512, 512, 65535, 10_000_000 - 65535),
    # `t2skip` on 2^19 code-blocks, 724 x 724, and 65535 layers.
    "t2skip_2896_65535.j2k": lambda: t2skip(2896, 2896, 65535),
    # `t2skip` in 153 tiles of 512 x 512, 65535 layers each: 10 MB of
    # packets, past the bound for their size.
    "t2skip_tiles_10mb.j2k": lambda: t2skip(512, 512, 65535, 153),
    # The #247 shapes. 16 precincts, 65535 layers, 31 identical volumes: 1 MB.
    "pocdup_8x8_65535_31.j2k": lambda: pocdup(8, 8, 1, 65535, 31),
    # The same in 100 KB: 4 precincts, 25600 layers.
    "pocdup_4x4_25600_31.j2k": lambda: pocdup(4, 4, 1, 25600, 31),
    # 160 components of one precinct, 65535 layers, two volumes: 10.5 MB.
    "pocdup_160c_65535_2.j2k": lambda: pocdup(1, 1, 160, 65535, 2),
    # 160 precincts, 65535 layers, 32 volumes, past OpenJPEG's 31: 10.5 MB.
    "pocdup_8x80_65535_32.j2k": lambda: pocdup(8, 80, 1, 65535, 32),
    # Two volumes over 2^14 precincts and 65535 layers, 1.1e9 packets, but
    # only the first 10^7 in the data: more packets than the walk can read,
    # in `decode` or `read_header`. In LRCP it is refused before the walk;
    # in RPCL the walk reads to the end of the data.
    "pocdup_256x256_65535_2_10mb.j2k": lambda: pocdup(256, 256, 1, 65535, 2, 10_000_000),
    "pocdup_256x256_65535_2_10mb_rpcl.j2k": lambda: pocdup(
        256, 256, 1, 65535, 2, 10_000_000, RPCL
    ),
    # Two volumes over 1000 components of one precinct: 6.6e7 packets, just
    # under the bit per visit `read_header` holds for 10^7 bytes, then the
    # first 10^7 of them. 4800 components, 3.1e8 packets, for `decode`.
    "pocdup_1000c_65535_2_10mb.j2k": lambda: pocdup(1, 1, 1000, 65535, 2, 10_000_000),
    "pocdup_4800c_65535_2_10mb.j2k": lambda: pocdup(1, 1, 4800, 65535, 2, 10_000_000),
    # 6.5e8 packets reached, 1.4e7 read and 4.2e8 skipped from the set: 14 MB.
    "setskip_65535_31.j2k": lambda: setskip(65535, 31),
    # The shape `tests/work_bound.rs` builds: 442 KB, refused; and with one
    # volume, which decodes.
    "setskip_2048_31.j2k": lambda: setskip(2048, 31),
    "setskip_2048_1.j2k": lambda: setskip(2048, 1),
}


def box(kind: bytes, payload: bytes) -> bytes:
    return struct.pack(">I", 8 + len(payload)) + kind + payload


def jp2_psot0(cs: bytes) -> bytes:
    """`cs` in a JP2 file, with its tile-part's `Psot` set to 0 and its EOC
    dropped, in a `jp2c` box that ends where the packets end, then an empty
    `free` box. That is the shape whose packets `read_header` walks, to see
    whether the codestream ends at the box's end (`gen-psot0-jp2-fixtures.py`
    has the same wrapper)."""
    xsiz, ysiz, x0, y0 = struct.unpack(">IIII", cs[8:24])
    csiz = struct.unpack(">H", cs[40:42])[0]
    sot = cs.index(b"\xff\x90")
    body = cs[: sot + 6] + b"\0\0\0\0" + cs[sot + 10 : -2]
    ihdr = struct.pack(">IIHBBBB", ysiz - y0, xsiz - x0, csiz, 7, 7, 0, 0)
    prefix = (
        box(b"jP  ", b"\x0d\x0a\x87\x0a")
        + box(b"ftyp", b"jp2 " + b"\0\0\0\0" + b"jp2 ")
        + box(b"jp2h", box(b"ihdr", ihdr) + box(b"colr", bytes([1, 0, 0]) + struct.pack(">I", 17)))
    )
    return prefix + box(b"jp2c", body) + box(b"free", b"")


# Committed too: `t2skip` over 128 x 128 code-blocks, 20000 layers, as a
# `Psot = 0` JP2. 256 steps a layer, 5.1e6 in all: within the decode's bound,
# past the bound on the walk `read_header` makes.
FILES["jp2_t2skip_512_20000.jp2"] = jp2_psot0(t2skip(512, 512, 20000))


def patch_image(size: int) -> bytes:
    """A `size` x `size` 8-bit PGM of 128, with one 8 x 8 patch of noise in
    its top-left corner (Python's `random`, seeded with 1)."""
    import random

    random.seed(1)
    px = bytearray([128]) * (size * size)
    for y in range(8):
        for x in range(8):
            px[y * size + x] = random.randrange(256)
    return b"P5\n%d %d\n255\n" % (size, size) + bytes(px)


# Encoder output, `--opj`: name -> (layers, the PSNR list's step in dB from
# 20). OpenJPEG's encoder sets the "present" bit of every packet, so each
# layer's header names every code-block; in a nearly flat image most are
# never included.
OPJ = {
    "opj_patch2048_b4_q64.j2k": (64, 0.625),
    "opj_patch2048_b4_q100.j2k": (100, 0.4),
}


def opj_files() -> list[pathlib.Path]:
    """Encode `patch_image(2048)` with `opj_compress -b 4,4 -q <list>` for
    each entry of `OPJ`, the list being `layers` PSNR targets from 20 dB up
    by the step."""
    from oracle_tools import require_openjpeg

    require_openjpeg("opj_compress")
    written = []
    with tempfile.TemporaryDirectory() as tmp:
        image = pathlib.Path(tmp) / "patch2048.pgm"
        image.write_bytes(patch_image(2048))
        for name, (layers, step) in OPJ.items():
            psnr = ",".join(str(round(20 + step * i, 4)) for i in range(layers))
            subprocess.run(
                ["opj_compress", "-i", str(image), "-o", str(DIR / name), "-b", "4,4", "-q", psnr],
                check=True,
                capture_output=True,
            )
            written.append(DIR / name)
    return written


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
        # Each shape as a raw codestream, for `decode`, and as a `Psot = 0`
        # JP2, for the walk `read_header` makes. The committed ones too.
        shapes = {name: make() for name, make in BIG.items()} | FILES
        for name, data in shapes.items():
            if name.endswith(".jp2"):
                continue
            for file, data in ((name, data), (name[:-4] + ".jp2", jp2_psot0(data))):
                (out / file).write_bytes(data)
                written.append(out / file)
    elif "--opj" in sys.argv:
        written = opj_files()
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
