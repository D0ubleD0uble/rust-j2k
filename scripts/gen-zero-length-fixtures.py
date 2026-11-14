#!/usr/bin/env python3
"""Generate the zero-length contribution fixtures and their OpenJPEG oracles.

A packet header may include a code-block with coding passes and a length of
zero (ISO/IEC 15444-1 B.10.7). Those passes bring no bytes. A later layer can
still bring the bytes that code them, and passes that never get any are
decoded from the `0xFF` bytes the decoder appends (D.4.1). See
docs/openjpeg-departures.md, "Code-block contributions of zero bytes".

Each fixture is one 8-bit component, 8 rows high, with no wavelet level, in
8x8 code-blocks side by side and two quality layers (LRCP, reversible 5/3).
`opj_compress` codes each 8x8 block of `source()` alone, in one lossless
layer; this script reads that block's zero bit-planes, passes and codeword
segments, then writes the codestream by hand with each block's passes split
over the two layers as its plan says. A plan gives, per layer, the passes the
block adds and, for each codeword segment those passes touch, `a` (all the
segment's bytes not yet sent) or `z` (a length of zero).

`zero_length_contribution` (default code-block style, 16x8):

- block 0: layer 0 adds 3 passes with no bytes; layer 1 adds the rest with
  every byte. It decodes to the source.
- block 1: layers 0 and 1 add passes with no bytes. The block has passes and
  no bytes at all, so it decodes from the fill alone.

`zero_length_contribution_bypass` (`-M 1`, 24x8). The lazy mode splits a
block's passes into codeword segments of `10, 2, 1, 2, 1, ...` passes, the
2-pass ones raw:

- block 0: layer 0 brings segment 0 whole and the first pass of segment 1
  (raw) with no bytes; layer 1 brings segment 1's bytes with its second pass,
  then the rest. It decodes to the source.
- block 1: three segments, no bytes at all.
- block 2: segment 0 whole in layer 0, then in layer 1 segment 1 (raw) and
  segment 2 with no bytes. The raw passes read the fill as 1-bits.

The script checks that `opj_decompress` returns the source wherever a plan
sends every byte, and writes its decode as the snapshot.

Usage: python3 scripts/gen-zero-length-fixtures.py
Requires OpenJPEG 2.5.0 `opj_compress` and `opj_decompress` on PATH.
"""

from __future__ import annotations

import json
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

from oracle_tools import require_openjpeg

FIXTURES = Path(__file__).resolve().parent.parent / "tests" / "fixtures"

HEIGHT, BLOCK = 8, 8
LAZY = 0x01
REST = -1  # every pass not yet sent

# name -> (opj_compress -M mask, style byte, per block: [(passes, lengths)]
# for layer 0 and layer 1).
VARIANTS = {
    "zero_length_contribution": (
        0,
        0x00,
        [
            [(3, "z"), (REST, "a")],
            [(2, "z"), (1, "z")],
        ],
    ),
    "zero_length_contribution_bypass": (
        1,
        LAZY,
        [
            [(11, "az"), (REST, "a*")],
            [(12, "zz"), (1, "z")],
            [(10, "a"), (3, "zz")],
        ],
    ),
}


def source(width: int) -> bytes:
    """A `width` x 8 8-bit image: a different pattern in each 8x8 block."""
    out = bytearray()
    for y in range(HEIGHT):
        for x in range(width):
            b = x // BLOCK
            out.append((x * (29 + 6 * b) + y * (13 + 24 * b) + (x * y) % 7 * 31 + 77 * b) % 256)
    return bytes(out)


def seg(code: int, body: bytes) -> bytes:
    return struct.pack(">HH", code, 2 + len(body)) + body


def run(*argv: str) -> None:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}\n{result.stderr}")


def segment_max_passes(style: int, maxima: list[int]) -> int:
    """`opj_t2_init_seg`: how many passes the next segment may hold, given the
    maximum of each segment already opened."""
    if style & 0x04:
        return 1
    if style & LAZY:
        if not maxima:
            return 10
        return 2 if maxima[-1] in (1, 10) else 1
    return 109


