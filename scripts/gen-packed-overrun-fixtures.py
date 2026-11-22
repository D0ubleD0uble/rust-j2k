#!/usr/bin/env python3
"""Generate the packed packet header fixtures in tests/fixtures/packed_overrun/.

Each file is a 1 x 1 image, one 8-bit unsigned component, one tile, one
layer, LRCP, no decomposition level, the 5/3 wavelet, no quantization. Its
one packet header is packed in a `PPT` (A.7.5) or `PPM` (A.7.4) marker and
its body is seven 0x00 bytes. The header is `0xC4`: present, included, zero
bit-planes 0, one coding pass, then the first bits of the length. Read
whole, the header needs more bits than that byte holds.

- `ppt_short.j2k`: the header is `C4` alone, one bit short.
- `ppt_ff_nostuff.j2k`: the header is `C4 FF`. It ends on 0xFF, so B.10.1
  puts a stuffed byte after it, and that byte is missing.
- `ppm_short.j2k`: as `ppt_short.j2k`, with the header in a `PPM` marker.
- `ppt_whole.j2k` and `ppm_whole.j2k`: the header whole, `C4 FF 00`. They
  are the controls: OpenJPEG decodes each to 176.
- `ppm_two_parts.j2k`: as `ppm_whole.j2k` with two layers, in two
  tile-parts. The `PPM` carries `C4 FF 00` for the first and `00` (an empty
  packet) for the second. The first tile-part holds a `COM` that sizes it
  so the prefixes `rust_j2k::fuzz::tile_parts_walks_agree` cuts end inside
  its header, inside its data and one byte after it: input that ends before
  the last tile-part, with `PPM` chunks left over. OpenJPEG decodes it to
  176.

The bits these headers need are not in the codestream, so the decoder
refuses each one, with `strict` on or off. The script also runs OpenJPEG
2.5.0 `opj_decompress` on each file and checks what it does, which is not a
deliberate correction: on `ppt_short.j2k` and `ppm_short.j2k` it reads the
missing bits as 0 (`opj_bio_getbit` ignores the failed `opj_bio_bytein`) and
decodes the sample to 128 with no error, and it refuses `ppt_ff_nostuff.j2k`
(`opj_bio_inalign` fails, t2.c:1323).

Usage: python3 scripts/gen-packed-overrun-fixtures.py [--no-oracle]
The check needs OpenJPEG 2.5.0 `opj_decompress` on PATH; `--no-oracle`
writes the files without it.
"""

from __future__ import annotations

import struct
import subprocess
import sys
import tempfile
from pathlib import Path

DIR = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "packed_overrun"

BODY = bytes(7)


def segment(code: int, body: bytes) -> bytes:
    """A marker segment: `code`, its length counting itself, then `body`."""
    return struct.pack(">HH", code, len(body) + 2) + body


def main_header(layers: int = 1) -> bytes:
    """SOC, SIZ, COD and QCD for the 1 x 1 image."""
    siz = segment(0xFF51, struct.pack(">HIIIIIIIIH", 0, 1, 1, 0, 0, 1, 1, 0, 0, 1) + bytes([7, 1, 1]))
    # Scod 0; LRCP, `layers` layers, no MCT; no level, 64 x 64 blocks, style 0, 5/3.
    cod = segment(0xFF52, bytes([0, 0]) + struct.pack(">H", layers) + bytes([0, 0, 4, 4, 0, 1]))
    # No quantization, two guard bits less one; exponent 9 for the one band.
    qcd = segment(0xFF5C, bytes([0x20, 9 << 3]))
    return b"\xff\x4f" + siz + cod + qcd


def tile_part(extra: bytes, part: int = 0, parts: int = 1, body: bytes = BODY) -> bytes:
    """SOT for tile 0, `part` of `parts`, then `extra`, SOD and `body`."""
    psot = 12 + len(extra) + 2 + len(body)
    sot = segment(0xFF90, struct.pack(">HIBB", 0, psot, part, parts))
    return sot + extra + b"\xff\x93" + body


def ppt(header: bytes) -> bytes:
    """The header packed in the tile-part's PPT marker (Zppt 0)."""
    return main_header() + tile_part(segment(0xFF61, bytes([0]) + header)) + b"\xff\xd9"


def ppm(header: bytes) -> bytes:
    """The header packed in a main-header PPM marker (Zppm 0, one Nppm chunk)."""
    marker = segment(0xFF60, bytes([0]) + struct.pack(">I", len(header)) + header)
    return main_header() + marker + tile_part(b"") + b"\xff\xd9"


def ppm_two_parts() -> bytes:
    """Two layers in two tile-parts, each packet header a `PPM` chunk."""
    chunks = [bytes([0xC4, 0xFF, 0x00]), bytes([0x00])]
    body = bytes([0]) + b"".join(struct.pack(">I", len(c)) + c for c in chunks)
    # Rcme 1 (Latin text) and four bytes: the first tile-part is 31 bytes, and
    # the 47 after the main header are cut at 47 * k // 16, so the cut for
    # k = 9 and 10 ends in its data and the one for 11 a byte after it.
    com = segment(0xFF64, struct.pack(">H", 1) + b"cuts")
    parts = tile_part(com, 0, 2) + tile_part(b"", 1, 2, b"")
    return main_header(2) + segment(0xFF60, body) + parts + b"\xff\xd9"


FIXTURES = {
    "ppt_short.j2k": (ppt(bytes([0xC4])), 128),
    "ppt_ff_nostuff.j2k": (ppt(bytes([0xC4, 0xFF])), None),
    "ppm_short.j2k": (ppm(bytes([0xC4])), 128),
    "ppt_whole.j2k": (ppt(bytes([0xC4, 0xFF, 0x00])), 176),
    "ppm_whole.j2k": (ppm(bytes([0xC4, 0xFF, 0x00])), 176),
    "ppm_two_parts.j2k": (ppm_two_parts(), 176),
}


def opj_sample(path: Path, tmp: Path) -> int | None:
    """The one sample `opj_decompress` decodes `path` to, or None if it fails."""
    out = tmp / "out.pgm"
    out.unlink(missing_ok=True)
    subprocess.run(["opj_decompress", "-i", str(path), "-o", str(out)], capture_output=True)
    return out.read_bytes()[-1] if out.exists() else None


def main() -> None:
    oracle = "--no-oracle" not in sys.argv[1:]
    if oracle:
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        from oracle_tools import require_openjpeg

        require_openjpeg("opj_decompress")
    DIR.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        for name, (data, want) in FIXTURES.items():
            path = DIR / name
            path.write_bytes(data)
            if oracle:
                got = opj_sample(path, Path(tmp))
                if got != want:
                    sys.exit(f"{name}: opj_decompress gives {got}, expected {want}")
            print(f"{name}: {len(data)} bytes")


if __name__ == "__main__":
    main()
