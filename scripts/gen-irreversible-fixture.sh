#!/usr/bin/env bash
#
# Regenerate tests/fixtures/eta_lambert_irreversible.j2k and its snapshot:
# decode the committed lossless Eta fixture with OpenJPEG, re-encode the
# samples with the irreversible 9/7 transform, then write the oracle.
#
# Refuses before it writes anything unless opj_decompress and opj_compress are
# the pinned OpenJPEG version. Run from the repository root.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 "$script_dir/oracle_tools.py" opj_decompress opj_compress

fixtures=tests/fixtures
workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

opj_decompress -i "$fixtures/eta_lambert_lossless.j2k" -o "$workdir/eta.pgx" >/dev/null
opj_compress -i "$workdir/eta_0.pgx" -o "$fixtures/eta_lambert_irreversible.j2k" -I >/dev/null
"$script_dir/gen-oracle.sh" "$fixtures/eta_lambert_irreversible.j2k" \
	--tolerance 2.0 \
	--source "9/7 irreversible re-encode of the committed eta_lambert_lossless.j2k (NOAA Eta, Lambert 93x65, 13-bit) via opj_compress -I" \
	--notes "Synthetic 9/7: no operational producer ships lossy 9/7 JPEG2000 (HRRR/NDFD are complex-packed, ECMWF is CCSDS), so the irreversible codestream is produced by OpenJPEG from a real grid. Bound is our-vs-OpenJPEG reconstruction tolerance, not the lossy compression error."
