#!/usr/bin/env bash
# Installs the Flatpak bundle in a clean system, checks `--version`, that the sandbox resolves
# every library the executable links, and that the launcher window opens under Xvfb with its
# settings in ~/.var/app (check-start.sh), that a second build installed over it with --reinstall
# replaces it and keeps those settings, then uninstalls it. The release workflow runs it as root
# in a fresh, privileged Fedora container (bubblewrap needs the namespaces):
#
#   check-bundle.sh <nightjar-desktop-<version>-<arch>.flatpak> <version> <check-start.sh>
#
# The runtime comes from Flathub, which the bundle names. Installs flatpak, Xvfb, xwininfo and xprop
# with dnf when missing.
set -euo pipefail
[[ $# == 3 ]] || { echo "usage: check-bundle.sh <bundle> <version> <check-start.sh>" >&2; exit 64; }
bundle=$1 version=$2 check_start=$3
app_id=io.github.ramazankara.nightjar
if ! command -v flatpak >/dev/null || ! command -v Xvfb >/dev/null; then
	dnf install -y -q --setopt=install_weak_deps=False flatpak xorg-x11-server-Xvfb xwininfo xprop >/dev/null
fi
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/tmp/runtime-$(id -u)}
mkdir -p -m 700 "$XDG_RUNTIME_DIR"
branch=$(bash "$(dirname "${BASH_SOURCE[0]}")/runtime-version.sh")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# The runtime from Flathub first, in three tries 20 and 40 s apart, so the bundle installs offline
# (--or-update: a retry finds it installed and does not fail on that).
for try in 1 2 3; do
	flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo &&
		flatpak install --user -y --noninteractive --or-update flathub "org.freedesktop.Platform//$branch" >/dev/null && break
	[[ $try == 3 ]] && { echo "error: the $branch runtime did not install from Flathub after 3 tries" >&2; exit 1; }
	echo "Flathub try $try failed, retrying in $((try * 20)) s" >&2
	sleep $((try * 20))
done
flatpak install --user -y --noninteractive "$bundle" >/dev/null
printed=$(flatpak run "$app_id" --version)
[[ $printed == "Nightjar $version" ]] || { echo "error: the bundle's nightjar --version printed '$printed'" >&2; exit 1; }
missing=$(flatpak run --command=sh "$app_id" -c 'ldd /app/lib/nightjar/nightjar | grep "not found"' || true)
[[ -z $missing ]] || { echo "error: libraries not found in the sandbox: $missing" >&2; exit 1; }
flatpak info --show-permissions "$app_id" | grep -q '^devices=.*all' || { echo "error: the bundle has no --device=all" >&2; exit 1; }
# check-start.sh reads the log from NIGHTJAR_CONFIG_DIR; the app runs without it, so its log is
# there only if its default settings folder is the sandbox's own one in ~/.var/app.
export NIGHTJAR_CONFIG_DIR=$HOME/.var/app/$app_id/config/nightjar
mkdir -p "$NIGHTJAR_CONFIG_DIR"
bash "$check_start" flatpak run --unset-env=NIGHTJAR_CONFIG_DIR "$app_id"
[[ -s $NIGHTJAR_CONFIG_DIR/logs/nightjar.log ]] || { echo "error: no log in $NIGHTJAR_CONFIG_DIR/logs" >&2; exit 1; }
# The update steps/steam-deck.md gives, with a second build: the installed app's files plus a
# marker, exported as a new commit and bundled like a release. --reinstall must replace the first
# (a plain install stops at "already installed") and keep ~/.var/app.
ref=$(flatpak info --show-ref "$app_id")
first=$(flatpak info --show-commit "$app_id")
deploy=$(flatpak info --show-location "$app_id")
mkdir -p "$work/next"
cp -a "$deploy/files" "$deploy/metadata" "$work/next/"
[[ -d $deploy/export ]] && cp -a "$deploy/export" "$work/next/"
chmod -R u+w "$work/next"
# An installed desktop file has the Exec flatpak wrote (`@@u %u @@`), which an install refuses in a
# bundle: back to the one a build exports.
sed -i 's/^Exec=.*/Exec=nightjar %u/' "$work/next/export/share/applications/$app_id.desktop"
echo "$first" >"$work/next/files/lib/nightjar/update-check"
flatpak build-export "$work/repo" "$work/next" "${ref##*/}" >/dev/null
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo "$work/repo" "$work/next.flatpak" "$app_id" "${ref##*/}"
echo kept >"$NIGHTJAR_CONFIG_DIR/update-check"
flatpak install --user -y --noninteractive --reinstall "$work/next.flatpak" >/dev/null
second=$(flatpak info --show-commit "$app_id")
[[ $second != "$first" ]] || { echo "error: the update left the first build ($first) installed" >&2; exit 1; }
[[ $(flatpak run --command=cat "$app_id" /app/lib/nightjar/update-check) == "$first" ]] || { echo "error: the updated app does not run the second build's files" >&2; exit 1; }
[[ $(flatpak run "$app_id" --version) == "Nightjar $version" ]] || { echo "error: the updated bundle does not start" >&2; exit 1; }
[[ $(cat "$NIGHTJAR_CONFIG_DIR/update-check" 2>/dev/null) == kept ]] || { echo "error: updating the bundle lost the settings in ~/.var/app" >&2; exit 1; }
echo "flatpak: the update from $first to $second keeps the settings"
for name in org.freedesktop.login1 org.freedesktop.NetworkManager org.freedesktop.RealtimeKit1; do
	flatpak info --show-permissions "$app_id" | grep -qxF "$name=talk" || { echo "error: the bundle cannot reach $name on the system bus" >&2; exit 1; }
done
flatpak info --show-permissions "$app_id" | grep -q 'xdg-run/gamescope-0' || { echo "error: the bundle cannot reach gamescope's socket (xdg-run/gamescope-0)" >&2; exit 1; }
grep -q '^MimeType=.*x-scheme-handler/rubylight;' "$(flatpak info --show-location "$app_id")/export/share/applications/$app_id.desktop" || { echo "error: the bundle does not take rubylight: pairing links" >&2; exit 1; }
flatpak uninstall --user -y --noninteractive "$app_id" >/dev/null
echo "flatpak: the bundle installs, starts and uninstalls"
