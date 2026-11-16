#!/usr/bin/env python3
"""Checks and times the decoders compared in docs/performance.md.

    run.py check [--only SUBSTR]            check every decoder's output
    run.py time  [--only SUBSTR] [--runs N] time the decoders, interleaved
    run.py incremental [--runs N]           the incremental-decoder table

Tool locations come from the environment (defaults in parentheses):
    PERF_DECODE  the tools/perf binary (target/release/perf-decode)
    OPJ          opj_decompress (opj_decompress on PATH)
    GROK         grk_decompress (grk_decompress on PATH)
    OJPH         ojph_expand (ojph_expand on PATH)
    OPJ_COMPRESS, OJPH_COMPRESS  to make the 8x8 file for the empty run
    WORK         scratch directory for outputs (a fresh directory in /dev/shm)

Needs python3 with numpy and /usr/bin/time (GNU).
"""

import argparse
import atexit
import glob
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

import numpy as np

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
FIX = os.path.join(ROOT, "tests", "fixtures")


def fx(rel):
    return os.path.join(FIX, rel)


TILED = fx("bench/tiled_2048_gray.j2k")
HT = "conformance/ht/codestreams/"

# name, path, kind ("p1" or "ht"), extra options for the decoders that take
# a region or tile: (window "x0,y0,x1,y1" or None, tile or None)
GENERATED = "ht_gen_2048_gray"
FILES = [
    ("p0_07", fx("conformance/codestreams/p0_07.j2k"), "p1", None, None),
    ("p0_08", fx("conformance/codestreams/p0_08.j2k"), "p1", None, None),
    ("p0_05", fx("conformance/codestreams/p0_05.j2k"), "p1", None, None),
    ("p1_03", fx("conformance/codestreams/p1_03.j2k"), "p1", None, None),
    ("file7.jp2", fx("conformance/jp2/files/file7.jp2"), "p1", None, None),
    ("grib2_shape", fx("bench/grib2_shape_1440x721_u16.j2k"), "p1", None, None),
    ("regular_latlon", fx("jpeg2000_regular_latlon.j2k"), "p1", None, None),
    ("tiled_2048 whole", TILED, "p1", None, None),
    ("tiled_2048 256x256 window", TILED, "p1", "896,896,1152,1152", None),
    ("tiled_2048 64x64 window", TILED, "p1", "800,800,864,864", None),
    ("tiled_2048 tile 27", TILED, "p1", None, 27),
    ("ds0_ht_01_b11", fx(HT + "ds0_ht_01_b11.j2k"), "ht", None, None),
    ("hifi_ht1_02", fx(HT + "hifi_ht1_02.j2k"), "ht", None, None),
    ("ds1_ht_05_b11", fx(HT + "ds1_ht_05_b11.j2k"), "ht", None, None),
    ("ds1_ht_04_b9", fx(HT + "ds1_ht_04_b9.j2k"), "ht", None, None),
    ("ds0_hm_15_b8", fx(HT + "ds0_hm_15_b8.j2k"), "ht", None, None),
    ("ds0_hm_06_b11", fx(HT + "ds0_hm_06_b11.j2k"), "ht", None, None),
    ("ds1_ht_02_b11", fx(HT + "ds1_ht_02_b11.j2k"), "ht", None, None),
    ("ds0_ht_05_b11", fx(HT + "ds0_ht_05_b11.j2k"), "ht", None, None),
    ("ds0_ht_07_b16", fx(HT + "ds0_ht_07_b16.j2k"), "ht", None, None),
    # Made at run time by ojph_compress; see generate().
    (GENERATED, "GENERATED", "ht", None, None),
]

# Files for the incremental table, and the piece size.
INCREMENTAL = ["grib2_shape", "ds0_ht_05_b11"]
PIECE = 65536


# Logical CPUs: what opj_decompress's ALL_CPUS means, and Grok's -H here.
CORES = str(os.cpu_count())


def governor():
    try:
        with open("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor") as fh:
            return fh.read().strip()
    except OSError:
        return "unknown"