class Segments:
    """A block's codeword segments as passes fill them: each one's maximum and
    the passes it holds so far."""

    def __init__(self, style: int) -> None:
        self.style = style
        self.maxima: list[int] = []
        self.filled: list[int] = []

    def add(self, passes: int) -> list[tuple[int, int]]:
        """Add `passes` passes; return (segment index, passes added) per
        segment they touch, in order."""
        parts = []
        while passes:
            if not self.maxima or self.filled[-1] == self.maxima[-1]:
                self.maxima.append(segment_max_passes(self.style, self.maxima))
                self.filled.append(0)
            n = min(passes, self.maxima[-1] - self.filled[-1])
            self.filled[-1] += n
            parts.append((len(self.maxima) - 1, n))
            passes -= n
        return parts


class BitReader:
    """Packet header bits as B.10.1 packs them: after 0xFF, 7 bits."""

    def __init__(self, data: bytes) -> None:
        self.data, self.pos, self.buf, self.ct = data, 0, 0, 0

    def bit(self) -> int:
        if self.ct == 0:
            self.ct = 7 if self.buf == 0xFF else 8
            self.buf = self.data[self.pos]
            self.pos += 1
        self.ct -= 1
        return (self.buf >> self.ct) & 1

    def bits(self, n: int) -> int:
        v = 0
        for _ in range(n):
            v = (v << 1) | self.bit()
        return v

    def align(self) -> int:
        """Bytes the header used, with the stuffed byte after a final 0xFF."""
        return self.pos + (1 if self.buf == 0xFF else 0)


class Bits:
    """Packet header bits, packed as B.10.1 has it."""

    def __init__(self) -> None:
        self.out = bytearray()
        self.byte = 0
        self.room = 8
        self.used = 0

    def put(self, bit: int) -> None:
        self.byte = (self.byte << 1) | bit
        self.used += 1
        if self.used == self.room:
            self.out.append(self.byte)
            self.room = 7 if self.byte == 0xFF else 8
            self.byte = 0
            self.used = 0

    def put_int(self, value: int, width: int) -> None:
        assert 0 <= value < (1 << width), (value, width)
        for shift in range(width - 1, -1, -1):
            self.put((value >> shift) & 1)

    def finish(self) -> bytes:
        if self.used:
            self.out.append(self.byte << (self.room - self.used))
        if self.out and self.out[-1] == 0xFF:
            self.out.append(0)
        return bytes(self.out)


def read_num_passes(r: BitReader) -> int:
    if r.bit() == 0:
        return 1
    if r.bit() == 0:
        return 2
    n = r.bits(2)
    if n != 3:
        return 3 + n
    n = r.bits(5)
    if n != 31:
        return 6 + n
    return 37 + r.bits(7)


def put_num_passes(bits: Bits, passes: int) -> None:
    """Table B.4."""
    if passes == 1:
        bits.put(0)
    elif passes == 2:
        bits.put_int(0b10, 2)
    elif passes <= 5:
        bits.put_int(0b11, 2)
        bits.put_int(passes - 3, 2)
    elif passes <= 36:
        bits.put_int(0b1111, 4)
        bits.put_int(passes - 6, 5)
    else:
        bits.put_int(0b111111111, 9)
        bits.put_int(passes - 37, 7)


def parse_block(codestream: bytes, style: int) -> tuple[int, int, list[bytes]]:
    """An `opj_compress` codestream of one 8x8 code-block in one layer: its
    zero bit-planes, its passes and the bytes of each codeword segment."""
    sod = codestream.index(b"\xff\x93") + 2
    r = BitReader(codestream[sod:])
    assert r.bit() == 1, "empty packet"
    assert r.bit() == 1, "block not included"
    zbp = 0
    while r.bit() == 0:
        zbp += 1
    passes = read_num_passes(r)
    lblock = 3
    while r.bit() == 1:
        lblock += 1
    parts = Segments(style).add(passes)
    lengths = [r.bits(lblock + (n.bit_length() - 1)) for _, n in parts]
    pos = sod + r.align()
    chunks = []
    for length in lengths:
        chunks.append(codestream[pos : pos + length])
        pos += length
    assert codestream[pos : pos + 2] == b"\xff\xd9", "bytes left after the packet"
    return zbp, passes, chunks


