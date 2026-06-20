#!/usr/bin/env python3
"""Generate golden Tier-1 (EBCOT) code-block vectors for src/tier1/passes.rs.

This is a developer/oracle tool. It is NOT run at `cargo test` time — the emitted
vectors are committed and become the contract, exactly like the conformance
snapshots (see docs/correctness.md, docs/development.md). CI never invokes it.

How a vector is made decoder-independently:

  1. Build a tiny grayscale PGM with known samples.
  2. Compress it reversibly (5/3) with `opj_compress` at ONE resolution level
     (`-n 1`), i.e. NO wavelet transform, one large code-block, default style.
     The lone subband is then the LL band and its quantized coefficients are
     just the DC-level-shifted samples.
  3. Decompress with `opj_decompress`. Because the 5/3 path is lossless, the
     decoded samples equal the originals, so the ground-truth coefficient grid
     is `sample - 128` — recovered from the reference decoder, never from ours.
  4. Parse the single packet header (Annex B.10, the Phase-1 subset: one tile,
     one component, one resolution, one code-block, one layer, LRCP, no
     SOP/EPH/precincts) to read `zero_bit_planes` and `num_passes` and to slice
     out the raw MQ code-block segment that Tier-1 consumes.

The parse is self-checked: packet-header bytes + segment length must equal the
whole packet body, so a misread of any field is caught here, at authoring time.

Why all vectors are reversible (5/3): Tier-1 block decoding is identical for 5/3
and 9/7 — `decode_block` has no kernel branch — so a reversible block, whose
expected coefficients are bit-exact, fully exercises the code. A 0-level 9/7
codestream is NOT lossless (quantization), so `sample - 128` would not be a
trustworthy coefficient oracle; the 9/7 quantization/DWT path is graded instead
at integration (issue #17) against the OpenJPEG oracle on the lossy fixture.

Usage:
    scripts/gen-tier1-vectors.py            # writes src/tier1/golden_vectors.rs
    scripts/gen-tier1-vectors.py -o -       # print to stdout
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

DC_SHIFT = 128  # 8-bit unsigned samples -> signed via -2^(bitdepth-1)


def need(tool: str) -> str:
    path = shutil.which(tool)
    if path is None:
        sys.exit(
            f"gen-tier1-vectors: {tool} not found — run "
            "scripts/install-oracle-tools.sh (see docs/development.md)"
        )
    return path


def write_pgm(path: Path, width: int, height: int, samples: list[int]) -> None:
    assert len(samples) == width * height
    assert all(0 <= s <= 255 for s in samples)
    with path.open("wb") as f:
        f.write(f"P5\n{width} {height}\n255\n".encode())
        f.write(bytes(samples))


def read_pgm(path: Path) -> tuple[int, int, list[int]]:
    data = path.read_bytes()
    assert data[:2] == b"P5", "expected a binary PGM"
    idx, toks = 2, []
    while len(toks) < 3:
        while data[idx] in b" \t\n\r":
            idx += 1
        if data[idx : idx + 1] == b"#":  # OpenJPEG writes a comment line
            while data[idx] not in b"\n":
                idx += 1
            continue
        start = idx
        while data[idx] not in b" \t\n\r":
            idx += 1
        toks.append(int(data[start:idx]))
    idx += 1  # single whitespace after maxval
    width, height, _maxval = toks
    return width, height, list(data[idx : idx + width * height])


class Bio:
    """Packet-header bit reader with JPEG 2000 bit unstuffing (Annex B.10.1):
    the byte after a 0xFF carries only its low 7 bits (the MSB is a stuffed 0)."""

    def __init__(self, data: bytes):
        self.data = data
        self.bp = 0
        self.buf = 0
        self.ct = 0

    def _bytein(self) -> None:
        prev = self.data[self.bp - 1] if self.bp > 0 else 0
        self.buf = self.data[self.bp] if self.bp < len(self.data) else 0xFF
        self.ct = 7 if prev == 0xFF else 8
        self.bp += 1

    def read1(self) -> int:
        if self.ct == 0:
            self._bytein()
        self.ct -= 1
        return (self.buf >> self.ct) & 1

    def read(self, n: int) -> int:
        v = 0
        for _ in range(n):
            v = (v << 1) | self.read1()
        return v

    def byte_pos(self) -> int:
        # Index of the next whole byte, accounting for a partially consumed one.
        return self.bp if self.ct == 0 else self.bp


def read_num_passes(bio: Bio) -> int:
    """Annex B Table B.4 (matches OpenJPEG opj_t2_getnumpasses)."""
    if bio.read1() == 0:
        return 1
    if bio.read1() == 0:
        return 2
    n = bio.read(2)
    if n != 3:
        return 3 + n
    n = bio.read(5)
    if n != 31:
        return 6 + n
    return 37 + bio.read(7)


def find_marker(data: bytes, marker: int) -> int:
    needle = bytes((marker >> 8, marker & 0xFF))
    pos = data.find(needle)
    if pos < 0:
        raise ValueError(f"marker {marker:04X} not found")
    return pos


def extract_segment(j2k: bytes) -> tuple[int, int, bytes]:
    """Return (zero_bit_planes, num_passes, code-block segment) for a codestream
    with a single tile / component / resolution / code-block / layer."""
    sod = find_marker(j2k, 0xFF93)
    eoc = find_marker(j2k, 0xFFD9)
    body = j2k[sod + 2 : eoc]

    bio = Bio(body)
    present = bio.read1()
    if present != 1:
        raise ValueError("empty packet: no code-block content")
    inclusion = bio.read1()  # 1-node tag tree, first-layer inclusion
    if inclusion != 1:
        raise ValueError("code-block not included in the first layer")
    zero_bit_planes = 0  # 1-node tag tree: unary run of zeros then a 1
    while bio.read1() == 0:
        zero_bit_planes += 1
    num_passes = read_num_passes(bio)
    lblock = 3  # initial Lblock, raised by a unary run of 1s
    while bio.read1() == 1:
        lblock += 1
    length_bits = lblock + (num_passes.bit_length() - 1)  # + floor(log2 passes)
    seg_len = bio.read(length_bits)

    # The code-block data is byte-aligned right after the header (no EPH here).
    if bio.ct != 0:
        bio.ct = 0
        bio.bp += 0  # the partial byte is consumed; next read starts at bp
    header_bytes = bio.bp
    segment = body[header_bytes : header_bytes + seg_len]
    if header_bytes + seg_len != len(body):
        raise ValueError(
            f"packet parse mismatch: {header_bytes} header + {seg_len} segment "
            f"!= {len(body)} body bytes"
        )
    return zero_bit_planes, num_passes, bytes(segment)


def make_vector(name: str, width: int, height: int, samples: list[int], note: str):
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        pgm = tmp / "in.pgm"
        j2k = tmp / "out.j2k"
        out = tmp / "out.pgm"
        write_pgm(pgm, width, height, samples)
        # -n 1: one resolution => no DWT.  -b: one code-block covers the image.
        # -r 1: lossless rate.  Default (reversible) 5/3, default code-block style.
        subprocess.run(
            [need("opj_compress"), "-i", str(pgm), "-o", str(j2k),
             "-n", "1", "-b", "64,64", "-r", "1"],
            check=True, capture_output=True,
        )
        subprocess.run(
            [need("opj_decompress"), "-i", str(j2k), "-o", str(out)],
            check=True, capture_output=True,
        )
        _, _, decoded = read_pgm(out)
        if decoded != samples:
            raise SystemExit(f"{name}: 5/3 roundtrip not lossless; cannot trust oracle")
        zbp, num_passes, segment = extract_segment(j2k.read_bytes())
        coeffs = [s - DC_SHIFT for s in decoded]
        return {
            "name": name,
            "note": note,
            "width": width,
            "height": height,
            "zero_bit_planes": zbp,
            "num_passes": num_passes,
            "segment": segment,
            "coeffs": coeffs,
        }


def gradient_8x8() -> list[int]:
    vals = [(x * 16 + y * 2) % 256 for y in range(8) for x in range(8)]
    vals[0] = 200   # a positive spike at the origin
    vals[9] = 5     # a strong negative coefficient (5 - 128)
    vals[63] = 255  # the maximum positive coefficient (127)
    return vals


def sparse_8x8() -> list[int]:
    # Mostly the DC value (coefficient 0) so the cleanup pass runs its run-length
    # mode over whole insignificant columns, with a few signed significant
    # coefficients scattered to exercise sign coding and refinement.
    vals = [DC_SHIFT] * 64
    for (x, y, v) in [(1, 1, 150), (6, 2, 96), (2, 5, 140), (5, 6, 100), (3, 3, 160)]:
        vals[y * 8 + x] = v
    return vals


def small_4x4() -> list[int]:
    # Small magnitudes => few bit-planes => small num_passes, easy to reason
    # about and a different pass count from the 8x8 blocks (so an exact-grid
    # match proves the loop stops at the right pass).
    base = [
        130, 124, 128, 131,
        128, 120, 136, 128,
        125, 128, 128, 122,
        128, 133, 127, 128,
    ]
    return base


def emit_rust(vectors: list[dict]) -> str:
    def hexbytes(b: bytes) -> str:
        return ", ".join(f"0x{x:02x}" for x in b)

    lines = [
        "// @generated by scripts/gen-tier1-vectors.py — do not edit by hand.",
        "//",
        "// Golden Tier-1 code-block vectors. Each is a real MQ code-block segment",
        "// sliced from a reversible (5/3), single-resolution OpenJPEG codestream;",
        "// `coeffs` is the decoder-independent ground truth (decoded sample minus",
        "// the 2^7 DC level shift). See the generator for the full provenance.",
        "",
        "/// One golden code-block: the coded MQ `segment` plus the Tier-2-supplied",
        "/// `num_passes`/`zero_bit_planes` decode to the signed `coeffs` grid.",
        "pub(super) struct GoldenBlock {",
        "    pub name: &'static str,",
        "    pub width: u32,",
        "    pub height: u32,",
        "    pub num_passes: u32,",
        "    pub zero_bit_planes: u32,",
        "    pub segment: &'static [u8],",
        "    pub coeffs: &'static [i32],",
        "}",
        "",
        "pub(super) const GOLDEN_BLOCKS: &[GoldenBlock] = &[",
    ]
    for v in vectors:
        lines += [
            "    GoldenBlock {",
            f'        name: "{v["name"]}", // {v["note"]}',
            f'        width: {v["width"]},',
            f'        height: {v["height"]},',
            f'        num_passes: {v["num_passes"]},',
            f'        zero_bit_planes: {v["zero_bit_planes"]},',
            f"        segment: &[{hexbytes(v['segment'])}],",
            f"        coeffs: &{v['coeffs']},",
            "    },",
        ]
    lines += ["];", ""]
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "-o", "--output", default="src/tier1/golden_vectors.rs",
        help="output path, or - for stdout (default: src/tier1/golden_vectors.rs)",
    )
    args = parser.parse_args()

    vectors = [
        make_vector("gradient_8x8", 8, 8, gradient_8x8(),
                    "gradient + spikes: all three passes, signs, refinement"),
        make_vector("sparse_8x8", 8, 8, sparse_8x8(),
                    "mostly-DC: heavy cleanup run-length over insignificant columns"),
        make_vector("small_4x4", 4, 4, small_4x4(),
                    "small magnitudes: few bit-planes, low num_passes"),
    ]
    rust = emit_rust(vectors)
    if args.output == "-":
        sys.stdout.write(rust)
    else:
        Path(args.output).write_text(rust)
        print(f"gen-tier1-vectors: wrote {args.output}")


if __name__ == "__main__":
    main()