def env(name, default):
    return os.environ.get(name, default)


PERF = env("PERF_DECODE", os.path.join(ROOT, "tools/perf/target/release/perf-decode"))
OPJ = env("OPJ", "opj_decompress")
GROK = env("GROK", "grk_decompress")
OJPH = env("OJPH", "ojph_expand")
OPJ_COMPRESS = env("OPJ_COMPRESS", "opj_compress")
OJPH_COMPRESS = env("OJPH_COMPRESS", "ojph_compress")


# ---------------------------------------------------------------------------
# Decoders. Each variant is (label, supports(file), argv(file, out)).
# `out` is an output prefix; the variant writes PGX files PREFIX_K.pgx or
# PREFIX.pgx (the decoders differ), or for ojph a raw planar file PREFIX.yuv.
# ---------------------------------------------------------------------------


def region(f):
    args = []
    if f[3]:
        args += ["-d", f[3]]
    if f[4] is not None:
        args += ["-t", str(f[4])]
    return args


def opj(threads):
    def argv(f, out):
        a = [OPJ, "-quiet", "-i", f[1], "-o", out + ".pgx"] + region(f)
        if threads:
            a += ["-threads", threads]
        return a

    return argv


def grok(threads):
    def argv(f, out):
        a = [GROK, "-i", f[1], "-o", out + ".pgx"] + region(f)
        a += ["-H", threads]
        return a

    return argv


def components(path):
    """Csiz from the SIZ marker of a codestream, or of a JP2 file's."""
    with open(path, "rb") as fh:
        data = fh.read(4096)
    i = data.find(b"\xff\x4f\xff\x51")
    return int.from_bytes(data[i + 40 : i + 42], "big")


def ojph_ext(f):
    # ojph_expand writes PGM (1 component), PPM (3, not subsampled), or raw
    # planar (anything else); it refuses raw output for a color-transformed
    # 3-component file.
    return {1: ".pgm", 3: ".ppm"}.get(components(f[1]), ".yuv")


def ojph(f, out):
    return [OJPH, "-i", f[1], "-o", out + ojph_ext(f)]


def srgb(f, engine):
    # opj_decompress and Grok convert a JP2 file's colors to sRGB.
    return ["--srgb"] if engine == "rust-j2k" and f[1].endswith(".jp2") else []


def perf(engine, extra=()):
    def argv(f, out):
        a = [PERF, engine, f[1], "--out", out] + list(extra) + srgb(f, engine)
        if f[3]:
            a += ["--window", f[3]]
        if f[4] is not None:
            a += ["--tile", str(f[4])]
        return a

    return argv


def perf_timed(engine):
    """The command-line run to time: hayro writes its 8-bit bytes."""

    def argv(f, out):
        a = [PERF, engine, f[1]]
        a += ["--out8", out + ".raw"] if engine == "hayro" else ["--out", out]
        a += srgb(f, engine)
        if f[3]:
            a += ["--window", f[3]]
        if f[4] is not None:
            a += ["--tile", str(f[4])]
        return a

    return argv


def is_region(f):
    return bool(f[3]) or f[4] is not None


def any_file(f):
    return True


def p1_only(f):
    return f[2] == "p1"


def ht_only(f):
    return f[2] == "ht"


def no_region(f):
    return not is_region(f)


def p1_no_region(f):
    return f[2] == "p1" and not is_region(f)


def ht_no_region(f):
    return f[2] == "ht" and not is_region(f)


# (label, supports, argv for checking, argv for timing, threads text)
VARIANTS = [
    ("opj 1 thread", any_file, opj(None), opj(None), "1"),
    ("opj all cores", any_file, opj("ALL_CPUS"), opj("ALL_CPUS"), CORES),
    ("grok 1 thread", any_file, grok("1"), grok("1"), "1"),
    ("grok all cores", any_file, grok(CORES), grok(CORES), CORES),
    ("openjph", ht_no_region, ojph, ojph, "1"),
    ("rust-j2k cli", any_file, perf("rust-j2k"), perf_timed("rust-j2k"), "1"),
    ("hayro cli", p1_no_region, perf("hayro"), perf_timed("hayro"), "1"),
]
ORACLE = VARIANTS[0]


