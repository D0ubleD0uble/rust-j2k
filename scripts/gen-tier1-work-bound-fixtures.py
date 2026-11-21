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
the README there), and a seed of the `incremental_decode` fuzz target, a
row of such tiles, to tests/fixtures/incremental/ (see
tests/fixtures/README.md). With `--big DIR`, the issue's timing shape is
written to DIR instead; it is not committed. `--check` runs each through
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


def t1bomb(size: int, cb: int, passes: int, tiles: int = 1) -> bytes:
    """One `size` x `size` tile in `2^cb` x `2^cb` code-blocks, each
    included in the one layer with `passes` coding passes and one byte; or
    a row of `tiles` such tiles, each the same."""
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
        struct.pack(">HIIIIIIIIH", 0, size * tiles, size, 0, 0, size, size, 0, 0, 1)
        + bytes([7, 1, 1]),
    )
    psot = 12 + 2 + len(data)
    parts = b"".join(
        struct.pack(">HHHIBB", 0xFF90, 10, tile, psot, 0, 1) + b"\xff\x93" + data
        for tile in range(tiles)
    )
    return b"\xff\x4f" + siz + cod + qcd + parts + b"\xff\xd9"


def shaped_bomb(size: int, cb: int, passes: int, termall: bool = False, last_len: int = 1) -> bytes:
    """A `t1bomb` variant for the memory and time figures in
    docs/resource-bounds.md. With `termall`, the COD code-block style is 0x04
    (terminate on each pass): each pass is its own codeword segment of zero
    bytes, which costs about 3 header bits and the decoder about 130 bytes of
    heap. Otherwise each code-block has one segment of one byte, except the
    last, which has `last_len` bytes of zeros. A length of `last_len` needs
    its Lblock raised: Lblock starts at 3 and the length field has
    Lblock + floor(log2(passes)) bits, so one 1 bit per increment and a 0 go
    before the field (B.10.7.1)."""
    assert 2 <= cb <= 6 and 1 <= passes <= 164
    cols = rows = ceil(size, 1 << cb)
    levels = tag_tree_nodes(cols, rows)
    bits = Bits()
    bits.put(1)
    known = [set(), set()]
    lg = passes.bit_length() - 1
    body = 0
    for y in range(rows):
        for x in range(cols):
            for tree in range(2):
                for level in range(len(levels) - 1, -1, -1):
                    node = (level, x >> level, y >> level)
                    if node not in known[tree]:
                        known[tree].add(node)
                        bits.put(1)
            num_passes_code(bits, passes)
            if termall:
                bits.put(0)
                for _ in range(passes):
                    bits.put_int(0, 3)  # one pass: 3 + floor(log2(1)) bits
            elif last_len > 1 and y == rows - 1 and x == cols - 1:
                k = max(0, last_len.bit_length() - lg - 3)
                for _ in range(k):
                    bits.put(1)
                bits.put(0)
                bits.put_int(last_len, 3 + k + lg)
                body += last_len
            else:
                bits.put(0)
                bits.put_int(1, 3 + lg)
                body += 1
    data = bits.finish() + b"\x00" * body
    style = 0x04 if termall else 0
    cod = seg(0xFF52, bytes([0, 0]) + struct.pack(">H", 1) + bytes([0, 0, cb - 2, cb - 2, style, 1]))
    qcd = seg(0xFF5C, bytes([0x40, 29 << 3]))
    siz = seg(0xFF51, struct.pack(">HIIIIIIIIH", 0, size, size, 0, 0, size, size, 0, 0, 1) + bytes([7, 1, 1]))
    psot = 12 + 2 + len(data)
    part = struct.pack(">HHHIBB", 0xFF90, 10, 0, psot, 0, 1) + b"\xff\x93" + data
    return b"\xff\x4f" + siz + cod + qcd + part + b"\xff\xd9"


# Committed: name -> bytes.
FILES = {
    # 1024^2 in 64x64 code-blocks, 88 passes: 9.2e7 sample-passes in 1.3 KB.
    "t1bomb_1024_64_88.j2k": t1bomb(1024, 6, 88),
}

# Committed seeds of the `incremental_decode` fuzz target, in
# tests/fixtures/incremental/: name -> bytes.
INCREMENTAL_DIR = DIR.parent / "incremental"
INCREMENTAL = {
    # A row of 8 tiles of 128^2, each 1.5e6 sample-passes under the target's
    # cap (which charges 4 for each sample on top): tiles 0 and 1 fit its
    # bound of 2^22, and tile 2 passes it.
    "t1bomb_row_128_8.seed": t1bomb(128, 6, 88, 8),
}

# Not committed (`--memory DIR`): the shapes behind the memory and time
# figures in docs/resource-bounds.md ("Figures for a decode service").
MEMORY = {
    # Code-blocks of 4x4 with one pass-terminated segment per pass: 1024^2 is
    # 2.3 MB and peaks at 720 MiB under AtMost(2^20); 2048^2 is 9.3 MB.
    "termall_1024_2_88.j2k": lambda: shaped_bomb(1024, 2, 88, termall=True),
    "termall_2048_2_88.j2k": lambda: shaped_bomb(2048, 2, 88, termall=True),
    "termall_4096_2_88.j2k": lambda: shaped_bomb(4096, 2, 88, termall=True),
    # The last code-block of an 8192^2 tile holds 120,000 zero bytes: 196,526
    # bytes that gzip to 944 and decode in 13 s at the defaults.
    "zeros_8192_6_88.j2k": lambda: shaped_bomb(8192, 6, 88, last_len=120000),
    "zeros_none_8192_6_88.j2k": lambda: shaped_bomb(8192, 6, 88),
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
    elif "--memory" in sys.argv:
        out = pathlib.Path(sys.argv[sys.argv.index("--memory") + 1])
        out.mkdir(parents=True, exist_ok=True)
        for name, make in MEMORY.items():
            (out / name).write_bytes(make())
            written.append(out / name)
    elif "--opj" in sys.argv:
        written = opj_files()
    else:
        DIR.mkdir(exist_ok=True)
        for name, data in FILES.items():
            (DIR / name).write_bytes(data)
            written.append(DIR / name)
        for name, data in INCREMENTAL.items():
            (INCREMENTAL_DIR / name).write_bytes(data)
    if "--check" in sys.argv:
        sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
        from oracle_tools import require_openjpeg

        require_openjpeg("opj_decompress")
        for path in written:
            print(f"{path.name} ({path.stat().st_size} B): {check(path)}")


if __name__ == "__main__":
    main()
