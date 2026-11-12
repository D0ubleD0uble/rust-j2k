#!/usr/bin/env bash
#
# Regenerate tests/fixtures/jp2_real/references/<stem>_<k>.pgx: one PGX per
# channel of opj_decompress's output for each vendored .jp2 that
# tests/jp2_real.rs grades against a reference.
#
# Refuses before it writes anything unless opj_decompress is the pinned
# OpenJPEG version. Replaces references/ only after every decode succeeded.
# Works from any directory.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 "$script_dir/oracle_tools.py" opj_decompress

dir="$script_dir/../tests/fixtures/jp2_real"
stems=(
	3_13bit_and_1bit
	byte
	byte_tlm_plt
	byte_with_xmp
	int16
	rgbwcmyk01_YeGeo_kakadu
	small_200ppcm
	stefan_full_rgba
)

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

for stem in "${stems[@]}"; do
	opj_decompress -i "$dir/$stem.jp2" -o "$workdir/$stem.pgx" >/dev/null
done
# Every decode succeeded, so delete the old references and copy the new ones
# in. The two steps are not atomic: a failed copy leaves the directory short.
rm -f "$dir"/references/*.pgx
cp "$workdir"/*.pgx "$dir/references/"
