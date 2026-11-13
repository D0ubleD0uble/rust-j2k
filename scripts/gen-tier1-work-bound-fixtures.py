#!/usr/bin/env python3
"""Write the codestreams that test the decode's tier-1 work bound.

Each is a raw codestream (SOC ... EOC) of one tile, one 8-bit component and
no wavelet levels, so the whole tile is one LL band. Every code-block is
included in the one layer with `passes` coding passes and one byte of data
(0x00). A pass scans its whole code-block however few bytes it carries, so
the decode costs about `passes` x the tile's samples, for a file of a few
bytes per code-block: the packet header spends about 28 bits on each one.

QCD gives the band 30 magnitude bit-planes (guard bits 2, exponent 29) and
every code-block has no zero bit-planes, so the block decoder runs up to
3 x 30 - 2 = 88 passes on each. The shape is the one from issue #250:
`t1bomb(8192, 6, 88)`, 8192^2 samples in 64x64 code-blocks, 76 KB.

The committed files are written to tests/fixtures/tier1_work_bound/ (see
the README there). With `--big DIR`, the issue's timing shape is written to
DIR instead; it is not committed. `--check` runs each through
`opj_decompress` 2.5.0 and prints what it does and the time.

`--opj` writes, instead, the committed `opj_compress` files, which the
bound must admit. One is a 16-bit image of 1000 with 128 specks of 60000,
coded losslessly at the defaults: each speck makes its code-blocks run
every pass for a few bytes. The other is a flat 16-bit image of 0 coded
with no wavelet level (`-n 1`): each 64x64 code-block runs about 46
passes for about 14 bytes, the most work per byte seen from an encoder,
42% of the bound.

Usage: scripts/gen-tier1-work-bound-fixtures.py [--check] [--big DIR | --opj]
"""

import pathlib
import struct
import subprocess
import sys
import tempfile
import time

DIR = pathlib.Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "tier1_work_bound"


def seg(code: int, body: bytes) -> bytes:
    return struct.pack(">HH", code, 2 + len(body)) + body