def put_tag(bits: Bits, levels: list[list[int]], known: list[list[int]], index: int,
            threshold: int) -> None:
    """Code leaf `index` of a one-row tag tree up to `threshold`
    (`opj_tgt_encode`). `levels[0]` holds the leaves; each node is the
    minimum of its two children. `known[l][i]` is node i's current value, or
    -1-value once it is known."""
    low = 0
    for level in range(len(levels) - 1, -1, -1):
        i = index >> level
        value = levels[level][i]
        state = known[level][i]
        if state < 0:  # known
            low = max(low, -1 - state)
            continue
        low = max(low, state)
        while low < threshold:
            if low >= value:
                bits.put(1)
                known[level][i] = -1 - value
                break
            bits.put(0)
            low += 1
        else:
            known[level][i] = low


def tag_levels(leaves: list[int]) -> list[list[int]]:
    levels = [leaves]
    while len(levels[-1]) > 1:
        row = levels[-1]
        levels.append([min(row[i : i + 2]) for i in range(0, len(row), 2)])
    return levels


def build(style: int, qcd: bytes, width: int, blocks, plans) -> tuple[bytes, list[bool]]:
    """The codestream, and per block whether every byte was sent."""
    count = len(blocks)
    segments = [Segments(style) for _ in range(count)]
    sent = [[0] * len(chunks) for _, _, chunks in blocks]
    passes_left = [passes for _, passes, _ in blocks]
    lblock = [3] * count
    inclusion = tag_levels([0] * count)
    inclusion_known = [[0] * len(row) for row in inclusion]
    zbp = tag_levels([z for z, _, _ in blocks])
    zbp_known = [[0] * len(row) for row in zbp]

    data = b""
    for layer in range(2):
        bits = Bits()
        bits.put(1)  # not empty
        body = b""
        for b, (_, _, chunks) in enumerate(blocks):
            passes, spec = plans[b][layer]
            if passes == REST:
                passes = passes_left[b]
            assert 0 < passes <= passes_left[b], (b, layer, passes)
            passes_left[b] -= passes
            if layer == 0:
                # Every block is first included in layer 0.
                put_tag(bits, inclusion, inclusion_known, b, 1)
                put_tag(bits, zbp, zbp_known, b, 999)
            else:
                bits.put(1)  # contributes
            parts = segments[b].add(passes)
            if spec.endswith("*"):
                spec = spec[:-1] * len(parts)
            assert len(spec) == len(parts), (b, layer, spec, parts)
            lengths = []
            for (index, _), kind in zip(parts, spec):
                piece = chunks[index][sent[b][index] :] if kind == "a" else b""
                sent[b][index] += len(piece)
                lengths.append(len(piece))
                body += piece
            # Lblock grows once, before the lengths, so that every one fits
            # (B.10.7.1, B.10.7.2).
            put_num_passes(bits, passes)
            need = max(max(length.bit_length() - (n.bit_length() - 1), 3)
                       for (_, n), length in zip(parts, lengths))
            grow = max(need - lblock[b], 0)
            for _ in range(grow):
                bits.put(1)
            bits.put(0)
            lblock[b] += grow
            for (_, n), length in zip(parts, lengths):
                bits.put_int(length, lblock[b] + (n.bit_length() - 1))
        data += bits.finish() + body

    complete = [
        passes_left[b] == 0 and sent[b] == [len(c) for c in blocks[b][2]] for b in range(count)
    ]
    siz = seg(
        0xFF51,
        struct.pack(">HIIIIIIIIH", 0, width, HEIGHT, 0, 0, width, HEIGHT, 0, 0, 1) + bytes([7, 1, 1]),
    )
    # LRCP, 2 layers, no MCT; no wavelet level, 8x8 code-blocks, 5/3.
    cod = seg(0xFF52, bytes([0, 0]) + struct.pack(">H", 2) + bytes([0, 0, 1, 1, style, 1]))
    psot = 12 + 2 + len(data)
    part = struct.pack(">HHHIBB", 0xFF90, 10, 0, psot, 0, 1) + b"\xff\x93" + data
    return b"\xff\x4f" + siz + cod + qcd + part + b"\xff\xd9", complete


