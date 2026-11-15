#!/usr/bin/env python3
"""Generate the HT VLC stuffing-bit fixtures in tests/fixtures/ht_vlc_stuffing/.

Each fixture is a pair of single-block HT codestreams, cleanup pass only. The
`.set.j2k` file's VLC stream has one or two set stuffing bits; the
`.clear.j2k` file is the same block with those bits cleared. T.814 7.1.4 skips
a stuffing bit, so this decoder reads the two files the same. OpenJPEG 2.5.0
ORs a set bit into the next byte, so it decodes the `.set` file differently
or refuses it as malformed, and decodes the `.clear` file as this decoder
does. On the `abort_mel_` pairs the set bit sits after a MEL 0xFF, and
OpenJPEG's `mel_init` assertion aborts the process on the `.set` file. See docs/openjpeg-departures.md, "Set stuffing bits in the HT VLC and
MagRef streams".

The blocks were picked from a random differential of crafted cleanup-only
blocks against `opj_decompress` 2.5.0 (issue #368): every block on which the
two decoders differed had a set VLC stuffing bit, and none differed once the
bits were cleared. Each row below keeps the block's parameters and its
cleanup segment as found.

The script writes both files of each pair, checks that they differ only in
the top bit of the bytes that carry a VLC stuffing bit, runs `opj_decompress`
on each, and writes `manifest.tsv`: the SHA-256 of OpenJPEG's samples (each as
a little-endian i32, as `tests/ht_vlc_stuffing.rs` hashes them) for the
`.clear` file, and the same for the `.set` file, `refused` when OpenJPEG
fails on it, or `abort` when it dies on a signal. It exits non-zero if OpenJPEG decodes a `.set` file the same as
its `.clear` file, since that pair would not show the difference.

Usage: python3 scripts/gen-ht-vlc-stuffing-fixtures.py
Requires OpenJPEG 2.5.0 `opj_decompress` on PATH.
"""

from __future__ import annotations

import hashlib
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

from oracle_tools import require_openjpeg

DIR = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "ht_vlc_stuffing"

# name, width, height, bit depth, guard bits, exponent, irreversible,
# mantissa, zero bit-planes, code-block style, cleanup segment (hex, as set).
# `Mb` is guard bits + exponent - 1. The 2x17 pairs are the blocks issue #368
# names (`r2_230_...` and `r5_217_...`), with their cleanup pass alone.
BLOCKS = [
    ("nibble_2x17_i12", 2, 17, 12, 1, 9, True, 0, 6, 0x40,
     "b63cbe60f61d1fc8097fb6cde6f800"),
    ("nibble_2x17_r12_causal", 2, 17, 12, 1, 7, False, 0, 3, 0x48,
     "47ba39702e6d2e53d83102bda8651c0d1d8e70482ef2fa00"),
    ("nibble_3x8_r12_causal", 3, 8, 12, 1, 9, False, 0, 6, 0x48,
     "420fa044f5b4b77cf57a06a8800d18f900"),
    ("nibble_5x2_r12", 5, 2, 12, 1, 5, False, 0, 3, 0x40,
     "bc672853ef9445253a0053fe2ea1c21ef500"),
    ("nibble_4x8_i12", 4, 8, 12, 3, 9, True, 1360, 7, 0x40,
     "07403de112bfdbb2c04512ff24fc3acefc00"),
    ("nibble_4x8_r8", 4, 8, 8, 1, 7, False, 0, 2, 0x40,
     "792bde79e9518b56f500"),
    ("nibble_4x8_r16", 4, 8, 16, 2, 10, False, 0, 4, 0x40,
     "f2016f8a14fc483c8a2383517071f3cd973ed6f301"),
    ("nibble_2x2_i16", 2, 2, 16, 1, 6, True, 1307, 1, 0x40,
     "56f300"),
    ("byte_8x2_r8", 8, 2, 8, 3, 6, False, 0, 0, 0x40,
     "fb7d6b9d241834e6ffe900"),
    ("byte_4x8_r16", 4, 8, 16, 3, 3, False, 0, 2, 0x40,
     "038db807b526f6cfed4754ffc89d00"),
    ("byte_1x12_r16", 1, 12, 16, 1, 5, False, 0, 4, 0x40,
     "b9da241c1cd7a87eff9bbb00"),
    ("refused_outside_2x17_i16", 2, 17, 16, 2, 13, True, 51, 10, 0x40,
     "f018993282c4b281b0f800"),
    ("refused_uq_4x8_r16", 4, 8, 16, 1, 4, False, 0, 2, 0x40,
     "7c637a82f2c8cbf146f800"),
    ("refused_uq_two_bits_4x18_i16", 4, 18, 16, 3, 8, True, 427, 8, 0x40,
     "8d00f98a6d32832507f0ffa85afc00"),
    ("abort_mel_4x16_r12", 4, 16, 12, 2, 12, False, 0, 7, 0x40,
     "c6a5d9ffc01f657800"),
    ("abort_mel_2x17_r8", 2, 17, 8, 3, 2, False, 0, 2, 0x40,
     "15110f857e0f95f6ffac2d6700"),
]


