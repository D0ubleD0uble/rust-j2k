#!/usr/bin/env python3
"""Wrap the `Psot = 0` codestreams of tests/fixtures/psot0/ in JP2 files.

Each file is a signature, an `ftyp`, a `jp2h` (`ihdr` 48x40, one 8-bit
component, and a grayscale `colr`), a `jp2c` box holding one of the committed
`*_psot0_*.j2k` codestreams, then the boxes listed below. The `jp2c` length is
either right (it ends where the codestream bytes end), too short (`LBox` 108
or 200, as in the PR #186 shapes), or 0 (to the end of the file).

`bypass_psot0_eoc.j2k` is written too: `bypass_base.j2k` with its only
tile-part's `Psot` set to 0.

The `jp2_t2bomb_*` files are written from scratch (see `t2bomb`): codestreams
whose packet headers are one byte each yet visit every code-block of a large
precinct, to test the bounds on the check that walks the packets.

With `--check`, each file is run through `opj_decompress` and its output is
compared with the base codestream's `*.expected.pgm`; the result is printed.
The outcomes are recorded in tests/fixtures/psot0/README.md.

Usage: scripts/gen-psot0-jp2-fixtures.py [--check]
"""

import pathlib
import struct
import subprocess
import sys
import tempfile
from oracle_tools import require_openjpeg

DIR = pathlib.Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "psot0"


def box(kind: bytes, payload: bytes) -> bytes:
    return struct.pack(">I", 8 + len(payload)) + kind + payload


def header(lbox: int, kind: bytes) -> bytes:
    return struct.pack(">I", lbox) + kind


PREFIX = (
    box(b"jP  ", b"\x0d\x0a\x87\x0a")
    + box(b"ftyp", b"jp2 " + b"\0\0\0\0" + b"jp2 ")
    + box(
        b"jp2h",
        box(b"ihdr", struct.pack(">IIHBBBB", 40, 48, 1, 7, 7, 0, 0))
        + box(b"colr", bytes([1, 0, 0]) + struct.pack(">I", 17)),
    )
)
XML = box(b"xml ", b"<a>hello</a>")
UUID = box(b"uuid", bytes(range(16)) + b"payload")
BAD_COLR = box(b"colr", bytes([1, 0]))


def cs(name: str) -> bytes:
    return (DIR / name).read_bytes()


def cut_in_com() -> bytes:
    """`single_psot0_eoc` with a COM segment after SIZ whose text is a box
    header (`free`) as long as the rest of the codestream, in a `jp2c` box
    that ends just before that text. The bytes after the box read as one
    whole box, but the box ends inside the main header."""
    base = cs("single_psot0_eoc.j2k")
    siz_end = 4 + struct.unpack(">H", base[4:6])[0]
    cut = siz_end + 6
    rest = len(base) + 14 - cut
    com = b"\xff\x64" + struct.pack(">H", 12) + b"\x00\x01" + struct.pack(">I", rest) + b"free"
    codestream = base[:siz_end] + com + base[siz_end:]
    return PREFIX + header(8 + cut, b"jp2c") + codestream


def psot_zero(base: bytes) -> bytes:
    """The codestream with its only tile-part's `Psot` set to 0."""
    sot = base.index(b"\xff\x90")
    return base[: sot + 6] + b"\0\0\0\0" + base[sot + 10 :]


def cut_in_tile_part_com(kind: bytes = b"free") -> bytes:
    """`single_psot0_eoc` with a COM segment in its tile-part header, just
    before SOD, whose text is a box header (`kind`) as long as the rest of the
    codestream, in a `jp2c` box that ends just before that text. The box holds
    the tile-part's SOT, but ends inside its header."""
    base = cs("single_psot0_eoc.j2k")
    sod = base.index(b"\xff\x93")
    cut = sod + 6
    rest = len(base) + 14 - cut
    com = b"\xff\x64" + struct.pack(">H", 12) + b"\x00\x01" + struct.pack(">I", rest) + kind
    codestream = base[:sod] + com + base[sod:]
    return PREFIX + header(8 + cut, b"jp2c") + codestream


