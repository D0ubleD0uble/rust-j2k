#!/usr/bin/env python3
"""Write seeds for the `jp2_boxes` fuzz target into fuzz/corpus/jp2_boxes/.

Run from the repository root:

    python3 scripts/gen-jp2-fuzz-seeds.py

The target takes `variant byte + body` and appends one of the pinned
codestreams (see fuzz/fuzz_targets/jp2_boxes.rs): bits 4-6 of the variant pick
it. Each ISO/IEC 15444-4 JP2 reader file
(tests/fixtures/conformance/jp2/files/fileN.jp2) and each synthetic JP2
fixture (tests/fixtures/jp2_*.jp2) gives two seeds, each paired with the
pinned 8-bit single-component codestream if the file has one component, else
the 8-bit un-sub-sampled three-component one, so that a file's ICC profile
reaches the `ColourOutput::Srgb` conversion:

- `NAME_prefix`: variant 0x00 or 0x10, and the file's bytes before its first
  `jp2c` box, signature included, with the `ihdr` rewritten to the pinned
  codestream's size and component count so the seed gets past the check
  against SIZ. The fuzzer starts from a real top-level layout.
- `NAME_jp2h`: variant 0x0C or 0x1C (pinned signature, `ftyp`, and an `ihdr`
  that matches the pinned codestream), and the file's other `jp2h` children,
  so the fuzzer starts from real `colr`, `pclr`, `cmap` and `cdef` boxes.

Plus `minimal_jp2c_formK`: variant 0x0C | K with an empty body, a minimal file
that decodes as is, once per `jp2c` length form.

Plus seeds for the `ColourOutput::Srgb` conversions, each a `colr` box built
here (variant 0x0C | codestream << 4, body one `colr`):

- `icc_rgb_*`: METH 2 with a three-component matrix/TRC profile (`rXYZ`,
  `gXYZ`, `bXYZ`, `wtpt`, three TRCs) with the sRGB primaries adapted to D50,
  on the 8-bit three-component codestream. The TRCs are `curv` gamma 2.2,
  a 256-entry `curv` table, an identity `curv`, and `para` function types
  0-4 (type 3 with the sRGB parameters).
- `icc_grey_*`: a monochrome profile (`kTRC`, `wtpt`) with the same curve
  kinds, on the 8-bit single-component codestream.
- `icc_grey_tinted`: the monochrome profile at absolute colorimetric intent
  with a D65 media white. Its sRGB output is not neutral, so it has three
  channels for the one coded channel.
- `icc_grey_palette_opacity`: that profile with a two-column palette and a
  `cdef` of grey plus opacity: two coded channels, four after conversion, as
  OpenJPEG gives.
- `ci_crash_grey_icc`: the input that failed the CI fuzz smoke, a grey profile
  with an exact D50 white point and a `para` type 4 curve, at intent 0. The
  curve and rounding make its output non-neutral.
- `icc_rgb_subsampled`: the RGB profile on the three-component codestream
  sub-sampled 2x2 in all three components. The channels agree, so it converts.
- `icc_grey_16bit`: the monochrome profile on the 16-bit single-component
  codestream, which the conversion refuses after reading the profile (8-bit
  only).
- `icc_rgb_mixed`: the RGB profile on the sYCC 4:2:0 codestream (luma full
  size, chroma 2x2), refused for channels that differ in sub-sampling.
- `sycc_420`, `sycc_422`: METH 1, EnumCS 18 on the sYCC codestreams (Y full
  size, chroma 2x2 and 2x1 sub-sampled), as the committed jp2_sycc_*.jp2
  fixtures. `sycc_subsampled_all` is sYCC on the 2x2 three-component one.

The profiles are built from the ICC.1 layout, not copied from a file, so the
seeds are reproducible from this script alone. The corpus directory is
git-ignored; seeds are regenerated, not committed.
"""

import os
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIXTURES = os.path.join(ROOT, "tests", "fixtures")
FILES = os.path.join(FIXTURES, "conformance", "jp2", "files")
OUT = os.path.join(ROOT, "fuzz", "corpus", "jp2_boxes")

# The target's pinned codestreams, by component count, with their variant bit.
PINNED = {
    1: ("derived_quant_lossy.j2k", 0x20),
    3: ("multicomponent_lossless.j2k", 0x30),
}

