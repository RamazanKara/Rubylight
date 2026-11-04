#!/usr/bin/env bash
# Builds the Flatpak bundle from the release tar.gz that package-linux.sh made:
#
#   build-bundle.sh [-o <folder>] <nightjar-desktop-<version>-linux-x86_64.tar.gz>
#
# Writes nightjar-desktop-<version>-x86_64.flatpak beside the archive (or into -o). The manifest is
# io.github.ramazankara.nightjar.yml.in with the archive itself as its source (the release it will
# be downloaded from is not published yet while the release job runs), so the bundle carries
# exactly the archive's files. The bundle names Flathub as its runtime's repository, so
# `flatpak install --user nightjar-desktop-<version>-x86_64.flatpak` fetches
# org.freedesktop.Platform 24.08 where it is missing. Needs flatpak and flatpak-builder; the
# runtime and SDK come from Flathub (added as a user remote when missing).
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
out=
while getopts "o:" opt; do
	case $opt in
		o) out=$OPTARG ;;
		*) exit 2 ;;
	esac
done
shift $((OPTIND - 1))
[[ $# == 1 ]] || { echo "usage: build-bundle.sh [-o <folder>] <tar.gz>" >&2; exit 2; }
archive=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
name=$(basename "$archive" .tar.gz)
[[ $name =~ ^nightjar-desktop-(.+)-linux-x86_64$ ]] || { echo "error: $archive is not named nightjar-desktop-<version>-linux-x86_64.tar.gz" >&2; exit 2; }
version=${BASH_REMATCH[1]}
[[ -n $out ]] || out=$(dirname "$archive")
mkdir -p "$out"
out=$(cd "$out" && pwd)
command -v flatpak >/dev/null || { echo "error: flatpak not found" >&2; exit 1; }
command -v flatpak-builder >/dev/null || { echo "error: flatpak-builder not found" >&2; exit 1; }
app_id=io.github.ramazankara.nightjar
flathub=https://dl.flathub.org/repo/flathub.flatpakrepo

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
manifest=$work/$app_id.yml
sha=$(sha256sum "$archive" | cut -d' ' -f1)
# The download source becomes the local archive; a template without that line fails here.
sed -e "s|^\( *\)url: .*\$|\1path: $archive|" -e "s/@VERSION@/$version/g" -e "s/@SHA256@/$sha/g" "$here/$app_id.yml.in" >"$manifest"
grep -q "path: $archive" "$manifest" || { echo "error: the manifest template has no url line to replace" >&2; exit 1; }
flatpak remote-add --user --if-not-exists flathub "$flathub"
flatpak-builder --user --install-deps-from=flathub --disable-rofiles-fuse --force-clean --repo="$work/repo" --state-dir="$work/state" "$work/build" "$manifest" >"$work/builder.log" 2>&1 ||
	{ tail -n 40 "$work/builder.log" >&2; exit 1; }
bundle=$out/nightjar-desktop-$version-x86_64.flatpak
flatpak build-bundle --runtime-repo="$flathub" "$work/repo" "$bundle" "$app_id"
echo "$bundle: $(du -h "$bundle" | cut -f1)"