class Bits:
    """Packet header bits, packed as B.10.1 has it: a byte after 0xFF carries 7."""

    def __init__(self) -> None:
        self.out = bytearray()
        self.byte = 0
        self.used = 0
        self.room = 8

    def put(self, bit: int) -> None:
        self.byte = (self.byte << 1) | bit
        self.used += 1
        if self.used == self.room:
            self.out.append(self.byte)
            self.room = 7 if self.byte == 0xFF else 8
            self.byte = 0
            self.used = 0

    def put_int(self, value: int, width: int) -> None:
        for shift in range(width - 1, -1, -1):
            self.put((value >> shift) & 1)

    def finish(self) -> bytes:
        if self.used:
            self.out.append((self.byte << (self.room - self.used)) & 0xFF)
        elif self.out and self.out[-1] == 0xFF:
            self.out.append(0)
        return bytes(self.out)


def segment(code: int, body: bytes) -> bytes:
    return struct.pack(">HH", code, 2 + len(body)) + body


def codestream(w, h, depth, guard, eps, irrev, mant, zbp, style, cleanup: bytes) -> bytes:
    """One tile, one unsigned component, no wavelet level, one HT code-block
    whose cleanup pass alone is in one layer."""
    bits = Bits()
    bits.put(1)  # a non-empty packet
    bits.put(1)  # included in layer 0
    for _ in range(zbp):
        bits.put(0)
    bits.put(1)
    bits.put(0)  # one coding pass
    increments = 0
    while 3 + increments < len(cleanup).bit_length():
        increments += 1
    for _ in range(increments):
        bits.put(1)
    bits.put(0)
    bits.put_int(len(cleanup), 3 + increments)
    data = bits.finish() + cleanup

    # Rsiz bit 14: the codestream uses Part 15.
    siz = struct.pack(">HIIIIIIIIH", 0x4000, w, h, 0, 0, w, h, 0, 0, 1) + bytes([depth - 1, 1, 1])
    out = b"\xff\x4f" + segment(0xFF51, siz)
    # CAP: Part 15 only; Ccap15 with no RGN and magnitude bound 74 (none).
    out += segment(0xFF50, struct.pack(">IH", 0x00020000, 0x001F))
    # COD: 64x64 code-blocks, no wavelet level, 9/7 or 5/3.
    out += segment(0xFF52, bytes([0, 0, 0, 1, 0, 0, 4, 4, style, 0 if irrev else 1]))
    if irrev:
        # Scalar expounded: one exponent and mantissa for the one subband.
        out += segment(0xFF5C, bytes([(guard << 5) | 2]) + struct.pack(">H", (eps << 11) | mant))
    else:
        out += segment(0xFF5C, bytes([guard << 5, eps << 3]))
    out += segment(0xFF90, struct.pack(">HIBB", 0, 12 + 2 + len(data), 0, 1))
    out += b"\xff\x93" + data + b"\xff\xd9"
    return out


def vlc_stuffing_bytes(seg: bytes) -> list[int]:
    """The bytes whose top bit is a VLC stuffing bit (7.1.4), read from the
    end: the first nibble when its three low bits are ones, then each byte
    whose seven low bits are ones after a byte above 0x8F."""
    lcup = len(seg)
    scup = 16 * seg[lcup - 1] + (seg[lcup - 2] & 0x0F)
    found = []
    if (seg[lcup - 2] >> 4) & 7 == 7:
        found.append(lcup - 2)
    last = seg[lcup - 2]
    for pos in range(lcup - 3, lcup - scup - 1, -1):
        if last > 0x8F and seg[pos] & 0x7F == 0x7F:
            found.append(pos)
        last = seg[pos]
    return found


