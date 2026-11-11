#!/usr/bin/env python3
"""Write seeds for the `jp2_boxes` fuzz target into fuzz/corpus/jp2_boxes/.

Run from the repository root:

    python3 scripts/gen-jp2-fuzz-seeds.py

The target takes `variant byte + body` and appends one of the pinned
codestreams (see fuzz/fuzz_targets/jp2_boxes.rs): bits 4-6 of the variant pick
it, and bit 7 decodes one resolution level down. Each ISO/IEC 15444-4 JP2
reader file (tests/fixtures/conformance/jp2/files/fileN.jp2) and each
synthetic JP2 fixture (tests/fixtures/jp2_*.jp2) gives two seeds, each paired
with the pinned 8-bit single-component codestream if the file has one
component, the 16-bit four-component one if it has four, else the 8-bit
un-sub-sampled three-component one, so that a file's ICC profile and its
`cdef` reach the `ColourOutput::Srgb` conversion:

- `NAME_prefix`: variant 0x20, 0x60 or 0x30, and the file's bytes before its first
  `jp2c` box, signature included, with the `ihdr` rewritten to the pinned
  codestream's size and component count so the seed gets past the check
  against SIZ. The fuzzer starts from a real top-level layout.
- `NAME_jp2h`: variant 0x2C, 0x6C or 0x3C (pinned signature, `ftyp`, and an `ihdr`
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

Plus seeds for the metadata boxes, on the 8-bit single-component codestream
with a pinned prefix (variant 0x04 | codestream << 4, body a `jp2h` holding a
matching `ihdr` and a greyscale `colr`, then the boxes):

- `ipr_uinf_before`: a `jp2i` and a `uinf` (a `ulst` of two UUIDs and a
  NUL-terminated `url `) before the codestream;
- `ipr_uinf_after`: the same body with `jp2c` form 3, so the boxes come again
  after the codestream;
- `uinf_unterminated`: a `uinf` with one UUID, an unknown box, and a `url `
  with no NUL.

Plus one seed per refusal and pass-through of `src/jp2/srgb.rs` that a
pinned codestream reaches (`srgb_branch_seeds`, variant 0x0C | codestream
<< 4, 0x80 added for the `_reduced` ones):

- sRGB and greyscale: declared over a 4:2:0 or 4:2:2 codestream at either
  origin, even with a `cdef` naming colour 1 only (`*_as_sycc`,
  `srgb_420_colour1_cdef`, refused); channels of mixed depth or sub-sampling
  from a `pclr` (`srgb_mixed_*`, refused); RGBA on the 16-bit codestream
  (`srgb_rgba_16bit`, passes through).
- sYCC: one channel (passes through), 4:4:4, the even-origin 4:2:0, RGBA on
  the 16-bit codestream, and the 4:2:0 and 4:2:2 ones reduced (convert); two
  colours from a `cdef`, mixed depth and signed channels from a `pclr`
  (refused).
- CMYK (`enum_cmyk_*`) and no `colr` (`no_colr_three`): refused on three
  channels, passed through on one.
- `icc_*`: the RGB profile with one header field, tag or curve changed, so
  each refusal of the profile reader and of LittleCMS's checks has a seed
  (short, no `acsp`, class, version, tag count, table, duplicate and
  out-of-range tags, named colour and link classes, intent, connection
  space, each lookup table type, XYZ and curve tags of the wrong type or
  cut short, `para` above type 4, and the Corbis and Monaco types); the
  CMYK and YCbCr colour spaces; an RGB profile on one channel, two colours,
  channels of mixed depth (OpenJPEG's "AFL test"), 12-bit or signed
  channels, and the 16-bit codestream; a refused profile on a 4:2:0 or
  4:2:2 codestream; a monochrome profile on three channels or on no colour
  channel.

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
    4: ("multicomponent_16bit_lossless.j2k", 0x60),
}

# Variant bits for each pinned codestream (see the target), and the bit that
# decodes one resolution level down.
ONE_16, THREE_SUB, ONE_8, THREE_8, SYCC_420, SYCC_422, FOUR_16, SYCC_420_EVEN = (
    n << 4 for n in range(8)
)
REDUCE = 0x80


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


def box(kind, payload):
    return be32(8 + len(payload)) + kind + payload


def uinf(ids, location, extra=b""):
    """A `uinf` box: a `ulst` of `ids`, `extra` boxes, then a `url ` (VERS 0,
    FLAG 0) holding `location`."""
    ulst = box(b"ulst", struct.pack(">H", len(ids)) + b"".join(ids))
    return box(b"uinf", ulst + extra + box(b"url ", b"\0\0\0\0" + location))


def patch(profile, at, value):
    """`profile` with the four bytes at `at` replaced by `value`."""
    out = bytearray(profile)
    out[at:at + 4] = value
    return bytes(out)


def tag_entry(profile, signature):
    """The tag table offset of `signature`'s entry."""
    count = struct.unpack(">I", profile[128:132])[0]
    for i in range(count):
        at = 132 + 12 * i
        if profile[at:at + 4] == signature:
            return at
    raise KeyError(signature)


