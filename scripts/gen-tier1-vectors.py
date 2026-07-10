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

The detail-band vectors (HL/LH/HH context tables, which a no-DWT codestream
never reaches) come from a ONE-level 5/3 codestream instead. Their coefficient
oracle is a forward 5/3 transform implemented here from Annex F — independent
of the Rust decoder — applied to the DC-shifted samples. It is self-checked
two ways: the Python inverse must reconstruct the samples bit-exactly, and the
Rust golden test decodes OpenJPEG's actual coded segments against these
coefficients, so a transform or parse mistake here fails at authoring time
rather than committing a wrong oracle.

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

    def align(self) -> None:
        """Byte-align at the end of a packet header (Annex B.10.1): drop any
        partially-read byte, and if the last whole byte was 0xFF consume the
        stuffed byte that follows it — its bits are not packet-body content."""
        if self.buf == 0xFF:
            self._bytein()
        self.ct = 0


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


def read_block_header(bio: Bio) -> tuple[int, int, int]:
    """One included code-block's (zero_bit_planes, num_passes, segment_length)
    from a packet header — the single-block-per-band shape every vector uses."""
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
    return zero_bit_planes, num_passes, seg_len


def parse_packet(body: bytes, nbands: int) -> tuple[list[tuple[int, int, bytes]], int]:
    """Parse one packet holding one code-block per band. Returns each block's
    (zero_bit_planes, num_passes, segment) plus the bytes the packet spans."""
    bio = Bio(body)
    if bio.read1() != 1:
        raise ValueError("empty packet: no code-block content")
    heads = [read_block_header(bio) for _ in range(nbands)]
    bio.align()
    at = bio.byte_pos()
    blocks = []
    for zbp, passes, seg_len in heads:
        blocks.append((zbp, passes, bytes(body[at : at + seg_len])))
        at += seg_len
    return blocks, at


def codestream_body(j2k: bytes) -> bytes:
    return j2k[find_marker(j2k, 0xFF93) + 2 : find_marker(j2k, 0xFFD9)]


def extract_segment(j2k: bytes) -> tuple[int, int, bytes]:
    """Return (zero_bit_planes, num_passes, code-block segment) for a codestream
    with a single tile / component / resolution / code-block / layer."""
    body = codestream_body(j2k)
    blocks, consumed = parse_packet(body, 1)
    if consumed != len(body):
        raise ValueError(
            f"packet parse mismatch: consumed {consumed} != {len(body)} body bytes"
        )
    return blocks[0]


# ---- Forward 5/3 (Annex F), the detail-band coefficient oracle --------------


def reflect(i: int, n: int) -> int:
    """Whole-sample symmetric extension (F.3.6), period 2(n-1)."""
    if n == 1:
        return 0
    period = 2 * (n - 1)
    k = i % period  # Python % is already non-negative
    return period - k if k >= n else k


def forward_5_3(sig: list[int]) -> list[int]:
    """One 1-D forward 5/3 lifting pass, in place order: predict the odd
    (high-pass) samples, then update the even (low-pass) ones (F.4.8.2 run
    forward; Python // floors like the standard's floor)."""
    n = len(sig)
    s = list(sig)
    if n <= 1:
        return s
    for i in range(1, n, 2):
        s[i] -= (s[reflect(i - 1, n)] + s[reflect(i + 1, n)]) // 2
    for i in range(0, n, 2):
        s[i] += (s[reflect(i - 1, n)] + s[reflect(i + 1, n)] + 2) // 4
    return s


def inverse_5_3(sig: list[int]) -> list[int]:
    """The exact inverse of forward_5_3, for the round-trip self-check."""
    n = len(sig)
    s = list(sig)
    if n <= 1:
        return s
    for i in range(0, n, 2):
        s[i] -= (s[reflect(i - 1, n)] + s[reflect(i + 1, n)] + 2) // 4
    for i in range(1, n, 2):
        s[i] += (s[reflect(i - 1, n)] + s[reflect(i + 1, n)]) // 2
    return s


def dwt_1level(width: int, height: int, grid: list[int]) -> dict[str, list[int]]:
    """One 5/3 decomposition of a DC-shifted sample grid into its four
    subbands. Columns first, then rows — the inverse composition of the
    decoder's rows-then-columns inverse (and OpenJPEG's encode order)."""
    a = [list(grid[y * width : (y + 1) * width]) for y in range(height)]
    for x in range(width):
        col = forward_5_3([a[y][x] for y in range(height)])
        for y in range(height):
            a[y][x] = col[y]
    for y in range(height):
        a[y] = forward_5_3(a[y])

    # Self-check: the Python inverse (rows then columns) must round-trip.
    b = [inverse_5_3(row) for row in a]
    for x in range(width):
        col = inverse_5_3([b[y][x] for y in range(height)])
        for y in range(height):
            b[y][x] = col[y]
    flat = [v for row in b for v in row]
    if flat != list(grid):
        raise SystemExit("forward/inverse 5/3 do not round-trip; oracle untrusted")

    # Deinterleave: even/even LL, odd-column HL, odd-row LH, odd/odd HH.
    def band(px: int, py: int) -> list[int]:
        return [
            a[y][x]
            for y in range(py, height, 2)
            for x in range(px, width, 2)
        ]

    return {"ll": band(0, 0), "hl": band(1, 0), "lh": band(0, 1), "hh": band(1, 1)}


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
            "orient": "Ll",
            "width": width,
            "height": height,
            "zero_bit_planes": zbp,
            "num_passes": num_passes,
            "segment": segment,
            "coeffs": coeffs,
        }


