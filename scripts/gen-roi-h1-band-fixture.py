#!/usr/bin/env python3
"""Write tests/fixtures/roi_h1_band/: a lossless 8x8 encode with a maxshift
of 7 whose background coefficients sit in the band 2^(s-1)..2^s-1.

`opj_compress -ROI` only signals SPrgn and raises no zero bit-planes, so it
cannot make a stream whose background sits in the band. This script
encodes without ROI (`-n 1`, so the coefficients are the level-shifted
samples), then adds the RGN marker (Srgn = 0, SPrgn = 7) and raises the
packet's zero bit-planes by 7, which is what a maxshift encoder signals for
a block with no region (H.2: the count is against Mb + s). The coded bytes
are unchanged. Every sample then has |sample - 128| < 2^7, so all are
background and H.1 returns the source.

Usage: gen-roi-h1-band-fixture.py [outdir]   (needs OpenJPEG 2.5.0 opj_compress)
"""
import subprocess
import sys
from pathlib import Path

S = 7
out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/fixtures/roi_h1_band")
out.mkdir(parents=True, exist_ok=True)

# |v - 128| < 128 everywhere; 64, 192, 228, 255 and 200 have bit 6 (s - 1) set
# in |v - 128|.
vals = [128, 133, 123, 192, 64, 228, 28, 255,
        1, 160, 96, 127, 129, 200, 50, 128]
rows = [(vals * 4)[i * 8:(i + 1) * 8] for i in range(8)]
(out / "source.pgm").write_bytes(b"P5\n8 8\n255\n" + bytes(sum(rows, [])))
base = out / "base.tmp.j2k"
subprocess.run(
    ["opj_compress", "-i", str(out / "source.pgm"), "-o", str(base), "-n", "1"],
    check=True,
    capture_output=True,
)

d = base.read_bytes()
base.unlink()
s = S
sod = d.index(b"\xff\x93") + 2
eoc = len(d) - 2
assert d[eoc:] == b"\xff\xd9"
pos = sod
cur = 0
n = 0


def rd():
    """One packet-header bit; the byte after 0xFF carries 7 bits."""
    global pos, cur, n
    if n == 0:
        stuffed = pos > sod and d[pos - 1] == 0xFF
        cur = d[pos]
        pos += 1
        n = 7 if stuffed else 8
    n -= 1
    return (cur >> n) & 1


assert rd() == 1  # non-empty packet
assert rd() == 1  # the block is included in layer 0
P = 0
while rd() == 0:
    P += 1
if rd() == 0:
    passes = 1
elif rd() == 0:
    passes = 2
else:
    v = (rd() << 1) | rd()
    if v < 3:
        passes = 3 + v
    else:
        v = 0
        for _ in range(5):
            v = (v << 1) | rd()
        assert v < 31
        passes = 6 + v
lb = 3
while rd() == 1:
    lb += 1
nb = lb + passes.bit_length() - 1
length = 0
for _ in range(nb):
    length = (length << 1) | rd()
hdr_end = pos
body = d[hdr_end:eoc]
assert length == len(body)

bits = [1, 1] + [0] * (P + s) + [1]
if passes == 1:
    bits += [0]
elif passes == 2:
    bits += [1, 0]
elif passes <= 5:
    bits += [1, 1, ((passes - 3) >> 1) & 1, (passes - 3) & 1]
else:
    assert passes <= 36
    bits += [1, 1, 1, 1] + [((passes - 6) >> i) & 1 for i in (4, 3, 2, 1, 0)]
bits += [1] * (lb - 3) + [0]
bits += [(length >> i) & 1 for i in range(nb - 1, -1, -1)]
header = bytearray()
cur = 0
cnt = 0
cap = 8
for b in bits:
    cur = (cur << 1) | b
    cnt += 1
    if cnt == cap:
        header.append(cur)
        cap = 7 if cur == 0xFF else 8
        cur = 0
        cnt = 0
if cnt:
    header.append(cur << (cap - cnt))
    if header[-1] == 0xFF:
        header.append(0)

rgn = bytes([0xFF, 0x5E, 0, 5, 0, 0, s])
sot = d.index(b"\xff\x90")
new = bytearray(d[:sot] + rgn + d[sot:sod] + bytes(header) + body + d[eoc:])
psot = sot + len(rgn) + 6
old = int.from_bytes(new[psot:psot + 4], "big")
new[psot:psot + 4] = (old - (hdr_end - sod) + len(header)).to_bytes(4, "big")
(out / f"roi_band_s{s}.j2k").write_bytes(new)