# ---------------------------------------------------------------------------
# Output reading
# ---------------------------------------------------------------------------


def read_pgx(path):
    with open(path, "rb") as fh:
        head_line = fh.readline()
        data = fh.read()
    # PG ML|LM [+|-]depth width height; the sign is optional (unsigned).
    m = re.match(rb"PG\s+(ML|LM)\s*([+-]?)\s*(\d+)\s+(\d+)\s+(\d+)", head_line)
    endian = ">" if m.group(1) == b"ML" else "<"
    signed = m.group(2) == b"-"
    depth, w, h = int(m.group(3)), int(m.group(4)), int(m.group(5))
    if depth <= 8:
        dt = np.dtype("i1" if signed else "u1")
    else:
        dt = np.dtype(endian + ("i2" if signed else "u2"))
    arr = np.frombuffer(data, dtype=dt).astype(np.int32)
    return depth, w, h, arr, signed


def pgx_planes(out):
    single = out + ".pgx"
    if os.path.exists(single):
        return [read_pgx(single)]
    files = glob.glob(out + "_*.pgx")
    files.sort(key=lambda p: int(re.search(r"_(\d+)\.pgx$", p).group(1)))
    return [read_pgx(p) for p in files]


def pnm_planes(path, oracle):
    """PGM or PPM from ojph_expand, split into components."""
    with open(path, "rb") as fh:
        data = fh.read()
    tok, pos = [], 0
    while len(tok) < 4:  # magic, width, height, maxval
        while data[pos : pos + 1].isspace():
            pos += 1
        end = pos
        while not data[end : end + 1].isspace():
            end += 1
        tok.append(data[pos:end])
        pos = end
    pos += 1
    n = 3 if tok[0] == b"P6" else 1
    w, h, maxval = int(tok[1]), int(tok[2]), int(tok[3])
    dt = ">u2" if maxval > 255 else "u1"
    arr = np.frombuffer(data, dtype=dt, offset=pos).astype(np.int32)
    depth = maxval.bit_length()
    whole = arr.size == w * h * n
    return [(depth, w, h, arr[k::n], False) for k in range(n)], whole


def yuv_planes(out, oracle):
    """Planar raw from ojph_expand, read at the oracle's sizes and depths."""
    with open(out + ".yuv", "rb") as fh:
        data = fh.read()
    planes, pos = [], 0
    for depth, w, h, _, _ in oracle:
        n = w * h
        if depth <= 8:
            arr = np.frombuffer(data, dtype="u1", count=n, offset=pos).astype(np.int32)
            pos += n
        else:
            arr = np.frombuffer(data, dtype="<u2", count=n, offset=pos).astype(np.int32)
            pos += 2 * n
        planes.append((depth, w, h, arr, False))
    return planes, pos == len(data)


def compare(got, want, bound=None, unsigned_out=False):
    """'exact', 'PAE n', or a description of why the shapes differ.

    `bound` is the largest PAE the file's reference allows, per component, or
    None. `unsigned_out` is a decoder that writes every component unsigned
    (hayro-jpeg2000): a signed oracle component is offset to match.
    """
    if len(got) != len(want):
        return f"{len(got)} components, oracle {len(want)}"
    worst, over, skipped = 0, False, 0
    for k, (g, w) in enumerate(zip(got, want)):
        if (g[1], g[2]) != (w[1], w[2]):
            skipped += 1  # sub-sampled component returned upsampled
            continue
        if g[0] != w[0]:
            return f"comp {k} depth {g[0]}, oracle {w[0]}"
        ref = w[3]
        if unsigned_out and w[4]:
            ref = ref + (1 << (w[0] - 1))
        pae = int(np.abs(g[3] - ref).max(initial=0))
        worst = max(worst, pae)
        if bound is not None and pae > bound[k]:
            over = True
    text = "exact" if worst == 0 else f"PAE {worst}"
    if bound is not None and worst:
        text += " OVER BOUND" if over else " in bound"
    if skipped:
        text += f" ({skipped} upsampled comps not compared)"
    return text


