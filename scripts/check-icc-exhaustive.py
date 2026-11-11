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

Two kinds of case are not compared pixel by pixel. If LittleCMS refuses the
profile or the transform, OpenJPEG leaves the samples as coded, and
`tests/jp2_icc.rs` already checks that the decoder does the same. If the
decoder declines the profile with `Error::Unsupported` (a lookup table
profile, say), there is nothing to compare. Both are counted and listed.

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
import random
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

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
    cs = compress(work, "grey8", b"P5", 64, 64, 255, levels)
    kinds["grey8"] = Kind("grey8", TYPE_GRAY_8, TYPE_RGB_8, 4096, levels, cs)
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
    """(name, kind, profile bytes) for every committed case."""
    profiles = json.loads((FIXTURES / "icc" / "profiles.json").read_text())["cases"]
    for case in profiles:
        yield f"{case['codestream']}/{case['name']}", case["codestream"], bytes.fromhex(case["profile"])
    variants = json.loads((FIXTURES / "conformance" / "jp2" / "icc_variants.json").read_text())["variants"]
    for v in variants:
        path = FIXTURES / "conformance" / "jp2" / v["file"]
        if not path.exists():
            sys.exit(f"{path} is missing; the conformance corpus is not part of the crate")
        profile = bytearray(jp2_profile(path.read_bytes()))
        patch(profile, v["intent"], v["white"])
        space = bytes(profile[16:20])
        kind = "grey8" if space == b"GRAY" else ("rgb8" if v["bit_depth"] == 8 else "rgb16")
        yield f"{v['file'].split('/')[-1]}/{v['variant']}", kind, bytes(profile)


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


def compare(lcms: Lcms, kind: Kind, profile: bytes, tmp: Path):
    """("compared", mismatches, detail), ("lcms refuses", ...) or
    ("declined", ...)."""
    expected = lcms.convert(profile, kind.in_type, kind.out_type, kind.pixels,
                            kind.count, kind.out_size)
    if expected is None:
        return "lcms refuses", 0, ""
    profile_path = tmp / "profile.icc"
    profile_path.write_bytes(profile)
    status, channels, got = port_convert(kind, profile_path, tmp / "out.raw")
    if status == "declined":
        return "declined", 0, got
    if status == "error":
        return "compared", kind.count, f"decoder failed: {got}"
    if kind.name == "grey8" and channels == 1:
        # A neutral monochrome profile gives one grey channel: LittleCMS's
        # three must agree and equal it.
        if not (expected[0::3] == expected[1::3] == expected[2::3]):
            return "compared", kind.count, "decoder gives one channel; LittleCMS's three differ"
        count, first = differing(expected[0::3], got, 1)
    else:
        if channels != 3:
            return "compared", kind.count, f"decoder gives {channels} channels"
        count, first = differing(expected, got, kind.stride)
    detail = ""
    if count:
        n = kind.in_stride
        detail = "first at pixel " + ", ".join(
            f"{i} (in {kind.pixels[i * n:(i + 1) * n].hex()})" for i in first)
    return "compared", count, detail


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--only")
    parser.add_argument("--work")
    args = parser.parse_args()
    for tool in ("opj_compress", "cargo"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found on PATH")
    lcms = Lcms()
    print(f"LittleCMS {lcms.version // 1000}.{lcms.version % 1000 // 10} "
          f"(encoded {lcms.version}); OpenJPEG 2.5.0 and LittleCMS 2.14 are the oracle versions")
    build = run("cargo", "build", "--release", "--example", "icc_convert", cwd=ROOT)
    if build.returncode != 0:
        sys.exit(build.stderr)

    with tempfile.TemporaryDirectory() as scratch:
        work = Path(args.work) if args.work else Path(scratch)
        work.mkdir(parents=True, exist_ok=True)
        tmp = Path(scratch)
        kinds = make_kinds(work)
        total, bad, notes = 0, 0, []
        tally = {"compared": 0, "lcms refuses": 0, "declined": 0}
        started = time.time()
        for name, kind_name, profile in cases(args.only):
            if args.only and args.only not in name:
                continue
            kind = kinds[kind_name]
            outcome, mismatches, detail = compare(lcms, kind, profile, tmp)
            tally[outcome] += 1
            if outcome == "compared":
                total += kind.count
                bad += mismatches
                print(f"{name:60} {kind.count:>9} pixels  {mismatches} mismatches {detail}".rstrip(), flush=True)
            else:
                notes.append(f"{name}: {outcome}" + (f" ({detail})" if detail else ""))
                print(f"{name:60} {outcome}", flush=True)
        print()
        for note in notes:
            print("not compared:", note)
        print(f"\n{tally['compared']} cases compared ({total} pixels), "
              f"{tally['lcms refuses']} refused by LittleCMS, "
              f"{tally['declined']} declined by the decoder; "
              f"{bad} mismatches in {time.time() - started:.0f} s")
        sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