# Variant bits for each pinned codestream (see the target).
ONE_16, THREE_SUB, ONE_8, THREE_8, SYCC_420, SYCC_422 = (n << 4 for n in range(6))


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


def be32(n):
    return struct.pack(">I", n)


def s15(x):
    return struct.pack(">i", round(x * 65536))


def xyz_tag(x, y, z):
    return b"XYZ \0\0\0\0" + s15(x) + s15(y) + s15(z)


def curv(*values):
    return b"curv\0\0\0\0" + be32(len(values)) + b"".join(struct.pack(">H", v) for v in values)


def para(function, *params):
    return b"para\0\0\0\0" + struct.pack(">HH", function, 0) + b"".join(s15(p) for p in params)


# The sRGB colorants adapted to D50 (as in the sRGB IEC61966-2.1 profile), and
# the D50 white point.
RXYZ, GXYZ, BXYZ = (0.4361, 0.2225, 0.0139), (0.3851, 0.7169, 0.0971), (0.1431, 0.0606, 0.7141)
D50 = (0.9642, 1.0, 0.8249)

# Curves by name: curv gamma 2.2 (u8Fixed8), a table, identity, and the five
# parametric function types (3 has the sRGB parameters).
CURVES = {
    "gamma": curv(0x0233),
    "table": curv(*(round(65535 * (i / 255) ** 2.2) for i in range(256))),
    "identity": curv(),
    "para0": para(0, 2.2),
    "para1": para(1, 2.4, 1 / 1.055, 0.055 / 1.055),
    "para2": para(2, 2.4, 1 / 1.055, 0.055 / 1.055, 0.0),
    "para3": para(3, 2.4, 1 / 1.055, 0.055 / 1.055, 1 / 12.92, 0.04045),
    "para4": para(4, 2.4, 1 / 1.055, 0.055 / 1.055, 1 / 12.92, 0.04045, 0.0, 0.0),
}


def icc(space, tags):
    """An ICC.1 v2 input profile (class scnr, PCS XYZ) with the given tags."""
    table = be32(len(tags))
    data = b""
    base = 132 + 12 * len(tags)
    for sig, payload in tags:
        table += sig + be32(base + len(data)) + be32(len(payload))
        data += payload + b"\0" * (-len(payload) % 4)
    header = bytearray(128)
    header[0:4] = be32(128 + len(table) + len(data))
    header[8:12] = bytes([2, 0x40, 0, 0])  # version 2.4
    header[12:16] = b"scnr"
    header[16:20] = space
    header[20:24] = b"XYZ "
    header[36:40] = b"acsp"
    return bytes(header) + table + data


def icc_grey_tinted(curve):
    """A monochrome profile whose sRGB output is not neutral (absolute
    colorimetric intent with a D65 media white) and so has three channels."""
    d65 = (0.9505, 1.0, 1.089)
    profile = bytearray(icc(b"GRAY", [(b"wtpt", xyz_tag(*d65)), (b"kTRC", curve)]))
    profile[64:68] = be32(3)
    return bytes(profile)


def box(kind, payload):
    return be32(8 + len(payload)) + kind + payload


def grey_palette_opacity():
    """The monochrome profile, a two-column palette (entry i is (i, i)), a
    `cmap` using both columns of component 0, and a `cdef` giving the first
    column as the grey colour channel and the second as opacity. The sRGB
    conversion applies the palette, gives two channels, and returns four."""
    pclr = struct.pack(">HBBB", 256, 2, 7, 7) + b"".join(bytes([i, i]) for i in range(256))
    cmap = struct.pack(">HBBHBB", 0, 1, 0, 0, 1, 1)
    cdef = struct.pack(">HHHHHHH", 2, 0, 0, 1, 1, 1, 0)
    return (colr(2, icc_grey_tinted(CURVES["gamma"])) + box(b"pclr", pclr)
            + box(b"cmap", cmap) + box(b"cdef", cdef))