# The BYPASS codestream (`opj_compress -M 1`) with `Psot = 0`, ending in EOC.
BYPASS_PSOT0 = psot_zero(cs("bypass_base.j2k"))


def t2bomb(width: int, height: int, layers: int) -> bytes:
    """A JP2 of one tile, one 8-bit component and one resolution (no wavelet
    levels) under 4x4 code-blocks and one precinct, with `layers` quality
    layers. Each packet is the single byte 0x80, whose header visits every
    code-block of the precinct and includes none. The tile-part has
    `Psot = 0` and no EOC, and its `jp2c` box ends where the packets end; an
    empty `free` box follows, so the box ends before the file does."""

    def seg(code: int, body: bytes) -> bytes:
        return struct.pack(">HH", code, 2 + len(body)) + body

    siz = seg(
        0xFF51,
        struct.pack(">HIIIIIIIIH", 0, width, height, 0, 0, width, height, 0, 0, 1)
        + bytes([7, 1, 1]),
    )
    cod = seg(0xFF52, bytes([0, 0]) + struct.pack(">H", layers) + bytes([0, 0, 0, 0, 0, 1]))
    qcd = seg(0xFF5C, bytes([0x40, 0x40]))
    sot = struct.pack(">HHHIBB", 0xFF90, 10, 0, 0, 0, 1)
    codestream = b"\xff\x4f" + siz + cod + qcd + sot + b"\xff\x93" + b"\x80" * layers
    prefix = (
        box(b"jP  ", b"\x0d\x0a\x87\x0a")
        + box(b"ftyp", b"jp2 " + b"\0\0\0\0" + b"jp2 ")
        + box(
            b"jp2h",
            box(b"ihdr", struct.pack(">IIHBBBB", height, width, 1, 7, 7, 0, 0))
            + box(b"colr", bytes([1, 0, 0]) + struct.pack(">I", 17)),
        )
    )
    return prefix + box(b"jp2c", codestream) + box(b"free", b"")


RPCL, PCRL = 2, 3