def run_quiet(argv):
    r = subprocess.run(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    return r.returncode, (r.stderr or r.stdout).strip().splitlines()


def work_dir():
    base = env("WORK", "")
    if base:
        os.makedirs(base, exist_ok=True)
        return base
    path = tempfile.mkdtemp(prefix="j2kperf-", dir="/dev/shm")
    atexit.register(shutil.rmtree, path, ignore_errors=True)
    return path


def clean(work, out):
    for p in glob.glob(out + "*"):
        os.remove(p)


# ---------------------------------------------------------------------------
# check
# ---------------------------------------------------------------------------


GEN_DIR = None


def generate():
    """The generated HT file: the tiled 2048 x 2048 gray image, decoded by
    opj_decompress to PGM and compressed losslessly, one layer, by
    ojph_compress. Not a conformance file, and made fresh each run."""
    global GEN_DIR
    if GEN_DIR is None:
        GEN_DIR = tempfile.mkdtemp(prefix="j2kgen-", dir="/dev/shm")
        atexit.register(shutil.rmtree, GEN_DIR, ignore_errors=True)
        pgm = os.path.join(GEN_DIR, "gray.pgm")
        subprocess.run(
            [OPJ, "-quiet", "-i", TILED, "-o", pgm], check=True, stdout=subprocess.DEVNULL
        )
        subprocess.run(
            [OJPH_COMPRESS, "-i", pgm, "-o", os.path.join(GEN_DIR, GENERATED + ".j2c"),
             "-reversible", "true", "-num_decomps", "5"],
            check=True,
            stdout=subprocess.DEVNULL,
        )
    return os.path.join(GEN_DIR, GENERATED + ".j2c")


def chosen(only):
    out = []
    for f in FILES:
        if only and only not in f[0]:
            continue
        if f[1] == "GENERATED":
            f = (f[0], generate()) + f[2:]
        out.append(f)
    return out


_HT_MANIFEST = None


def ht_reference(f):
    """The ISO reference planes and per-component PAE bounds for an HT
    conformance file, or None."""
    global _HT_MANIFEST
    import json

    base = fx("conformance/ht")
    if _HT_MANIFEST is None:
        with open(os.path.join(base, "manifest.json")) as fh:
            _HT_MANIFEST = json.load(fh)
    for c in _HT_MANIFEST["codestreams"]:
        if os.path.basename(c["codestream"]) == os.path.basename(f[1]):
            planes = [read_pgx(os.path.normpath(os.path.join(base, r))) for r in c["references"]]
            return planes, c["bounds"]["pae"]
    return None


def check_file(f, work):
    """Run every decoder that takes `f` and compare with the oracle.

    Returns (oracle label, [(variant, result text)]).
    """
    out = os.path.join(work, "o")
    clean(work, out)
    rc, msg = run_quiet(ORACLE[2](f, out + "_oracle"))
    bound, label = None, "opj_decompress 2.5.0"
    if rc == 0:
        want = pgx_planes(out + "_oracle")
    else:
        ref = ht_reference(f) if f[2] == "ht" else None
        if ref is None:
            return f"none: opj failed ({msg[:1]})", []
        want, bound = ref
        label = "ISO reference (opj fails)"
        if all(b == 0 for b in bound):
            bound = None
    cells = []
    for v in VARIANTS:
        if not v[1](f):
            cells.append((v, "n/a"))
            continue
        if v[0].startswith("opj") and rc != 0:
            cells.append((v, "cannot decode"))
            continue
        o = out + "_" + re.sub(r"\W", "", v[0])
        code, msg = run_quiet(v[2](f, o))
        if code != 0:
            cells.append((v, "FAILED: " + (msg[0][:60] if msg else str(code))))
            continue
        try:
            if v[0] == "openjph":
                ext = ojph_ext(f)
                if ext == ".yuv":
                    got, whole = yuv_planes(o, want)
                else:
                    got, whole = pnm_planes(o + ext, want)
                res = compare(got, want, bound)
                if not whole:
                    res += " (output size differs)"
            else:
                res = compare(pgx_planes(o), want, bound, unsigned_out=v[0].startswith("hayro"))
        except Exception as e:  # unreadable output is a finding too
            res = f"unreadable output ({type(e).__name__}: {e})"
        cells.append((v, res))
    clean(work, out)
    return label, cells


# Lossless files: the output must match exactly.
LOSSLESS = {
    "p0_07", "p0_08", "file7.jp2", "grib2_shape", "regular_latlon", "ds0_ht_01_b11",
    "ds0_hm_15_b8", "ds0_ht_07_b16", GENERATED,
}


def is_lossless(f):
    return f[0] in LOSSLESS or f[0].startswith("tiled_2048")


def output_ok(text, f):
    """Whether a check result is a right image: exact; for a lossy file also
    inside the file's reference bound, or within 5 of OpenJPEG (the loosest
    bound the project uses for a lossy file, at 16 bits)."""
    if text == "exact" or text.startswith("exact "):
        return True
    if is_lossless(f):
        return False
    if " in bound" in text:
        return True
    m = re.match(r"PAE (\d+)\b", text)
    return bool(m) and "OVER BOUND" not in text and int(m.group(1)) <= 5


def cmd_check(args):
    work = work_dir()
    print("| file | oracle | " + " | ".join(v[0] for v in VARIANTS) + " |")
    for f in chosen(args.only):
        label, cells = check_file(f, work)
        print(f"| {f[0]} | {label} | " + " | ".join(t for _, t in cells) + " |")
    shutil.rmtree(work, ignore_errors=True)


# ---------------------------------------------------------------------------
# time
# ---------------------------------------------------------------------------


def timed(argv, work):
    """Wall milliseconds and peak resident KiB of one run."""
    rss = os.path.join(work, "rss")
    t = time.perf_counter()
    r = subprocess.run(
        ["/usr/bin/time", "-f", "%M", "-o", rss] + argv,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    ms = (time.perf_counter() - t) * 1e3
    if r.returncode != 0:
        return None
    with open(rss) as fh:
        return ms, int(fh.read().split()[-1])


def make_empty(work):
    """An 8x8 gray image, as a Part 1 file and as an HT file."""
    pgm = os.path.join(work, "empty.pgm")
    with open(pgm, "wb") as fh:
        fh.write(b"P5\n8 8\n255\n" + bytes(range(64)))
    p1 = os.path.join(work, "empty_p1.j2k")
    ht = os.path.join(work, "empty_ht.j2c")
    subprocess.run([OPJ_COMPRESS, "-n", "1", "-i", pgm, "-o", p1], check=True, stdout=subprocess.DEVNULL)
    subprocess.run([OJPH_COMPRESS, "-i", pgm, "-o", ht], check=True, stdout=subprocess.DEVNULL)
    return p1, ht


def empty_file(f, p1, ht):
    return ("empty", p1, "p1", None, None)


def cmd_time(args):
    work = work_dir()
    p1, ht = make_empty(work)
    print(f"runs per cell: {args.runs} (median); times in ms, RSS in MiB")
    print(f"cpu governor: {governor()}; logical CPUs: {CORES}; load: {os.getloadavg()}")
    print("| file | decoder | threads | median ms | min ms | empty ms | net ms | peak MiB |")
    for f in chosen(args.only):
        label, checked = check_file(f, work)
        rows = []
        for v, text in checked:
            if text == "n/a" or text == "cannot decode":
                continue
            if output_ok(text, f):
                rows.append(v)
            else:
                print(f"| {f[0]} | {v[0]} | {v[4]} | not timed: output check says {text[:50]} |")
        # Interleave: one run of every variant per round; the empty run of
        # each variant goes in the same round.
        samples = {v[0]: [] for v in rows}
        empties = {v[0]: [] for v in rows}
        rss = {v[0]: [] for v in rows}
        bad = {}
        for _ in range(args.runs):
            for v in rows:
                if v[0] in bad:
                    continue
                out = os.path.join(work, "t")
                r = timed(v[3](f, out), work)
                clean(work, out)
                if r is None:
                    bad[v[0]] = "failed"
                    continue
                samples[v[0]].append(r[0])
                rss[v[0]].append(r[1])
                ef = ("empty", ht if v[0] == "openjph" else p1, "p1", None, None)
                e = timed(v[3](ef, out), work)
                clean(work, out)
                if e is not None:
                    empties[v[0]].append(e[0])
        for v in rows:
            if v[0] in bad:
                print(f"| {f[0]} | {v[0]} | {v[4]} | {bad[v[0]]} |")
                continue
            med = statistics.median(samples[v[0]])
            emp = statistics.median(empties[v[0]])
            print(
                f"| {f[0]} | {v[0]} | {v[4]} | {med:.1f} | {min(samples[v[0]]):.1f} "
                f"| {emp:.1f} | {med - emp:.1f} | {statistics.median(rss[v[0]]) / 1024:.1f} |"
            )
        # In-process rows, interleaved the same way: one warm-up plus one
        # timed decode per call.
        for engine in ("rust-j2k", "hayro"):
            if not any(v[0].startswith(engine.split("-")[0]) for v in rows):
                continue  # not supported, or its output check failed
            times = []
            for _ in range(args.runs):
                a = [PERF, engine, f[1], "--iters", "1"] + srgb(f, engine)
                if f[3]:
                    a += ["--window", f[3]]
                if f[4] is not None:
                    a += ["--tile", str(f[4])]
                r = subprocess.run(a, capture_output=True, text=True)
                if r.returncode != 0:
                    times = None
                    break
                times.append(float(r.stdout.split()[0]))
            if times:
                print(
                    f"| {f[0]} | {engine} in-process | 1 | {statistics.median(times):.1f} "
                    f"| {min(times):.1f} | - | - | - |"
                )
    shutil.rmtree(work, ignore_errors=True)


def cmd_incremental(args):
    work = work_dir()
    print(f"pieces of {PIECE} bytes; medians of {args.runs} in-process runs, ms")
    print("| file | output check | one decode | 64 KB pieces | ratio |")
    for name in INCREMENTAL:
        f = next(x for x in chosen(name) if x[0] == name)
        # The output check: finish() after the pieces must equal one decode.
        one_out, inc_out = os.path.join(work, "one"), os.path.join(work, "inc")
        base = [PERF, "rust-j2k", f[1]] + srgb(f, "rust-j2k")
        for argv in (base + ["--out", one_out], base + ["--out", inc_out, "--incremental", str(PIECE)]):
            subprocess.run(argv, check=True)
        names = sorted(os.path.basename(x) for x in glob.glob(one_out + "_*.pgx"))
        same = bool(names)
        for n in names:
            with open(os.path.join(work, n), "rb") as a, open(
                os.path.join(work, n.replace("one", "inc", 1)), "rb"
            ) as b:
                same = same and a.read() == b.read()
        if not same:
            print(f"| {name} | DIFFERS | | | |")
            continue
        one, inc = [], []
        for _ in range(args.runs):
            for lst, extra in ((one, []), (inc, ["--incremental", str(PIECE)])):
                r = subprocess.run(
                    [PERF, "rust-j2k", f[1], "--iters", "1"] + srgb(f, "rust-j2k") + extra,
                    capture_output=True,
                    text=True,
                )
                lst.append(float(r.stdout.split()[0]))
        a, b = statistics.median(one), statistics.median(inc)
        print(f"| {name} | identical | {a:.1f} | {b:.1f} | {b / a:.2f} |")


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    for name, fn in (("check", cmd_check), ("time", cmd_time), ("incremental", cmd_incremental)):
        s = sub.add_parser(name)
        s.add_argument("--only", default="")
        s.add_argument("--runs", type=int, default=7)
        s.set_defaults(fn=fn)
    args = p.parse_args()
    args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
