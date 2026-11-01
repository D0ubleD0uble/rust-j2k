#!/usr/bin/env python3
"""Write seeds for the `jp2_boxes` fuzz target into fuzz/corpus/jp2_boxes/.

Run from the repository root:

    python3 scripts/gen-jp2-fuzz-seeds.py

The target takes `variant byte + body` and appends a pinned codestream (see
fuzz/fuzz_targets/jp2_boxes.rs). Each ISO/IEC 15444-4 JP2 reader file
(tests/fixtures/conformance/jp2/files/fileN.jp2) gives two seeds:

- `fileN_prefix`: variant 0x00 and the file's bytes before its first `jp2c`
  box, signature included, so the fuzzer starts from a real top-level layout;
- `fileN_jp2h`: variant 0x0C (pinned signature, `ftyp`, and an `ihdr` that
  matches the pinned codestream), or 0x1C for a three-component file, and the
  file's `jp2h` children after `ihdr`, so the fuzzer starts from real `colr`,
  `pclr`, `cmap` and `cdef` boxes.

Plus `minimal_jp2c_formK`: variant 0x0C | K with an empty body, a minimal file
that decodes as is, once per `jp2c` length form. The corpus directory is git-ignored; seeds are
regenerated, not committed.
"""

import os
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FILES = os.path.join(ROOT, "tests", "fixtures", "conformance", "jp2", "files")
OUT = os.path.join(ROOT, "fuzz", "corpus", "jp2_boxes")


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
    written = 0

    def write(name, variant, body):
        nonlocal written
        with open(os.path.join(OUT, name), "wb") as f:
            f.write(bytes([variant]) + body)
        written += 1

    for n in range(1, 10):
        data = open(os.path.join(FILES, f"file{n}.jp2"), "rb").read()
        for at, header, length, kind in boxes(data):
            if kind == b"jp2c":
                write(f"file{n}_prefix", 0x00, data[:at])
                break
        for at, header, length, kind in boxes(data):
            if kind != b"jp2h":
                continue
            payload = data[at + header:at + length]
            children = list(boxes(payload))
            first = children[0]
            ihdr = payload[first[0] + first[1]:first[0] + first[2]]
            components = struct.unpack(">H", ihdr[8:10])[0]
            rest = payload[first[0] + first[2]:]
            write(f"file{n}_jp2h", 0x1C if components == 3 else 0x0C, rest)
            break

    # A minimal file in each jp2c length form; form 0 is plain `LBox`.
    for form in range(4):
        write(f"minimal_jp2c_form{form}", 0x0C | form, b"")
    print(f"wrote {written} seeds to {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