def ceil(a: int, b: int) -> int:
    return -(-a // b)


class Bits:
    """Packet header bits, packed as B.10.1 has it: a byte after 0xFF
    carries only 7 bits, its top bit 0."""

    def __init__(self) -> None:
        self.out = bytearray()
        self.byte = 0
        self.room = 8
        self.used = 0

    def put(self, bit: int) -> None:
        self.byte = (self.byte << 1) | bit
        self.used += 1
        if self.used == self.room:
            self.out.append(self.byte)
            self.room = 7 if self.byte == 0xFF else 8
            self.byte = 0
            self.used = 0

    def put_int(self, value: int, width: int) -> None:
        for shift in range(width - 1, -1, -1):
            self.put((value >> shift) & 1)

    def finish(self) -> bytes:
        if self.used:
            self.out.append(self.byte << (self.room - self.used))
        elif self.out and self.out[-1] == 0xFF:
            # The header may not end on 0xFF: the stuffed byte follows.
            self.out.append(0)
        return bytes(self.out)


def tag_tree_nodes(cols: int, rows: int) -> list[tuple[int, int]]:
    """The grid size of each level of a tag tree over `cols` x `rows`
    leaves, leaves first, root last."""
    levels = [(cols, rows)]
    while levels[-1] != (1, 1):
        c, r = levels[-1]
        levels.append((ceil(c, 2), ceil(r, 2)))
    return levels


def num_passes_code(bits: Bits, passes: int) -> None:
    """Table B.4."""
    if passes == 1:
        bits.put(0)
    elif passes == 2:
        bits.put_int(0b10, 2)
    elif passes <= 5:
        bits.put_int(0b11, 2)
        bits.put_int(passes - 3, 2)
    elif passes <= 36:
        bits.put_int(0b1111, 4)
        bits.put_int(passes - 6, 5)
    else:
        bits.put_int(0b111111111, 9)
        bits.put_int(passes - 37, 7)


def t1bomb(size: int, cb: int, passes: int) -> bytes:
    """One `size` x `size` tile in `2^cb` x `2^cb` code-blocks, each
    included in the one layer with `passes` coding passes and one byte."""
    assert 2 <= cb <= 6 and 1 <= passes <= 164
    cols = rows = ceil(size, 1 << cb)
    levels = tag_tree_nodes(cols, rows)
    bits = Bits()
    bits.put(1)  # the packet is not empty
    # Each tag tree's nodes already coded: the inclusion tree and the
    # zero-bit-plane tree. Every value is 0, so each node on a leaf's path
    # that is not yet known codes a single 1 (B.10.2).
    known = [set(), set()]
    length_bits = 3 + passes.bit_length() - 1  # Lblock + floor(log2(passes))
    for y in range(rows):
        for x in range(cols):
            # Inclusion, then the zero bit-planes, then the pass count.
            for tree in range(2):
                for level in range(len(levels) - 1, -1, -1):
                    node = (level, x >> level, y >> level)
                    if node not in known[tree]:
                        known[tree].add(node)
                        bits.put(1)
            num_passes_code(bits, passes)
            bits.put(0)  # Lblock stays 3
            bits.put_int(1, length_bits)
    header = bits.finish()
    body = b"\x00" * (cols * rows)
    data = header + body

    cod = seg(0xFF52, bytes([0, 0]) + struct.pack(">H", 1) + bytes([0, 0, cb - 2, cb - 2, 0, 1]))
    # No quantization, guard bits 2, exponent 29: Mb = 2 + 29 - 1 = 30.
    qcd = seg(0xFF5C, bytes([0x40, 29 << 3]))
    siz = seg(
        0xFF51,
        struct.pack(">HIIIIIIIIH", 0, size, size, 0, 0, size, size, 0, 0, 1) + bytes([7, 1, 1]),
    )
    psot = 12 + 2 + len(data)
    part = struct.pack(">HHHIBB", 0xFF90, 10, 0, psot, 0, 1) + b"\xff\x93" + data
    return b"\xff\x4f" + siz + cod + qcd + part + b"\xff\xd9"


# Committed: name -> bytes.
FILES = {
    # 1024^2 in 64x64 code-blocks, 88 passes: 9.2e7 sample-passes in 1.3 KB.
    "t1bomb_1024_64_88.j2k": t1bomb(1024, 6, 88),
}

# Not committed (`--big DIR`): the issue's timing shape.
BIG = {
    # 8192^2 in 64x64 code-blocks, 88 passes: 5.9e9 sample-passes in 76 KB.
    "t1bomb_8192_64_88.j2k": lambda: t1bomb(8192, 6, 88),
}


def specks_image(size: int, specks: int, background: int = 1000) -> bytes:
    """A `size` x `size` 16-bit PGM of `background`, with a speck of 60000 at
    `(i * 977 % size, i * 613 % size)` (x, y) for each `i` below `specks`."""
    px = bytearray(background.to_bytes(2, "big") * (size * size))
    for i in range(specks):
        x, y = i * 977 % size, i * 613 % size
        at = 2 * (y * size + x)
        px[at : at + 2] = (60000).to_bytes(2, "big")
    return b"P5\n%d %d\n65535\n" % (size, size) + bytes(px)


# Encoder output, `--opj`: name -> (size, specks, background, extra
# `opj_compress` arguments).
OPJ = {
    "opj_specks2048_16bit.j2k": (2048, 128, 1000, []),
    "opj_flat2048_16bit_n1.j2k": (2048, 0, 0, ["-n", "1"]),
}


def opj_files() -> list[pathlib.Path]:
    """Encode `specks_image` with `opj_compress` at its defaults (lossless
    5/3, 64x64 code-blocks, 5 levels, one layer) and the entry's extra
    arguments, for each entry of `OPJ`."""
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    from oracle_tools import require_openjpeg

    require_openjpeg("opj_compress")
    written = []
    DIR.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        for name, (size, specks, background, args) in OPJ.items():
            image = pathlib.Path(tmp) / "specks.pgm"
            image.write_bytes(specks_image(size, specks, background))
            subprocess.run(
                ["opj_compress", "-i", str(image), "-o", str(DIR / name), *args],
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
        if not list(pathlib.Path(tmp).glob("out*.pgx")):
            last = [l for l in (run.stdout + run.stderr).splitlines() if "ERROR" in l]
            return f"no output ({last[-1] if last else run.returncode}), {took:.2f} s"
        return f"decodes, {took:.2f} s"


def main() -> None:
    written = []
    if "--big" in sys.argv:
        out = pathlib.Path(sys.argv[sys.argv.index("--big") + 1])
        out.mkdir(parents=True, exist_ok=True)
        for name, make in BIG.items():
            (out / name).write_bytes(make())
            written.append(out / name)
    elif "--opj" in sys.argv:
        written = opj_files()
    else:
        DIR.mkdir(exist_ok=True)
        for name, data in FILES.items():
            (DIR / name).write_bytes(data)
            written.append(DIR / name)
    if "--check" in sys.argv:
        sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
        from oracle_tools import require_openjpeg

        require_openjpeg("opj_decompress")
        for path in written:
            print(f"{path.name} ({path.stat().st_size} B): {check(path)}")


if __name__ == "__main__":
    main()
