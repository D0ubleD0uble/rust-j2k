#!/usr/bin/env python3
"""Refuse to generate oracles with the wrong reference tools.

Every oracle and fixture generator in this directory calls `require_openjpeg`
before it runs OpenJPEG, so a machine with another version fails loudly
instead of writing different bytes under provenance that names 2.5.0. The
generator that goes through LittleCMS also calls `require_lcms`.

Run as `python3 scripts/oracle_tools.py opj_decompress [opj_compress ...]` to
make the same check from a shell script. Add `--lcms` to check LittleCMS too.
Exit status is non-zero, with a message on stderr, when a tool is missing or
is not the expected version.
"""

from __future__ import annotations

import ctypes
import ctypes.util
import re
import shutil
import subprocess
import sys

OPENJPEG_VERSION = "2.5.0"
LCMS_VERSION = "2.14"

_COMPILED_AGAINST = re.compile(r"compiled against openjp2 library v(\S+?)\.?\s*$", re.MULTILINE)


def opj_version(tool: str) -> str | None:
    """The openjp2 version `tool -h` says it was compiled against, or None."""
    path = shutil.which(tool)
    if path is None:
        return None
    # `-h` exits 1 in OpenJPEG 2.5; the banner is what matters.
    result = subprocess.run([path, "-h"], capture_output=True, text=True, stdin=subprocess.DEVNULL)
    found = _COMPILED_AGAINST.search(result.stdout + result.stderr)
    return found.group(1) if found else "unknown"


def require_openjpeg(*tools: str) -> str:
    """Exit unless every tool is on PATH and is OpenJPEG 2.5.0; return the version."""
    for tool in tools:
        version = opj_version(tool)
        if version is None:
            sys.exit(f"{tool} not found on PATH; run scripts/install-oracle-tools.sh "
                     f"(OpenJPEG {OPENJPEG_VERSION} is the oracle)")
        if version != OPENJPEG_VERSION:
            sys.exit(f"{tool} is OpenJPEG {version}, not {OPENJPEG_VERSION}: refusing to "
                     f"generate oracles with it (see docs/development.md)")
    return OPENJPEG_VERSION


def lcms_version_text(encoded: int) -> str:
    """`2140` as `2.14`."""
    return f"{encoded // 1000}.{encoded % 1000 // 10}"


def lcms_encoded_version() -> int:
    """`cmsGetEncodedCMMversion()` of the liblcms2 that `opj_decompress` links against.

    Falls back to the library `ctypes` finds when `ldd` does not name one.
    """
    path = None
    tool = shutil.which("opj_decompress")
    if tool is not None:
        ldd = subprocess.run(["ldd", tool], capture_output=True, text=True, stdin=subprocess.DEVNULL)
        found = re.search(r"liblcms2\S*\s+=>\s+(\S+)", ldd.stdout)
        path = found.group(1) if found else None
    path = path or ctypes.util.find_library("lcms2") or "liblcms2.so.2"
    lib = ctypes.CDLL(path)
    lib.cmsGetEncodedCMMversion.restype = ctypes.c_int
    return lib.cmsGetEncodedCMMversion()


def require_lcms(encoded: int | None = None) -> str:
    """Exit unless LittleCMS is 2.14; return the version text.

    Pass the encoded version of a library already loaded, or leave it out to
    read the one `opj_decompress` uses.
    """
    if encoded is None:
        try:
            encoded = lcms_encoded_version()
        except OSError as err:
            sys.exit(f"cannot load liblcms2 to check its version: {err}")
    version = lcms_version_text(encoded)
    if version != LCMS_VERSION:
        sys.exit(f"LittleCMS is {version}, not {LCMS_VERSION}: refusing to generate "
                 f"oracles with it (see docs/development.md)")
    return version


def main(argv: list[str]) -> None:
    lcms = "--lcms" in argv
    tools = [a for a in argv if a != "--lcms"]
    if not tools:
        sys.exit("usage: oracle_tools.py [--lcms] <opj_tool>...")
    require_openjpeg(*tools)
    if lcms:
        require_lcms()


if __name__ == "__main__":
    main(sys.argv[1:])