def rgb_tags(**override):
    """The RGB profile's tags, with any replaced by keyword (`rTRC=...`)."""
    tags = {
        b"wtpt": xyz_tag(*D50),
        b"rXYZ": xyz_tag(*RXYZ), b"gXYZ": xyz_tag(*GXYZ), b"bXYZ": xyz_tag(*BXYZ),
        b"rTRC": CURVES["gamma"], b"gTRC": CURVES["gamma"], b"bTRC": CURVES["gamma"],
    }
    for name, payload in override.items():
        tags[name.encode()] = payload
    return [(sig, payload) for sig, payload in tags.items() if payload is not None]


def lut(kind, inputs=3, grid=2):
    """A lookup table tag of type `kind` (`mft1`, `mft2`, `mAB `, `mpet`) that
    passes the checks LittleCMS makes before it reads the table: `inputs`
    channels in, three out."""
    if kind == b"mpet":
        return kind + b"\0\0\0\0" + struct.pack(">HHI", inputs, 3, 0)
    head = kind + b"\0\0\0\0" + bytes([inputs, 3, grid, 0])
    if kind == b"mAB ":
        return head + b"\0" * 20
    head += s15(1) + s15(0) * 3 + s15(1) + s15(0) * 3 + s15(1)
    clut = 3 * grid ** inputs
    if kind == b"mft1":
        return head + b"\0" * (256 * inputs + clut + 256 * 3)
    return head + struct.pack(">HH", 2, 2) + b"\0" * (2 * (inputs * 2 + clut + 3 * 2))


