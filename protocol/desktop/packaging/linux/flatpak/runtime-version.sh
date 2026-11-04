#!/usr/bin/env bash
# Prints the freedesktop runtime branch (runtime-version, e.g. 25.08) of the manifest beside this
# script, for build-bundle.sh, check-bundle.sh and tools/hw/nuc-gamescope/flatpak.sh; fails
# without one.
set -euo pipefail
manifest=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/io.github.ramazankara.nightjar.yml.in
branch=$(sed -n "s/^runtime-version: *'\{0,1\}\([0-9][0-9.]*\)'\{0,1\} *\$/\1/p" "$manifest")
[[ $branch =~ ^[0-9]+\.[0-9]+$ ]] || { echo "error: no runtime-version in $manifest" >&2; exit 1; }
echo "$branch"
