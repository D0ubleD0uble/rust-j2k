#!/usr/bin/env python3
"""Compare the ICC to sRGB conversion with the system LittleCMS, pixel for pixel.

An optional check, not part of `cargo test`. The tests grade the decoder
against SHA-256 hashes of what OpenJPEG gave on a few images. This runs the
decoder's port of LittleCMS against LittleCMS itself, called through ctypes
with the arguments OpenJPEG passes (`cmsCreateTransform` from the profile to
`cmsCreate_sRGBProfile()`, the intent from the profile header, flags 0), on:

- the full 8-bit RGB cube, all 2^24 colors, for each RGB profile on an 8-bit
  codestream (`TYPE_RGB_8`);
- a fixed-seed sample of one million 16-bit RGB pixels for each RGB profile
  on a 16-bit codestream (`TYPE_RGB_16`);
- all 256 levels for each monochrome profile (`TYPE_GRAY_8`, three channels
  out).

The profiles are every case in `tests/fixtures/icc/profiles.json` and in
`tests/fixtures/conformance/jp2/icc_variants.json` (files 5, 7 and 8 with the
rendering intent and white point patched, as `tests/jp2_icc.rs` does). The
pixels go through the decoder as a lossless codestream made with
`opj_compress`, wrapped in a JP2 file holding the profile, by
`examples/icc_convert.rs`. For each case it prints the number of pixels where
the two differ. The bar is 0 on every case.

The input format follows OpenJPEG's `color_apply_icc_profile` (color.c): a
monochrome profile always uses `TYPE_GRAY_8` to `TYPE_RGB_8`, an RGB profile
uses the sample depth and needs three or more components, and any other color
space leaves the image alone (see `openjpeg_route`).

Two kinds of case are not compared pixel by pixel, and the script checks them
instead. Where OpenJPEG leaves the samples as coded (LittleCMS refuses the
profile or the transform, or OpenJPEG returns before it calls LittleCMS), the
decoder must give back the samples unchanged. Where the decoder declines the
profile with `Error::Unsupported` (a lookup table profile, say), the case must
be one that `REFUSED_BY_US` in `tests/jp2_icc.rs` lists, and every listed case
must be declined. On a full run (no `--only`), a listed name that matches no
case is a surprise too. LittleCMS must also refuse exactly the profiles that
`icc/profiles.json` records as unconverted by OpenJPEG. Any other result is
printed as a SURPRISE and makes the script exit non-zero, as does any mismatch.

Assumes the platform of the oracles: LittleCMS 2.14 (Ubuntu 24.04's
`liblcms2-2`), x86-64. Needs `opj_compress` on PATH, the Rust toolchain, and
`liblcms2.so.2`. The full run is heavy (about a thousand 16-million-pixel
decodes at worst), so run it under `nice`.

Usage: python3 scripts/check-icc-exhaustive.py [--only SUBSTRING] [--work DIR]
  --only SUBSTRING  run only the cases whose name contains SUBSTRING
  --work DIR        keep the generated codestreams in DIR and reuse them
"""

from __future__ import annotations

import argparse
import array
import ctypes
import ctypes.util
import json
import re
import random
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from oracle_tools import require_lcms, require_openjpeg

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "tests" / "fixtures"
EXAMPLE = ROOT / "target" / "release" / "examples" / "icc_convert"

# lcms2.h: COLORSPACE_SH | CHANNELS_SH | BYTES_SH
TYPE_RGB_8 = 0x40019
TYPE_RGB_16 = 0x4001A
TYPE_GRAY_8 = 0x30009
SEED_16BIT = 191
SAMPLE_16BIT = 1 << 20


