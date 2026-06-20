#!/usr/bin/env bash
#
# Generate a conformance oracle snapshot for one JPEG 2000 codestream.
#
# Decodes <input>.j2k with OpenJPEG's opj_decompress (the reference decoder) and
# writes the sibling <input>.expected.json the conformance harness compares
# against (samples + geometry + tolerance + provenance). It echoes the exact
# command it ran so the snapshot's provenance is captured, per docs/correctness.md.
#
# This is a developer/oracle tool. It is NOT run at `cargo test` time — the
# committed snapshots are the contract, and CI never invokes it. See
# docs/development.md for the toolchain and scripts/install-oracle-tools.sh.
#
# Usage:
#   scripts/gen-oracle.sh <input.j2k> [options]
#
# Options:
#   --tolerance <exact|FLOAT>  exact for reversible 5/3 (default);
#                              an absolute bound for irreversible 9/7.
#   --source <STRING>          provenance: where the codestream came from
#                              (default: the input path).
#   --notes <STRING>          provenance: optional free-form note.
#   -o, --output <PATH>        output path (default: <input>.expected.json).
#
# GRIB2 note: a §5.40 (grid_jpeg) message embeds a raw codestream; extract that
# codestream first and pass it here. opj_decompress yields the raw integer
# samples our decoder emits. eccodes' grib_get_data yields *scaled* geophysical
# values, not those integers, so it is a higher-level cross-check, not this
# sample-level oracle (see docs/development.md).

set -euo pipefail

die() {
	echo "gen-oracle: $*" >&2
	exit 1
}

usage() {
	cat <<'EOF'
Generate a conformance oracle snapshot for one JPEG 2000 codestream.

Usage:
  scripts/gen-oracle.sh <input.j2k> [options]

Options:
  --tolerance <exact|FLOAT>  exact for reversible 5/3 (default);
                             an absolute bound for irreversible 9/7.
  --source <STRING>          provenance: where the codestream came from
                             (default: the input path).
  --notes <STRING>           provenance: optional free-form note.
  -o, --output <PATH>        output path (default: <input>.expected.json).
  -h, --help                 show this help.

Decodes the input with opj_decompress and writes the sibling
<input>.expected.json the conformance harness compares against. See
docs/development.md for the toolchain and the GRIB2 sample-mapping note.
EOF
}

input=""
tolerance="exact"
source=""
notes=""
output=""

while [[ $# -gt 0 ]]; do
	case "$1" in
	--tolerance)
		tolerance="${2:?--tolerance needs a value}"
		shift 2
		;;
	--source)
		source="${2:?--source needs a value}"
		shift 2
		;;
	--notes)
		notes="${2:?--notes needs a value}"
		shift 2
		;;
	-o | --output)
		output="${2:?--output needs a value}"
		shift 2
		;;
	-h | --help)
		usage
		exit 0
		;;
	-*)
		die "unknown option: $1"
		;;
	*)
		[[ -z "$input" ]] || die "only one input may be given (got '$input' and '$1')"
		input="$1"
		shift
		;;
	esac
done

[[ -n "$input" ]] || die "no input codestream given (try --help)"
[[ -f "$input" ]] || die "input not found: $input"
command -v opj_decompress >/dev/null 2>&1 ||
	die "opj_decompress not found — run scripts/install-oracle-tools.sh (see docs/development.md)"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
converter="$script_dir/pgx_to_expected.py"
[[ -f "$converter" ]] || die "missing converter: $converter"

[[ -n "$source" ]] || source="$input"
[[ -n "$output" ]] || output="${input%.*}.expected.json"

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT
pgx_base="$workdir/oracle.pgx"

# The exact reference-decode command, recorded as provenance in the snapshot.
oracle_command="opj_decompress -i $input -o $(basename "$pgx_base")"
echo "gen-oracle: running: $oracle_command"
opj_decompress -i "$input" -o "$pgx_base" >/dev/null

# opj_decompress writes one <base>_<component>.pgx per component; the
# single-component subset yields exactly one. Refuse anything else loudly.
mapfile -t pgx_files < <(ls "$workdir"/oracle_*.pgx 2>/dev/null || true)
[[ ${#pgx_files[@]} -gt 0 ]] || die "opj_decompress produced no .pgx output"
[[ ${#pgx_files[@]} -eq 1 ]] ||
	die "expected a single component, got ${#pgx_files[@]} .pgx files (multi-component is out of subset)"

converter_args=(
	--pgx "${pgx_files[0]}"
	--tolerance "$tolerance"
	--source "$source"
	--oracle-command "$oracle_command"
	-o "$output"
)
[[ -n "$notes" ]] && converter_args+=(--notes "$notes")

python3 "$converter" "${converter_args[@]}"
echo "gen-oracle: wrote $output"
