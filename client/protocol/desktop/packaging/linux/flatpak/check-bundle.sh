#!/usr/bin/env bash
# Installs the Flatpak bundle in a clean system, checks `--version`, that the sandbox resolves
# every library the executable links, and that the launcher window opens under Xvfb with its
# settings in ~/.var/app (check-start.sh), then uninstalls it. The release workflow runs it as root
# in a fresh, privileged Fedora container (bubblewrap needs the namespaces):
#
#   check-bundle.sh <nightjar-desktop-<version>-x86_64.flatpak> <version> <check-start.sh>
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
# The runtime from Flathub first, in three tries 20 and 40 s apart, so the bundle installs offline.
for try in 1 2 3; do
	flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo &&
		flatpak install --user -y --noninteractive flathub org.freedesktop.Platform//24.08 >/dev/null && break
	[[ $try == 3 ]] && { echo "error: the 24.08 runtime did not install from Flathub after 3 tries" >&2; exit 1; }
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
# The update steps/steam-deck.md gives: a plain install of a bundle over an installed one fails
# ("already installed"), --reinstall replaces it and keeps the settings in ~/.var/app.
echo kept >"$NIGHTJAR_CONFIG_DIR/update-check"
flatpak install --user -y --noninteractive --reinstall "$bundle" >/dev/null
[[ $(flatpak run "$app_id" --version) == "Nightjar $version" ]] || { echo "error: the reinstalled bundle does not start" >&2; exit 1; }
[[ $(cat "$NIGHTJAR_CONFIG_DIR/update-check" 2>/dev/null) == kept ]] || { echo "error: reinstalling the bundle lost the settings in ~/.var/app" >&2; exit 1; }
flatpak info --show-permissions "$app_id" | grep -q 'xdg-run/gamescope-0' || { echo "error: the bundle cannot reach gamescope's socket (xdg-run/gamescope-0)" >&2; exit 1; }
flatpak uninstall --user -y --noninteractive "$app_id" >/dev/null
echo "flatpak: the bundle installs, starts and uninstalls"
