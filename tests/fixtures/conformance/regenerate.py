#!/usr/bin/env python3
"""Regenerate the ISO/IEC 15444-4 Part 1 conformance corpus.

Downloads the Profile-0 (`p0_*`) and Profile-1 (`p1_*`) conformance codestreams
and their `.pgx` reference decodes from `uclouvain/openjpeg-data` at a pinned
commit, derives each codestream's parameters from `opj_dump` (the OpenJPEG
oracle), and writes `manifest.json`.

This is a development tool, not part of the crate: it needs network access and
`opj_dump` on PATH, neither of which the runtime test suite requires. The
committed corpus is self-contained; rerun this only to refresh or re-pin it.

    python3 regenerate.py

The per-file peak-absolute-error (PAE) and mean-squared-error (MSE) bounds are
transcribed verbatim from OpenJPEG's `tests/conformance/CMakeLists.txt`. The
class-1 (per-component) lists are the active grading tier upstream; the class-0
lists are commented out there (they grade the first component only) and are
recorded for reference, not used as the bar. See README.md and ./COPYRIGHT.
"""

from __future__ import annotations

import os
import re
import subprocess
import json

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

PROGRESSION = {0: "LRCP", 1: "RLCP", 2: "RPCL", 3: "PCRL", 4: "CPRL"}


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


def download(rel_path: str, dest: str):
    if os.path.exists(dest) and os.path.getsize(dest) > 0:
        return
    url = RAW % rel_path
    # Download to a temp path and rename on success, so an interrupted curl
    # never leaves a truncated file that later runs would skip as "present".
    tmp = dest + ".part"
    subprocess.run(["curl", "-fsSL", "-o", tmp, url], check=True)
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
        "features": features,
        "references": {"class0": class0_refs, "class1": class1_refs},
        # Class 1 is the active per-component grading tier.
        "bounds_class1": {"pae": pae1, "mse": mse1},
        # Class 0 grades the first component only (upstream-disabled tier).
        "bounds_class0_first_component": {"pae": pae0, "mse": mse0},
        # Every p0_/p1_ entry is a bare Part-1 codestream, all in Phase 2 scope.
        "phase2_in_scope": True,
    }


def main():
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
                "tool": "opj_dump -i",
                "note": "per-codestream parameters parsed from the OpenJPEG oracle",
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


if __name__ == "__main__":
    main()
