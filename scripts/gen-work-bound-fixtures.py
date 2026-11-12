#!/usr/bin/env python3
"""Write the codestreams that test the decode's tier-2 work bound.

Each is a raw codestream (SOC ... EOC) whose packets are a byte or less of
header each, yet make tier-2 walk far more than its bytes: a packet header
of one byte can visit every code-block of its precinct, and a positional
progression order sweeps the reference grid at every step, whether a packet
sits there or not. Every packet here is "present, nothing included" (0x80) or
empty (0x00), so a decoder that walks them all reads a flat image. OpenJPEG
2.5.0 has no work bound and walks them; `--check` prints what it does.

The shapes come from the reviews of PR #227 and issue #235:

- `t2bomb`: one tile, one component, no wavelet levels, 4x4 code-blocks in
  one precinct, `layers` layers, LRCP. Each packet is 0x80: present, then an
  inclusion tag-tree root bit of 0, so its header visits every code-block
  and includes none.
- `sweep`: one tile in RPCL or PCRL order, `components` components, one
  layer, one wavelet level. Two thin components set the sweep's step to 2 on
  both axes; the rest, subsampled 255 times both ways, hold two packets each
  but are checked at every position.
- `cprl_steps`: one component in CPRL order whose sweep step on each axis
  comes from a different resolution, so the sweep visits far more positions
  than there are precincts; a POC marker can repeat the whole order up to 31
  times.

The small files are written to tests/fixtures/work_bound/ (see the README
there). With `--big DIR`, the large timing shapes from the issue are written
to DIR instead; they are not committed.

Usage: scripts/gen-work-bound-fixtures.py [--check] [--big DIR]
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


def siz(x0: int, x1: int, height: int, comps: list[tuple[int, int, int]]) -> bytes:
    """SIZ for the image area `[x0, x1) x [0, height)` as one tile."""
    return seg(
        0xFF51,
        struct.pack(">HIIIIIIIIH", 0, x1, height, x0, 0, x1, height, 0, 0, len(comps))
        + b"".join(bytes(c) for c in comps),
    )


def codestream(main: bytes, data: bytes) -> bytes:
    """SOC, the main header segments, one tile-part holding `data`, EOC."""
    psot = 12 + 2 + len(data)
    sot = struct.pack(">HHHIBB", 0xFF90, 10, 0, psot, 0, 1)
    return b"\xff\x4f" + main + sot + b"\xff\x93" + data + b"\xff\xd9"


def t2bomb(width: int, height: int, layers: int, pad: int = 0) -> bytes:
    """`layers` packets of the byte 0x80, each visiting all
    `ceil(width / 4) * ceil(height / 4)` code-blocks, then `pad` zero bytes
    left after the last packet."""
    cod = seg(0xFF52, bytes([0, LRCP]) + struct.pack(">H", layers) + bytes([0, 0, 0, 0, 0, 1]))
    qcd = seg(0xFF5C, bytes([0x40, 0x40]))
    main = siz(0, width, height, [(7, 1, 1)]) + cod + qcd
    return codestream(main, b"\x80" * layers + b"\x00" * pad)


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


# Committed: name -> bytes. Each walks far past the decode's work bound for
# its size and is refused with `Error::Limit`.
FILES = {
    # 2^17 code-blocks, 1000 layers: 1.3e8 visits for 1000 bytes of packets.
    "t2bomb_2048x1024_1000.j2k": t2bomb(2048, 1024, 1000),
    # 64 components, 512^2 positions at 129 visits each: 3.4e7 visits.
    "sweep_rpcl_1024_64.j2k": sweep(1024, 64, RPCL),
    "sweep_pcrl_1024_64.j2k": sweep(1024, 64, PCRL),
    # 2^23 positions, 3 visits each, once and in 31 POC volumes.
    "cprl_tall.j2k": cprl_tall(0),
    "cprl_tall_poc31.j2k": cprl_tall(31),
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
            for file, data in ((name, data), (name[:-4] + ".jp2", jp2_psot0(data))):
                (out / file).write_bytes(data)
                written.append(out / file)
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
