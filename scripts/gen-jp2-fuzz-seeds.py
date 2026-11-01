#!/usr/bin/env python3
"""Write seeds for the `jp2_boxes` fuzz target into fuzz/corpus/jp2_boxes/.

Run from the repository root:

    python3 scripts/gen-jp2-fuzz-seeds.py

The target takes `variant byte + body` and appends one of two pinned
codestreams (see fuzz/fuzz_targets/jp2_boxes.rs): bit 4 of the variant picks
the three-component one. Each ISO/IEC 15444-4 JP2 reader file
(tests/fixtures/conformance/jp2/files/fileN.jp2) gives two seeds, each paired
with the pinned codestream that has the file's component count:

- `fileN_prefix`: variant 0x00 or 0x10, and the file's bytes before its first
  `jp2c` box, signature included, with the `ihdr` rewritten to the pinned
  codestream's size and component count so the seed gets past the check
  against SIZ. The fuzzer starts from a real top-level layout.
- `fileN_jp2h`: variant 0x0C or 0x1C (pinned signature, `ftyp`, and an `ihdr`
  that matches the pinned codestream), and the file's other `jp2h` children,
  so the fuzzer starts from real `colr`, `pclr`, `cmap` and `cdef` boxes.

Plus `minimal_jp2c_formK`: variant 0x0C | K with an empty body, a minimal file
that decodes as is, once per `jp2c` length form. The corpus directory is
git-ignored; seeds are regenerated, not committed.
"""

import os
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FILES = os.path.join(ROOT, "tests", "fixtures", "conformance", "jp2", "files")
OUT = os.path.join(ROOT, "fuzz", "corpus", "jp2_boxes")

# The target's pinned codestreams, by component count, with their variant bit.
PINNED = {
    1: ("regular_latlon_lossy.j2k", 0x00),
    3: ("multicomponent_subsampled_lossless.j2k", 0x10),
}


def siz_area(codestream):
    """(height, width, components) of a codestream's SIZ."""
    xsiz, ysiz, x0, y0 = struct.unpack(">IIII", codestream[8:24])
    components = struct.unpack(">H", codestream[40:42])[0]
    return ysiz - y0, xsiz - x0, components


def boxes(data, start=0):
    """Yield (offset, header length, total length, type) for each box."""
    at = start
    while at + 8 <= len(data):
        lbox, kind = struct.unpack(">I4s", data[at:at + 8])
        header = 8
        if lbox == 1:
            lbox = struct.unpack(">Q", data[at + 8:at + 16])[0]
            header = 16
        elif lbox == 0:
            lbox = len(data) - at
        yield at, header, lbox, kind
        at += lbox


def main():
    os.makedirs(OUT, exist_ok=True)
    pinned = {}
    for components, (name, bit) in PINNED.items():
        codestream = open(os.path.join(ROOT, "tests", "fixtures", name), "rb").read()
        area = siz_area(codestream)
        assert area[2] == components, name
        pinned[components] = (area, bit)

    written = 0

    def write(name, variant, body):
        nonlocal written
        with open(os.path.join(OUT, name), "wb") as f:
            f.write(bytes([variant]) + body)
        written += 1

    for n in range(1, 10):
        data = bytearray(open(os.path.join(FILES, f"file{n}.jp2"), "rb").read())
        top = list(boxes(data))
        jp2h = next(b for b in top if b[3] == b"jp2h")
        jp2c = next(b for b in top if b[3] == b"jp2c")
        start = jp2h[0] + jp2h[1]
        children = list(boxes(data[start:jp2h[0] + jp2h[2]]))
        ihdr = next(c for c in children if c[3] == b"ihdr")
        ihdr_at = start + ihdr[0] + ihdr[1]
        components = struct.unpack(">H", data[ihdr_at + 8:ihdr_at + 10])[0]
        (height, width, nc), bit = pinned[components]

        prefix = bytearray(data[:jp2c[0]])
        prefix[ihdr_at:ihdr_at + 10] = struct.pack(">IIH", height, width, nc)
        write(f"file{n}_prefix", bit, bytes(prefix))

        others = b"".join(
            bytes(data[start + c[0]:start + c[0] + c[2]]) for c in children if c[3] != b"ihdr"
        )
        write(f"file{n}_jp2h", 0x0C | bit, others)

    # A minimal file in each jp2c length form; form 0 is plain `LBox`.
    for form in range(4):
        write(f"minimal_jp2c_form{form}", 0x0C | form, b"")
    print(f"wrote {written} seeds to {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