class Lcms:
    """The few LittleCMS 2 functions OpenJPEG's `color_apply_icc_profile` uses."""

    def __init__(self) -> None:
        path = ctypes.util.find_library("lcms2") or "liblcms2.so.2"
        self.lib = lib = ctypes.CDLL(path)
        vp, u32 = ctypes.c_void_p, ctypes.c_uint32
        lib.cmsOpenProfileFromMem.restype = vp
        lib.cmsOpenProfileFromMem.argtypes = [ctypes.c_char_p, u32]
        lib.cmsCreate_sRGBProfile.restype = vp
        lib.cmsGetHeaderRenderingIntent.restype = u32
        lib.cmsGetHeaderRenderingIntent.argtypes = [vp]
        lib.cmsCreateTransform.restype = vp
        lib.cmsCreateTransform.argtypes = [vp, u32, vp, u32, u32, u32]
        lib.cmsDoTransform.restype = None
        lib.cmsDoTransform.argtypes = [vp, vp, vp, u32]
        lib.cmsDeleteTransform.argtypes = [vp]
        lib.cmsCloseProfile.argtypes = [vp]
        lib.cmsGetEncodedCMMversion.restype = ctypes.c_int
        self.version = lib.cmsGetEncodedCMMversion()
        self.srgb = lib.cmsCreate_sRGBProfile()

    def convert(self, profile: bytes, in_type: int, out_type: int,
                pixels: bytes, count: int, out_size: int) -> bytes | None:
        """The converted pixels, or None where LittleCMS refuses the profile
        or the transform (OpenJPEG then leaves the image as coded)."""
        lib = self.lib
        handle = lib.cmsOpenProfileFromMem(profile, len(profile))
        if not handle:
            return None
        try:
            intent = lib.cmsGetHeaderRenderingIntent(handle)
            xform = lib.cmsCreateTransform(handle, in_type, self.srgb, out_type, intent, 0)
            if not xform:
                return None
            try:
                out = ctypes.create_string_buffer(out_size)
                lib.cmsDoTransform(xform, pixels, out, count)
                return out.raw
            finally:
                lib.cmsDeleteTransform(xform)
        finally:
            lib.cmsCloseProfile(handle)


def run(*args: object, **kw) -> subprocess.CompletedProcess:
    return subprocess.run([str(a) for a in args], capture_output=True, text=True, **kw)


class Kind:
    """One input image: its pixels as lcms takes them and as a codestream."""

    def __init__(self, name: str, in_type: int, out_type: int, count: int,
                 pixels: bytes, codestream: Path) -> None:
        self.name = name
        self.in_type = in_type
        self.out_type = out_type
        self.count = count
        self.pixels = pixels
        self.codestream = codestream
        # Bytes per pixel in and out.
        self.in_stride = {TYPE_RGB_8: 3, TYPE_RGB_16: 6, TYPE_GRAY_8: 1}[in_type]
        self.stride = {TYPE_RGB_8: 3, TYPE_RGB_16: 6}[out_type]
        self.out_size = count * self.stride


def compress(work: Path, name: str, magic: bytes, width: int, height: int,
             maxval: int, body: bytes) -> Path:
    out = work / f"{name}.j2k"
    if out.exists():
        return out
    pnm = work / f"{name}.{'pgm' if magic == b'P5' else 'ppm'}"
    pnm.write_bytes(magic + f"\n{width} {height}\n{maxval}\n".encode() + body)
    result = run("opj_compress", "-i", pnm, "-o", out)
    pnm.unlink()
    if result.returncode != 0 or not out.exists():
        sys.exit(f"opj_compress failed for {name}: {result.stdout}{result.stderr}")
    return out