def marker_segment(codestream: bytes, code: int) -> bytes:
    offset = 2
    while offset < len(codestream) - 3:
        marker, length = struct.unpack(">HH", codestream[offset : offset + 4])
        if marker == code:
            return codestream[offset : offset + 2 + length]
        offset += 2 + length
    sys.exit(f"no marker {code:#06x}")


def read_pgx(path: Path, width: int) -> list[int]:
    raw = path.read_bytes()
    end = raw.index(b"\n")
    tokens = raw[:end].split()
    assert tokens[0] == b"PG" and tokens[2] == b"+" and int(tokens[3]) == 8, tokens
    assert (int(tokens[4]), int(tokens[5])) == (width, HEIGHT), tokens
    return list(raw[end + 1 : end + 1 + width * HEIGHT])


def main() -> None:
    require_openjpeg("opj_compress", "opj_decompress")
    for name, (mask, style, plans) in VARIANTS.items():
        width = BLOCK * len(plans)
        image = source(width)
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            blocks = []
            qcd = b""
            for b in range(len(plans)):
                pixels = bytes(
                    image[y * width + b * BLOCK + x] for y in range(BLOCK) for x in range(BLOCK)
                )
                pgm = tmp / f"block{b}.pgm"
                pgm.write_bytes(b"P5\n8 8\n255\n" + pixels)
                j2k = tmp / f"block{b}.j2k"
                run("opj_compress", "-i", str(pgm), "-o", str(j2k), "-n", "1", "-b", "8,8",
                    *(["-M", str(mask)] if mask else []))
                raw = j2k.read_bytes()
                cod = marker_segment(raw, 0xFF52)
                if cod[4 + 8] != style:
                    sys.exit(f"{name}: code-block style {cod[4 + 8]:#04x}, expected {style:#04x}")
                if qcd and marker_segment(raw, 0xFF5C) != qcd:
                    sys.exit(f"{name}: the blocks' QCD differ")
                qcd = marker_segment(raw, 0xFF5C)
                blocks.append(parse_block(raw, style))
            codestream, complete = build(style, qcd, width, blocks, plans)
            out = FIXTURES / f"{name}.j2k"
            out.write_bytes(codestream)

            run("opj_decompress", "-i", str(out), "-o", str(tmp / "out.pgx"))
            samples = read_pgx(tmp / "out_0.pgx", width)
            for b, whole in enumerate(complete):
                if not whole:
                    continue
                for y in range(HEIGHT):
                    for x in range(b * BLOCK, (b + 1) * BLOCK):
                        if samples[y * width + x] != image[y * width + x]:
                            sys.exit(f"{name}: block {b} does not decode to the source")

        snapshot = {
            "image": {"width": width, "height": HEIGHT},
            "tolerance": {"mode": "exact"},
            "components": [
                {
                    "width": width,
                    "height": HEIGHT,
                    "bit_depth": 8,
                    "signed": False,
                    "x_sampling": 1,
                    "y_sampling": 1,
                    "samples": samples,
                }
            ],
            "provenance": {
                "source": f"synthetic; generated by scripts/{Path(__file__).name}",
                "oracle_command": f"opj_decompress -i {name}.j2k -o out.pgx",
                "notes": "8x8 code-blocks over two layers whose contributions include coding "
                "passes with a length of zero; a block with every byte decodes to the source, "
                "the others in part or wholly from the 0xFF fill",
            },
        }
        (FIXTURES / f"{name}.expected.json").write_text(json.dumps(snapshot, indent=2) + "\n")
        summary = ", ".join(f"block {b}: {p} passes, zbp {z}" for b, (z, p, _) in enumerate(blocks))
        print(f"wrote {name}.j2k + {name}.expected.json ({len(codestream)} bytes; {summary})")


if __name__ == "__main__":
    main()