def make_dwt_vectors(prefix: str, width: int, height: int, samples: list[int]):
    """Four vectors (LL + HL/LH/HH) from a ONE-level 5/3 codestream, so the
    detail-band context tables are checked against real OpenJPEG segments.
    The coefficient oracle is `dwt_1level` over the DC-shifted samples."""
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        pgm = tmp / "in.pgm"
        j2k = tmp / "out.j2k"
        out = tmp / "out.pgm"
        write_pgm(pgm, width, height, samples)
        # -n 2: two resolutions => one decomposition level. One block per band.
        subprocess.run(
            [need("opj_compress"), "-i", str(pgm), "-o", str(j2k),
             "-n", "2", "-b", "64,64", "-r", "1"],
            check=True, capture_output=True,
        )
        subprocess.run(
            [need("opj_decompress"), "-i", str(j2k), "-o", str(out)],
            check=True, capture_output=True,
        )
        _, _, decoded = read_pgm(out)
        if decoded != samples:
            raise SystemExit(f"{prefix}: 5/3 roundtrip not lossless; cannot trust oracle")

        bands = dwt_1level(width, height, [s - DC_SHIFT for s in samples])

        # LRCP with one layer: packet 0 carries the LL block, packet 1 the
        # HL/LH/HH blocks in band order (B.10).
        body = codestream_body(j2k.read_bytes())
        (ll_block,), consumed = parse_packet(body, 1)
        detail, used = parse_packet(body[consumed:], 3)
        if consumed + used != len(body):
            raise ValueError(
                f"{prefix}: packets span {consumed + used} != {len(body)} body bytes"
            )

        bw, bh = width // 2 + width % 2, height // 2 + height % 2
        vectors = []
        for (orient, dims, block) in [
            ("Ll", (bw, bh), ll_block),
            ("Hl", (width - bw, bh), detail[0]),
            ("Lh", (bw, height - bh), detail[1]),
            ("Hh", (width - bw, height - bh), detail[2]),
        ]:
            zbp, num_passes, segment = block
            vectors.append({
                "name": f"{prefix}_{orient.lower()}_{dims[0]}x{dims[1]}",
                "note": f"{orient.upper()} band of a one-level 5/3 codestream",
                "orient": orient,
                "width": dims[0],
                "height": dims[1],
                "zero_bit_planes": zbp,
                "num_passes": num_passes,
                "segment": segment,
                "coeffs": bands[orient.lower()],
            })
        return vectors


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


def textured_8x8() -> list[int]:
    # Horizontal, vertical, and diagonal energy plus signed swings around the
    # DC value, so every detail band's block is included with nonzero
    # coefficients and the sign/run-length paths run in all three contexts.
    return [
        (128 + 61 * ((x + y) % 2) - 30 + 9 * x - 7 * y + (x * y) % 5 * 6) % 256
        for y in range(8)
        for x in range(8)
    ]


def emit_rust(vectors: list[dict]) -> str:
    def hexbytes(b: bytes) -> str:
        return ", ".join(f"0x{x:02x}" for x in b)

    lines = [
        "// @generated by scripts/gen-tier1-vectors.py — do not edit by hand.",
        "//",
        "// Golden Tier-1 code-block vectors. Each is a real MQ code-block segment",
        "// sliced from a reversible (5/3) OpenJPEG codestream. For the LL vectors",
        "// (a single-resolution codestream, so no wavelet ran) `coeffs` is the",
        "// decoded sample minus the 2^7 DC level shift, straight from",
        "// `opj_decompress`; for the HL/LH/HH vectors (one decomposition level)",
        "// it is the generator's own Annex F forward 5/3 of the shifted samples,",
        "// cross-checked by the lossless round trip. See the generator for the",
        "// full provenance.",
        "",
        "use super::Orientation;",
        "",
        "/// One golden code-block: the coded MQ `segment` plus the Tier-2-supplied",
        "/// `num_passes`/`zero_bit_planes` decode to the signed `coeffs` grid.",
        "pub(super) struct GoldenBlock {",
        "    pub name: &'static str,",
        "    pub orient: Orientation,",
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
            f'        orient: Orientation::{v["orient"]},',
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
    ] + make_dwt_vectors("dwt1", 8, 8, textured_8x8())
    rust = emit_rust(vectors)
    if args.output == "-":
        sys.stdout.write(rust)
    else:
        Path(args.output).write_text(rust)
        print(f"gen-tier1-vectors: wrote {args.output}")


if __name__ == "__main__":
    main()