def make_kinds(work: Path) -> dict[str, Kind]:
    """The three inputs. Each is made once and kept in `work`."""
    # Every 8-bit color: pixel i is (i >> 16, i >> 8, i) in raster order.
    cube = bytearray(3 * (1 << 24))
    cube[0::3] = b"".join(bytes([r]) * 65536 for r in range(256))
    cube[1::3] = b"".join(bytes([g]) * 256 for g in range(256)) * 256
    cube[2::3] = bytes(range(256)) * 65536
    cube = bytes(cube)
    cs = compress(work, "cube8", b"P6", 4096, 4096, 255, cube)
    kinds = {"rgb8": Kind("rgb8", TYPE_RGB_8, TYPE_RGB_8, 1 << 24, cube, cs)}

    rng = random.Random(SEED_16BIT)
    sample = array.array("H", (rng.randrange(65536) for _ in range(3 * SAMPLE_16BIT)))
    native = sample.tobytes()
    swapped = array.array("H", sample)
    swapped.byteswap()  # PNM stores 16-bit samples big-endian
    cs = compress(work, "sample16", b"P6", 1024, 1024, 65535, swapped.tobytes())
    kinds["rgb16"] = Kind("rgb16", TYPE_RGB_16, TYPE_RGB_16, SAMPLE_16BIT, native, cs)

    levels = bytes(range(256)) * 16  # every level, 64x64
    cs = compress(work, "gray8", b"P5", 64, 64, 255, levels)
    kinds["gray8"] = Kind("gray8", TYPE_GRAY_8, TYPE_RGB_8, 4096, levels, cs)
    return kinds


# ---------------------------------------------------------------------------
# The cases

def boxes(data: bytes, start: int, end: int):
    at = start
    while at < end:
        length = int.from_bytes(data[at:at + 4], "big")
        yield data[at + 4:at + 8], at + 8, at + length
        at += length


def jp2_profile(data: bytes) -> bytes:
    """The ICC profile in a JP2 file's `colr` box."""
    for kind, start, end in boxes(data, 12, len(data)):
        if kind == b"jp2h":
            for kind, start, end in boxes(data, start, end):
                if kind == b"colr" and data[start] == 2:
                    return data[start + 3:end]
    sys.exit("no ICC profile")


def patch(profile: bytearray, intent: int, white) -> None:
    """Set the rendering intent and, if given, the `wtpt` XYZ (as the test does)."""
    profile[64:68] = intent.to_bytes(4, "big")
    if white is None:
        return
    count = int.from_bytes(profile[128:132], "big")
    for i in range(count):
        entry = 132 + 12 * i
        if profile[entry:entry + 4] == b"wtpt":
            offset = int.from_bytes(profile[entry + 4:entry + 8], "big") + 8
            for k, v in enumerate(white):
                fixed = int(v * 65536.0 + (0.5 if v >= 0 else -0.5))
                profile[offset + 4 * k:offset + 4 * k + 4] = fixed.to_bytes(4, "big", signed=True)
            return
    sys.exit("no wtpt tag")


def cases(only: str | None):
    """(name, kind, profile bytes, OpenJPEG's outcome) for every committed
    case. The outcome is "converted" or "unconverted" for a profiles.json
    case, and None for a conformance variant."""
    profiles = json.loads((FIXTURES / "icc" / "profiles.json").read_text())["cases"]
    for case in profiles:
        yield (f"{case['codestream']}/{case['name']}", case["codestream"],
               bytes.fromhex(case["profile"]), case["openjpeg"])
    variants = json.loads((FIXTURES / "conformance" / "jp2" / "icc_variants.json").read_text())["variants"]
    for v in variants:
        path = FIXTURES / "conformance" / "jp2" / v["file"]
        if not path.exists():
            sys.exit(f"{path} is missing; the conformance corpus is not part of the crate")
        profile = bytearray(jp2_profile(path.read_bytes()))
        patch(profile, v["intent"], v["white"])
        space = bytes(profile[16:20])
        kind = "gray8" if space == b"GRAY" else ("rgb8" if v["bit_depth"] == 8 else "rgb16")
        yield f"{v['file'].split('/')[-1]}/{v['variant']}", kind, bytes(profile), None


# ---------------------------------------------------------------------------
# Comparison