def sweep(size: int, components: int, order: int) -> bytes:
    """A JP2 of one `size` x `size` tile in progression `order` (RPCL or
    PCRL), one layer and one wavelet level, whose packets are all empty (the
    byte 0x00). Two thin
    components set the precinct grid's step to 2 on both axes: the first is
    subsampled 255 times vertically under 1 x 2^15 precincts, the second 255
    times horizontally under 2^15 x 1 precincts. The other components, each
    subsampled 255 times both ways, hold two packets each but are checked at
    every point of the sweep, (size / 2)^2 points for each of their two
    resolutions. The tile-part has `Psot = 0` and no EOC, and its `jp2c` box
    ends where the packets end; an empty `free` box follows."""

    def seg(code: int, body: bytes) -> bytes:
        return struct.pack(">HH", code, 2 + len(body)) + body

    def ceil(a: int, b: int) -> int:
        return -(-a // b)

    def precincts(w: int, h: int, ppx: int, ppy: int) -> int:
        return (ceil(w, 1 << ppx) if w else 0) * (ceil(h, 1 << ppy) if h else 0)

    comps = [(7, 1, 255), (7, 255, 1)] + [(7, 255, 255)] * (components - 2)
    # (PPx, PPy) at resolution 0, then 1.
    sizes = [((0, 15), (1, 15)), ((15, 0), (15, 1))] + [((15, 15), (15, 15))] * (components - 2)
    siz = seg(
        0xFF51,
        struct.pack(">HIIIIIIIIH", 0, size, size, 0, 0, size, size, 0, 0, components)
        + b"".join(bytes(c) for c in comps),
    )
    # User precincts, `order`, one layer, no MCT; one level, 64x64
    # code-blocks, 5/3, 2^15 precincts.
    cod = seg(
        0xFF52, bytes([1, order]) + struct.pack(">H", 1) + bytes([0, 1, 4, 4, 0, 1, 0xFF, 0xFF])
    )
    coc = seg(0xFF53, bytes([0, 1, 1, 4, 4, 0, 1, 0xF0, 0xF1])) + seg(
        0xFF53, bytes([1, 1, 1, 4, 4, 0, 1, 0x0F, 0x1F])
    )
    qcd = seg(0xFF5C, bytes([0x40, 0x40, 0x48, 0x48, 0x50]))
    packets = 0
    for (_, dx, dy), (low, high) in zip(comps, sizes):
        w, h = ceil(size, dx), ceil(size, dy)
        packets += precincts(ceil(w, 2), ceil(h, 2), *low) + precincts(w, h, *high)
    sot = struct.pack(">HHHIBB", 0xFF90, 10, 0, 0, 0, 1)
    codestream = b"\xff\x4f" + siz + cod + coc + qcd + sot + b"\xff\x93" + b"\x00" * packets
    prefix = (
        box(b"jP  ", b"\x0d\x0a\x87\x0a")
        + box(b"ftyp", b"jp2 " + b"\0\0\0\0" + b"jp2 ")
        + box(
            b"jp2h",
            box(b"ihdr", struct.pack(">IIHBBBB", size, size, components, 7, 7, 0, 0))
            + box(b"colr", bytes([1, 0, 0]) + struct.pack(">I", 17)),
        )
    )
    return prefix + box(b"jp2c", codestream) + box(b"free", b"")


CPRL = 4


def cprl_sweep(size: int) -> bytes:
    """A JP2 of one `size` x `size` 8-bit component in CPRL order, one layer
    and no wavelet levels, under 1 x 1 user precincts, whose packets are all
    empty (the byte 0x00). CPRL sweeps each component on its own, at that
    component's own finest precinct step, so --- unlike PCRL --- its positions
    cannot be multiplied across components: they equal that component's
    precinct columns times its rows. A 1 x 1 precinct makes the step 1 on both
    axes, so the sweep visits `size`^2 positions, one per precinct. That is
    `size`^2 precincts, so any CPRL sweep past the visit bound is already past
    the geometry bound; this file (16900 precincts) is refused there, like
    `jp2_t2bomb_blocks`. The tile-part has `Psot = 0` and no EOC, and its
    `jp2c` box ends where the packets end; an empty `free` box follows."""

    def seg(code: int, body: bytes) -> bytes:
        return struct.pack(">HH", code, 2 + len(body)) + body

    siz = seg(
        0xFF51,
        struct.pack(">HIIIIIIIIH", 0, size, size, 0, 0, size, size, 0, 0, 1) + bytes([7, 1, 1]),
    )
    # User precincts, CPRL, one layer, no MCT; no levels, 16x16 code-blocks
    # (clamped to the 1 x 1 precinct), 5/3, 1 x 1 precincts (one per sample).
    cod = seg(0xFF52, bytes([1, CPRL]) + struct.pack(">H", 1) + bytes([0, 0, 2, 2, 0, 1, 0x00]))
    qcd = seg(0xFF5C, bytes([0x20, 0x40]))
    packets = size * size
    sot = struct.pack(">HHHIBB", 0xFF90, 10, 0, 0, 0, 1)
    codestream = b"\xff\x4f" + siz + cod + qcd + sot + b"\xff\x93" + b"\x00" * packets
    prefix = (
        box(b"jP  ", b"\x0d\x0a\x87\x0a")
        + box(b"ftyp", b"jp2 " + b"\0\0\0\0" + b"jp2 ")
        + box(
            b"jp2h",
            box(b"ihdr", struct.pack(">IIHBBBB", size, size, 1, 7, 7, 0, 0))
            + box(b"colr", bytes([1, 0, 0]) + struct.pack(">I", 17)),
        )
    )
    return prefix + box(b"jp2c", codestream) + box(b"free", b"")


# name -> (bytes, width, height); `--check` prints opj's outcome.
BOMBS = {
    # 128 x 128 = 2^14 code-blocks, within the geometry bound, and 300 layers:
    # 4.9 million visits, past the 2^22 work bound.
    "jp2_t2bomb_visits.jp2": (t2bomb(512, 512, 300), 512, 512),
    # 130 x 130 = 16900 code-blocks, past the 2^14 geometry bound.
    "jp2_t2bomb_blocks.jp2": (t2bomb(520, 520, 1), 520, 520),
    # 8 x 7 code-blocks and 19000 layers: about 1.08 million visits, within
    # the 2^22 work bound, so the walk finishes.
    "jp2_t2_many_layers.jp2": (t2bomb(32, 28, 19000), 32, 28),
}

# name -> bytes; `--check` prints opj's outcome. The RPCL and PCRL files
# sweep 256^2 positions, each checking 2 resolutions of 64 components: 8.4
# million visits, past the 2^22 work bound, for 1148 empty packets. The CPRL
# file cannot work that way --- each component sweeps alone --- so its sweep
# is one fine-stepped component of 16900 positions; that is 16900 precincts,
# past the geometry bound, so it is refused there, not at the visit bound.
SWEEPS = {
    "jp2_t2_sweep_rpcl.jp2": sweep(512, 64, RPCL),
    "jp2_t2_sweep_pcrl.jp2": sweep(512, 64, PCRL),
    "jp2_t2_sweep_cprl.jp2": cprl_sweep(130),
}

# name -> (bytes, base whose opj output it is compared with)
FILES = {
    # The `jp2c` length is right; the codestream has no EOC.
    "jp2_p0noeoc_xml.jp2": (
        PREFIX + box(b"jp2c", cs("single_psot0_no_eoc.j2k")) + XML,
        "single",
    ),
    "jp2_p0noeoc_badcolr.jp2": (
        PREFIX + box(b"jp2c", cs("single_psot0_no_eoc.j2k")) + BAD_COLR,
        "single",
    ),
    "jp2_p0noeoc_tileparts_xml_uuid.jp2": (
        PREFIX + box(b"jp2c", cs("tileparts_psot0_no_eoc.j2k")) + XML + UUID,
        "tileparts",
    ),
    # The `jp2c` length is right, but the boxes after it do not reach the end
    # of the file: three bytes follow the `xml ` box.
    "jp2_p0noeoc_xml_junk.jp2": (
        PREFIX + box(b"jp2c", cs("single_psot0_no_eoc.j2k")) + XML + b"abc",
        "single",
    ),
    # The `jp2c` length is too short: the box ends inside the codestream.
    "jp2_short108_p0eoc_xml.jp2": (
        PREFIX + header(108, b"jp2c") + cs("single_psot0_eoc.j2k") + XML,
        "single",
    ),
    "jp2_p0eoc_short_badcolr.jp2": (
        PREFIX + header(108, b"jp2c") + cs("single_psot0_eoc.j2k") + BAD_COLR,
        "single",
    ),
    "jp2_t2_last0_short.jp2": (
        PREFIX + header(200, b"jp2c") + cs("tileparts_psot0_eoc.j2k") + BAD_COLR,
        "tileparts",
    ),
    "jp2_p0eoc_cut_in_com.jp2": (cut_in_com(), "single"),
    "jp2_p0eoc_cut_in_tile_part_com.jp2": (cut_in_tile_part_com(), "single"),
    # The same with the COM text spelling an `xml `, `uuid` or `rreq` box
    # header: the bytes after the box read as one of those, but they are the
    # rest of the codestream, not metadata.
    "jp2_p0eoc_cut_in_com_xml.jp2": (cut_in_tile_part_com(b"xml "), "single"),
    "jp2_p0eoc_cut_in_com_uuid.jp2": (cut_in_tile_part_com(b"uuid"), "single"),
    "jp2_p0eoc_cut_in_com_rreq.jp2": (cut_in_tile_part_com(b"rreq"), "single"),
    # A box that ends 510 bytes before the end of the BYPASS codestream, where
    # the tile data happens to read as whole boxes; nothing follows.
    "jp2_bypass_short989.jp2": (
        PREFIX + header(8 + 989, b"jp2c") + BYPASS_PSOT0,
        "bypass",
    ),
    # `LBox` 0: the box holds the codestream and the `xml ` box.
    "jp2_lbox0_p0eoc_xml.jp2": (
        PREFIX + header(0, b"jp2c") + cs("single_psot0_eoc.j2k") + XML,
        "single",
    ),
}


def check(name: str, base: str) -> str:
    with tempfile.TemporaryDirectory() as tmp:
        out = pathlib.Path(tmp) / "out.pgm"
        run = subprocess.run(
            ["opj_decompress", "-i", str(DIR / name), "-o", str(out)],
            capture_output=True,
            text=True,
        )
        if not out.exists():
            last = [l for l in (run.stdout + run.stderr).splitlines() if "ERROR" in l]
            return f"no output ({last[-1] if last else run.returncode})"
        expected = (DIR / f"{base}.expected.pgm").read_bytes()
        same = out.read_bytes()[-48 * 40 :] == expected[-48 * 40 :]
        return f"decodes, {'equal to' if same else 'DIFFERS from'} {base}.expected.pgm"


def check_bomb(name: str, width: int, height: int) -> str:
    with tempfile.TemporaryDirectory() as tmp:
        out = pathlib.Path(tmp) / "out.pgm"
        run = subprocess.run(
            ["opj_decompress", "-i", str(DIR / name), "-o", str(out)],
            capture_output=True,
            text=True,
        )
        if not out.exists():
            last = [l for l in (run.stdout + run.stderr).splitlines() if "ERROR" in l]
            return f"no output ({last[-1] if last else run.returncode})"
        samples = sorted(set(out.read_bytes()[-width * height :]))
        return f"decodes, sample values {samples}"


def check_sweep(name: str) -> str:
    with tempfile.TemporaryDirectory() as tmp:
        run = subprocess.run(
            ["opj_decompress", "-i", str(DIR / name), "-o", str(pathlib.Path(tmp) / "out.pgx")],
            capture_output=True,
            text=True,
        )
        outs = sorted(pathlib.Path(tmp).glob("out*.pgx"))
        if not outs:
            last = [l for l in (run.stdout + run.stderr).splitlines() if "ERROR" in l]
            return f"no output ({last[-1] if last else run.returncode})"
        samples = set()
        for out in outs:
            # One PGX per component: a "PG ML + 8 W H" line, then the samples.
            head, data = out.read_bytes().split(b"\n", 1)
            w, h = map(int, head.split()[-2:])
            samples |= set(data[: w * h])
        return f"decodes, {len(outs)} components, sample values {sorted(samples)}"


def main() -> None:
    (DIR / "bypass_psot0_eoc.j2k").write_bytes(BYPASS_PSOT0)
    for name, (data, _) in FILES.items():
        (DIR / name).write_bytes(data)
    for name, (data, _, _) in BOMBS.items():
        (DIR / name).write_bytes(data)
    for name, data in SWEEPS.items():
        (DIR / name).write_bytes(data)
    if "--check" in sys.argv:
        require_openjpeg("opj_decompress")
        for name, (_, base) in FILES.items():
            print(f"{name}: {check(name, base)}")
        for name, (_, width, height) in BOMBS.items():
            print(f"{name}: {check_bomb(name, width, height)}")
        for name in SWEEPS:
            print(f"{name}: {check_sweep(name)}")


if __name__ == "__main__":
    main()