def pclr_box(depths, entries=256):
    """A `pclr` with one column per `Bi` in `depths` (entry j is j, masked to the
    column's depth) and the identity `cmap` over component 0."""
    out = struct.pack(">HB", entries, len(depths)) + bytes(depths)
    for j in range(entries):
        for b in depths:
            bits = (b & 0x7F) + 1
            out += (j % (1 << bits)).to_bytes((bits + 7) // 8, "big")
    cmap = b"".join(struct.pack(">HBB", 0, 1, i) for i in range(len(depths)))
    return box(b"pclr", out) + box(b"cmap", cmap)


def cdef_box(*entries):
    """A `cdef` from (Typ, Asoc) pairs, one per channel in order."""
    body = struct.pack(">H", len(entries))
    body += b"".join(struct.pack(">HHH", n, typ, asoc) for n, (typ, asoc) in enumerate(entries))
    return box(b"cdef", body)


SRGB, GREY, SYCC = (colr(1, be32(n)) for n in (16, 17, 18))
RGBA = cdef_box((0, 1), (0, 2), (0, 3), (1, 0))
TWO_COLOURS = cdef_box((0, 1), (0, 2), (65535, 65535))


def srgb_branch_seeds(write):
    """One seed per refusal and pass-through of `src/jp2/srgb.rs` that a
    pinned codestream reaches (variant 0x0C | codestream, body the `jp2h`
    children after the pinned `ihdr`)."""
    hdr = 0x0C
    rgb = icc_rgb(CURVES["gamma"])
    # sRGB and greyscale.
    write("srgb_420_as_sycc", hdr | SYCC_420, SRGB)
    write("grey_422_as_sycc", hdr | SYCC_422, GREY)
    write("srgb_420_even_as_sycc", hdr | SYCC_420_EVEN, SRGB)
    write("srgb_420_colour1_cdef", hdr | SYCC_420,
          SRGB + cdef_box((0, 1), (65535, 65535), (65535, 65535)))
    write("srgb_mixed_depth", hdr | ONE_8, SRGB + pclr_box([7, 7, 3]))
    # Channels 0 and 2 from component 0 (1x1), channel 1 from component 1
    # (2x2): not a layout OpenJPEG converts as sYCC, and the sub-sampling
    # differs.
    mixed = struct.pack(">HB", 256, 3) + bytes([7, 7, 7])
    mixed += b"".join(bytes([j, j, j]) for j in range(256))
    mixed_cmap = struct.pack(">HBBHBBHBB", 0, 0, 0, 1, 0, 0, 0, 1, 2)
    write("srgb_mixed_sampling", hdr | SYCC_420,
          SRGB + box(b"pclr", mixed) + box(b"cmap", mixed_cmap))
    write("srgb_rgba_16bit", hdr | FOUR_16, SRGB + RGBA)
    # sYCC.
    write("sycc_one_channel", hdr | ONE_8, SYCC)
    write("sycc_444", hdr | THREE_8, SYCC)
    write("sycc_two_colours", hdr | THREE_8, SYCC + TWO_COLOURS)
    write("sycc_mixed_depth", hdr | ONE_8, SYCC + pclr_box([7, 7, 3]))
    write("sycc_signed", hdr | ONE_8, SYCC + pclr_box([0x87, 0x87, 0x87]))
    write("sycc_420_even", hdr | SYCC_420_EVEN, SYCC)
    write("sycc_rgba_16bit", hdr | FOUR_16, SYCC + RGBA)
    for name, cs in [("420", SYCC_420), ("422", SYCC_422), ("420_even", SYCC_420_EVEN)]:
        write(f"sycc_{name}_reduced", REDUCE | hdr | cs, SYCC)
    # Another enumerated colour space, and none.
    write("enum_cmyk_three", hdr | THREE_8, colr(1, be32(12)))
    write("enum_cmyk_one", hdr | ONE_8, colr(1, be32(12)))
    write("no_colr_three", hdr | THREE_8, b"")
    # The profile reader's refusals (`Profile::parse`), on the RGB profile.
    write("icc_short", hdr | THREE_8, colr(2, rgb[:100]))
    write("icc_no_acsp", hdr | THREE_8, colr(2, patch(rgb, 36, b"xxxx")))
    write("icc_bad_class", hdr | THREE_8, colr(2, patch(rgb, 12, b"xxxx")))
    write("icc_version_6", hdr | THREE_8, colr(2, patch(rgb, 8, bytes([6, 0, 0, 0]))))
    write("icc_101_tags", hdr | THREE_8, colr(2, patch(rgb, 128, be32(101))))
    write("icc_table_past_end", hdr | THREE_8, colr(2, patch(rgb, 128, be32(99))))
    at = tag_entry(rgb, b"gTRC")
    write("icc_duplicate_tag", hdr | THREE_8, colr(2, patch(rgb, at, b"rTRC")))
    write("icc_tag_past_end", hdr | THREE_8, colr(2, patch(rgb, at + 8, be32(1 << 20))))
    # `input_profile`: class, intent, connection space, lookup tables.
    write("icc_named_colour", hdr | THREE_8, colr(2, patch(rgb, 12, b"nmcl")))
    write("icc_link_no_lut", hdr | THREE_8, colr(2, patch(rgb, 12, b"link")))
    write("icc_zero_class", hdr | THREE_8, colr(2, patch(rgb, 12, b"\0\0\0\0")))
    write("icc_intent_5", hdr | THREE_8, colr(2, patch(rgb, 64, be32(5))))
    write("icc_intent_12", hdr | THREE_8, colr(2, patch(rgb, 64, be32(12))))
    write("icc_intent_3", hdr | THREE_8, colr(2, patch(rgb, 64, be32(3))))
    display = patch(patch(rgb, 12, b"mntr"), 64, be32(3))
    write("icc_intent_3_display_v2", hdr | THREE_8, colr(2, display))
    write("icc_pcs_unknown", hdr | THREE_8, colr(2, patch(rgb, 20, b"xxxx")))
    write("icc_pcs_lab", hdr | THREE_8, colr(2, patch(rgb, 20, b"Lab ")))
    for kind, tag in [(b"mft1", "A2B0"), (b"mft2", "A2B0"), (b"mAB ", "A2B0"), (b"mpet", "D2B0")]:
        name = kind.decode().strip()
        profile = icc(b"RGB ", rgb_tags(**{tag: lut(kind)}))
        write(f"icc_lut_{name}", hdr | THREE_8, colr(2, profile))
    grey_lut = icc(b"GRAY", [(b"wtpt", xyz_tag(*D50)), (b"A2B0", lut(b"mft2", inputs=1))])
    write("icc_grey_lut_mft2", hdr | ONE_8, colr(2, grey_lut))
    write("icc_lut_grid_1", hdr | THREE_8, colr(2, icc(b"RGB ", rgb_tags(A2B0=lut(b"mft2", grid=1)))))
    write("icc_lut_short", hdr | THREE_8, colr(2, icc(b"RGB ", rgb_tags(A2B0=lut(b"mft2")[:60]))))
    write("icc_lut_wrong_inputs",
          hdr | THREE_8, colr(2, icc(b"RGB ", rgb_tags(A2B0=lut(b"mft2", inputs=1)))))
    abstract = patch(icc(b"RGB ", rgb_tags(A2B0=lut(b"mft2"))), 12, b"abst")
    write("icc_abstract_lut", hdr | THREE_8, colr(2, abstract))
    # Tags: XYZ, curves. LittleCMS reads a value past its tag's end into the
    # bytes after it, so a tag cut short is the last one (`bTRC`).
    write("icc_xyz_not_xyz", hdr | THREE_8, colr(2, icc(b"RGB ", rgb_tags(rXYZ=CURVES["gamma"]))))
    corbis = bytes.fromhex("17a505b8") + xyz_tag(*RXYZ)[4:]
    write("icc_xyz_corbis", hdr | THREE_8, colr(2, icc(b"RGB ", rgb_tags(rXYZ=corbis))))
    write("icc_missing_tag", hdr | THREE_8, colr(2, icc(b"RGB ", rgb_tags(bXYZ=None))))
    write("icc_tag_under_8", hdr | THREE_8, colr(2, icc(b"RGB ", rgb_tags(bTRC=b"curv"))))
    monaco = bytes.fromhex("9478ee00") + CURVES["gamma"][4:]
    write("icc_curve_monaco", hdr | THREE_8, colr(2, icc(b"RGB ", rgb_tags(rTRC=monaco))))
    short = b"curv\0\0\0\0" + be32(10) + b"\0\x01"
    write("icc_curve_short", hdr | THREE_8, colr(2, icc(b"RGB ", rgb_tags(bTRC=short))))
    long = b"curv\0\0\0\0" + be32(0x8000)
    write("icc_curve_too_long", hdr | THREE_8, colr(2, icc(b"RGB ", rgb_tags(rTRC=long))))
    write("icc_para_5", hdr | THREE_8, colr(2, icc(b"RGB ", rgb_tags(rTRC=para(5, 2.2)))))
    write("icc_para_short", hdr | THREE_8, colr(2, icc(b"RGB ", rgb_tags(bTRC=para(4, 2.2)))))
    unknown = b"xxxx\0\0\0\0" + be32(0)
    write("icc_curve_unknown_type", hdr | THREE_8, colr(2, icc(b"RGB ", rgb_tags(rTRC=unknown))))
    # Colour spaces, and the channels each form needs.
    write("icc_cmyk_space", hdr | THREE_8, colr(2, patch(rgb, 16, b"CMYK")))
    write("icc_ycbr_three", hdr | THREE_8, colr(2, patch(rgb, 16, b"YCbr")))
    write("icc_ycbr_one", hdr | ONE_8, colr(2, patch(rgb, 16, b"YCbr")))
    write("icc_rgb_one", hdr | ONE_8, colr(2, rgb))
    write("icc_rgb_two_colours", hdr | THREE_8, colr(2, rgb) + TWO_COLOURS)
    write("icc_rgb_first_four_differ", hdr | ONE_8, colr(2, rgb) + pclr_box([7, 7, 3]))
    # Five palette channels, channel 2 at 4 bits. A second `cdef` entry for
    # colour 3 swaps channel 4 into position 2 and relabels it opacity, which
    # leaves colour 3 (4-bit) at position 4: the first four positions agree,
    # so the AFL test passes, and the colours' formats differ.
    after_afl = [(0, 0, 1), (1, 0, 2), (2, 0, 3), (4, 0, 3), (4, 1, 0), (3, 65535, 65535)]
    after_afl = struct.pack(">H", len(after_afl)) + b"".join(
        struct.pack(">HHH", *e) for e in after_afl)
    write("icc_rgb_uniform_after_afl", hdr | ONE_8,
          colr(2, rgb) + pclr_box([7, 7, 3, 7, 7]) + box(b"cdef", after_afl))
    write("icc_rgb_12bit", hdr | ONE_8, colr(2, rgb) + pclr_box([11, 11, 11]))
    write("icc_rgb_signed", hdr | ONE_8, colr(2, rgb) + pclr_box([0x87, 0x87, 0x87]))
    write("icc_rgb_16bit", hdr | FOUR_16, colr(2, rgb))
    write("icc_rgb_16bit_reduced", REDUCE | hdr | FOUR_16, colr(2, rgb))
    write("icc_rgb_reduced", REDUCE | hdr | THREE_8, colr(2, rgb))
    write("icc_refused_420", hdr | SYCC_420, colr(2, rgb[:100]))
    write("icc_cmyk_422", hdr | SYCC_422, colr(2, patch(rgb, 16, b"CMYK")))
    grey = icc_grey(CURVES["gamma"])
    write("icc_grey_three", hdr | THREE_8, colr(2, grey))
    write("icc_grey_no_colour", hdr | ONE_8, colr(2, grey) + cdef_box((1, 0)))
    write("icc_grey_tinted_reduced", REDUCE | hdr | ONE_8,
          colr(2, icc_grey_tinted(CURVES["gamma"])))
    # Regression: a one-column `pclr` with no `cmap`, the body repeated after
    # `jp2c` (variant bits 0-1 = 3). The default decode drops the palette;
    # `raw_channels` keeps it and refuses the second `pclr`.
    no_cmap = struct.pack(">HB", 256, 1) + bytes([7]) + bytes(range(256))
    write("pclr_no_cmap_twice", 0x03 | hdr | ONE_8, GREY + box(b"pclr", no_cmap))


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
        (height, width, nc), bit = pinned[components if components in pinned else 3]

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
    srgb_branch_seeds(write)

    (height, width, nc), _ = pinned[1]
    ihdr = box(b"ihdr", struct.pack(">IIH", height, width, nc) + bytes([7, 7, 0, 0]))
    header = box(b"jp2h", ihdr + colr(1, be32(17)))
    ipr = box(b"jp2i", b"<rights>none</rights>")
    two = uinf([bytes(range(16)), b"\xab" * 16], b"http://example.com/ids\0")
    write("ipr_uinf_before", 0x04 | ONE_8, header + ipr + two)
    write("ipr_uinf_after", 0x07 | ONE_8, header + ipr + two)
    one = uinf([b"\x42" * 16], b"file:label.lbl", box(b"free", b""))
    write("uinf_unterminated", 0x04 | ONE_8, header + one)
    print(f"wrote {written} seeds to {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
