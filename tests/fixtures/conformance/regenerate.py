#!/usr/bin/env python3
"""Regenerate the ISO/IEC 15444-4 Part 1 conformance corpus.

Downloads the Profile-0 (`p0_*`) and Profile-1 (`p1_*`) conformance codestreams
and their `.pgx` reference decodes from `uclouvain/openjpeg-data` at a pinned
commit, derives each codestream's parameters from `opj_dump` (the OpenJPEG
oracle), and writes `manifest.json`.

This is a development tool, not part of the crate: it needs network access and
`opj_dump` on PATH, neither of which the runtime test suite requires. The
committed corpus is self-contained; rerun this only to refresh or re-pin it.

    python3 regenerate.py          # both corpora
    python3 regenerate.py part1    # the Part 1 codestreams only
    python3 regenerate.py jp2      # the JP2 reader files only (see jp2/)

The JP2 half also needs `Pillow` (and numpy) to read the TIFF references; see
README.md. The committed corpus never needs them.

The per-file peak-absolute-error (PAE) and mean-squared-error (MSE) bounds are
transcribed verbatim from OpenJPEG's `tests/conformance/CMakeLists.txt`. The
class-1 (per-component) lists are the active grading tier upstream; the class-0
lists are commented out there (they grade the first component only) and are
recorded for reference, not used as the bar. See README.md and ./COPYRIGHT.
"""

from __future__ import annotations

import hashlib
import os
import re
import subprocess
import json
import sys
import tempfile

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
    0xFF51: "SIZ",
    0xFF52: "COD",
    0xFF53: "COC",
    0xFF55: "TLM",
    0xFF57: "PLM",
    0xFF58: "PLT",
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
    url = RAW % rel_path
    # Download to a temp path and rename on success, so an interrupted curl
    # never leaves a truncated file that later runs would skip as "present".
    tmp = dest + ".part"
    subprocess.run(["curl", "-fsSL", "-o", tmp, url], check=True)
    if os.path.exists(dest):
        same = sha256_file(tmp) == sha256_file(dest)
        os.remove(tmp)
        if not same:
            raise ValueError(f"{dest} differs from {rel_path} at {DATA_COMMIT[:8]}")
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
# Greyscale files carry three identical channels, of which one PGX plane is kept.
# ---------------------------------------------------------------------------

JP2_DIR = os.path.join(HERE, "jp2")
JP2_FILES_DIR = os.path.join(JP2_DIR, "files")
JP2_REF_DIR = os.path.join(JP2_DIR, "references")

JP2_COUNT = 9
# Greyscale: the TIFF reference has three identical channels; keep one plane.
JP2_GREY = {4, 6, 8}
# Graded bit-exactly (measured: opj_decompress -p 8S matches the reference).
JP2_BIT_EXACT = {1, 4, 6, 9}
# Output depth scaling: `opj_decompress -p 8S` right-shifts to 8 bits.
JP2_SHIFT_RIGHT = {6: 4, 7: 8}
# The reference is sRGB display output the decoder must be asked for (ICC
# profile, sYCC, or palette+cdef-driven channel order).
JP2_NEEDS_SRGB = {2, 3, 5, 7, 8}
# The needs-sRGB files that decode as coded today. Each gets an as-coded
# reference per channel: the codestream samples at the native component size
# and depth, no colour conversion, no shift, in the order the file's `cdef`
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
    """Box layout, colour specification, and the jp2c payload of one JP2 file."""
    top, jp2h, colour, payload, cdef = [], [], None, None, None
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
                if ctype == b"colr" and colour is None:  # the first colr wins
                    meth = data[cs]
                    colour = {"method": meth}
                    if meth == 1:
                        colour["enum_cs"] = int.from_bytes(data[cs + 3 : cs + 7], "big")
                    elif meth == 2:
                        colour["icc_bytes"] = ce - (cs + 3)
        elif tbox == b"jp2c":
            payload = data[s:e]
    if payload is None or colour is None:
        raise ValueError("no jp2c or no colr box")
    return {
        "boxes": {"top": top, "jp2h": jp2h},
        "colour": colour,
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
    # A greyscale decode may come back as one channel or three identical ones;
    # either way it is graded against the single kept plane.
    want = planes if len(planes) == len(got) else [planes[0]] * len(got)
    if len(planes) == 1 and len(got) == 3:
        for c in (1, 2):
            if not (got[0] == got[c]).all():
                raise ValueError(f"file{n}: opj_decompress grey channel {c} differs from 0")
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
    `p`. Only the simple form is accepted (every entry a colour, each component
    and each colour once), where OpenJPEG's swaps put colour `a` at `a - 1`."""
    if sorted(cn for cn, _, _ in cdef) != list(range(count)) or sorted(
        asoc for _, _, asoc in cdef
    ) != list(range(1, count + 1)) or any(typ != 0 for _, typ, _ in cdef):
        raise ValueError(f"file{n}: cdef {cdef} is not a plain colour permutation")
    order = [0] * count
    for cn, _, asoc in cdef:
        order[asoc - 1] = cn
    return order


def opj_cdef_check(n: int, data: bytes, info: dict, raws: list, tmp: str) -> None:
    """Hold the permuted planes to OpenJPEG's own `cdef` handling: decode the
    `.jp2` whole with its first `colr` rewritten to sRGB (enum 16), which stops
    `opj_decompress` converting sYCC, and require the same planes."""
    if info["colour"].get("method") != 1:
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

    The `jp2c` payload is decoded as a bare codestream, so no box-layer colour
    handling (ICC, palette) can touch the samples; the file must have no
    palette. A `cdef` only reorders the channels: the planes are put in its
    order (see `cdef_order`), and that order is checked against OpenJPEG
    decoding the file itself with its colour space rewritten to sRGB
    (`opj_cdef_check`). Two OpenJPEG behaviours decide how:

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
    if n in JP2_GREY:
        for c in (1, 2):
            if not (channels[0] == channels[c]).all():
                raise ValueError(f"jp2_{n}.tif: grey channel {c} differs from channel 0")
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
        "colour": info["colour"],
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
                    "PGX plane per channel; greyscale files (4, 6, 8) keep one plane "
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
                    "colour conversion and no shift. -c avoids opj_decompress's sYCC "
                    "heuristic but skips the inverse MCT, so it is used only for the "
                    "sub-sampled file (mct 0); the bare codestream carries no ICC"
                ),
            },
            "features": {
                "tool": "opj_dump -i + direct marker-segment walk of the jp2c payload",
                "note": "boxes and colour are read from the JP2 box layer directly",
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


def main():
    which = sys.argv[1:] or ["part1", "jp2"]
    unknown = [w for w in which if w not in ("part1", "jp2")]
    if unknown:
        raise SystemExit(f"usage: regenerate.py [part1] [jp2]; got {unknown}")
    if "part1" in which:
        main_part1()
    if "jp2" in which:
        main_jp2()


if __name__ == "__main__":
    main()
