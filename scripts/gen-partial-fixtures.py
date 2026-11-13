#!/usr/bin/env python3
"""Generate the partial decode fixtures in tests/fixtures/partial/.

`tests/partial_decode.rs` decodes these with `strict` off. Each oracle is an
`opj_decompress -allow-partial` output, so the tests check this decoder
against OpenJPEG on the same bytes.

The script writes `source.pgm` (64 x 64, 8-bit, a gradient plus noise from a
fixed LCG) and `tp_r_source.raw` (2 x 99, 1-bit, from the same LCG), encodes
them with `opj_compress` 2.5.0, and writes:

- `lrcp.j2k`, one tile, one layer, five decomposition levels. For each
  reduction r from 0 to 5, `lrcp.prefixes` gives the shortest prefix whose
  `opj_decompress -allow-partial -r r` output equals the whole file's
  (`lrcp_r<r>.pgm`), and `lrcp_r<r>_short.pgm` is the output of the prefix
  one byte shorter. The script checks that output differs from the whole
  file's.
- `tiles.j2k`, four 32 x 32 tiles, cut in the middle of tile 1's tile-part
  (`tiles.cut`): `tiles_cut.pgm` is the output of that prefix, where tiles 2
  and 3 are 0.
- `overlap.j2k`, two POC volumes over the same packets, which `opj_compress`
  writes twice: OpenJPEG decodes it to the source.
- `tp_r.j2k`, `-TP R` with empty resolutions, where `opj_compress` writes a
  stray byte into each tile-part of an empty resolution: `tp_r.pgm` is
  OpenJPEG's output, which is not the source.
- `psot0_single_no_eoc.pgm` and `psot0_tileparts_no_eoc.pgm`, the outputs of
  `../psot0/single_psot0_no_eoc.j2k` and `../psot0/tileparts_psot0_no_eoc.j2k`,
  which `opj_decompress` refuses without `-allow-partial`.

Usage: python3 scripts/gen-partial-fixtures.py
Requires OpenJPEG 2.5.0 `opj_compress` and `opj_decompress` on PATH.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

from oracle_tools import require_openjpeg

ROOT = Path(__file__).resolve().parent.parent / "tests" / "fixtures"
DIR = ROOT / "partial"

SOT = b"\xff\x90"


def lcg(seed: int):
    state = seed
    while True:
        state = (state * 1103515245 + 12345) & 0x7FFFFFFF
        yield state >> 16


def source_pgm() -> bytes:
    """64 x 64, 8-bit: a gradient plus noise."""
    noise = lcg(214)
    px = bytearray()
    for y in range(64):
        for x in range(64):
            v = (x * 3 + y * 2) + next(noise) % 9 - 4
            px.append(max(0, min(255, v)))
    return b"P5\n64 64\n255\n" + bytes(px)


def tp_r_raw() -> bytes:
    """2 x 99, 1-bit, one byte per sample."""
    noise = lcg(167)
    return bytes(next(noise) & 1 for _ in range(2 * 99))


def run(*argv: str) -> subprocess.CompletedProcess:
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{argv[0]} failed:\n{result.stdout}{result.stderr}")
    return result


def pgm_samples(data: bytes) -> bytes:
    """The sample bytes of a P5 PGM, after its header and any comments."""
    fields, at = [], 0
    while len(fields) < 4:
        while data[at : at + 1].isspace():
            at += 1
        if data[at : at + 1] == b"#":
            at = data.index(b"\n", at)
            continue
        end = at
        while not data[end : end + 1].isspace():
            end += 1
        fields.append(data[at:end])
        at = end
    return data[at + 1 :]


def decode(path: Path, out: Path, reduction: int = 0, partial: bool = True) -> bytes | None:
    """`opj_decompress` of `path` to the PGM `out`; its samples, or None if it fails."""
    out.unlink(missing_ok=True)
    argv = ["opj_decompress", "-i", str(path), "-o", str(out), "-r", str(reduction)]
    if partial:
        argv.insert(1, "-allow-partial")
    subprocess.run(argv, capture_output=True)
    return pgm_samples(out.read_bytes()) if out.exists() else None


def shortest_prefix(whole: bytes, reduction: int, want: bytes, tmp: Path) -> int:
    """The shortest prefix of `whole` whose partial decode at `reduction` is `want`."""
    cut, out = tmp / "cut.j2k", tmp / "cut.pgm"

    def same(length: int) -> bool:
        cut.write_bytes(whole[:length])
        return decode(cut, out, reduction) == want

    low, high = 1, len(whole)
    while low < high:
        mid = (low + high) // 2
        if same(mid):
            high = mid
        else:
            low = mid + 1
    return low


def main() -> None:
    require_openjpeg("opj_compress", "opj_decompress")
    DIR.mkdir(parents=True, exist_ok=True)
    (DIR / "source.pgm").write_bytes(source_pgm())
    (DIR / "tp_r_source.raw").write_bytes(tp_r_raw())
    src = str(DIR / "source.pgm")
    run("opj_compress", "-i", src, "-o", str(DIR / "lrcp.j2k"), "-n", "6", "-p", "LRCP")
    run("opj_compress", "-i", src, "-o", str(DIR / "tiles.j2k"), "-t", "32,32", "-n", "3")
    run("opj_compress", "-i", src, "-o", str(DIR / "overlap.j2k"), "-n", "1",
        "-POC", "T1=0,0,1,1,1,LRCP/T1=0,0,1,1,1,PCRL")
    run("opj_compress", "-i", str(DIR / "tp_r_source.raw"), "-o", str(DIR / "tp_r.j2k"),
        "-F", "2,99,1,1,u", "-n", "6", "-t", "32,64", "-d", "22,14", "-TP", "R")

    with tempfile.TemporaryDirectory() as tmp_name:
        tmp = Path(tmp_name)
        whole = (DIR / "lrcp.j2k").read_bytes()
        lines = []
        for r in range(6):
            want = decode(DIR / "lrcp.j2k", DIR / f"lrcp_r{r}.pgm", r, partial=False)
            assert want is not None
            length = shortest_prefix(whole, r, want, tmp)
            (tmp / "short.j2k").write_bytes(whole[: length - 1])
            short = decode(tmp / "short.j2k", DIR / f"lrcp_r{r}_short.pgm", r)
            if short is None or short == want:
                sys.exit(f"lrcp r{r}: the prefix of {length - 1} bytes gives the whole image")
            lines.append(f"{r} {length}")
            print(f"lrcp.j2k r{r}: shortest whole prefix {length} of {len(whole)} bytes")
        (DIR / "lrcp.prefixes").write_text("\n".join(lines) + "\n")

        tiles = (DIR / "tiles.j2k").read_bytes()
        first = tiles.index(SOT)
        second = first + int.from_bytes(tiles[first + 6 : first + 10], "big")
        assert tiles[second : second + 2] == SOT and tiles[second + 5] == 1
        cut = second + int.from_bytes(tiles[second + 6 : second + 10], "big") // 2
        (DIR / "tiles.cut").write_text(f"{cut}\n")
        (tmp / "tiles_cut.j2k").write_bytes(tiles[:cut])
        if decode(tmp / "tiles_cut.j2k", DIR / "tiles_cut.pgm") is None:
            sys.exit("tiles.j2k: opj_decompress -allow-partial refuses the cut")
        print(f"tiles.j2k: cut at {cut} of {len(tiles)} bytes")

        overlap = decode(DIR / "overlap.j2k", tmp / "overlap.pgm", partial=False)
        if overlap != pgm_samples(source_pgm()):
            sys.exit("overlap.j2k: opj_decompress does not give the source")
        print(f"overlap.j2k: {(DIR / 'overlap.j2k').stat().st_size} B, decodes to the source")

        tp_r = decode(DIR / "tp_r.j2k", DIR / "tp_r.pgm", partial=False)
        assert tp_r is not None
        wrong = sum(a != b for a, b in zip(tp_r, tp_r_raw()))
        if wrong == 0:
            sys.exit("tp_r.j2k: opj_decompress gives the source")
        print(f"tp_r.j2k: opj_decompress differs from the source in {wrong} of 198 samples")

        for name in ["single", "tileparts"]:
            path = ROOT / "psot0" / f"{name}_psot0_no_eoc.j2k"
            if decode(path, tmp / "strict.pgm", partial=False) is not None:
                sys.exit(f"{path.name}: opj_decompress decodes it without -allow-partial")
            if decode(path, DIR / f"psot0_{name}_no_eoc.pgm") is None:
                sys.exit(f"{path.name}: opj_decompress -allow-partial refuses it")
            print(f"{path.name}: decoded with -allow-partial")


if __name__ == "__main__":
    main()
