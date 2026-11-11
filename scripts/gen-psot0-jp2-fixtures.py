#!/usr/bin/env python3
"""Wrap the `Psot = 0` codestreams of tests/fixtures/psot0/ in JP2 files.

Each file is a signature, an `ftyp`, a `jp2h` (`ihdr` 48x40, one 8-bit
component, and a grayscale `colr`), a `jp2c` box holding one of the committed
`*_psot0_*.j2k` codestreams, then the boxes listed below. The `jp2c` length is
either right (it ends where the codestream bytes end), too short (`LBox` 108
or 200, as in the PR #186 shapes), or 0 (to the end of the file).

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


def main() -> None:
    for name, (data, _) in FILES.items():
        (DIR / name).write_bytes(data)
    if "--check" in sys.argv:
        for name, (_, base) in FILES.items():
            print(f"{name}: {check(name, base)}")


if __name__ == "__main__":
    main()
