#!/usr/bin/env python3
"""Regenerate the ISO/IEC 15444-4 Part 1 conformance corpus.

Downloads the Profile-0 (`p0_*`) and Profile-1 (`p1_*`) conformance codestreams
and their `.pgx` reference decodes from `uclouvain/openjpeg-data` at a pinned
commit, derives each codestream's parameters from `opj_dump` (the OpenJPEG
oracle), and writes `manifest.json`.

This is a development tool, not part of the crate: it needs network access and
`opj_dump` on PATH, neither of which the runtime test suite requires. The
committed corpus is self-contained; rerun this only to refresh or re-pin it.

    python3 regenerate.py          # the Part 1 and JP2 corpora
    python3 regenerate.py part1    # the Part 1 codestreams only
    python3 regenerate.py jp2      # the JP2 reader files only (see jp2/)
    python3 regenerate.py ht       # the HTJ2K codestreams and JPH files (ht/)
    python3 regenerate.py openjph  # the OpenJPH test codestreams (../openjph/)

The JP2 half also needs `Pillow` (and numpy) to read the TIFF references; see
README.md. The committed corpus never needs them.

The HT set is pinned to a commit of `gitlab.com/wg1/htj2k-codestreams`. It
downloads the files it needs, or copies them from a local checkout at that
commit named by `HTJ2K_CODESTREAMS`. Its bounds come from T.803 Annex C (the
tables are transcribed below, `T803_*`), and it needs numpy, LittleCMS and
`opj_decompress` 2.5.0 for the OpenJPEG cross-check and snapshots. The OpenJPH
set is pinned to a commit of `aous72/jp2k_test_codestreams`; its references are
`opj_decompress` output.

The Part 1 per-file peak-absolute-error (PAE) and mean-squared-error (MSE) bounds are
transcribed verbatim from OpenJPEG's `tests/conformance/CMakeLists.txt`. The
class-1 (per-component) lists are the active grading tier upstream; the class-0
lists are commented out there (they grade the first component only) and are
recorded for reference, not used as the bar. See README.md and ./COPYRIGHT.
"""

from __future__ import annotations

import hashlib
import math
import os
import re
import shutil
import subprocess
import json
import sys
import tempfile

# oracle_tools.py lives in scripts/, three levels up from this directory; put
# that directory on sys.path so the version check can be imported.
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "scripts"))
from oracle_tools import require_lcms, require_openjpeg  # noqa: E402

# openjpeg-data commit the corpus is pinned to (master @ 2026-06).
DATA_COMMIT = "39524bd3a601d90ed8e0177559400d23945f96a9"
RAW = "https://raw.githubusercontent.com/uclouvain/openjpeg-data/" + DATA_COMMIT + "/%s"

HERE = os.path.dirname(os.path.abspath(__file__))
CS_DIR = os.path.join(HERE, "codestreams")
REF_DIR = os.path.join(HERE, "references")

# Per-file PAE/MSE bounds, transcribed verbatim from
# uclouvain/openjpeg tests/conformance/CMakeLists.txt. Semicolon-separated,
# 1-indexed (entry 0 is the unused "not_used" placeholder); each cell is
# colon-separated per component.
BOUNDS = {
    # Class 1 (per component) — the ACTIVE grading tier upstream.
    (1, 0, "pae"): "not_used;0;0;0;5:4:6;2:2:2:0;635:403:378:0;0:0:0;0:0:0;0;0:0:0;0;0;0:0:0:0;0:0:0;0;0",
    (1, 0, "mse"): "not_used;0;0;0;0.776:0.626:1.070;0.302:0.307:0.269:0;11287:6124:3968:0;0:0:0;0:0:0;0;0:0:0;0;0;0:0:0:0;0:0:0;0;0",
    (1, 1, "pae"): "not_used;0;5:4:6;2:2:1:0;624;40:40:40;2:2:2;0:0",
    (1, 1, "mse"): "not_used;0;0.765:0.616:1.051;0.3:0.210:0.200:0;3080;8.458:9.816:10.154;0.6:0.6:0.6;0:0",
    # Class 0 (first component only) — commented out upstream; recorded for
    # reference, not the active bar.
    (0, 0, "pae"): "not_used;0;0;0;33;54;109;10;7;4;10;0;0;0;0;0;0",
    (0, 0, "mse"): "not_used;0;0;0;55.8;68;743;0.34;6.72;1.47;2.84;0;0;0;0;0;0",
    (0, 1, "pae"): "not_used;0;35;28;2;128;128;0",
    (0, 1, "mse"): "not_used;0;74;18.8;0.550;16384;16384;0",
}

# Where OpenJPEG's list is looser than ITU-T T.803 Table C.7, the grade uses
# T.803's value: (profile, index, kind) -> {component: bound}. Only p1_05
# component 1 (T.803 9.716, OpenJPEG 9.816) is looser. p0_05 and p1_03 differ
# the other way (OpenJPEG is tighter), so they keep OpenJPEG's.
T803_TIGHTER = {
    (1, 5, "mse"): {1: 9.716},
}

# Class-0 references carry irregular resolution-level suffixes (rN) for the
# reduced-resolution decode tests; everything else is the plain c0p{P}_{NN}.pgx.
CLASS0_REF_SUFFIXES = {
    (0, 3): ["r0", "r1"],
    (0, 15): ["r0", "r1"],
    (1, 4): ["r0", "r3"],
}

# The resolution reduction each entry's class-1 references were decoded at:
# `C1P0_ResFactor_list` in uclouvain/openjpeg tests/conformance/CMakeLists.txt
# (semicolon-separated, 1-indexed like BOUNDS), passed as `-r` to
# opj_decompress. Only p0_08 reduces; the profile-1 class-1 tests hardcode
# `-r 0` and carry no list.
RES_FACTOR = {
    (0, idx): factor
    for idx, factor in enumerate(
        [None, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]
    )
}

PROGRESSION = {0: "LRCP", 1: "RLCP", 2: "RPCL", 3: "PCRL", 4: "CPRL"}

# Optional-marker names by code. Anything not listed here (e.g. the reserved
# segment-less 0xFF30-0xFF3F range that p0_02 carries) is recorded as hex.
MARKER_NAMES = {
    0xFF50: "CAP",
    0xFF51: "SIZ",
    0xFF52: "COD",
    0xFF53: "COC",
    0xFF55: "TLM",
    0xFF57: "PLM",
    0xFF58: "PLT",
    0xFF59: "CPF",
    0xFF5C: "QCD",
    0xFF5D: "QCC",
    0xFF5E: "RGN",
    0xFF5F: "POC",
    0xFF60: "PPM",
    0xFF61: "PPT",
    0xFF63: "CRG",
    0xFF64: "COM",
}

# Code-block style bits (SPcod/SPcoc, ISO/IEC 15444-1 Table A.19).
CBLKSTY_BITS = [
    ("bypass", 0x01),
    ("reset", 0x02),
    ("restart", 0x04),
    ("vert_causal", 0x08),
    ("pred_term", 0x10),
    ("segsym", 0x20),
]


def marker_features(path: str) -> dict:
    """Derive marker presence and coding-style flags from the bytes directly.

    `opj_dump` reports the resolved coding parameters but not which optional
    markers a codestream carries (and it silently skips reserved segment-less
    markers), so the per-feature fixture mapping walks the marker segments
    itself: main header and every tile-part header, using segment lengths and
    Psot to step over entropy data.
    """
    with open(path, "rb") as f:
        data = f.read()
    return marker_features_data(data, path)


def marker_features_data(data: bytes, path: str) -> dict:
    """`marker_features` over codestream bytes already in memory."""
    if data[:2] != b"\xff\x4f":
        raise ValueError(f"{path}: missing SOC marker")

    main: set[str] = set()
    tile: set[str] = set()
    sop = eph = precincts = False
    cblksty = 0
    csiz = 0
    tile_parts = 0
    in_main = True
    tile_end = 0
    pos = 2
    while pos < len(data):
        marker = int.from_bytes(data[pos : pos + 2], "big")
        pos += 2
        if marker == 0xFFD9:  # EOC
            break
        if 0xFF30 <= marker <= 0xFF3F:  # reserved, segment-less
            (main if in_main else tile).add(f"0x{marker:04X}")
            continue
        if marker == 0xFF90:  # SOT
            tile_parts += 1
            in_main = False
            lsot = int.from_bytes(data[pos : pos + 2], "big")
            psot = int.from_bytes(data[pos + 4 : pos + 8], "big")
            if psot == 0:
                # Legal ("extends to EOC/next SOT") but absent from this
                # corpus; scanning entropy data for the boundary is not
                # reliable, so fail loudly rather than guess.
                raise ValueError(f"{path}: Psot=0 tile-part is unsupported here")
            tile_end = pos - 2 + psot
            pos += lsot
            continue
        if marker == 0xFF93:  # SOD: step over the entropy data
            pos = tile_end
            continue
        length = int.from_bytes(data[pos : pos + 2], "big")
        seg = data[pos + 2 : pos + length]
        pos += length
        name = MARKER_NAMES.get(marker, f"0x{marker:04X}")
        (main if in_main else tile).add(name)
        if marker == 0xFF52:  # COD: Scod flags + SPcod code-block style
            scod = seg[0]
            precincts |= bool(scod & 0x01)
            sop |= bool(scod & 0x02)
            eph |= bool(scod & 0x04)
            cblksty |= seg[8]
        elif marker == 0xFF53:  # COC: Ccoc is 2 bytes when Csiz >= 257
            cidx = 2 if csiz >= 257 else 1
            precincts |= bool(seg[cidx] & 0x01)
            cblksty |= seg[cidx + 4]
        elif marker == 0xFF51:  # SIZ: Csiz sizes later component indices
            csiz = int.from_bytes(seg[34:36], "big")

    # SIZ/COD/QCD are mandatory in the main header; listing them per entry
    # would be constant noise. In a tile-part header the same markers are
    # per-tile overrides, so there they stay listed (see p1_04's tile QCDs).
    main -= {"SIZ", "COD", "QCD"}
    return {
        "markers_main": sorted(main),
        "markers_tile": sorted(tile),
        "tile_parts": tile_parts,
        "sop": sop,
        "eph": eph,
        "precincts": precincts,
        "cblksty": {name: bool(cblksty & bit) for name, bit in CBLKSTY_BITS},
    }


def codestreams():
    for i in range(1, 17):
        yield (0, i)
    for i in range(1, 8):
        yield (1, i)


def bound_cell(profile: int, idx: int, cls: int, kind: str):
    """Return the per-component bound list for one entry, or None if absent."""
    raw = BOUNDS.get((cls, profile, kind))
    if raw is None:
        return None
    cells = raw.split(";")
    if idx >= len(cells):
        return None
    cell = cells[idx]
    out = []
    for part in cell.split(":"):
        v = float(part)
        out.append(int(v) if v.is_integer() else v)
    if cls == 1:
        for comp, v in T803_TIGHTER.get((profile, idx, kind), {}).items():
            out[comp] = v
    return out


def sha256_file(path: str) -> str:
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def download(rel_path: str, dest: str):
    """Fetch `rel_path` from the pinned commit to `dest`.

    A file already at `dest` is not trusted: the pinned bytes are fetched again
    and compared, and a mismatch (a truncated or edited copy) raises. Delete the
    file to refresh it deliberately.
    """
    fetch(RAW % rel_path, dest, f"{rel_path} at {DATA_COMMIT[:8]}")


def fetch(url: str, dest: str, label: str, local: str | None = None):
    """Fetch `url` (or copy `local`, a file in a checkout at the pinned commit)
    to `dest`, with `download`'s rule: an existing `dest` must match."""
    # Download to a temp path and rename on success, so an interrupted curl
    # never leaves a truncated file that later runs would skip as "present".
    tmp = dest + ".part"
    if local is not None:
        shutil.copyfile(local, tmp)
    else:
        subprocess.run(["curl", "-fsSL", "-o", tmp, url], check=True)
    if os.path.exists(dest):
        same = sha256_file(tmp) == sha256_file(dest)
        os.remove(tmp)
        if not same:
            raise ValueError(f"{dest} differs from {label}")
        return
    os.replace(tmp, dest)


