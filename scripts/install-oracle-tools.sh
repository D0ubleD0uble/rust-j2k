#!/usr/bin/env bash
#
# Install the developer/oracle toolchain this repo uses to (re)generate
# conformance snapshots and run the extra quality gates.
#
# These tools are NOT needed to run the test suite: `cargo test` and CI work
# against the committed snapshots with none of them installed. They are only for
# regenerating oracles (opj_decompress / eccodes), inspecting headers (opj_dump),
# license/advisory gating (cargo-deny), and fuzzing (cargo-fuzz). See
# docs/development.md for versions and the macOS path.
#
# Supported automatically: Debian/Ubuntu (apt). On macOS this prints the
# Homebrew commands to run. Cargo/rustup tools install on any platform.
#
# Usage: scripts/install-oracle-tools.sh

set -euo pipefail

log() { echo "install-oracle-tools: $*"; }

have() { command -v "$1" >/dev/null 2>&1; }

install_apt_tools() {
	local pkgs=()
	have opj_decompress || pkgs+=(libopenjp2-tools)
	have grib_dump || pkgs+=(libeccodes-tools)
	if [[ ${#pkgs[@]} -gt 0 ]]; then
		log "apt-get install: ${pkgs[*]}"
		local sudo=""
		[[ $(id -u) -eq 0 ]] || sudo="sudo"
		$sudo apt-get update -qq
		$sudo apt-get install -y "${pkgs[@]}"
	else
		log "OpenJPEG + eccodes already present"
	fi
}

print_macos_tools() {
	log "on macOS, install the native tools with Homebrew:"
	echo "    brew install openjpeg eccodes"
}

install_cargo_tools() {
	if have cargo-deny; then
		log "cargo-deny already present"
	else
		log "cargo install cargo-deny"
		cargo install cargo-deny
	fi

	if have rustup; then
		log "rustup toolchain install nightly (cargo-fuzz needs nightly)"
		rustup toolchain install nightly --profile minimal
	else
		log "rustup not found — install nightly yourself for cargo-fuzz"
	fi

	if have cargo-fuzz; then
		log "cargo-fuzz already present"
	else
		log "cargo install cargo-fuzz"
		cargo install cargo-fuzz
	fi
}

case "$(uname -s)" in
Linux)
	if have apt-get; then
		install_apt_tools
	else
		log "non-apt Linux: install OpenJPEG ('opj_decompress','opj_dump') and"
		log "eccodes ('grib_dump') via your package manager, then re-run."
	fi
	;;
Darwin)
	print_macos_tools
	;;
*)
	log "unsupported OS $(uname -s); install OpenJPEG + eccodes manually."
	;;
esac

install_cargo_tools

log "verifying the toolchain is on PATH:"
missing=0
for tool in opj_dump opj_decompress grib_dump cargo-deny cargo-fuzz; do
	if have "$tool"; then
		printf '  %-16s ok\n' "$tool"
	else
		printf '  %-16s MISSING\n' "$tool"
		missing=1
	fi
done

if [[ $missing -ne 0 ]]; then
	log "some tools are still missing; see docs/development.md"
	exit 1
fi
log "all oracle tools present"