# The input that failed the CI fuzz smoke (variant byte, then body): a grey
# profile whose sRGB output is not neutral.
CI_CRASH_HEX = (
    "2c000000e3636f6c72020002000000d8000000000240000073636e7247524159"
    "58595a2000000000000000000000000061637370000000000000000000000000"
    "0000000000000000000000000000000000000000000000000000000000000000"
    "0000000000000000000000000000000000000000000000000000000000000000"
    "00000000000000000000000000000002777470740000009c000000146b545243"
    "000000b00000002858595a20000000000000f6d6000100000000d32d70617261"
    "0000000000040010020066660000f2a7009d0d59000013d000000a5b00000000"
    "00000000"
)


def icc_rgb(curve):
    return icc(b"RGB ", [
        (b"wtpt", xyz_tag(*D50)),
        (b"rXYZ", xyz_tag(*RXYZ)), (b"gXYZ", xyz_tag(*GXYZ)), (b"bXYZ", xyz_tag(*BXYZ)),
        (b"rTRC", curve), (b"gTRC", curve), (b"bTRC", curve),
    ])


def icc_grey(curve):
    return icc(b"GRAY", [(b"wtpt", xyz_tag(*D50)), (b"kTRC", curve)])


def colr(meth, payload):
    """A `colr` box: METH, PREC 0, APPROX 0, then the EnumCS or the profile."""
    body = bytes([meth, 0, 0]) + payload
    return be32(8 + len(body)) + b"colr" + body


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

    sources = [(f"file{n}", os.path.join(FILES, f"file{n}.jp2")) for n in range(1, 10)]
    sources += [
        (name[:-4], os.path.join(FIXTURES, name))
        for name in sorted(os.listdir(FIXTURES))
        if name.startswith("jp2_") and name.endswith(".jp2")
    ]
    for stem, path in sources:
        data = bytearray(open(path, "rb").read())
        top = list(boxes(data))
        jp2h = next(b for b in top if b[3] == b"jp2h")
        jp2c = next(b for b in top if b[3] == b"jp2c")
        start = jp2h[0] + jp2h[1]
        children = list(boxes(data[start:jp2h[0] + jp2h[2]]))
        ihdr = next(c for c in children if c[3] == b"ihdr")
        ihdr_at = start + ihdr[0] + ihdr[1]
        components = struct.unpack(">H", data[ihdr_at + 8:ihdr_at + 10])[0]
        (height, width, nc), bit = pinned[1 if components == 1 else 3]

        prefix = bytearray(data[:jp2c[0]])
        prefix[ihdr_at:ihdr_at + 10] = struct.pack(">IIH", height, width, nc)
        write(f"{stem}_prefix", bit, bytes(prefix))

        others = b"".join(
            bytes(data[start + c[0]:start + c[0] + c[2]]) for c in children if c[3] != b"ihdr"
        )
        write(f"{stem}_jp2h", 0x0C | bit, others)

    # A minimal file in each jp2c length form; form 0 is plain `LBox`.
    for form in range(4):
        write(f"minimal_jp2c_form{form}", 0x0C | form, b"")

    for name, curve in CURVES.items():
        write(f"icc_rgb_{name}", 0x0C | THREE_8, colr(2, icc_rgb(curve)))
        write(f"icc_grey_{name}", 0x0C | ONE_8, colr(2, icc_grey(curve)))
    write("icc_grey_tinted", 0x0C | ONE_8, colr(2, icc_grey_tinted(CURVES["gamma"])))
    write("icc_grey_palette_opacity", 0x0C | ONE_8, grey_palette_opacity())
    ci = bytes.fromhex(CI_CRASH_HEX)
    write("ci_crash_grey_icc", ci[0], ci[1:])
    write("icc_rgb_subsampled", 0x0C | THREE_SUB, colr(2, icc_rgb(CURVES["gamma"])))
    write("icc_grey_16bit", 0x0C | ONE_16, colr(2, icc_grey(CURVES["gamma"])))
    write("icc_rgb_mixed", 0x0C | SYCC_420, colr(2, icc_rgb(CURVES["gamma"])))
    write("sycc_420", 0x0C | SYCC_420, colr(1, be32(18)))
    write("sycc_422", 0x0C | SYCC_422, colr(1, be32(18)))
    write("sycc_subsampled_all", 0x0C | THREE_SUB, colr(1, be32(18)))
    print(f"wrote {written} seeds to {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
