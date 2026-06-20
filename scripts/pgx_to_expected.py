#!/usr/bin/env python3
"""Convert an OpenJPEG ``.pgx`` decode into a conformance ``expected.json``.

``opj_decompress -o out.pgx`` writes one ``out_<c>.pgx`` per component; for the
single-component GRIB2 subset there is exactly one. Each ``.pgx`` is an ASCII
header line followed by raw samples::

    PG <ML|LM> <+|-> <depth> <width> <height>\\n<raw samples>

where ``ML``/``LM`` is byte order (most/least significant first), ``+``/``-`` is
unsigned/signed, ``depth`` is bits per sample, and each sample occupies
``ceil(depth/8)`` bytes. This script parses that and emits the snapshot schema
the conformance harness reads (see ``tests/support/mod.rs`` and
``tests/fixtures/README.md``): ``geometry``, ``tolerance``, ``samples``,
``provenance``.

It is a developer/oracle tool — never run at ``cargo test`` time. Pure Python 3
standard library, no third-party packages.
"""

from __future__ import annotations

import argparse
import json
import sys


def parse_pgx(data: bytes) -> tuple[dict, list[int]]:
    """Parse ``.pgx`` bytes into a geometry dict and a row-major sample list."""
    newline = data.find(b"\n")
    if newline < 0:
        raise ValueError("not a .pgx file: no header newline")
    tokens = data[:newline].split()
    if not tokens or tokens[0] != b"PG":
        raise ValueError(f"not a .pgx file: header is {tokens!r}")

    # PG <endianness> [sign] <depth> <width> <height>. The sign token is
    # optional in the wild; default to unsigned when it is absent.
    endianness = tokens[1].decode()
    rest = tokens[2:]
    if rest and rest[0] in (b"+", b"-"):
        signed = rest[0] == b"-"
        rest = rest[1:]
    else:
        signed = False
    if len(rest) != 3:
        raise ValueError(f"malformed .pgx header: {tokens!r}")
    depth, width, height = (int(t) for t in rest)

    if endianness not in ("ML", "LM"):
        raise ValueError(f"unknown .pgx byte order {endianness!r}")
    if not 1 <= depth <= 32:
        raise ValueError(f"unsupported .pgx bit depth {depth}")

    byte_order = "big" if endianness == "ML" else "little"
    bytes_per_sample = (depth + 7) // 8
    body = data[newline + 1 :]
    count = width * height
    expected_len = count * bytes_per_sample
    if len(body) < expected_len:
        raise ValueError(
            f"truncated .pgx: header says {count} samples "
            f"({expected_len} bytes) but body has {len(body)}"
        )

    samples: list[int] = []
    sign_bit = 1 << (depth - 1)
    span = 1 << depth
    for i in range(count):
        start = i * bytes_per_sample
        value = int.from_bytes(body[start : start + bytes_per_sample], byte_order)
        if signed and value & sign_bit:
            value -= span  # sign-extend from `depth` bits
        samples.append(value)

    geometry = {
        "width": width,
        "height": height,
        "bit_depth": depth,
        "signed": signed,
    }
    return geometry, samples


def build_snapshot(
    geometry: dict,
    samples: list[int],
    tolerance: str,
    source: str,
    oracle_command: str,
    notes: str | None,
) -> dict:
    """Assemble the ``expected.json`` document from the parsed decode."""
    if tolerance == "exact":
        tol = {"mode": "exact"}
    else:
        tol = {"mode": "absolute", "max_abs_error": float(tolerance)}

    provenance = {"source": source, "oracle_command": oracle_command}
    if notes:
        provenance["notes"] = notes

    return {
        "geometry": geometry,
        "tolerance": tol,
        "samples": samples,
        "provenance": provenance,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pgx", required=True, help="the .pgx file to convert")
    parser.add_argument(
        "--tolerance",
        default="exact",
        help='"exact" (reversible) or an absolute float bound (irreversible)',
    )
    parser.add_argument(
        "--source", required=True, help="where the codestream came from"
    )
    parser.add_argument(
        "--oracle-command",
        required=True,
        help="exact command that regenerates this snapshot",
    )
    parser.add_argument("--notes", default=None, help="optional free-form note")
    parser.add_argument(
        "-o", "--output", default=None, help="output path (default: stdout)"
    )
    args = parser.parse_args(argv)

    with open(args.pgx, "rb") as f:
        geometry, samples = parse_pgx(f.read())

    snapshot = build_snapshot(
        geometry,
        samples,
        args.tolerance,
        args.source,
        args.oracle_command,
        args.notes,
    )
    text = json.dumps(snapshot, indent=2) + "\n"

    if args.output:
        with open(args.output, "w") as f:
            f.write(text)
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