def opj_features(path: str) -> dict:
    out = subprocess.run(
        ["opj_dump", "-i", path], capture_output=True, text=True, check=True
    ).stdout

    def first_int(pat, default=None):
        m = re.search(pat, out)
        return int(m.group(1)) if m else default

    # The image extent is `x1`/`y1`; the negative lookbehind keeps the pattern
    # off the tile-coordinate tokens (`tx1`, `ty1`) that share the suffix.
    width = first_int(r"(?<![a-z])x1=(\d+)")
    height = first_int(r"(?<![a-z])y1=(\d+)")
    numcomps = first_int(r"numcomps=(\d+)")

    comps = []
    for m in re.finditer(
        r"component \d+ \{\s*dx=(\d+), dy=(\d+)\s*prec=(\d+)\s*sgnd=(\d+)", out
    ):
        dx, dy, prec, sgnd = (int(g) for g in m.groups())
        comps.append({"dx": dx, "dy": dy, "prec": prec, "signed": bool(sgnd)})

    # opj_dump prints the progression order as either hex (prg=0x2) or
    # decimal (prg=0) depending on the value; int(_, 0) auto-detects the base.
    prg_m = re.search(r"prg=(0x[0-9a-fA-F]+|\d+)", out)
    prg = int(prg_m.group(1), 0) if prg_m else None
    if prg not in PROGRESSION:
        raise ValueError(f"{path}: unrecognized progression order {prg!r}")

    features = {
        "width": width,
        "height": height,
        "components": numcomps,
        "subsampling": [[c["dx"], c["dy"]] for c in comps],
        "precision": [c["prec"] for c in comps],
        "signed": [c["signed"] for c in comps],
        "tiles": [first_int(r"tw=(\d+)"), first_int(r"th=(\d+)")],
        # Always a string (the order name), never a raw int — the assert above
        # guarantees the lookup hits.
        "progression": PROGRESSION[prg],
        "layers": first_int(r"numlayers=(\d+)"),
        "mct": first_int(r"mct=(\d+)"),
        "resolutions": first_int(r"numresolutions=(\d+)"),
        # cblkw/h are printed as 2^N; we record the exponent N.
        "code_block": [first_int(r"cblkw=2\^(\d+)"), first_int(r"cblkh=2\^(\d+)")],
        # qmfbid: 1 = reversible 5/3 (lossless), 0 = irreversible 9/7 (lossy).
        "reversible": first_int(r"qmfbid=(\d+)") == 1,
    }
    # Fail loudly if any scalar field went unparsed: a silent None here would
    # bake a malformed feature into the manifest if opj_dump's format ever drifts.
    unparsed = [k for k, v in features.items() if v is None]
    if unparsed:
        raise ValueError(f"{path}: opj_dump fields not parsed: {unparsed}")
    return features


def build_entry(profile: int, idx: int) -> dict:
    name = f"p{profile}_{idx:02d}.j2k"
    cs_path = os.path.join(CS_DIR, name)
    download(f"input/conformance/{name}", cs_path)

    pae1 = bound_cell(profile, idx, 1, "pae")
    mse1 = bound_cell(profile, idx, 1, "mse")
    # Components graded at class 1 = bound arity = class-1 reference count. This
    # can be fewer than the image's component count: p0_13 is a 257-component
    # stress image of which only the first 4 are graded.
    graded = len(pae1)

    features = opj_features(cs_path)
    features.update(marker_features(cs_path))
    if features["tile_parts"] < features["tiles"][0] * features["tiles"][1]:
        raise ValueError(
            f"{name}: {features['tile_parts']} tile-parts for "
            f"{features['tiles']} tiles — marker walk went off the rails"
        )
    if len(mse1) != graded:
        raise ValueError(
            f"{name}: class-1 PAE/MSE arity mismatch ({len(pae1)} vs {len(mse1)})"
        )
    if graded > features["components"]:
        raise ValueError(
            f"{name}: grades {graded} components but the image declares "
            f"{features['components']}"
        )

    # class-1 references: one .pgx per graded component, in component order.
    class1_refs = []
    for k in range(graded):
        ref = f"c1p{profile}_{idx:02d}_{k}.pgx"
        download(f"baseline/conformance/{ref}", os.path.join(REF_DIR, ref))
        class1_refs.append(f"references/{ref}")

    # class-0 references: first component only, with optional rN suffixes.
    class0_refs = []
    suffixes = CLASS0_REF_SUFFIXES.get((profile, idx), [""])
    for sfx in suffixes:
        ref = f"c0p{profile}_{idx:02d}{sfx}.pgx"
        download(f"baseline/conformance/{ref}", os.path.join(REF_DIR, ref))
        class0_refs.append(f"references/{ref}")

    pae0 = bound_cell(profile, idx, 0, "pae")
    mse0 = bound_cell(profile, idx, 0, "mse")
    # Graded bit-exactly when every class-1 bound is zero. This is the grading
    # bar, not the wavelet: p0_09 is 9/7 (irreversible) yet bit_exact, so this
    # is deliberately independent of features.reversible.
    bit_exact = all(v == 0 for v in pae1) and all(v == 0 for v in mse1)

    return {
        "codestream": f"codestreams/{name}",
        "profile": profile,
        "index": idx,
        # Components graded at class 1 (= class-1 references = bound arity).
        # May be fewer than features.components (see p0_13).
        "graded_components": graded,
        "bit_exact": bit_exact,
        # The resolution reduction the class-1 references were decoded at;
        # grading must decode at the same reduction to compare geometry.
        "reduction": RES_FACTOR.get((profile, idx), 0),
        "features": features,
        "references": {"class0": class0_refs, "class1": class1_refs},
        # Class 1 is the active per-component grading tier.
        "bounds_class1": {"pae": pae1, "mse": mse1},
        # Class 0 grades the first component only (upstream-disabled tier).
        "bounds_class0_first_component": {"pae": pae0, "mse": mse0},
        # Every p0_/p1_ entry is a bare Part-1 codestream, all in Phase 2 scope.
        "phase2_in_scope": True,
    }


