#!/usr/bin/env python3
"""Extract the raw JPEG 2000 codestream from a GRIB2 §5.40 (``grid_jpeg``) message.

A GRIB2 template 5.40 message carries its data as a raw JPEG 2000 codestream
(Annex A, no JP2 boxes) in Section 7 (the Data section). The conformance corpus
and ``scripts/gen-oracle.sh`` operate on that bare ``.j2k`` codestream, so this
script walks the GRIB2 section layout, lifts Section 7's payload out of the
chosen message, and writes it as ``<name>.j2k``.

GRIB2 layout (Edition 2): Section 0 is a fixed 16-byte indicator ending with the
8-byte total message length; Sections 1..7 each begin with a 4-byte big-endian
length and a 1-byte section number; Section 8 is the 4-byte literal ``7777``.
For ``grid_jpeg`` packing, Section 7's body (everything after its 5-byte header)
is exactly the codestream, framed by the SOC (``FF4F``) and EOC (``FFD9``)
markers.

This is a developer/oracle tool — never run at ``cargo test`` time. Pure
Python 3 standard library, no third-party packages and no eccodes dependency.
"""

from __future__ import annotations

import argparse
import sys

SOC = b"\xff\x4f"  # Start of codestream (Annex A.4.1)
EOC = b"\xff\xd9"  # End of codestream (Annex A.4.4)


def _iter_messages(data: bytes):
    """Yield ``(start, end)`` byte ranges for each GRIB2 message in ``data``."""
    offset = 0
    while True:
        start = data.find(b"GRIB", offset)
        if start < 0:
            return
        if data[start + 7] != 2:
            raise ValueError(
                f"unsupported GRIB edition {data[start + 7]} at offset {start} "
                "(only Edition 2 carries JPEG 2000)"
            )
        total_len = int.from_bytes(data[start + 8 : start + 16], "big")
        if total_len <= 0 or start + total_len > len(data):
            raise ValueError(f"bad message length {total_len} at offset {start}")
        yield start, start + total_len
        offset = start + total_len


def extract_codestream(data: bytes, message_index: int = 0) -> bytes:
    """Return the JPEG 2000 codestream from message ``message_index`` (0-based)."""
    messages = list(_iter_messages(data))
    if not messages:
        raise ValueError("no GRIB2 message found (missing 'GRIB' indicator)")
    if not 0 <= message_index < len(messages):
        raise ValueError(
            f"message index {message_index} out of range (file has {len(messages)})"
        )

    start, end = messages[message_index]
    # Skip the fixed 16-byte Section 0, then walk length-prefixed sections.
    pos = start + 16
    while pos < end:
        if data[pos : pos + 4] == b"7777":  # Section 8: end of message
            break
        section_len = int.from_bytes(data[pos : pos + 4], "big")
        section_num = data[pos + 4]
        if section_len < 5 or pos + section_len > end:
            raise ValueError(f"malformed section header at offset {pos}")
        if section_num == 7:  # Data section
            codestream = data[pos + 5 : pos + section_len]
            if not codestream.startswith(SOC):
                raise ValueError(
                    "Section 7 is not a JPEG 2000 codestream (no SOC marker); "
                    "is this a grid_jpeg / template 5.40 message?"
                )
            if not codestream.endswith(EOC):
                raise ValueError("codestream does not end with an EOC marker")
            return codestream
        pos += section_len

    raise ValueError(f"message {message_index} has no Section 7 (data)")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", help="the source .grib2 file")
    parser.add_argument(
        "-o", "--output", required=True, help="the .j2k codestream to write"
    )
    parser.add_argument(
        "--message",
        type=int,
        default=0,
        help="0-based message index to extract (default: 0, the first message)",
    )
    args = parser.parse_args(argv)

    with open(args.input, "rb") as f:
        data = f.read()
    codestream = extract_codestream(data, args.message)
    with open(args.output, "wb") as f:
        f.write(codestream)
    print(
        f"extract-grib2-codestream: wrote {len(codestream)} bytes to {args.output}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
