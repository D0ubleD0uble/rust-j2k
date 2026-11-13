#!/usr/bin/env python3
"""Write the sample-limit fixtures of tests/fixtures/sample_limit/.

- `rgb_6000x4000.jp2`: a 6000x4000 8-bit RGB image (72 M samples, over the
  old 2^26 limit and under the 2^28 default), coded lossless by
  `opj_compress` with its default options. The picture is flat blocks, so the
  file is small and quick to decode: red steps every 256 columns, green every
  256 rows, and blue is a checkerboard of 512 x 512 squares.
- `flat_16385_tiled_lossless.j2k`: a flat (all zero) 16385x16385 8-bit gray
  image (just over 2^28 samples) in 4096x4096 tiles with 4 wavelet levels,
  so it decodes cheaply at a resolution reduction.

It checks that `opj_decompress` decodes the RGB file to exactly the picture
it was coded from, so tests/sample_limit.rs can compare the decode with the
picture's formula (`picture` below, repeated there) instead of committing
72 MB of reference samples. It prints the SHA-256 of each plane
`opj_decompress` decodes (the plane's samples in raster order, one byte
each), which the fixture README records.

Usage: scripts/gen-sample-limit-fixtures.py
"""

import hashlib
import pathlib
import subprocess
import sys
import tempfile
from oracle_tools import require_openjpeg

DIR = pathlib.Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "sample_limit"
W, H = 6000, 4000
FLAT = 16385


def picture(c: int, x: int, y: int) -> int:
    """Sample (x, y) of component c."""
    return (((x >> 8) * 10, (y >> 8) * 15, ((x >> 9) ^ (y >> 9)) * 30)[c]) & 255


def planes() -> list[bytes]:
    return [bytes(picture(c, x, y) for y in range(H) for x in range(W)) for c in range(3)]


def rgb_ppm(planes: list[bytes]) -> bytes:
    pixels = bytearray(W * H * 3)
    for c in range(3):
        pixels[c::3] = planes[c]
    return b"P6\n%d %d\n255\n" % (W, H) + bytes(pixels)


def pgx_samples(path: pathlib.Path) -> bytes:
    data = path.read_bytes()
    return data[data.index(b"\n") + 1 :]


def main() -> None:
    require_openjpeg("opj_compress", "opj_decompress")
    DIR.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        tmp = pathlib.Path(tmp)
        ppm = tmp / "rgb.ppm"
        source = planes()
        ppm.write_bytes(rgb_ppm(source))
        rgb = DIR / "rgb_6000x4000.jp2"
        subprocess.run(["opj_compress", "-i", ppm, "-o", rgb], check=True, capture_output=True)

        raw = tmp / "flat.raw"
        raw.write_bytes(bytes(FLAT * FLAT))
        flat = DIR / "flat_16385_tiled_lossless.j2k"
        subprocess.run(
            ["opj_compress", "-i", raw, "-o", flat, "-F", f"{FLAT},{FLAT},1,8,u",
             "-n", "4", "-t", "4096,4096"],
            check=True,
            capture_output=True,
        )

        subprocess.run(
            ["opj_decompress", "-i", rgb, "-o", tmp / "rgb.pgx"], check=True, capture_output=True
        )
        for c in range(3):
            plane = pgx_samples(tmp / f"rgb_{c}.pgx")
            assert plane == source[c], f"opj_decompress plane {c} differs from the picture"
            print(f"rgb_6000x4000.jp2 plane {c}: {hashlib.sha256(plane).hexdigest()}")
    for path in sorted(DIR.glob("*.j2k")) + sorted(DIR.glob("*.jp2")):
        print(f"{path.name}: {path.stat().st_size} bytes, {hashlib.sha256(path.read_bytes()).hexdigest()}")


if __name__ == "__main__":
    sys.exit(main())