def clear_vlc_stuffing(seg: bytes) -> bytes:
    """`seg` with each set VLC stuffing bit cleared, in reading order: a
    cleared 0xFF reads as 0x7F, so the byte before it no longer carries one."""
    out = bytearray(seg)
    lcup = len(out)
    scup = 16 * out[lcup - 1] + (out[lcup - 2] & 0x0F)
    if (out[lcup - 2] >> 4) & 7 == 7:
        out[lcup - 2] &= 0x7F
    last = out[lcup - 2]
    for pos in range(lcup - 3, lcup - scup - 1, -1):
        if last > 0x8F and out[pos] & 0x7F == 0x7F:
            out[pos] &= 0x7F
        last = out[pos]
    return bytes(out)


def opj_digest(path: Path, work: Path) -> str:
    """SHA-256 of OpenJPEG's samples as little-endian i32, `refused`, or
    `abort` when the process dies on a signal (an assertion)."""
    pgx = work / (path.stem + ".pgx")
    result = subprocess.run(["opj_decompress", "-i", str(path), "-o", str(pgx)],
                            capture_output=True, stdin=subprocess.DEVNULL, timeout=60)
    outs = sorted(work.glob(path.stem + "*.pgx"))
    if result.returncode < 0:
        return "abort"
    if result.returncode != 0 or len(outs) != 1:
        return "refused"
    raw = outs[0].read_bytes()
    outs[0].unlink()
    nl = raw.index(b"\n")
    head = raw[:nl].split()
    if head[:2] != [b"PG", b"ML"] or head[2] != b"+":
        sys.exit(f"{path.name}: unexpected PGX header {raw[:nl]!r}")
    depth, w, h = int(head[3]), int(head[4]), int(head[5])
    size = 1 if depth <= 8 else 2
    body = raw[nl + 1:]
    samples = [int.from_bytes(body[i:i + size], "big") for i in range(0, w * h * size, size)]
    return hashlib.sha256(b"".join(struct.pack("<i", s) for s in samples)).hexdigest()


def main() -> None:
    require_openjpeg("opj_decompress")
    DIR.mkdir(parents=True, exist_ok=True)
    rows = []
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        for name, w, h, depth, guard, eps, irrev, mant, zbp, style, hexseg in BLOCKS:
            seg = bytes.fromhex(hexseg)
            stuffed = [p for p in vlc_stuffing_bytes(seg) if seg[p] & 0x80]
            if not stuffed:
                sys.exit(f"{name}: no set VLC stuffing bit")
            cleared = clear_vlc_stuffing(seg)
            differ = [i for i in range(len(seg)) if seg[i] != cleared[i]]
            if differ != sorted(stuffed) or any(seg[i] ^ cleared[i] != 0x80 for i in differ):
                sys.exit(f"{name}: the twins differ beyond the stuffing bits")
            digests = []
            for tag, body in (("set", seg), ("clear", cleared)):
                path = DIR / f"{name}.{tag}.j2k"
                path.write_bytes(codestream(w, h, depth, guard, eps, irrev, mant, zbp, style, body))
                digests.append(opj_digest(path, work))
            if digests[1] in ("refused", "abort"):
                sys.exit(f"{name}: OpenJPEG refuses the cleared block")
            if name.startswith("abort_") != (digests[0] == "abort"):
                sys.exit(f"{name}: OpenJPEG gave {digests[0]} on the set block")
            if digests[0] == digests[1]:
                sys.exit(f"{name}: OpenJPEG decodes both files the same")
            rows.append(f"{name}\t{digests[0]}\t{digests[1]}\n")
            print(f"{name}: set {digests[0][:16]}, clear {digests[1][:16]}")
    (DIR / "manifest.tsv").write_text(
        "# name\tOpenJPEG 2.5.0 on .set.j2k\tOpenJPEG 2.5.0 on .clear.j2k\n" + "".join(rows))


if __name__ == "__main__":
    main()
