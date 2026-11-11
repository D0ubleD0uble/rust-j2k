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


# name -> (bytes, width, height); `--check` prints opj's outcome.
BOMBS = {
    # 128 x 128 = 2^14 code-blocks, within the geometry bound, and 100 layers:
    # 1.6 million visits, past the 2^20 work bound.
    "jp2_t2bomb_visits.jp2": (t2bomb(512, 512, 100), 512, 512),
    # 130 x 130 = 16900 code-blocks, past the 2^14 geometry bound.
    "jp2_t2bomb_blocks.jp2": (t2bomb(520, 520, 1), 520, 520),
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


def main() -> None:
    (DIR / "bypass_psot0_eoc.j2k").write_bytes(BYPASS_PSOT0)
    for name, (data, _) in FILES.items():
        (DIR / name).write_bytes(data)
    for name, (data, _, _) in BOMBS.items():
        (DIR / name).write_bytes(data)
    if "--check" in sys.argv:
        for name, (_, base) in FILES.items():
            print(f"{name}: {check(name, base)}")
        for name, (_, width, height) in BOMBS.items():
            print(f"{name}: {check_bomb(name, width, height)}")


if __name__ == "__main__":
    main()
