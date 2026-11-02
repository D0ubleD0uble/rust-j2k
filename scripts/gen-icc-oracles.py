#!/usr/bin/env python3
"""Generate the ICC to sRGB oracles: what `opj_decompress -force-rgb` gives.

OpenJPEG converts an ICC-tagged JP2 file to sRGB with LittleCMS, and the
decoder's `ColourOutput::Srgb` must match it sample for sample. This script
records OpenJPEG's output as a SHA-256 per plane, so the tests need neither
tool and the snapshots stay small. Each plane is hashed as its samples in
raster order, each a 4-byte big-endian signed integer.

Two snapshot files:

- `tests/fixtures/conformance/jp2/icc_variants.json`: ISO/IEC 15444-4 files
  5, 7 and 8 with their ICC profile patched in place: rendering intent 0, 1,
  2 and 3, then intent 3 with the media white point (`wtpt`) set to D65,
  (1.2, 1.1, 1.0) and (0.9, 0.95, 0.7). The JP2 files are rebuilt in memory
  by the test from the committed originals, so only the hashes are new.
  8-bit files are decoded with `-p 8S`; file7 keeps its 16 bits.

- `tests/fixtures/icc/profiles.json`: synthetic profiles over three small
  lossless codestreams this script makes with `opj_compress` and writes to
  `tests/fixtures/icc/`: 256x256 RGB at 8 bits (every R and G level), 64x64
  RGB at 16 bits (random), and 64x64 grey at 8 bits (every level), each
  holding black and white. The profiles are edge cases (degenerate and
  descending curves, every parametric type, extreme matrices and white
  points, black point compensation, the gamma 1.6 threshold of LittleCMS's
  16-bit shortcut), a set of malformed ones, and cases on either side of
  LittleCMS's header checks (`_cmsReadHeader`). For each, OpenJPEG either converts (the hashes are recorded) or leaves the
  image unconverted (LittleCMS refused the profile), which the decoder must
  answer with a typed error.

OpenJPEG 2.5.0 with LittleCMS 2.14 (Ubuntu 24.04's packages) was used.

Usage: python3 scripts/gen-icc-oracles.py [--fuzz-seeds]
(`--fuzz-seeds` only writes the icc_srgb fuzz seeds from icc/profiles.json.)
Requires `opj_compress` and `opj_decompress` on PATH.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import random
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "tests" / "fixtures"
JP2_DIR = FIXTURES / "conformance" / "jp2"
SCRIPT = f"scripts/{Path(__file__).name}"

_spec = importlib.util.spec_from_file_location("gen_jp2", ROOT / "scripts" / "gen-jp2-fixtures.py")
gen = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(gen)


def run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(args, capture_output=True, text=True)


def plane_hash(values: list[int]) -> str:
    return hashlib.sha256(struct.pack(f">{len(values)}i", *values)).hexdigest()


def opj_planes(data: bytes, extra: list[str]) -> list[dict] | None:
    """Decode a JP2 file with opj_decompress; None when it fails."""
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        (tmp / "in.jp2").write_bytes(data)
        result = run("opj_decompress", "-i", str(tmp / "in.jp2"), "-o", str(tmp / "out.pgx"), *extra)
        if result.returncode != 0:
            return None
        planes = sorted(tmp.glob("out*.pgx"), key=lambda p: int(p.stem.rsplit("_", 1)[1]))
        out = []
        for plane in planes:
            w, h, depth, signed, values = gen.read_pgx(plane)
            out.append({"width": w, "height": h, "depth": depth, "signed": signed, "values": values})
        return out


# ---------------------------------------------------------------------------
# Conformance variants
# ---------------------------------------------------------------------------

def colr_profile_span(data: bytes) -> tuple[int, int]:
    """Byte range of the ICC profile in the `colr` box of `jp2h`."""
    for kind, payload in gen.top_boxes(data):
        if kind == b"jp2h":
            start = data.index(payload)
            for k2, p2 in gen.top_boxes(gen.SIGNATURE + payload):
                if k2 == b"colr" and p2[0] == 2:
                    at = start + payload.index(p2) + 3
                    return at, start + payload.index(p2) + len(p2)
    sys.exit("no ICC colr box")


def patch_profile(profile: bytes, intent: int, white: list[float] | None) -> bytes:
    p = bytearray(profile)
    p[64:68] = struct.pack(">I", intent)
    if white is not None:
        count = struct.unpack(">I", p[128:132])[0]
        for i in range(count):
            sig, off, _ = struct.unpack(">4sII", p[132 + 12 * i : 144 + 12 * i])
            if sig == b"wtpt":
                p[off + 8 : off + 20] = struct.pack(">iii", *[round(v * 65536) for v in white])
                break
        else:
            sys.exit("profile has no wtpt tag")
    return bytes(p)


VARIANTS = [
    ("intent0", 0, None),
    ("intent1", 1, None),
    ("intent2", 2, None),
    ("intent3", 3, None),
    ("intent3_d65", 3, [0.9505, 1.0, 1.089]),
    ("intent3_white_1", 3, [1.2, 1.1, 1.0]),
    ("intent3_white_2", 3, [0.9, 0.95, 0.7]),
]


def conformance_variants() -> None:
    entries = []
    for n in (5, 7, 8):
        original = (JP2_DIR / "files" / f"file{n}.jp2").read_bytes()
        start, end = colr_profile_span(original)
        for name, intent, white in VARIANTS:
            profile = patch_profile(original[start:end], intent, white)
            data = original[:start] + profile + original[end:]
            depth = 16 if n == 7 else 8
            extra = ["-force-rgb"] + (["-p", "8S"] if depth == 8 else [])
            planes = opj_planes(data, extra)
            if planes is None or len(planes) != 3:
                sys.exit(f"file{n} {name}: opj_decompress did not give three planes")
            for p in planes:
                if (p["depth"], p["signed"]) != (depth, False):
                    sys.exit(f"file{n} {name}: plane is {p['depth']} bits")
            entries.append({
                "file": f"files/file{n}.jp2",
                "variant": name,
                "intent": intent,
                "white": white,
                "width": planes[0]["width"],
                "height": planes[0]["height"],
                "bit_depth": depth,
                "planes": [plane_hash(p["values"]) for p in planes],
            })
            print(f"file{n} {name}: {[e[:12] for e in entries[-1]['planes']]}")
    snapshot = {
        "provenance": {
            "source": "tests/fixtures/conformance/jp2/files/file{5,7,8}.jp2 with the ICC "
            "profile's rendering intent (header bytes 64..68) and, where `white` is set, its "
            f"wtpt XYZ (s15Fixed16) patched in place, by {SCRIPT}",
            "oracle_command": "opj_decompress -i <variant>.jp2 -o out.pgx -force-rgb "
            "[-p 8S for 8-bit files]",
            "oracle_version": "OpenJPEG 2.5.0 with LittleCMS 2.14",
            "hash": "SHA-256 of each output plane's samples in raster order, each a "
            "4-byte big-endian signed integer",
        },
        "variants": entries,
    }
    (JP2_DIR / "icc_variants.json").write_text(json.dumps(snapshot, indent=2) + "\n")
    print(f"wrote {JP2_DIR / 'icc_variants.json'}")


# ---------------------------------------------------------------------------
# Synthetic profiles
# ---------------------------------------------------------------------------

def s15(v: float) -> bytes:
    return struct.pack(">i", max(-(2**31), min(2**31 - 1, int(round(v * 65536)))))


def xyz(x: float, y: float, z: float) -> bytes:
    return b"XYZ " + b"\0" * 4 + s15(x) + s15(y) + s15(z)


def curv(values: list[int]) -> bytes:
    return b"curv" + b"\0" * 4 + struct.pack(">I", len(values)) + b"".join(struct.pack(">H", v) for v in values)


def para(kind: int, params: list[float]) -> bytes:
    return b"para" + b"\0" * 4 + struct.pack(">HH", kind, 0) + b"".join(s15(p) for p in params)


def profile(tags, space=b"RGB ", intent=0, version=bytes([2, 0x20, 0, 0]), count=None, table=None) -> bytes:
    header = bytearray(128)
    header[8:12] = version
    header[12:16] = b"scnr"
    header[16:20] = space
    header[20:24] = b"XYZ "
    header[36:40] = b"acsp"
    header[64:68] = struct.pack(">I", intent)
    offset = 128 + 4 + 12 * len(tags)
    entries, data = b"", b""
    for sig, body in tags:
        entries += sig + struct.pack(">II", offset + len(data), len(body))
        data += body
        data += b"\0" * ((-len(data)) % 4)
    if table is not None:
        entries = table
    out = bytes(header) + struct.pack(">I", len(tags) if count is None else count) + entries + data
    return struct.pack(">I", len(out)) + out[4:]


SRGB_D50 = ((0.4361, 0.2225, 0.0139), (0.3851, 0.7169, 0.0971), (0.1431, 0.0606, 0.7141))
WIDE = ((0.7161, 0.2587, 0.0), (0.1009, 0.7247, 0.0518), (0.1471, 0.0166, 0.7733))
D50_WHITE = (0.9642, 1, 0.8249)


def rgb_tags(rc=None, gc=None, bc=None, m=SRGB_D50, white=D50_WHITE):
    rc = curv([]) if rc is None else rc
    gc = rc if gc is None else gc
    bc = rc if bc is None else bc
    tags = [(b"rXYZ", xyz(*m[0])), (b"gXYZ", xyz(*m[1])), (b"bXYZ", xyz(*m[2])),
            (b"rTRC", rc), (b"gTRC", gc), (b"bTRC", bc)]
    if white is not None:
        tags.append((b"wtpt", xyz(*white)))
    return tags


def rgb(intent=0, **kw) -> bytes:
    return profile(rgb_tags(**kw), intent=intent)


def grey(kc, white=D50_WHITE, intent=0) -> bytes:
    tags = [(b"kTRC", kc)]
    if white is not None:
        tags.append((b"wtpt", xyz(*white)))
    return profile(tags, space=b"GRAY", intent=intent)


def malformed() -> dict[str, bytes]:
    """The PR #184 review's hostile set: the header and tag table lie."""
    # That review's profiles leave the version at 0.
    v0 = bytes(4)
    base = rgb_tags()
    six = [b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC", b"bTRC"]
    return {
        "bad_identity": profile(base, version=v0),
        "bad_curv_huge_n": profile(rgb_tags(rc=b"curv" + b"\0" * 4 + struct.pack(">I", 0xFFFFFFFF)), version=v0),
        "bad_curv_n_80000000": profile(rgb_tags(rc=b"curv" + b"\0" * 4 + struct.pack(">I", 0x80000000)), version=v0),
        "bad_curv_gamma0": profile(rgb_tags(rc=curv([0])), version=v0),
        "bad_curv_gamma_max": profile(rgb_tags(rc=curv([0xFFFF])), version=v0),
        "bad_curv_two_desc": profile(rgb_tags(rc=curv([65535, 0])), version=v0),
        "bad_para5": profile(rgb_tags(rc=para(5, [1] * 7)), version=v0),
        "bad_para_trunc": profile(rgb_tags(rc=b"para" + b"\0" * 4 + struct.pack(">HH", 4, 0) + s15(1)), version=v0),
        "bad_para4_neg": profile(rgb_tags(rc=para(4, [-32768] * 7)), version=v0),
        "bad_para3_huge": profile(rgb_tags(rc=para(3, [32767, 32767, 32767, 32767, 0.5])), version=v0),
        "bad_para0_neg_gamma": profile(rgb_tags(rc=para(0, [-5])), version=v0),
        "bad_zero_matrix": profile(rgb_tags(m=((0, 0, 0),) * 3), version=v0),
        "bad_huge_matrix": profile(rgb_tags(m=((32767, -32768, 32767),) * 3), version=v0),
        "bad_tag_count_max": profile(base, count=0xFFFFFFFF, version=v0),
        "bad_tag_count_15bit": profile(base, count=0x15555556, version=v0),
        "bad_tag_off_overflow": profile([], count=6, version=v0,
                                        table=b"".join(s + struct.pack(">II", 0xFFFFFFF0, 0x20) for s in six)),
        "bad_tag_size_zero": profile([], count=6, version=v0,
                                     table=b"".join(s + struct.pack(">II", 0, 0) for s in six)),
        "bad_tag_into_header": profile([], count=6, version=v0,
                                       table=b"".join(s + struct.pack(">II", 0, 20) for s in six)),
        "bad_short_header": b"\0" * 100,
        "bad_exact_132": profile([], version=v0)[:132],
        "bad_gray_on_rgb": profile([(b"kTRC", curv([]))], space=b"GRAY", version=v0),
        "bad_rgb_missing_tag": profile(base[:5], version=v0),
        "bad_lut": profile(base + [(b"A2B0", b"mft2" + b"\0" * 40)], version=v0),
        "bad_xyz_short": profile([(b"rXYZ", b"XYZ " + b"\0" * 8)] + base[1:], version=v0),
    }


def edge_rgb() -> dict[str, bytes]:
    ramp17 = [min(65535, int(65535 * (i / 16) ** 2.2)) for i in range(17)]
    wiggle = [0, 9000, 8000, 30000, 29000, 50000, 65535]
    nonzero_black = para(2, [2.2, 1.0, 0.1, 0.05])
    return {
        "identity": rgb(),
        "identity_wide": rgb(m=WIDE),
        "gamma0": rgb(rc=curv([0])),
        "gamma_max": rgb(rc=curv([0xFFFF])),
        "gamma1": rgb(rc=curv([256])),
        "gamma12": rgb(rc=curv([307]), m=WIDE),
        "gamma22": rgb(rc=curv([563]), m=WIDE),
        "two_desc": rgb(rc=curv([65535, 0])),
        "two_asc": rgb(rc=curv([0, 65535]), m=WIDE),
        "ramp17": rgb(rc=curv(ramp17), m=WIDE),
        "wiggle": rgb(rc=curv(wiggle), gc=curv(ramp17), bc=curv([563]), m=WIDE),
        "para0": rgb(rc=para(0, [2.2]), m=WIDE),
        "para1": rgb(rc=para(1, [2.4, 0.947, 0.052]), m=WIDE),
        "para2": rgb(rc=para(2, [2.4, 0.947, 0.052, 0.0]), m=WIDE),
        "para3": rgb(rc=para(3, [2.4, 1 / 1.055, 0.055 / 1.055, 1 / 12.92, 0.04045]), m=WIDE),
        "para4": rgb(rc=para(4, [2.2, 0.9, 0.1, 0.5, 0.2, 0.0, 0.0]), m=WIDE),
        "para1_zero_a": rgb(rc=para(1, [2.0, 0.0, 0.5]), m=WIDE),
        "neg_matrix": rgb(rc=curv([563]), m=((-0.5, 0.2, 0.1), (0.3, -0.7, 0.2), (0.1, 0.2, -0.9))),
        "white_zero": rgb(rc=curv([563]), m=WIDE, white=(0, 0, 0), intent=3),
        "white_negative": rgb(rc=curv([563]), m=WIDE, white=(-1, -2, -0.5), intent=3),
        "white_huge": rgb(rc=curv([563]), m=WIDE, white=(32767, 32767, 32767), intent=3),
        "white_min": rgb(rc=curv([563]), m=WIDE, white=(-32768, -32768, -32768), intent=3),
        "white_small": rgb(rc=curv([563]), m=WIDE, white=(0.001, 0.002, 0.0015), intent=3),
        "white_missing": rgb(rc=curv([563]), m=WIDE, white=None, intent=3),
        "white_near_d50": rgb(rc=curv([563]), m=WIDE, white=(0.9643, 1, 0.8249), intent=3),
        "white_d65_linear": rgb(rc=curv([256]), m=WIDE, white=(0.9505, 1, 1.089), intent=3),
        "black_intent0": rgb(rc=nonzero_black, m=WIDE, intent=0),
        "black_intent1": rgb(rc=nonzero_black, m=WIDE, intent=1),
        "black_intent2": rgb(rc=nonzero_black, gc=curv([563]), m=WIDE, intent=2),
        "black_intent3": rgb(rc=nonzero_black, m=WIDE, intent=3),
        "black_negative": rgb(rc=para(4, [2.2, 1.0, 0.0, 1.0, 0.1, 0.0, -0.01]), m=WIDE, intent=0),
        "black_tiny": rgb(rc=curv([1, 65535]), m=WIDE, intent=0),
        "gamma_1_598": rgb(rc=curv([409]), m=WIDE),
        "gamma_1_602": rgb(rc=curv([410]), m=WIDE),
        "gamma_1_602_srgb": rgb(rc=curv([410])),
        "gamma_mixed": rgb(rc=curv([300]), gc=curv([420]), bc=curv([700]), m=WIDE),
        "table_gamma_155": rgb(rc=curv([int(65535 * (i / 255) ** 1.55) for i in range(256)]), m=WIDE),
        "table_noisy": rgb(rc=curv([int(65535 * (i / 15) ** (1.0 + (i % 2))) for i in range(16)]), m=WIDE),
        "v4_header": profile(rgb_tags(rc=curv([563]), m=WIDE), version=bytes([4, 0x30, 0, 0])),
    }


def with_size(icc: bytes, size: int) -> bytes:
    """`icc` with the header's profile size set to `size`."""
    return struct.pack(">I", size) + icc[4:]


def header() -> dict[str, bytes]:
    """The checks of LittleCMS's `_cmsReadHeader` (cmsio0.c): the `acsp`
    signature, the version, the tag count, entries it skips (zero offset or
    size, or past the header's declared size) and duplicate signatures."""
    base = rgb_tags(rc=curv([563]), m=WIDE)
    filler = [(b"z%03d" % i, b"\0" * 4) for i in range(100 - len(base))]
    plain = profile(base)
    wtpt_last = profile(rgb_tags(rc=curv([563]), m=WIDE, white=(0.9, 0.95, 0.7)), intent=3)
    lut_last = profile(base + [(b"A2B0", b"mft2" + b"\0" * 40)])
    zero_dup = bytearray(profile(base + [(b"rTRC", curv([256]))]))
    # The second rTRC entry is the eighth: give it a zero offset.
    zero_dup[132 + 7 * 12 + 4:132 + 7 * 12 + 8] = bytes(4)
    return {
        "hdr_bad_magic": plain[:36] + b"acsP" + plain[40:],
        "hdr_version_5_0": profile(base, version=bytes([5, 0, 0xFF, 0xFF])),
        "hdr_version_5_1": profile(base, version=bytes([5, 0x10, 0, 0])),
        "hdr_version_4_ff": profile(base, version=bytes([4, 0xFF, 0, 0])),
        "hdr_version_ff": profile(base, version=bytes([0xFF, 0, 0, 0])),
        "hdr_tags_100": profile(base + filler),
        "hdr_tags_101": profile(base + filler + [(b"zzzz", b"\0" * 4)]),
        "hdr_dup_rtrc": profile(base + [(b"rTRC", curv([256]))]),
        "hdr_dup_rxyz": profile(base + [(b"rXYZ", xyz(*WIDE[1]))]),
        "hdr_dup_skipped": bytes(zero_dup),
        "hdr_size_0": with_size(plain, 0),
        "hdr_size_140": with_size(plain, 140),
        "hdr_size_huge": with_size(plain, 0xFFFFFFFF),
        "hdr_size_drops_wtpt": with_size(wtpt_last, len(wtpt_last) - 20),
        "hdr_size_drops_lut": with_size(lut_last, len(lut_last) - 48),
    }


def edge_grey() -> dict[str, bytes]:
    ramp17 = [min(65535, int(65535 * (i / 16) ** 2.2)) for i in range(17)]
    return {
        "grey_gamma18": grey(curv([461])),
        "grey_table": grey(curv(ramp17)),
        "grey_descending": grey(curv([65535, 0])),
        "grey_gamma0": grey(curv([0])),
        "grey_para4_negative": grey(para(4, [-32768] * 7)),
        "grey_black_intent0": grey(para(2, [2.2, 1.0, 0.1, 0.05]), intent=0),
        "grey_white_zero": grey(curv([563]), white=(0, 0, 0), intent=3),
        "grey_white_huge": grey(curv([563]), white=(32767, 1, -32768), intent=3),
        "grey_white_d65": grey(curv([256]), white=(0.9505, 1, 1.089), intent=3),
        "grey_white_2": grey(curv([461]), white=(0.9, 0.95, 0.7), intent=3),
    }


ICC_DIR = FIXTURES / "icc"
CORNERS = [(0, 0, 0), (255, 255, 255), (255, 0, 0), (0, 255, 0), (0, 0, 255),
           (0, 255, 255), (255, 0, 255), (255, 255, 0), (128, 128, 128)]


def rgb8_planes() -> tuple[int, list[list[int]], str]:
    """256x256: R = x, G = y, B = (37x + 91y) mod 256, so every R and G level
    meets a spread of B; the first pixels are black, white, the primaries,
    the secondaries and mid grey."""
    size = 256
    pixels = [(x, y, (37 * x + 91 * y) % 256) for y in range(size) for x in range(size)]
    pixels[: len(CORNERS)] = CORNERS
    return size, [[p[c] for p in pixels] for c in range(3)], (
        "R = x, G = y, B = (37x + 91y) mod 256, the first nine pixels black, white, "
        "R, G, B, C, M, Y and grey 128")


def rgb16_planes() -> tuple[int, list[list[int]], str]:
    """64x64 random 16-bit samples, the first pixels the corners scaled by 257."""
    size = 64
    rng = random.Random(187)
    pixels = [tuple(rng.randrange(65536) for _ in range(3)) for _ in range(size * size)]
    pixels[: len(CORNERS)] = [tuple(257 * v for v in c) for c in CORNERS]
    return size, [[p[c] for p in pixels] for c in range(3)], (
        "random.Random(187) samples, the first nine pixels the 8-bit corners times 257")


def grey8_planes() -> tuple[int, list[list[int]], str]:
    """64x64: the first 256 samples every grey level, then random ones."""
    size = 64
    rng = random.Random(188)
    values = list(range(256)) + [rng.randrange(256) for _ in range(size * size - 256)]
    return size, [values], "grey 0..255, then random.Random(188) samples"


CODESTREAMS = {
    "rgb8": ("rgb8_lossless.j2k", 8, rgb8_planes),
    "rgb16": ("rgb16_lossless.j2k", 16, rgb16_planes),
    "grey8": ("grey8_lossless.j2k", 8, grey8_planes),
}


def make_codestreams() -> dict[str, dict]:
    out = {}
    ICC_DIR.mkdir(exist_ok=True)
    for key, (name, depth, make) in CODESTREAMS.items():
        size, planes, description = make()
        components = len(planes)
        fmt = ">H" if depth == 16 else "B"
        # opj_compress reads a `.raw` file planar and big-endian.
        raw = b"".join(struct.pack(fmt, v) for plane in planes for v in plane)
        flags = ["-F", f"{size},{size},{components},{depth},u", "-n", "3"]
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            (tmp / "in.raw").write_bytes(raw)
            result = run("opj_compress", "-i", str(tmp / "in.raw"), "-o", str(tmp / name), *flags)
            if result.returncode != 0:
                sys.exit(f"opj_compress failed: {result.stderr}")
            codestream = (tmp / name).read_bytes()
        decoded, _ = gen.opj_decode(codestream, ".j2k", verified=True)
        if [p["samples"] for p in decoded] != planes:
            sys.exit(f"{name}: the codestream did not round-trip losslessly")
        (ICC_DIR / name).write_bytes(codestream)
        out[key] = {
            "file": f"icc/{name}",
            "command": f"opj_compress -i in.raw -o {name} " + " ".join(flags)
            + f" (in.raw: {size}x{size}, {description})",
            "codestream": codestream,
            "planes": planes,
            "components": components,
            "depth": depth,
            "size": size,
        }
        print(f"wrote icc/{name}")
    return out


def wrap(cs: dict, icc: bytes) -> bytes:
    geometry = {"width": cs["size"], "height": cs["size"], "components": cs["components"],
                "depth": cs["depth"], "signed": False}
    colr = gen.box(b"colr", bytes([2, 0, 0]) + icc)
    return gen.jp2(cs["codestream"], geometry, [], colr=colr)


def synthetic() -> None:
    codestreams = make_codestreams()
    groups = [("rgb8", {**malformed(), **edge_rgb(), **header()}),
              ("rgb16", {**malformed(), **edge_rgb(), **header()}),
              ("grey8", edge_grey())]
    cases = []
    for key, profiles in groups:
        cs = codestreams[key]
        extra = ["-force-rgb"] + (["-p", "8S"] if cs["depth"] == 8 else [])
        for name, icc in profiles.items():
            planes = opj_planes(wrap(cs, icc), extra)
            entry = {"name": name, "codestream": key, "profile": icc.hex()}
            values = None if planes is None else [p["values"] for p in planes]
            as_coded = cs["planes"] * (3 // cs["components"])
            if values is None or values == as_coded:
                entry["openjpeg"] = "unconverted"
            else:
                if len(planes) != 3 or any(p["depth"] != cs["depth"] for p in planes):
                    sys.exit(f"{key} {name}: unexpected output layout")
                entry["openjpeg"] = "converted"
                entry["planes"] = [plane_hash(v) for v in values]
            cases.append(entry)
            print(f"{key} {name}: {entry['openjpeg']}")
    snapshot = {
        "provenance": {
            "source": f"profiles built by {SCRIPT}, each wrapped in a JP2 file (colr method 2) "
            "around one of the codestreams below",
            "oracle_command": "opj_decompress -i <case>.jp2 -o out.pgx -force-rgb "
            "[-p 8S for 8-bit codestreams]",
            "oracle_version": "OpenJPEG 2.5.0 with LittleCMS 2.14",
            "hash": "SHA-256 of each output plane's samples in raster order, each a "
            "4-byte big-endian signed integer",
            "unconverted": "opj_decompress failed or gave the samples as coded: LittleCMS "
            "refused the profile",
        },
        "codestreams": {k: {"file": v["file"], "command": v["command"]} for k, v in codestreams.items()},
        "cases": cases,
    }
    (ICC_DIR / "profiles.json").write_text(json.dumps(snapshot, indent=2) + "\n")
    print(f"wrote {ICC_DIR / 'profiles.json'}")


def fuzz_seeds() -> None:
    """Write the profiles of icc/profiles.json as seeds for the icc_srgb fuzz
    target: one byte for the codestream (0 RGB 8-bit, 1 RGB 16-bit, 2 grey),
    then the profile."""
    out = ROOT / "fuzz" / "corpus" / "icc_srgb"
    out.mkdir(parents=True, exist_ok=True)
    selector = {"rgb8": 0, "rgb16": 1, "grey8": 2}
    cases = json.loads((ICC_DIR / "profiles.json").read_text())["cases"]
    for case in cases:
        seed = bytes([selector[case["codestream"]]]) + bytes.fromhex(case["profile"])
        (out / f"{case['codestream']}_{case['name']}").write_bytes(seed)
    print(f"wrote {len(cases)} seeds to {out}")


def main() -> None:
    if sys.argv[1:] == ["--fuzz-seeds"]:
        fuzz_seeds()
        return
    for tool in ("opj_compress", "opj_decompress"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH; install OpenJPEG to regenerate the oracles")
    conformance_variants()
    synthetic()


if __name__ == "__main__":
    main()