def main_part1():
    require_openjpeg("opj_dump", "opj_decompress")
    os.makedirs(CS_DIR, exist_ok=True)
    os.makedirs(REF_DIR, exist_ok=True)

    entries = [build_entry(p, i) for (p, i) in codestreams()]

    manifest = {
        "_comment": (
            "ISO/IEC 15444-4 Part 1 conformance corpus. Generated by "
            "regenerate.py; do not edit by hand. See README.md and COPYRIGHT."
        ),
        "compliance_class": 1,
        "provenance": {
            "codestreams": {
                "repo": "uclouvain/openjpeg-data",
                "commit": DATA_COMMIT,
                "path": "input/conformance/",
            },
            "references": {
                "repo": "uclouvain/openjpeg-data",
                "commit": DATA_COMMIT,
                "path": "baseline/conformance/",
            },
            "bounds": {
                "repo": "uclouvain/openjpeg",
                "path": "tests/conformance/CMakeLists.txt",
                "lists": [
                    "C1P0_PEAK_list",
                    "C1P0_MSE_list",
                    "C1P1_PEAK_list",
                    "C1P1_MSE_list",
                ],
                "note": (
                    "Class-1 (per-component) is the active grading tier upstream; "
                    "the class-0 lists (C0P*) are commented out there (first "
                    "component only) and are recorded per entry for reference, not "
                    "used as the bar."
                ),
                "t803_override": (
                    "p1_05 component 1 MSE is 9.716, from ITU-T T.803 Table C.7; "
                    "OpenJPEG's C1P1_MSE_list has 9.816. Every other bound is "
                    "OpenJPEG's."
                ),
            },
            "features": {
                "tool": "opj_dump -i + direct marker-segment walk",
                "note": (
                    "coding parameters parsed from the OpenJPEG oracle; "
                    "optional-marker presence (markers_main/markers_tile), "
                    "SOP/EPH/precinct flags, and code-block style bits walked "
                    "from the marker segments directly, since opj_dump does "
                    "not report them"
                ),
            },
            "license": (
                "Conformance-only grant; see ./COPYRIGHT. Excluded from the "
                "published crate via Cargo.toml `exclude`."
            ),
        },
        "entries": entries,
    }

    with open(os.path.join(HERE, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")

    print(f"wrote manifest.json with {len(entries)} entries")


# ---------------------------------------------------------------------------
# JP2 reader files (ISO/IEC 15444-4 Annex G): file1.jp2 .. file9.jp2.
#
# The references (`jp2_N.tif`) are what OpenJPEG's conformance run grades
# against: 8-bit RGB *display output*, produced after palette expansion, cdef,
# sYCC -> RGB and ICC -> sRGB, then `-p 8S` (a plain right shift to 8 bits).
# Grayscale files carry three identical channels, of which one PGX plane is kept.
# ---------------------------------------------------------------------------

JP2_DIR = os.path.join(HERE, "jp2")
JP2_FILES_DIR = os.path.join(JP2_DIR, "files")
JP2_REF_DIR = os.path.join(JP2_DIR, "references")

JP2_COUNT = 9
# Grayscale: the TIFF reference has three identical channels; keep one plane.
JP2_GRAY = {4, 6, 8}
# Graded bit-exactly (measured: opj_decompress -p 8S matches the reference).
JP2_BIT_EXACT = {1, 4, 6, 9}
# Output depth scaling: `opj_decompress -p 8S` right-shifts to 8 bits.
JP2_SHIFT_RIGHT = {6: 4, 7: 8}
# The reference is sRGB display output the decoder must be asked for (ICC
# profile, sYCC, or palette+cdef-driven channel order).
JP2_NEEDS_SRGB = {2, 3, 5, 7, 8}
# The needs-sRGB files that decode as coded today. Each gets an as-coded
# reference per channel: the codestream samples at the native component size
# and depth, no color conversion, no shift, in the order the file's `cdef`
# gives (file2's puts its components in reverse).
JP2_AS_CODED = {2, 3, 5, 7, 8}
# OpenJPEG tests/conformance/CMakeLists.txt, "Tests about JP2 file": Part 4
# Table G.1, `compare_images -n 3 -p 4:4:4 -m 1:1:1`.
JP2_PAE = 4
JP2_MSE = 1


def read_boxes(data: bytes, start: int, end: int):
    """Yield (type, payload_start, payload_end) for the boxes in data[start:end]."""
    pos = start
    while pos < end:
        if pos + 8 > end:
            raise ValueError("truncated box header")
        lbox = int.from_bytes(data[pos : pos + 4], "big")
        tbox = data[pos + 4 : pos + 8]
        hdr = 8
        if lbox == 1:
            lbox = int.from_bytes(data[pos + 8 : pos + 16], "big")
            hdr = 16
        elif lbox == 0:
            lbox = end - pos
        if lbox < hdr or pos + lbox > end:
            raise ValueError(f"box {tbox!r} at {pos} overruns its parent")
        yield tbox, pos + hdr, pos + lbox
        pos += lbox


def jp2_structure(data: bytes) -> dict:
    """Box layout, color specification, and the jp2c payload of one JP2 file."""
    top, jp2h, color, payload, cdef = [], [], None, None, None
    for tbox, s, e in read_boxes(data, 0, len(data)):
        top.append(tbox.decode("latin-1"))
        if tbox == b"jp2h":
            for ctype, cs, ce in read_boxes(data, s, e):
                jp2h.append(ctype.decode("latin-1"))
                if ctype == b"cdef" and cdef is None:
                    n = int.from_bytes(data[cs : cs + 2], "big")
                    cdef = [
                        tuple(
                            int.from_bytes(data[at + 2 * f : at + 2 * f + 2], "big")
                            for f in range(3)
                        )
                        for at in range(cs + 2, cs + 2 + 6 * n, 6)
                    ]
                    if cs + 2 + 6 * n > ce:
                        raise ValueError("truncated cdef box")
                if ctype == b"colr" and color is None:  # the first colr wins
                    meth = data[cs]
                    color = {"method": meth}
                    if meth == 1:
                        color["enum_cs"] = int.from_bytes(data[cs + 3 : cs + 7], "big")
                    elif meth == 2:
                        color["icc_bytes"] = ce - (cs + 3)
        elif tbox == b"jp2c":
            payload = data[s:e]
    if payload is None or color is None:
        raise ValueError("no jp2c or no colr box")
    return {
        "boxes": {"top": top, "jp2h": jp2h},
        "color": color,
        "payload": payload,
        "cdef": cdef,
    }


def read_tiff_planes(path: str):
    """A reference TIFF as a list of per-channel 2-D uint8 arrays.

    Read with Pillow and numpy (Pillow decodes the references' LZW without
    extra codecs). Layout verified against the nine references: 8-bit
    contiguous RGB, one page.
    """
    import numpy as np
    from PIL import Image

    with Image.open(path) as im:
        if getattr(im, "n_frames", 1) != 1:
            raise ValueError(f"{path}: expected one page, got {im.n_frames}")
        if im.mode not in ("RGB", "L"):
            raise ValueError(f"{path}: expected 8-bit RGB or L, got mode {im.mode}")
        arr = np.asarray(im)
    if arr.dtype.name != "uint8":
        raise ValueError(f"{path}: expected 8-bit samples, got {arr.dtype}")
    if arr.ndim == 2:
        return [arr]
    if arr.ndim != 3 or arr.shape[2] != 3:
        raise ValueError(f"{path}: expected HxWx3, got shape {arr.shape}")
    return [arr[:, :, c] for c in range(3)]


def write_pgx(path: str, plane) -> None:
    """One unsigned 8-bit plane as a big-endian PGX file."""
    h, w = plane.shape
    with open(path, "wb") as f:
        f.write(f"PG ML 8 {w} {h}\n".encode("ascii"))
        f.write(plane.astype("uint8").tobytes())


def plane_metrics(actual, reference):
    diff = actual.astype("int64") - reference.astype("int64")
    return int(abs(diff).max()), float((diff * diff).mean())


def regrade_with_opj(n: int, jp2_path: str, planes: list, bit_exact: bool) -> list:
    """Decode with `opj_decompress -p 8S -force-rgb` and grade against `planes`.

    This is the script's own check that the converted PGX references are the
    ones the conformance decode matches, within the recorded bounds. It raises
    rather than loosen a bound.
    """
    bound_pae = 0 if bit_exact else JP2_PAE
    bound_mse = 0 if bit_exact else JP2_MSE
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, f"file{n}.tif")
        subprocess.run(
            ["opj_decompress", "-i", jp2_path, "-o", out, "-p", "8S", "-force-rgb"],
            check=True,
            capture_output=True,
        )
        got = read_tiff_planes(out)
    # A grayscale decode may come back as one channel or three identical ones;
    # either way it is graded against the single kept plane.
    want = planes if len(planes) == len(got) else [planes[0]] * len(got)
    if len(planes) == 1 and len(got) == 3:
        for c in (1, 2):
            if not (got[0] == got[c]).all():
                raise ValueError(f"file{n}: opj_decompress gray channel {c} differs from 0")
    elif len(got) != len(planes):
        raise ValueError(f"file{n}: opj output has {len(got)} channels, expected {len(planes)}")
    results = []
    for c, (g, w) in enumerate(zip(got, want)):
        if g.shape != w.shape:
            raise ValueError(f"file{n} ch{c}: shape {g.shape} vs reference {w.shape}")
        pae, mse = plane_metrics(g, w)
        results.append((pae, mse))
        if pae > bound_pae or mse > bound_mse:
            raise ValueError(
                f"file{n} ch{c}: opj_decompress -p 8S -force-rgb is PAE {pae}, MSE {mse:.4f}; "
                f"bound is PAE {bound_pae}, MSE {bound_mse} (bit_exact={bit_exact})"
            )
    return results


def cdef_order(n: int, cdef: list, count: int) -> list:
    """The output order a `cdef` gives: `order[p]` is the component at position
    `p`. Only the simple form is accepted (every entry a color, each component
    and each color once), where OpenJPEG's swaps put color `a` at `a - 1`."""
    if sorted(cn for cn, _, _ in cdef) != list(range(count)) or sorted(
        asoc for _, _, asoc in cdef
    ) != list(range(1, count + 1)) or any(typ != 0 for _, typ, _ in cdef):
        raise ValueError(f"file{n}: cdef {cdef} is not a plain color permutation")
    order = [0] * count
    for cn, _, asoc in cdef:
        order[asoc - 1] = cn
    return order


def opj_cdef_check(n: int, data: bytes, info: dict, raws: list, tmp: str) -> None:
    """Hold the permuted planes to OpenJPEG's own `cdef` handling: decode the
    `.jp2` whole with its first `colr` rewritten to sRGB (enum 16), which stops
    `opj_decompress` converting sYCC, and require the same planes."""
    if info["color"].get("method") != 1:
        raise ValueError(f"file{n}: the cdef check rewrites an enumerated colr")
    at = data.index(b"colr") + 4
    patched = data[: at + 3] + (16).to_bytes(4, "big") + data[at + 7 :]
    jp2 = os.path.join(tmp, "srgb.jp2")
    with open(jp2, "wb") as f:
        f.write(patched)
    subprocess.run(
        ["opj_decompress", "-i", jp2, "-o", os.path.join(tmp, "srgb.pgx")],
        check=True,
        capture_output=True,
    )
    for k, raw in enumerate(raws):
        with open(os.path.join(tmp, f"srgb_{k}.pgx"), "rb") as f:
            if f.read() != raw:
                raise ValueError(f"file{n} channel {k}: OpenJPEG's cdef order disagrees")


def as_coded_references(n: int, info: dict, features: dict, data: bytes) -> list:
    """One PGX per channel of the codestream as coded, from OpenJPEG.

    The `jp2c` payload is decoded as a bare codestream, so no box-layer color
    handling (ICC, palette) can touch the samples; the file must have no
    palette. A `cdef` only reorders the channels: the planes are put in its
    order (see `cdef_order`), and that order is checked against OpenJPEG
    decoding the file itself with its color space rewritten to sRGB
    (`opj_cdef_check`). Two OpenJPEG behaviors decide how:

    - `opj_decompress` converts sYCC to RGB by heuristic whenever a
      3-component image has sub-sampled chroma (opj_decompress.c:1620-1626),
      whatever `colr` says. A sub-sampled file is therefore decoded one
      component per run with `-c k`.
    - `-c k` skips the inverse multiple component transform (it needs all
      three components), so `-c` is only used when `mct` is 0. A file with
      `mct` 1 is decoded whole, and must not be sub-sampled.

    Each output is checked to be the native component size and depth (chroma
    is not upsampled).
    """
    jp2h = info["boxes"]["jp2h"]
    for banned in ("pclr", "cmap"):
        if banned in jp2h:
            raise ValueError(f"file{n}: has {banned}; not an as-coded file")
    count = features["components"]
    use_c = any(sub != [1, 1] for sub in features["subsampling"])
    if use_c and features["mct"] != 0:
        raise ValueError(f"file{n}: sub-sampled with mct; -c would skip the transform")
    order = list(range(count))
    if info["cdef"] is not None:
        if use_c:
            raise ValueError(f"file{n}: cdef on a sub-sampled file; the check would hit sYCC")
        order = cdef_order(n, info["cdef"], count)
    raws = []
    with tempfile.TemporaryDirectory() as tmp:
        cs = os.path.join(tmp, "cs.j2k")
        with open(cs, "wb") as f:
            f.write(info["payload"])
        if use_c:
            for k in range(count):
                out = os.path.join(tmp, f"c{k}.pgx")
                subprocess.run(
                    ["opj_decompress", "-i", cs, "-o", out, "-c", str(k)],
                    check=True,
                    capture_output=True,
                )
                # With -c, OpenJPEG names the single output <stem>_0.pgx.
                with open(os.path.join(tmp, f"c{k}_0.pgx"), "rb") as f:
                    raws.append(f.read())
        else:
            out = os.path.join(tmp, "all.pgx")
            subprocess.run(
                ["opj_decompress", "-i", cs, "-o", out], check=True, capture_output=True
            )
            for k in range(count):
                name = f"all_{k}.pgx"
                with open(os.path.join(tmp, name), "rb") as f:
                    raws.append(f.read())
        raws = [raws[c] for c in order]
        if info["cdef"] is not None:
            opj_cdef_check(n, data, info, raws, tmp)
    refs = []
    for k, raw in enumerate(raws):
        c = order[k]
        dx, dy = features["subsampling"][c]
        want = (features["precision"][c], -(-features["width"] // dx), -(-features["height"] // dy))
        header = raw[: raw.index(b"\n")].decode("ascii").split()
        got = (int(header[-3].lstrip("+-")), int(header[-2]), int(header[-1]))
        if got != want:
            raise ValueError(f"file{n} comp {k}: opj gave depth/w/h {got}, expected {want}")
        rel = f"references/jp2_{n}_ascoded_{k}.pgx"
        with open(os.path.join(JP2_DIR, rel), "wb") as f:
            f.write(raw)
        refs.append(rel)
    return refs


def digest(path: str) -> dict:
    """Size and SHA-256 of a committed file, as the manifest records them."""
    return {"size": os.path.getsize(path), "sha256": sha256_file(path)}


def build_jp2_entry(n: int) -> dict:
    name = f"file{n}.jp2"
    jp2_path = os.path.join(JP2_FILES_DIR, name)
    download(f"input/conformance/{name}", jp2_path)
    # The TIFF is only an intermediate: fetch it aside and keep the PGX planes.
    tif_path = os.path.join(JP2_REF_DIR, f"jp2_{n}.tif")
    download(f"baseline/conformance/jp2_{n}.tif", tif_path)
    try:
        channels = read_tiff_planes(tif_path)
    finally:
        os.remove(tif_path)

    if len(channels) != 3:
        raise ValueError(f"jp2_{n}.tif: expected 3 channels, got {len(channels)}")
    if n in JP2_GRAY:
        for c in (1, 2):
            if not (channels[0] == channels[c]).all():
                raise ValueError(f"jp2_{n}.tif: gray channel {c} differs from channel 0")
        planes = [channels[0]]
    else:
        planes = channels

    with open(jp2_path, "rb") as f:
        data = f.read()
    if data[:12] != b"\x00\x00\x00\x0cjP  \r\n\x87\n":
        raise ValueError(f"{name}: not a JP2 signature box")
    info = jp2_structure(data)

    refs = []
    for k, plane in enumerate(planes):
        rel = f"references/jp2_{n}_{k}.pgx"
        write_pgx(os.path.join(JP2_DIR, rel), plane)
        refs.append(rel)

    bit_exact = n in JP2_BIT_EXACT
    results = regrade_with_opj(n, jp2_path, planes, bit_exact)
    print(
        f"  file{n}: "
        + ", ".join(f"PAE {p} MSE {m:.4f}" for p, m in results)
        + (" (bit-exact bound)" if bit_exact else f" (bound PAE {JP2_PAE} MSE {JP2_MSE})")
    )

    features = opj_features(jp2_path)
    features.update(marker_features_data(info["payload"], name))
    graded = len(planes)
    entry = {
        "file": f"files/{name}",
        "index": n,
        "graded_components": graded,
        "bit_exact": bit_exact,
        # `opj_decompress -p 8S` shifts the decoded samples right by this much.
        "shift_right": JP2_SHIFT_RIGHT.get(n, 0),
        # The reference is sRGB display output the decoder must be asked for.
        "needs_srgb": n in JP2_NEEDS_SRGB,
        "boxes": info["boxes"],
        "color": info["color"],
        "features": features,
        "references": refs,
        # Pins the committed bytes; tests/conformance_jp2.rs re-hashes them.
        "file_digest": digest(jp2_path),
        "reference_digests": [digest(os.path.join(JP2_DIR, r)) for r in refs],
        "bounds": {"pae": [JP2_PAE] * graded, "mse": [JP2_MSE] * graded},
    }
    if n in JP2_AS_CODED:
        as_coded = as_coded_references(n, info, features, data)
        entry["as_coded"] = {
            "references": as_coded,
            "reference_digests": [digest(os.path.join(JP2_DIR, r)) for r in as_coded],
        }
    return entry


def main_jp2():
    require_openjpeg("opj_dump", "opj_decompress")
    require_lcms()  # the -force-rgb regrade goes through LittleCMS
    os.makedirs(JP2_FILES_DIR, exist_ok=True)
    os.makedirs(JP2_REF_DIR, exist_ok=True)
    print("re-grading opj_decompress -p 8S -force-rgb against the converted references:")
    entries = [build_jp2_entry(n) for n in range(1, JP2_COUNT + 1)]
    manifest = {
        "_comment": (
            "ISO/IEC 15444-4 Annex G JP2 reader files. Generated by regenerate.py; "
            "do not edit by hand. See ../README.md and ../COPYRIGHT."
        ),
        "compliance_class": 1,
        "provenance": {
            "files": {
                "repo": "uclouvain/openjpeg-data",
                "commit": DATA_COMMIT,
                "path": "input/conformance/file{1..9}.jp2",
            },
            "references": {
                "repo": "uclouvain/openjpeg-data",
                "commit": DATA_COMMIT,
                "path": "baseline/conformance/jp2_{1..9}.tif",
                "conversion": (
                    "8-bit RGB TIFF read with Pillow, written as one unsigned 8-bit "
                    "PGX plane per channel; grayscale files (4, 6, 8) keep one plane "
                    "after asserting the three channels are identical"
                ),
                "oracle_command": "opj_decompress -i fileN.jp2 -o fileN.tif -p 8S -force-rgb",
            },
            "bounds": {
                "repo": "uclouvain/openjpeg",
                "path": "tests/conformance/CMakeLists.txt",
                "block": "Tests about JP2 file (Part 4 Table G.1): compare_images -n 3 -p 4:4:4 -m 1:1:1",
                "note": (
                    "files 1, 4, 6 and 9 are graded bit-exact (measured: the OpenJPEG "
                    "decode equals the reference); the rest use PAE 4 / MSE 1"
                ),
            },
            "as_coded": {
                "files": "2, 3, 5, 7, 8",
                "oracle_command": (
                    "the jp2c payload as a bare codestream: opj_decompress -i cs.j2k "
                    "-o out.pgx; the sub-sampled file (3) one component at a time with -c K; "
                    "file2's planes put in its cdef order and checked against opj_decompress "
                    "on file2.jp2 with its colr rewritten to sRGB (enum 16)"
                ),
                "note": (
                    "codestream samples at the native component size and depth, no "
                    "color conversion and no shift. -c avoids opj_decompress's sYCC "
                    "heuristic but skips the inverse MCT, so it is used only for the "
                    "sub-sampled file (mct 0); the bare codestream carries no ICC"
                ),
            },
            "features": {
                "tool": "opj_dump -i + direct marker-segment walk of the jp2c payload",
                "note": "boxes and color are read from the JP2 box layer directly",
            },
            "license": (
                "Conformance-only grant; see ../COPYRIGHT. Excluded from the "
                "published crate via Cargo.toml `exclude`."
            ),
        },
        "entries": entries,
    }
    with open(os.path.join(JP2_DIR, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    print(f"wrote jp2/manifest.json with {len(entries)} entries")


# ---------------------------------------------------------------------------
# HTJ2K (ISO/IEC 15444-15) conformance files: the derived-set codestreams of
# ISO/IEC 15444-4:2024 Annex C and the JPH reader files of Annex G.5, from the
# JPEG committee's repository, graded with the tolerances of ITU-T T.803
# (V3, 02/2024), which is the same text as ISO/IEC 15444-4:2024.
# ---------------------------------------------------------------------------

HT_COMMIT = "f6b9ede094a0bd6e1e0427e12721e3f3ee1b704b"
HT_REPO = "https://gitlab.com/wg1/htj2k-codestreams"
HT_RAW = HT_REPO + "/-/raw/" + HT_COMMIT + "/%s"
# Set to a checkout of HT_REPO to copy from it instead of downloading 47 MB.
# Its HEAD must be HT_COMMIT.
HT_CHECKOUT_ENV = "HTJ2K_CODESTREAMS"
HT_DIR = os.path.join(HERE, "ht")
HT_CS_DIR = os.path.join(HT_DIR, "codestreams")
HT_JPH_DIR = os.path.join(HT_DIR, "jph")
HT_REF_DIR = os.path.join(HT_DIR, "references")
HT_ORACLE_DIR = os.path.join(HT_DIR, "oracle")

# T.803 Tables C.6 and C.7: the class-1 tolerance of each Part 1 codestream,
# peak and MSE per graded component. An HT tolerance starts from these. They
# are not OpenJPEG's lists in BOUNDS, which differ for p0_05, p1_03 and p1_05.
T803_PART1 = {
    "p0_01": ([0], [0]),
    "p0_02": ([0], [0]),
    "p0_03": ([0], [0]),
    "p0_04": ([5, 4, 6], [0.776, 0.626, 1.070]),
    "p0_05": ([2, 2, 2, 0], [0.319, 0.323, 0.317, 0]),
    "p0_06": ([635, 403, 378, 0], [11287, 6124, 3968, 0]),
    "p0_07": ([0, 0, 0], [0, 0, 0]),
    "p0_08": ([0, 0, 0], [0, 0, 0]),
    "p0_09": ([0], [0]),
    "p0_10": ([0, 0, 0], [0, 0, 0]),
    "p0_11": ([0], [0]),
    "p0_12": ([0], [0]),
    "p0_13": ([0, 0, 0, 0], [0, 0, 0, 0]),
    "p0_14": ([0, 0, 0], [0, 0, 0]),
    "p0_15": ([0], [0]),
    "p0_16": ([0], [0]),
    "p1_01": ([0], [0]),
    "p1_02": ([5, 4, 6], [0.765, 0.616, 1.051]),
    "p1_03": ([2, 2, 1, 0], [0.311, 0.280, 0.267, 0]),
    "p1_04": ([624], [3080]),
    "p1_05": ([40, 40, 40], [8.458, 9.716, 10.154]),
    "p1_06": ([2, 2, 2], [0.600, 0.600, 0.600]),
    "p1_07": ([0, 0], [0, 0]),
}

# T.803 Tables C.1bis (derived set 0) and C.4bis (derived set 1): the
# additional peak and MSE an HT decoder is allowed per component, with the
# reference depth Dref. Added to T803_PART1 after scaling the peak by
# 2^(D - Dref), rounded up, and the MSE by 4^(D - Dref), where D is the
# component depth in Table C.6 or C.7.
T803_ADDITIONAL = {
    "ds0_ht_01_b11": (8, [0], [0]),
    "ds0_ht_02_b11": (8, [1], [0.001]),
    "ds0_ht_02_b12": (8, [0], [0]),
    "ds0_ht_03_b11": (4, [17], [0.15]),
    "ds0_ht_03_b14": (4, [0], [0]),
    "ds0_ht_04_b11": (8, [2, 2, 2], [0.1, 0.1, 0.1]),
    "ds0_ht_04_b12": (8, [0, 0, 0], [0, 0, 0]),
    "ds0_ht_05_b11": (8, [0, 1, 1, 2], [0, 0.001, 0.001, 0.001]),
    "ds0_ht_05_b12": (8, [0, 0, 0, 0], [0, 0, 0, 0]),
    "ds0_ht_06_b11": (12, [2500, 0, 0, 200], [75000, 0, 0, 2000]),
    "ds0_ht_06_b15": (12, [0, 0, 0, 12], [0, 0, 0, 10.0]),
    "ds0_ht_06_b18": (12, [0, 0, 0, 0], [0, 0, 0, 0]),
    "ds0_hm_06_b11": (12, [0, 0, 0, 200], [0, 0, 0, 2000]),
    "ds0_hm_06_b18": (12, [0, 0, 0, 0], [0, 0, 0, 0]),
    "ds0_ht_07_b11": (12, [40, 40, 50], [25.0, 25.0, 25.0]),
    "ds0_ht_07_b15": (12, [2, 2, 2], [0.075, 0.05, 0.075]),
    "ds0_ht_07_b16": (12, [0, 0, 0], [0, 0, 0]),
    "ds0_ht_08_b11": (12, [40, 40, 40], [45.0, 30.0, 45.0]),
    "ds0_ht_08_b15": (12, [0, 1, 0], [0, 0.001, 0]),
    "ds0_ht_08_b16": (12, [0, 0, 0], [0, 0, 0]),
    "ds0_ht_09_b11": (8, [0], [0]),
    "ds0_ht_10_b11": (8, [0, 0, 0], [0, 0, 0]),
    "ds0_ht_11_b10": (8, [0], [0]),
    "ds0_ht_12_b11": (8, [0], [0]),
    "ds0_ht_13_b11": (8, [0, 0, 0, 0], [0, 0, 0, 0]),
    "ds0_ht_14_b11": (8, [0, 0, 0], [0, 0, 0]),
    "ds0_ht_15_b11": (4, [17], [0.15]),
    "ds0_ht_15_b14": (4, [0], [0]),
    "ds0_hm_15_b8": (4, [0], [0]),
    "ds0_ht_16_b11": (8, [0], [0]),
    "ds1_ht_01_b11": (8, [1], [0.001]),
    "ds1_ht_01_b12": (8, [0], [0]),
    "ds1_ht_02_b11": (8, [2, 2, 2], [0.1, 0.1, 0.1]),
    "ds1_ht_02_b12": (8, [0, 0, 0], [0, 0, 0]),
    "ds1_ht_03_b11": (8, [0, 1, 2, 2], [0, 0.03, 0.05, 0.001]),
    "ds1_ht_03_b12": (8, [0, 0, 0, 0], [0, 0, 0, 0]),
    "ds1_ht_04_b9": (8, [0], [0]),
    "ds1_ht_05_b11": (8, [0, 0, 0], [0, 0, 0]),
    "ds1_ht_06_b11": (8, [0, 0, 0], [0, 0, 0]),
    "ds1_ht_07_b11": (8, [0, 0], [0, 0]),
}

# T.803 Table C.8 (class 1HF, profile 1): the tolerances are given whole, for
# the HT codestream and for the Part 1 codestream it was made from.
T803_HIFI = {
    "hifi_ht1_02": ([47, 36, 42], [82, 65, 86]),
    "hifi_p1_02": ([43, 33, 38], [80, 62, 72]),
}

# T.803 Tables C.2bis and C.5bis: the BSETs described as "multiple layers with
# placeholder passes".
T803_PLACEHOLDER_BSETS = {("ds0", n) for n in (2, 3, 4, 5, 6, 7, 8, 10, 15)} | {
    ("ds1", n) for n in (1, 2, 3, 5)
}

# Coding passes per HT code-block, measured by running opj_decompress 2.5.0
# with one line added to `opj_t1_ht_decode_cblk` (src/lib/openjp2/ht_dec.c, at
# the `num_passes > 1 && lengths2 == 0` check) that prints the sum of
# `real_num_passes` over the block's segments. Only files it decodes in full
# are listed: name -> (blocks, {passes: blocks with that many}, blocks whose
# refinement segment has zero length). A block with placeholder passes has at
# least four (three placeholders and a cleanup pass), so a file listed here
# with at most three per block has none.
HT_PASSES = {
    "ds0_ht_01_b11": (10, {1: 10}, 0),
    "ds0_ht_09_b11": (14, {3: 14}, 0),
    "ds0_ht_11_b10": (2, {1: 2}, 0),
    "ds0_ht_12_b11": (8, {1: 6, 3: 2}, 0),
    "ds0_ht_14_b11": (42, {1: 31, 3: 11}, 0),
    "ds1_ht_04_b9": (618, {1: 618}, 0),
    "ds1_ht_05_b11": (7485, {1: 3039, 2: 2235, 3: 2211}, 1849),
    "ds1_ht_06_b11": (180, {1: 63, 2: 2, 3: 115}, 2),
    "ds1_ht_07_b11": (30, {1: 30}, 0),
    "hifi_ht1_02": (57, {2: 21, 3: 36}, 0),
    "file1_b11": (309, {1: 207, 3: 102}, 0),
    "file1_b12": (309, {1: 309}, 0),
    "file2_b11": (273, {1: 183, 3: 90}, 0),
    "file2_b12": (273, {1: 273}, 0),
    "file3_b11": (159, {1: 107, 3: 52}, 0),
    "file3_b12": (159, {1: 159}, 0),
    "file4_b11": (103, {1: 69, 3: 34}, 0),
    "file4_b12": (103, {1: 103}, 0),
    "file5_b11": (309, {1: 207, 3: 102}, 0),
    "file5_b12": (309, {1: 309}, 0),
    "file6_b11": (103, {3: 103}, 0),
    "file6_b15": (103, {1: 69, 3: 34}, 0),
    "file7_b11": (273, {3: 273}, 0),
    "file7_b15": (273, {3: 273}, 0),
    "file7_b19": (273, {1: 183, 3: 90}, 0),
    "file8_b11": (103, {1: 103}, 0),
    "file9_b11": (103, {1: 69, 3: 34}, 0),
    "file9_b12": (103, {1: 103}, 0),
    "simple_dec_irv97_64x64_16bit_gray": (39, {1: 32, 3: 7}, 0),
    "simple_dec_irv97_64x64_16bit": (40, {1: 14, 2: 17, 3: 9}, 0),
    "simple_dec_irv97_64x64_gray_tiles": (637, {1: 215, 2: 250, 3: 172}, 0),
    "simple_dec_irv97_64x64_tiles_yuv": (315, {1: 110, 2: 85, 3: 120}, 0),
    "simple_dec_irv97_64x64_yuv": (58, {1: 40, 2: 7, 3: 11}, 0),
    "simple_dec_rev53_64x64_tiles_yuv": (879, {1: 879}, 0),
    "simple_dec_rev53_64x64_yuv": (99, {1: 99}, 0),
    "simple_enc_irv97_64x64_16bit_gray": (49, {1: 49}, 0),
    "simple_enc_irv97_64x64_16bit": (96, {1: 96}, 0),
    "simple_enc_irv97_64x64": (438, {1: 438}, 0),
    "simple_enc_irv97_64x64_yuv": (73, {1: 73}, 0),
    "simple_enc_rev53_64x64_yuv": (99, {1: 99}, 0),
    "simple_enc_irv97_tall_narrow1": (32, {1: 32}, 0),
    "simple_enc_irv97_tall_narrow": (34, {1: 34}, 0),
    "simple_enc_rev53_tall_narrow1": (62, {1: 62}, 0),
    "simple_enc_rev53_tall_narrow": (51, {1: 51}, 0),
}

# Coding passes per HT code-block, from this crate's own Tier-2 walk over every
# packet header (`tier2::tests::ht::the_corpus_pass_counts_are_the_manifests`;
# `cargo test --lib the_corpus_pass_counts -- --nocapture` prints them). The
# walk needs no HT decoder, so it covers the files OpenJPEG refuses. name ->
# (blocks with any pass, blocks with placeholder passes, {passes past the
# placeholders: blocks}, blocks with refinement passes and no refinement
# bytes), counting HT blocks only; the MIXED files add a fifth count, their
# Part 1 blocks with any pass. Where HT_PASSES has the file too, the two
# agree, which the test checks.
HT_WALKED = {
    "ds0_hm_06_b11": (84, 79, {1: 60, 3: 24}, 0, 17),
    "ds0_hm_06_b18": (84, 80, {1: 77, 3: 7}, 0, 17),
    "ds0_hm_15_b8": (12, 7, {1: 12}, 0, 4),
    "ds0_ht_01_b11": (10, 0, {1: 10}, 0),
    "ds0_ht_02_b11": (13, 13, {1: 9, 3: 4}, 0),
    "ds0_ht_02_b12": (13, 13, {1: 13}, 0),
    "ds0_ht_03_b11": (16, 11, {1: 12, 3: 4}, 0),
    "ds0_ht_03_b14": (16, 11, {1: 16}, 0),
    "ds0_ht_04_b11": (280, 258, {1: 204, 3: 76}, 0),
    "ds0_ht_04_b12": (280, 258, {1: 280}, 0),
    "ds0_ht_05_b11": (2534, 1944, {1: 1379, 2: 531, 3: 624}, 1),
    "ds0_ht_05_b12": (2534, 1944, {1: 1682, 2: 531, 3: 321}, 1),
    "ds0_ht_06_b11": (98, 93, {1: 60, 2: 1, 3: 37}, 1),
    "ds0_ht_06_b15": (101, 94, {1: 76, 2: 1, 3: 24}, 1),
    "ds0_ht_06_b18": (101, 94, {1: 93, 2: 1, 3: 7}, 1),
    "ds0_ht_07_b11": (7621, 7528, {3: 7621}, 0),
    "ds0_ht_07_b15": (7680, 7671, {1: 5376, 3: 2304}, 0),
    "ds0_ht_07_b16": (7680, 7671, {1: 7680}, 0),
    "ds0_ht_08_b11": (2523, 2517, {3: 2523}, 0),
    "ds0_ht_08_b15": (2523, 2523, {1: 1741, 3: 782}, 0),
    "ds0_ht_08_b16": (2523, 2523, {1: 2523}, 0),
    "ds0_ht_09_b11": (14, 0, {3: 14}, 0),
    "ds0_ht_10_b11": (120, 108, {1: 84, 3: 36}, 0),
    "ds0_ht_11_b10": (2, 0, {1: 2}, 0),
    "ds0_ht_12_b11": (8, 0, {1: 6, 3: 2}, 0),
    "ds0_ht_13_b11": (256, 0, {1: 255, 3: 1}, 0),
    "ds0_ht_14_b11": (42, 0, {1: 31, 3: 11}, 0),
    "ds0_ht_15_b11": (16, 11, {1: 12, 3: 4}, 0),
    "ds0_ht_15_b14": (16, 11, {1: 16}, 0),
    "ds0_ht_16_b11": (10, 7, {1: 10}, 0),
    "ds1_ht_01_b11": (13, 11, {1: 9, 3: 4}, 0),
    "ds1_ht_01_b12": (13, 11, {1: 13}, 0),
    "ds1_ht_02_b11": (280, 259, {1: 204, 3: 76}, 0),
    "ds1_ht_02_b12": (280, 259, {1: 280}, 0),
    "ds1_ht_03_b11": (2534, 2082, {1: 1415, 3: 1119}, 0),
    "ds1_ht_03_b12": (2534, 2292, {1: 2534}, 0),
    "ds1_ht_04_b9": (618, 0, {1: 618}, 0),
    "ds1_ht_05_b11": (7485, 0, {1: 3039, 2: 2235, 3: 2211}, 1849),
    "ds1_ht_06_b11": (180, 0, {1: 63, 2: 2, 3: 115}, 2),
    "ds1_ht_07_b11": (30, 0, {1: 30}, 0),
    "hifi_ht1_02": (57, 0, {2: 21, 3: 36}, 0),
    "file1_b11": (309, 0, {1: 207, 3: 102}, 0),
    "file1_b12": (309, 0, {1: 309}, 0),
    "file2_b11": (273, 0, {1: 183, 3: 90}, 0),
    "file2_b12": (273, 0, {1: 273}, 0),
    "file3_b11": (159, 0, {1: 107, 3: 52}, 0),
    "file3_b12": (159, 0, {1: 159}, 0),
    "file4_b11": (103, 0, {1: 69, 3: 34}, 0),
    "file4_b12": (103, 0, {1: 103}, 0),
    "file5_b11": (309, 0, {1: 207, 3: 102}, 0),
    "file5_b12": (309, 0, {1: 309}, 0),
    "file6_b11": (103, 0, {3: 103}, 0),
    "file6_b15": (103, 0, {1: 69, 3: 34}, 0),
    "file7_b11": (273, 0, {3: 273}, 0),
    "file7_b15": (273, 0, {3: 273}, 0),
    "file7_b19": (273, 0, {1: 183, 3: 90}, 0),
    "file8_b11": (103, 0, {1: 103}, 0),
    "file9_b11": (103, 0, {1: 69, 3: 34}, 0),
    "file9_b12": (103, 0, {1: 103}, 0),
    "file10_b11": (125, 0, {1: 110, 3: 15}, 0),
    "file10_b12": (125, 0, {1: 125}, 0),
    "simple_dec_irv97_64x64_16bit_gray": (39, 0, {1: 32, 3: 7}, 0),
    "simple_dec_irv97_64x64_16bit": (40, 0, {1: 14, 2: 17, 3: 9}, 0),
    "simple_dec_irv97_64x64_gray_tiles": (637, 0, {1: 215, 2: 250, 3: 172}, 0),
    "simple_dec_irv97_64x64_tiles_yuv": (315, 0, {1: 110, 2: 85, 3: 120}, 0),
    "simple_dec_irv97_64x64_yuv": (58, 0, {1: 40, 2: 7, 3: 11}, 0),
    "simple_dec_rev53_64x64_tiles_yuv": (879, 0, {1: 879}, 0),
    "simple_dec_rev53_64x64_yuv": (99, 0, {1: 99}, 0),
    "simple_enc_irv97_64x64_16bit_gray": (49, 0, {1: 49}, 0),
    "simple_enc_irv97_64x64_16bit": (96, 0, {1: 96}, 0),
    "simple_enc_irv97_64x64": (438, 0, {1: 438}, 0),
    "simple_enc_irv97_64x64_yuv": (73, 0, {1: 73}, 0),
    "simple_enc_irv97_tall_narrow1": (32, 0, {1: 32}, 0),
    "simple_enc_irv97_tall_narrow": (34, 0, {1: 34}, 0),
    "simple_enc_rev53_64x64_yuv": (99, 0, {1: 99}, 0),
    "simple_enc_rev53_tall_narrow1": (62, 0, {1: 62}, 0),
    "simple_enc_rev53_tall_narrow": (51, 0, {1: 51}, 0),
}

# The JPH files: N -> the magnitude bounds provided. The highest bound of each
# set is lossless against fileN.jp2's codestream (Kakadu's stats are 0).
JPH_BOUNDS = {
    1: [11, 12], 2: [11, 12], 3: [11, 12], 4: [11, 12], 5: [11, 12],
    6: [11, 15], 7: [11, 15, 19], 8: [11], 9: [11, 12], 10: [11, 12],
}

# OpenJPEG agreement: how far a decode of the same codestream may drift from
# OpenJPEG's. Exact for 5/3; for 9/7 the bounds of the existing irreversible
# fixtures (1 to 12 bits: tiles_odd_origin_irreversible; 16 bits:
# irreversible_16bit). MSE cannot exceed PAE squared, so it is bounded there.
def opj_agreement_bound(irreversible: bool, max_depth: int) -> dict:
    if not irreversible:
        return {"pae": 0, "mse": 0}
    pae = 1 if max_depth <= 12 else 5
    return {"pae": pae, "mse": pae * pae}


def be(data: bytes, at: int, n: int) -> int:
    return int.from_bytes(data[at : at + n], "big")


def segments(data: bytes, path: str):
    """Yield (marker, segment payload, in_main_header) for every marker segment
    of a codestream, stepping over entropy data by Psot (as marker_features_data
    does)."""
    if data[:2] != b"\xff\x4f":
        raise ValueError(f"{path}: missing SOC marker")
    pos, in_main, tile_end = 2, True, 0
    while pos < len(data):
        marker = be(data, pos, 2)
        pos += 2
        if marker == 0xFFD9:
            return
        if 0xFF30 <= marker <= 0xFF3F:
            continue
        if marker == 0xFF93:
            pos = tile_end
            continue
        length = be(data, pos, 2)
        seg = data[pos + 2 : pos + length]
        if marker == 0xFF90:
            in_main = False
            psot = be(seg, 2, 4)
            if psot == 0:
                raise ValueError(f"{path}: Psot=0 tile-part is unsupported here")
            tile_end = pos - 2 + psot
        yield marker, seg, in_main
        pos += length


def ht_magnitude_bound(p: int) -> int:
    """T.814 A.3.7: the magnitude bound B from Ccap15 bits 0-4."""
    if p < 20:
        return p + 8
    if p < 31:
        return 4 * (p - 19) + 27
    return 74


def ht_header(data: bytes, path: str) -> dict:
    """The main header's SIZ, COD, CAP and CPF fields, read from the bytes."""
    siz = cod = cap = cpf = None
    cblksty, irreversible = set(), False
    for marker, seg, in_main in segments(data, path):
        if marker == 0xFF51:
            siz = seg
        elif marker == 0xFF52:
            cod = cod if cod is not None else seg
            cblksty.add(seg[8])
            irreversible |= seg[9] == 0
        elif marker == 0xFF53:
            cidx = 2 if be(siz, 34, 2) >= 257 else 1
            cblksty.add(seg[cidx + 4])
            irreversible |= seg[cidx + 5] == 0
        elif marker == 0xFF50 and in_main:
            cap = seg
        elif marker == 0xFF59 and in_main:
            cpf = seg
    if siz is None or cod is None:
        raise ValueError(f"{path}: no SIZ or COD in the main header")
    csiz = be(siz, 34, 2)
    comps = [siz[36 + 3 * i : 39 + 3 * i] for i in range(csiz)]
    x1, y1, x0, y0 = be(siz, 2, 4), be(siz, 6, 4), be(siz, 10, 4), be(siz, 14, 4)
    xt, yt, xt0, yt0 = be(siz, 18, 4), be(siz, 22, 4), be(siz, 26, 4), be(siz, 30, 4)
    features = {
        "width": x1 - x0,
        "height": y1 - y0,
        "components": csiz,
        "subsampling": [[c[1], c[2]] for c in comps],
        "precision": [(c[0] & 0x7F) + 1 for c in comps],
        "signed": [bool(c[0] & 0x80) for c in comps],
        "tiles": [-(-(x1 - xt0) // xt), -(-(y1 - yt0) // yt)],
        "progression": PROGRESSION[cod[1]],
        "layers": be(cod, 2, 2),
        "mct": cod[4],
        "resolutions": cod[5] + 1,
        "code_block": [cod[6] + 2, cod[7] + 2],
        "reversible": cod[9] == 1,
        # Every code-block style byte in COD and COC, main and tile-part
        # headers: 0x40 is HT, 0xC0 MIXED (T.814 A.4).
        "cblksty_bytes": sorted(cblksty),
        # Whether any COD or COC selects the 9/7 transform.
        "irreversible_anywhere": irreversible,
    }
    if cap is None:
        raise ValueError(f"{path}: no CAP marker segment")
    pcap = be(cap, 0, 4)
    if pcap != 0x00020000 or len(cap) != 6:
        raise ValueError(f"{path}: Pcap {pcap:#010x} with {len(cap)} bytes, expected Part 15 only")
    ccap15 = be(cap, 4, 2)
    sets = {0b00: "HTONLY", 0b10: "HTDECLARED", 0b11: "MIXED"}
    if (ccap15 >> 14) not in sets:
        raise ValueError(f"{path}: reserved Ccap15 bits 14-15 in {ccap15:#06x}")
    cpf_num = None
    if cpf is not None:
        words = [be(cpf, 2 * i, 2) for i in range(len(cpf) // 2)]
        cpf_num = -1 + sum(w << (16 * i) for i, w in enumerate(words))
    ht = {
        "pcap": pcap,
        "ccap15": ccap15,
        "coder_set": sets[ccap15 >> 14],
        "multiht": bool(ccap15 & 0x2000),
        "rgn": bool(ccap15 & 0x1000),
        "heterogeneous": bool(ccap15 & 0x0800),
        "irreversible": bool(ccap15 & 0x0020),
        "magb_bound": ht_magnitude_bound(ccap15 & 0x1F),
        "cpf": cpf_num,
    }
    return {"features": features, "ht": ht}


def parse_stats(text: str, path: str) -> dict:
    """Kakadu's stats file: {(B, htmix): {"depth", "pae", "mse"}}.

    Each `----B=NN----` section lists, per component, `pnm_psnr -stats` output
    in one of two layouts: a (peak, MSE, PSNR) row per component, or three
    rows (peaks, MSEs, PSNRs) with a trailing max or average column.
    """
    sections, cur = {}, None
    for line in text.splitlines():
        s = line.strip()
        m = re.match(r"^----B=(\d+)( HTMIX)?----$", s)
        if m:
            cur = {"depth": None, "rows": [], "columns": False}
            sections[(int(m.group(1)), bool(m.group(2)))] = cur
            continue
        if cur is None:
            continue
        d = re.match(r"^(\d+)b\b", s)
        if d:
            cur["depth"] = int(d.group(1))
            cur["columns"] |= "C1-" in s
        elif re.fullmatch(r"[\d.inf, ]+", s):
            cur["rows"].append([v.strip() for v in s.split(",")])
    out = {}
    for key, cur in sections.items():
        rows = cur["rows"]
        if cur["columns"] or (rows and len(rows[0]) >= 4):
            if len(rows) != 3:
                raise ValueError(f"{path} B={key[0]}: expected 3 rows, got {rows}")
            pae, mse = [int(v) for v in rows[0][:-1]], [float(v) for v in rows[1][:-1]]
        else:
            if not rows or any(len(r) != 3 for r in rows):
                raise ValueError(f"{path} B={key[0]}: unexpected rows {rows}")
            pae, mse = [int(r[0]) for r in rows], [float(r[1]) for r in rows]
        out[key] = {"depth": cur["depth"], "pae": pae, "mse": mse}
    return out


def read_pgx(raw: bytes):
    """A PGX file as (signed, depth, 2-D int64 array)."""
    import numpy as np

    nl = raw.index(b"\n")
    tokens = raw[:nl].decode("ascii").split()
    if tokens[0] != "PG" or tokens[1] not in ("ML", "LM"):
        raise ValueError(f"not a PGX header: {tokens}")
    rest = tokens[2:]
    if rest[0] in ("+", "-"):
        rest = [rest[0] + rest[1]] + rest[2:]
    depth_token, w, h = rest[0], int(rest[1]), int(rest[2])
    signed = depth_token.startswith("-")
    depth = int(depth_token.lstrip("+-"))
    width = 1 if depth <= 8 else 2 if depth <= 16 else 4
    order = ">" if tokens[1] == "ML" else "<"
    kind = "i" if signed else "u"
    body = raw[nl + 1 :]
    if len(body) != w * h * width:
        raise ValueError(f"PGX body is {len(body)} bytes, expected {w * h * width}")
    arr = np.frombuffer(body, dtype=f"{order}{kind}{width}").astype("int64")
    return signed, depth, arr.reshape(h, w)


def samples_sha256(raw: bytes) -> str:
    """The SHA-256 of a PGX file's samples, row-major, each a little-endian
    `i32`: the digest `tests/support/ht.rs` takes of a decoded component."""
    _, _, arr = read_pgx(raw)
    return hashlib.sha256(arr.astype("<i4").tobytes()).hexdigest()


def pgx_metrics(actual_raw: bytes, reference_raw: bytes, what: str):
    sa, da, a = read_pgx(actual_raw)
    sr, dr, r = read_pgx(reference_raw)
    if (sa, da, a.shape) != (sr, dr, r.shape):
        raise ValueError(f"{what}: {sa, da, a.shape} vs reference {sr, dr, r.shape}")
    return plane_metrics(a, r)


class OpjRefused(Exception):
    pass


def opj_planes(codestream: bytes, features: dict, count: int, reduction: int = 0):
    """OpenJPEG's decode of a bare codestream: one PGX per component, the first
    `count` of them, and the first warning it printed (or None).

    `opj_decompress` converts the output from sYCC to RGB whenever there are
    three components, the first with equal sampling factors and the second
    with a horizontal factor other than 1 (opj_decompress.c:1620-1626). Such a
    file is decoded once whole, for its status, then one component per run
    with `-c k`, which skips the inverse MCT, so it must not use one.
    """
    sub = features["subsampling"]
    sycc = len(sub) == 3 and sub[0][0] == sub[0][1] and sub[1][0] != 1
    with tempfile.TemporaryDirectory() as tmp:
        cs = os.path.join(tmp, "cs.j2k")
        with open(cs, "wb") as f:
            f.write(codestream)
        runs = [("all", [], [f"all_{k}.pgx" for k in range(count)])]
        if sycc:
            runs += [(f"c{k}", ["-c", str(k)], [f"c{k}_0.pgx"]) for k in range(count)]
        raws, warning = [], None
        for stem, extra, names in runs:
            if stem == "c0":
                if features["mct"] != 0:
                    raise ValueError("OpenJPEG would read it as sYCC, and -c skips the MCT")
                raws = []
            cmd = ["opj_decompress", "-i", cs, "-o", os.path.join(tmp, stem + ".pgx")]
            cmd += ["-r", str(reduction)] + extra
            run = subprocess.run(cmd, capture_output=True, text=True)
            text = run.stdout + run.stderr
            errors = [l.strip() for l in text.splitlines() if "[ERROR]" in l]
            if run.returncode != 0 or errors:
                raise OpjRefused(errors[0] if errors else text.strip().splitlines()[-1])
            warnings = [l.strip() for l in text.splitlines() if "[WARNING]" in l]
            warning = warning or (warnings[0] if warnings else None)
            for name in names:
                with open(os.path.join(tmp, name), "rb") as f:
                    raws.append(f.read())
    return raws, warning


def jp2c_range(data: bytes, path: str):
    """The byte range of the (only) contiguous codestream box's payload."""
    found = [(s, e) for t, s, e in read_boxes(data, 0, len(data)) if t == b"jp2c"]
    if len(found) != 1:
        raise ValueError(f"{path}: {len(found)} jp2c boxes")
    return list(found[0])


def passes_record(name: str, placeholder_bset: bool) -> dict:
    """What is known about the coding passes of `name`'s HT code-blocks."""
    measured = HT_PASSES.get(name)
    walked = HT_WALKED.get(name)
    record = {"t803_placeholder_bset": placeholder_bset}
    record.update(placeholder=None, refinement=None, measured=None, walked=None)
    if measured is not None:
        blocks, histogram, zero_refinement = measured
        if sum(histogram.values()) != blocks:
            raise ValueError(f"{name}: pass histogram does not sum to {blocks}")
        record.update(
            placeholder=max(histogram) > 3,
            refinement=max(histogram) > 1,
            measured={
                "blocks": blocks,
                "histogram": {str(k): v for k, v in sorted(histogram.items())},
                "zero_length_refinement": zero_refinement,
            },
        )
    if walked is not None:
        blocks, placeholder, histogram, zero_refinement, *part1 = walked
        if sum(histogram.values()) != blocks:
            raise ValueError(f"{name}: walked histogram does not sum to {blocks}")
        walked = {
            "blocks": blocks,
            "placeholder": placeholder,
            "histogram": {str(k): v for k, v in sorted(histogram.items())},
            "zero_length_refinement": zero_refinement,
        }
        if part1:
            walked["part1"] = part1[0]
        record.update(
            placeholder=placeholder > 0,
            refinement=max(histogram) > 1,
            walked=walked,
        )
    return record


def ht_source_path(name: str) -> str:
    """The path of HT codestream `name` in HT_REPO."""
    if name.startswith("hifi_"):
        return f"codestreams_hifi/{name}.j2k"
    ds, idx = name[:3], name[7:9]
    profile = ds[2]
    return f"htj2k_bsets_profile{profile}/p{profile}_{idx}_bset/{name}.j2k"


def ht_fetch(rel: str, dest: str):
    checkout = os.environ.get(HT_CHECKOUT_ENV)
    local = os.path.join(checkout, rel) if checkout else None
    fetch(HT_RAW % rel, dest, f"{rel} at {HT_COMMIT[:8]}", local)


def ht_read(rel: str) -> bytes:
    """A text or stats file of HT_REPO, fetched to a temporary path."""
    with tempfile.TemporaryDirectory() as tmp:
        dest = os.path.join(tmp, os.path.basename(rel))
        ht_fetch(rel, dest)
        with open(dest, "rb") as f:
            return f.read()


def check_ht_checkout():
    checkout = os.environ.get(HT_CHECKOUT_ENV)
    if not checkout:
        return
    head = subprocess.run(
        ["git", "-C", checkout, "rev-parse", "HEAD"], capture_output=True, text=True, check=True
    ).stdout.strip()
    if head != HT_COMMIT:
        raise SystemExit(f"{HT_CHECKOUT_ENV} is at {head}, not {HT_COMMIT}")


def native_stats(stats: dict | None, depth: list) -> dict | None:
    """Kakadu's stats at the components' own depth. `kdu_expand` writes the
    4-bit p0_03 and p0_15 samples as 8-bit PGM, so their errors are scaled by
    2^(8-4) in PAE and by its square in MSE."""
    if stats is None:
        return None
    shifts = [stats["depth"] - d for d in depth]
    if any(s < 0 for s in shifts):
        raise ValueError(f"stats depth {stats['depth']} below a component depth {depth}")
    return {
        "pae": [p / 2**s for p, s in zip(stats["pae"], shifts)],
        "mse": [m / 4**s for m, s in zip(stats["mse"], shifts)],
    }


def build_ht_codestream(name: str, part1: dict, stats_cache: dict) -> dict:
    rel = f"codestreams/{name}.j2k"
    path = os.path.join(HT_DIR, rel)
    ht_fetch(ht_source_path(name), path)
    with open(path, "rb") as f:
        data = f.read()
    header = ht_header(data, name)
    features, ht = header["features"], header["ht"]
    features.update(marker_features_data(data, name))
    # The derived-set names end in the bound (`_b11`); hifi_ht1_02 has none.
    filename_bound = int(name.rsplit("_b", 1)[1]) if "_b" in name else ht["magb_bound"]
    if ht["magb_bound"] != filename_bound:
        raise ValueError(f"{name}: CAP says B={ht['magb_bound']}, the name {filename_bound}")
    if ht["multiht"]:
        raise ValueError(f"{name}: MULTIHT is set")
    mixed = name.startswith("ds0_hm_")
    if (ht["coder_set"] == "MIXED") != mixed:
        raise ValueError(f"{name}: coder set {ht['coder_set']}")

    if name.startswith("hifi_"):
        source = "hifi_p1_02"
        graded, reduction = 3, 0
        references = [f"references/hifi_02-{k}.pgx" for k in range(graded)]
        for ref in references:
            ht_fetch(f"reference_class1HF_profile1/{os.path.basename(ref)}", os.path.join(HT_DIR, ref))
        pae, mse = T803_HIFI[name]
        depth = [12] * graded
        tolerance = {
            "table": "C.8",
            "part1": {"pae": T803_HIFI[source][0], "mse": T803_HIFI[source][1]},
            "additional": None,
            "reference_depth": None,
            "depth": depth,
        }
        stats = None
        placeholder_bset = False
    else:
        ds, idx = name[:3], int(name[7:9])
        profile = int(ds[2])
        source = f"p{profile}_{idx:02d}"
        p1 = part1[(profile, idx)]
        graded, reduction = p1["graded_components"], p1["reduction"]
        references = ["../" + r for r in p1["references"]["class1"]]
        depth = p1["features"]["precision"][:graded]
        p1_pae, p1_mse = T803_PART1[source]
        dref, add_pae, add_mse = T803_ADDITIONAL[name]
        if not (len(p1_pae) == len(add_pae) == graded):
            raise ValueError(f"{name}: {graded} graded, {len(p1_pae)}/{len(add_pae)} tolerances")
        pae = [p + math.ceil(a * 2.0 ** (d - dref)) for p, a, d in zip(p1_pae, add_pae, depth)]
        mse = [round(m + a * 4.0 ** (d - dref), 9) for m, a, d in zip(p1_mse, add_mse, depth)]
        pae = [int(v) for v in pae]
        mse = [int(v) if float(v).is_integer() else v for v in mse]
        tolerance = {
            "table": "C.1bis" if ds == "ds0" else "C.4bis",
            "part1": {"pae": p1_pae, "mse": p1_mse},
            "additional": {"pae": add_pae, "mse": add_mse},
            "reference_depth": dref,
            "depth": depth,
        }
        stats_rel = f"htj2k_bsets_profile{profile}/p{profile}_{idx:02d}_bset/stats.txt"
        if stats_rel not in stats_cache:
            stats_cache[stats_rel] = parse_stats(ht_read(stats_rel).decode(), stats_rel)
        stats = stats_cache[stats_rel].get((filename_bound, mixed))
        if stats is None or len(stats["pae"]) < graded:
            raise ValueError(f"{name}: no stats for {graded} components in {stats_rel}")
        stats = {"depth": stats["depth"], "pae": stats["pae"][:graded], "mse": stats["mse"][:graded]}
        placeholder_bset = (ds, idx) in T803_PLACEHOLDER_BSETS

    # Cross-check: each bound covers the Part 1 tolerance plus the transcode
    # error Kakadu measured (PAE adds; MSE adds as a variance would).
    native = native_stats(stats, depth)
    if native is not None:
        for k in range(graded):
            if pae[k] < tolerance["part1"]["pae"][k] + native["pae"][k] or (
                mse[k] < tolerance["part1"]["mse"][k] + native["mse"][k]
            ):
                raise ValueError(f"{name} comp {k}: bound {pae[k]}/{mse[k]} under part1 + stats")
    bit_exact = all(v == 0 for v in pae) and all(v == 0 for v in mse)

    oracle = {"decoded": False, "message": None, "warning": None, "metrics": None}
    try:
        raws, warning = opj_planes(data, features, graded, reduction)
    except OpjRefused as refused:
        oracle["message"] = str(refused)
    else:
        oracle.update(decoded=True, warning=warning)
        metrics = []
        for k, raw in enumerate(raws):
            with open(os.path.join(HT_DIR, references[k]), "rb") as f:
                metrics.append(pgx_metrics(raw, f.read(), f"{name} comp {k}"))
        oracle["metrics"] = {"pae": [m[0] for m in metrics], "mse": [m[1] for m in metrics]}
        for k, (p, m) in enumerate(metrics):
            if p > pae[k] or m > mse[k]:
                raise ValueError(f"{name} comp {k}: OpenJPEG is {p}/{m}, bound {pae[k]}/{mse[k]}")
        exact = all(p == 0 for p, _ in metrics)
        oracle["matches_references"] = exact
        oracle["agreement_bound"] = opj_agreement_bound(
            features["irreversible_anywhere"], max(features["precision"][:graded])
        )
        if exact:
            oracle["snapshot"] = None
        else:
            snap = []
            for k, raw in enumerate(raws):
                rel_snap = f"oracle/{name}_{k}.pgx"
                with open(os.path.join(HT_DIR, rel_snap), "wb") as f:
                    f.write(raw)
                snap.append(rel_snap)
            oracle["snapshot"] = {
                "references": snap,
                "reference_digests": [digest(os.path.join(HT_DIR, r)) for r in snap],
            }
    if not oracle["decoded"]:
        oracle.update(matches_references=None, agreement_bound=None, snapshot=None)

    print(f"  {name}: bound PAE {pae} MSE {mse}; OpenJPEG "
          + (f"{oracle['metrics']}" if oracle["decoded"] else f"refused: {oracle['message']}"))
    return {
        "name": name,
        "codestream": rel,
        "source": source,
        "graded_components": graded,
        "bit_exact": bit_exact,
        "reduction": reduction,
        "references": references,
        "bounds": {"pae": pae, "mse": mse},
        "tolerance": tolerance,
        "kakadu_stats": stats,
        "features": features,
        "ht": ht,
        "passes": passes_record(name, placeholder_bset),
        "file_digest": digest(path),
        "reference_digests": [digest(os.path.join(HT_DIR, r)) for r in references],
        "openjpeg": oracle,
    }


def jph_planes_match_jp2(n: int, payload: bytes, features: dict, jp2_entry: dict):
    """The top-bound JPH codestream must decode to the same samples as the
    codestream of fileN.jp2, plane for plane, under OpenJPEG."""
    with open(os.path.join(JP2_DIR, jp2_entry["file"]), "rb") as f:
        jp2_payload = jp2_structure(f.read())["payload"]
    count = features["components"]
    ours, _ = opj_planes(payload, features, count)
    theirs, _ = opj_planes(jp2_payload, jp2_entry["features"], count)
    for k, (a, b) in enumerate(zip(ours, theirs)):
        if a != b:
            raise ValueError(f"file{n} comp {k}: the JPH codestream decodes differently")


def build_jph_entry(n: int, bound: int, jp2_entries: dict) -> dict:
    name = f"file{n}_b{bound}"
    rel = f"jph/{name}.jph"
    path = os.path.join(HT_DIR, rel)
    ht_fetch(f"testfiles_jph/{name}.jph", path)
    with open(path, "rb") as f:
        data = f.read()
    start, end = jp2c_range(data, name)
    payload = data[start:end]
    header = ht_header(payload, name)
    features, ht = header["features"], header["ht"]
    features.update(marker_features_data(payload, name))
    if ht["magb_bound"] != bound:
        raise ValueError(f"{name}: CAP says B={ht['magb_bound']}")
    boxes = [t.decode("latin-1") for t, _, _ in read_boxes(data, 0, len(data))]

    stats_name = f"testfiles_jph/stats_{n}.txt" if n != 4 else "testfiles_jph/stats_4.ttx"
    stats_all = parse_stats(ht_read(stats_name).decode(), stats_name)
    stats = stats_all[(bound, False)]
    highest = bound == max(JPH_BOUNDS[n])
    if highest and (any(stats["pae"]) or any(stats["mse"])):
        raise ValueError(f"{name}: highest bound but Kakadu's stats are not zero")

    entry = {
        "name": name,
        "file": rel,
        "index": n,
        "magb_bound": bound,
        "highest_bound": highest,
        "codestream_range": [start, end],
        "boxes": boxes,
        "features": features,
        "ht": ht,
        "passes": passes_record(name, False),
        "kakadu_stats": stats,
        "file_digest": digest(path),
        "graded_as": None,
        "not_graded": None,
        "references": [],
        "reference_digests": [],
    }

    try:
        oracle_whole = opj_whole_status(path)
    except OpjRefused as refused:
        oracle_whole = str(refused)
    entry["openjpeg"] = {"decoded": oracle_whole is None, "message": oracle_whole}
    # OpenJPEG's decode of the bare codestream, which it decodes for every
    # file, as a digest per component: the test suite checks the crate's
    # decode against it bit for bit without OpenJPEG.
    raws, _ = opj_planes(payload, features, features["components"])
    entry["openjpeg"]["codestream_sha256"] = [samples_sha256(raw) for raw in raws]

    if not highest:
        entry["not_graded"] = (
            "a lower magnitude bound: T.803 gives no JPH tolerances (G.5), and a "
            "decoder is tested on the highest-bound file of each set (B.2)"
        )
    elif n <= 9:
        # Graded as fileN.jp2 is, against its references: the codestream is
        # lossless against fileN.jp2's, and OpenJPEG agrees.
        jp2_entry = jp2_entries[n]
        jph_planes_match_jp2(n, payload, features, jp2_entry)
        planes = [read_pgx(open(os.path.join(JP2_DIR, r), "rb").read())[2] for r in jp2_entry["references"]]
        regrade_with_opj(n, path, planes, jp2_entry["bit_exact"])
        entry["graded_as"] = f"file{n}"
    else:
        # file10: five components with references from reference_jph/. The
        # whole file defeats OpenJPEG's cdef handling, so the bare codestream
        # is checked against the references instead.
        refs = []
        for k in range(features["components"]):
            ref = f"references/jph_{n}_{k}.pgx"
            pgm = os.path.join(HT_REF_DIR, f"jph_{n}_{k}.pgm")
            ht_fetch(f"reference_jph/jph_{n}_{k}.pgm", pgm)
            try:
                write_pgx(os.path.join(HT_DIR, ref), read_pgm(pgm))
            finally:
                os.remove(pgm)
            refs.append(ref)
        raws, _ = opj_planes(payload, features, len(refs))
        for k, raw in enumerate(raws):
            with open(os.path.join(HT_DIR, refs[k]), "rb") as f:
                pae, mse = pgx_metrics(raw, f.read(), f"{name} comp {k}")
            if pae or mse:
                raise ValueError(f"{name} comp {k}: OpenJPEG's codestream decode is {pae}/{mse}")
        entry["graded_as"] = "references"
        entry["references"] = refs
        entry["reference_digests"] = [digest(os.path.join(HT_DIR, r)) for r in refs]
    print(f"  {name}: " + (entry["graded_as"] and f"graded as {entry['graded_as']}" or "not graded"))
    return entry


def opj_whole_status(path: str):
    """None when `opj_decompress` decodes the whole file, else raise."""
    with tempfile.TemporaryDirectory() as tmp:
        run = subprocess.run(
            ["opj_decompress", "-i", path, "-o", os.path.join(tmp, "out.pgx")],
            capture_output=True,
            text=True,
        )
    text = run.stdout + run.stderr
    errors = [l.strip() for l in text.splitlines() if "[ERROR]" in l]
    if run.returncode != 0 or errors:
        raise OpjRefused(errors[0] if errors else text.strip().splitlines()[-1])
    return None


def read_pgm(path: str):
    """An 8-bit binary PGM (P5, maxval 255) as a 2-D uint8 array."""
    import numpy as np

    with open(path, "rb") as f:
        data = f.read()
    tokens, pos = [], 0
    while len(tokens) < 4:
        while data[pos : pos + 1].isspace():
            pos += 1
        start = pos
        while not data[pos : pos + 1].isspace():
            pos += 1
        tokens.append(data[start:pos].decode("ascii"))
    pos += 1
    magic, w, h, maxval = tokens[0], int(tokens[1]), int(tokens[2]), int(tokens[3])
    if magic != "P5" or maxval != 255 or len(data) - pos != w * h:
        raise ValueError(f"{path}: expected an 8-bit P5 PGM, got {tokens}")
    return np.frombuffer(data[pos:], dtype="uint8").reshape(h, w)


def main_ht():
    require_openjpeg("opj_decompress")
    require_lcms()  # the top-bound JPH files are regraded with -force-rgb
    check_ht_checkout()
    for d in (HT_CS_DIR, HT_JPH_DIR, HT_REF_DIR, HT_ORACLE_DIR):
        os.makedirs(d, exist_ok=True)
    ht_fetch("COPYRIGHT.txt", os.path.join(HT_DIR, "COPYRIGHT.txt"))
    with open(os.path.join(HERE, "manifest.json")) as f:
        part1 = {(e["profile"], e["index"]): e for e in json.load(f)["entries"]}
    with open(os.path.join(JP2_DIR, "manifest.json")) as f:
        jp2_entries = {e["index"]: e for e in json.load(f)["entries"]}
    stats_cache = {}
    print("HT codestreams:")
    names = list(T803_ADDITIONAL) + ["hifi_ht1_02"]
    codestreams = [build_ht_codestream(name, part1, stats_cache) for name in names]
    print("JPH files:")
    jph = [build_jph_entry(n, b, jp2_entries) for n, bounds in JPH_BOUNDS.items() for b in bounds]
    manifest = {
        "_comment": (
            "ISO/IEC 15444-4:2024 HTJ2K conformance files. Generated by regenerate.py; "
            "do not edit by hand. See ../README.md and ./COPYRIGHT.txt."
        ),
        "compliance_class": 1,
        "provenance": {
            "files": {"repo": HT_REPO, "commit": HT_COMMIT},
            "bounds": {
                "document": "ITU-T T.803 (V3, 02/2024), the same text as ISO/IEC 15444-4:2024",
                "tables": "C.1bis, C.4bis, C.6, C.7, C.8",
                "rule": (
                    "bound = Part 1 class-1 tolerance (C.6, C.7) + the additional "
                    "tolerance (C.1bis, C.4bis), peak scaled by 2^(D - Dref) and "
                    "rounded up, MSE by 4^(D - Dref); hifi_ht1_02 from C.8 directly"
                ),
                "cross_check": (
                    "each bound is at least the Part 1 tolerance plus the error Kakadu "
                    "recorded between the transcode and its source (stats.txt), at the "
                    "component's own depth, PAE and MSE each added; and where "
                    "opj_decompress 2.5.0 decodes a codestream, its error is within the bound"
                ),
            },
            "references": (
                "the Part 1 class-1 references (../references/) for ds0 and ds1; "
                "reference_class1HF_profile1/ for hifi_ht1_02, copied verbatim; "
                "reference_jph/*.pgm for file10, written as 8-bit PGX"
            ),
            "jph_grading": (
                "the highest-bound file of each set; file1..file9 as fileN.jp2 is graded "
                "(../jp2/manifest.json), since the codestream decodes to the same samples "
                "under OpenJPEG; file10_b12 bit-exact against reference_jph"
            ),
            "passes": (
                "measured: opj_decompress 2.5.0 with a print of each HT block's pass count "
                "added to opj_t1_ht_decode_cblk, see HT_PASSES in regenerate.py; walked: "
                "this crate's Tier-2 walk of every packet header, see HT_WALKED"
            ),
            "openjpeg": (
                "opj_decompress 2.5.0 on the bare codestream; sub-sampled files with -c K; "
                "for each JPH file, codestream_sha256 is the SHA-256 of each component it "
                "decodes from the bare codestream, its samples as little-endian i32"
            ),
            "license": (
                "Conformance-only grant; see ./COPYRIGHT.txt. Excluded from the published "
                "crate via Cargo.toml `exclude`."
            ),
        },
        "codestreams": codestreams,
        "jph": jph,
    }
    with open(os.path.join(HT_DIR, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    print(f"wrote ht/manifest.json with {len(codestreams)} codestreams and {len(jph)} JPH files")


# ---------------------------------------------------------------------------
# OpenJPH test codestreams: the files under 100 KB in aous72/jp2k_test_codestreams
# `openjph/`, with OpenJPEG's decode as the oracle. See ../../openjph/.
# ---------------------------------------------------------------------------

OPENJPH_COMMIT = "1ccd349d12dced98eafabe641e5dac8c42a7c909"
OPENJPH_REPO = "https://github.com/aous72/jp2k_test_codestreams"
OPENJPH_RAW = "https://raw.githubusercontent.com/aous72/jp2k_test_codestreams/" + OPENJPH_COMMIT + "/%s"
# The OpenJPH release pinned as a regeneration tool (decision 3 of
# docs/roadmap-phase4.md). No file here was made with it.
OPENJPH_RELEASE = {"tag": "0.33.0", "commit": "345a8bcd1c7a3908e3870797057277660a412073"}
OPENJPH_DIR = os.path.join(HERE, "..", "openjph")
OPENJPH_FILES = [
    "simple_dec_irv97_64x64_16bit_gray.jph",
    "simple_dec_irv97_64x64_16bit.jph",
    "simple_dec_irv97_64x64_gray_tiles.jph",
    "simple_dec_irv97_64x64_tiles_yuv.jph",
    "simple_dec_irv97_64x64_yuv.jph",
    "simple_dec_rev53_64x64_tiles_yuv.jph",
    "simple_dec_rev53_64x64_yuv.jph",
    "simple_enc_irv97_64x64_16bit_gray.j2c",
    "simple_enc_irv97_64x64_16bit.j2c",
    "simple_enc_irv97_64x64.j2c",
    "simple_enc_irv97_64x64_yuv.j2c",
    "simple_enc_irv97_tall_narrow1.j2c",
    "simple_enc_irv97_tall_narrow.j2c",
    "simple_enc_rev53_64x64_yuv.j2c",
    "simple_enc_rev53_tall_narrow1.j2c",
    "simple_enc_rev53_tall_narrow.j2c",
]


def build_openjph_entry(filename: str) -> dict:
    rel = f"files/{filename}"
    path = os.path.join(OPENJPH_DIR, rel)
    fetch(OPENJPH_RAW % f"openjph/{filename}", path, f"{filename} at {OPENJPH_COMMIT[:8]}")
    if os.path.getsize(path) >= 100_000:
        raise ValueError(f"{filename} is not under 100 KB")
    with open(path, "rb") as f:
        data = f.read()
    stem = filename.rsplit(".", 1)[0]
    span = jp2c_range(data, stem) if filename.endswith(".jph") else [0, len(data)]
    payload = data[span[0] : span[1]]
    header = ht_header(payload, stem)
    features, ht = header["features"], header["ht"]
    features.update(marker_features_data(payload, stem))
    count = features["components"]
    raws, warning = opj_planes(payload, features, count)
    if warning is not None:
        raise ValueError(f"{stem}: OpenJPEG warned: {warning}")
    refs = []
    for k, raw in enumerate(raws):
        ref = f"references/{stem}_{k}.pgx"
        with open(os.path.join(OPENJPH_DIR, ref), "wb") as f:
            f.write(raw)
        refs.append(ref)
    bound = opj_agreement_bound(features["irreversible_anywhere"], max(features["precision"]))
    print(f"  {stem}: {count} components, agreement bound {bound}")
    return {
        "name": stem,
        "file": rel,
        "codestream_range": span,
        "graded_components": count,
        "bit_exact": bound["pae"] == 0,
        "bounds": {"pae": [bound["pae"]] * count, "mse": [bound["mse"]] * count},
        "references": refs,
        "features": features,
        "ht": ht,
        "passes": passes_record(stem, False),
        "file_digest": digest(path),
        "reference_digests": [digest(os.path.join(OPENJPH_DIR, r)) for r in refs],
    }


def main_openjph():
    require_openjpeg("opj_decompress")
    os.makedirs(os.path.join(OPENJPH_DIR, "files"), exist_ok=True)
    os.makedirs(os.path.join(OPENJPH_DIR, "references"), exist_ok=True)
    fetch(OPENJPH_RAW % "LICENSE", os.path.join(OPENJPH_DIR, "LICENSE"), "LICENSE")
    print("OpenJPH test codestreams:")
    entries = [build_openjph_entry(f) for f in OPENJPH_FILES]
    manifest = {
        "_comment": (
            "OpenJPH test codestreams under 100 KB. Generated by "
            "tests/fixtures/conformance/regenerate.py; do not edit by hand. See README.md."
        ),
        "provenance": {
            "files": {"repo": OPENJPH_REPO, "commit": OPENJPH_COMMIT, "path": "openjph/"},
            "openjph_release": OPENJPH_RELEASE,
            "references": (
                "opj_decompress 2.5.0 on the bare codestream (the jp2c payload of a .jph), "
                "one PGX per component; sub-sampled files one component at a time with -c K"
            ),
            "bounds": (
                "agreement with OpenJPEG: exact for 5/3; for 9/7 PAE 1 up to 12 bits and "
                "5 at 16 bits, the bounds of the existing irreversible fixtures"
            ),
            "license": "BSD-2-Clause; see ./LICENSE. Excluded from the published crate.",
        },
        "entries": entries,
    }
    with open(os.path.join(OPENJPH_DIR, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    print(f"wrote openjph/manifest.json with {len(entries)} entries")


def main():
    corpora = ("part1", "jp2", "ht", "openjph")
    which = sys.argv[1:] or ["part1", "jp2"]
    unknown = [w for w in which if w not in corpora]
    if unknown:
        raise SystemExit(f"usage: regenerate.py [part1] [jp2] [ht] [openjph]; got {unknown}")
    if "part1" in which:
        main_part1()
    if "jp2" in which:
        main_jp2()
    if "ht" in which:
        main_ht()
    if "openjph" in which:
        main_openjph()


if __name__ == "__main__":
    main()