def differing(a: bytes, b: bytes, stride: int) -> tuple[int, list[int]]:
    """The number of `stride`-byte pixels where `a` and `b` differ, and the
    first few of them."""
    if a == b:
        return 0, []
    count, first = 0, []
    chunk = stride * 4096
    for base in range(0, len(a), chunk):
        if a[base:base + chunk] == b[base:base + chunk]:
            continue
        for at in range(base, min(base + chunk, len(a)), stride):
            if a[at:at + stride] != b[at:at + stride]:
                count += 1
                if len(first) < 3:
                    first.append(at // stride)
    return count, first


def port_convert(kind: Kind, profile_path: Path, out_path: Path):
    """(status, channels, bytes): status is "ok", "declined" or "error"."""
    result = run(EXAMPLE, kind.codestream, profile_path, out_path)
    if result.returncode == 3:
        return "declined", 0, result.stderr.strip()
    if result.returncode != 0:
        return "error", 0, result.stderr.strip()
    channels = int(result.stdout.split("channels=")[1].split()[0])
    return "ok", channels, out_path.read_bytes()


TYPE_YCBCR_16 = 0x7001A


def declined_by_design() -> set[str]:
    """The case names `tests/jp2_icc.rs` lists in `REFUSED_BY_US`: the
    profiles the decoder declines with `Error::Unsupported`."""
    text = (ROOT / "tests" / "jp2_icc.rs").read_text()
    block = text[text.index("const REFUSED_BY_US"):]
    block = block[:block.index("];")]
    return set(re.findall(r'\("([a-z0-9_]+)", "', block))


def openjpeg_route(kinds: dict[str, Kind], kind: Kind, profile: bytes):
    """What OpenJPEG 2.5.0's `color_apply_icc_profile` (color.c) does with this
    profile on this image: (source kind, in_type, out_type), or None when it
    returns before it calls LittleCMS and leaves the samples as coded. The
    input format follows the profile's color space (header bytes 16..20) and
    the component count, not the codestream:

    - GRAY: `TYPE_GRAY_8` to `TYPE_RGB_8`, whatever the image;
    - RGB with three or more components: `TYPE_RGB_8` or `TYPE_RGB_16` by the
      sample depth; with fewer, nothing is done;
    - YCbCr: `TYPE_YCbCr_16` to `TYPE_RGB_16` (the same component rule);
    - any other space: nothing is done.
    """
    space = profile[16:20]
    components = 1 if kind.name == "gray8" else 3
    if space == b"GRAY":
        gray = kinds["gray8"]
        return gray, TYPE_GRAY_8, TYPE_RGB_8
    if space == b"RGB " and components >= 3:
        return kind, kind.in_type, kind.out_type
    if space == b"YCbr" and components >= 3:
        return kinds["rgb16"], TYPE_YCBCR_16, TYPE_RGB_16
    return None


def compare(lcms: Lcms, kinds: dict[str, Kind], kind: Kind, name: str,
            profile: bytes, tmp: Path, declined: set[str]):
    """(outcome, mismatches, detail, surprises, converted). The outcome is "compared",
    "as coded" (OpenJPEG leaves the samples, and the decoder must give them
    back unchanged) or "declined" (the decoder says Unsupported, as
    `REFUSED_BY_US` lists). `surprises` are the ways the case broke those
    expectations; any is a failure."""
    surprises: list[str] = []
    route = openjpeg_route(kinds, kind, profile)
    expected = None
    if route is not None:
        src, in_type, out_type = route
        out_size = src.count * (6 if out_type == TYPE_RGB_16 else 3)
        expected = lcms.convert(profile, in_type, out_type, src.pixels, src.count, out_size)
    profile_path = tmp / "profile.icc"
    profile_path.write_bytes(profile)
    status, channels, got = port_convert(kind, profile_path, tmp / "out.raw")
    listed = name.split("/")[-1] in declined
    if status == "declined":
        if not listed:
            surprises.append(f"{name}: the decoder declines a profile `REFUSED_BY_US` does not list: {got[:100]}")
        return "declined", 0, got, surprises, expected is not None
    if listed:
        surprises.append(f"{name}: listed in `REFUSED_BY_US`, but the decoder gave {status}")
    if status == "error":
        return "compared", kind.count, f"decoder failed: {got}", surprises, expected is not None
    if expected is None:
        # OpenJPEG leaves the samples as coded.
        want = 3 if kind.name != "gray8" else 1
        if channels != want or got != kind.pixels:
            surprises.append(f"{name}: OpenJPEG leaves the samples as coded, but the decoder changed them")
        return "as coded", 0, "", surprises, False
    if route[0] is not kind or route[1] != kind.in_type:
        surprises.append(f"{name}: LittleCMS converts a different input than the image, and the decoder converts it")
        return "compared", kind.count, "not comparable", surprises, True
    if kind.name == "gray8" and channels == 1:
        # A neutral monochrome profile gives one gray channel: LittleCMS's
        # three must agree and equal it.
        if not (expected[0::3] == expected[1::3] == expected[2::3]):
            return "compared", kind.count, "decoder gives one channel; LittleCMS's three differ", surprises, True
        count, first = differing(expected[0::3], got, 1)
    else:
        if channels != 3:
            return "compared", kind.count, f"decoder gives {channels} channels", surprises, True
        count, first = differing(expected, got, kind.stride)
    detail = ""
    if count:
        n = kind.in_stride
        detail = "first at pixel " + ", ".join(
            f"{i} (in {kind.pixels[i * n:(i + 1) * n].hex()})" for i in first)
    return "compared", count, detail, surprises, True


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--only")
    parser.add_argument("--work")
    args = parser.parse_args()
    for tool in ("opj_compress", "cargo"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH")
    opj = require_openjpeg("opj_compress")
    lcms = Lcms()
    print(f"LittleCMS {require_lcms(lcms.version)} (encoded {lcms.version}) and OpenJPEG {opj}")
    build = run("cargo", "build", "--release", "--example", "icc_convert", cwd=ROOT)
    if build.returncode != 0:
        sys.exit(build.stderr)

    with tempfile.TemporaryDirectory() as scratch:
        work = Path(args.work) if args.work else Path(scratch)
        work.mkdir(parents=True, exist_ok=True)
        tmp = Path(scratch)
        kinds = make_kinds(work)
        total, bad, notes = 0, 0, []
        tally = {"compared": 0, "as coded": 0, "declined": 0}
        started = time.time()
        surprises: list[str] = []
        declined = declined_by_design()
        seen: set[str] = set()
        for name, kind_name, profile, openjpeg in cases(args.only):
            if args.only and args.only not in name:
                continue
            seen.add(name.split("/")[-1])
            kind = kinds[kind_name]
            outcome, mismatches, detail, extra, converted = compare(
                lcms, kinds, kind, name, profile, tmp, declined)
            tally[outcome] += 1
            surprises += extra
            # OpenJPEG converts exactly when it gets as far as a LittleCMS
            # transform that builds. A declined case is checked too: the
            # decoder still must not decline a profile OpenJPEG leaves as coded
            # unless `REFUSED_BY_US` says so, and `compare` checks that.
            if openjpeg and (openjpeg == "converted") != converted:
                surprises.append(f"{name}: icc/profiles.json says OpenJPEG {openjpeg}, "
                                 f"LittleCMS {'converts' if converted else 'refuses'}")
            if outcome == "compared":
                total += kind.count
                bad += mismatches
                print(f"{name:60} {kind.count:>9} pixels  {mismatches} mismatches {detail}".rstrip(), flush=True)
            else:
                notes.append(f"{name}: {outcome}" + (f" ({detail[:100]})" if detail else ""))
                print(f"{name:60} {outcome}", flush=True)
        # A listed name that matches no case would go unchecked, so it is a
        # surprise too, on a full run.
        if not args.only:
            for name in sorted(declined - seen):
                surprises.append(f"{name}: listed in `REFUSED_BY_US`, but no case has that name")
        print()
        for note in notes:
            print("not compared:", note)
        for note in surprises:
            print("SURPRISE:", note)
        print(f"\n{tally['compared']} cases compared ({total} pixels), "
              f"{tally['as coded']} left as coded by OpenJPEG and by the decoder, "
              f"{tally['declined']} declined by the decoder as listed; "
              f"{len(surprises)} surprises; "
              f"{bad} mismatches in {time.time() - started:.0f} s")
        sys.exit(1 if bad or surprises else 0)


if __name__ == "__main__":
    main()
