#!/bin/bash
# flatpak.sh <bundle>: installs the release's Flatpak bundle and gamescope's Vulkan layer
# (org.freedesktop.Platform.VulkanLayer.gamescope, as client/docs/steam-deck.md says) for run.sh's
# fp modes, in the nj-gamescope container as the user, into $W/fphome (its own HOME and user
# installation, so the NUC's own Flatpak setup is untouched; nuc-gamescope.ps1 removes it).
set -eu
W=${W:-$HOME/deck}
export HOME=$W/fphome FLATPAK_USER_DIR=$W/fphome/flatpak
mkdir -p $FLATPAK_USER_DIR
for try in 1 2 3; do
  flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo &&
    flatpak install --user -y --noninteractive flathub org.freedesktop.Platform//24.08 org.freedesktop.Platform.VulkanLayer.gamescope//24.08 >/dev/null && break
  [ $try = 3 ] && { echo "Flathub failed 3 times" >&2; exit 1; }
  sleep $((try * 20))
done
flatpak install --user -y --noninteractive --reinstall "$1" >/dev/null
flatpak run io.github.ramazankara.nightjar --version
flatpak info --show-permissions io.github.ramazankara.nightjar | grep -E '^(filesystems|sockets)='
flatpak list --user --columns=application,branch | grep -i -E 'nightjar|gamescope'
