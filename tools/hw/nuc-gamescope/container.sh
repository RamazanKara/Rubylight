#!/bin/bash
# container.sh setup|build|rm: the throwaway Arch Linux container for the NUC's gamescope runs
# (gamescope is not packaged for Debian). build compiles $W/src/client/protocol (synced by
# nuc-gamescope.ps1) as the user with the host's Rust toolchain against Arch's FFmpeg 8, into
# $W/target-arch. rm removes the container; the archlinux image stays.
set -u
W=${W:-$HOME/deck}
name=nj-gamescope
case $1 in
setup)
  docker rm -f $name >/dev/null 2>&1
  docker run -d --name $name --privileged --network host -v /dev:/dev -v $HOME:$HOME \
    --group-add $(getent group render | cut -d: -f3) --group-add $(getent group video | cut -d: -f3) \
    archlinux:latest sleep infinity >/dev/null || exit 1
  docker exec $name sh -c 'pacman -Syu --noconfirm --needed base-devel git clang cmake ninja pkgconf ffmpeg opus \
      gamescope xorg-xwayland xorg-server-xvfb vulkan-intel vulkan-tools mesa mesa-utils glslang flatpak python-evdev \
      libx11 libxext libxrandr libxcursor libxi libxss libxtst libxkbcommon wayland wayland-protocols libdecor \
      libpulse alsa-lib libusb systemd-libs libva libdrm dbus >/tmp/pacman.log 2>&1 || { tail -5 /tmp/pacman.log; exit 1; }'
  docker exec $name sh -c "useradd -u $(id -u) -M -d $HOME $(id -un) 2>/dev/null; install -d -m 700 -o $(id -u) -g $(id -g) /run/user/$(id -u); gamescope --version 2>&1 | grep -o 'gamescope version [^ ]*'"
  ;;
build)
  docker exec -u $(id -u) -e HOME=$HOME -e PATH=$HOME/.cargo/bin:/usr/bin -w $W/src/client/protocol $name \
    sh -c "CARGO_TARGET_DIR=$W/target-arch cargo build -p nightjar-desktop --release --locked -j 6 >$W/arch-build.log 2>&1" ||
    { grep -E '^error' -A5 $W/arch-build.log | head -30; exit 1; }
  echo "built $W/target-arch/release/nightjar"
  ;;
test)
  # The desktop tests in the container, with the ones that need a Vulkan device or glslang.
  docker exec -u $(id -u) -e HOME=$HOME -e PATH=$HOME/.cargo/bin:/usr/bin -w $W/src/client/protocol $name \
    sh -c "CARGO_TARGET_DIR=$W/target-arch-test cargo test -p nightjar-desktop --locked -j 6 -- pq_present --include-ignored --nocapture --test-threads 1 >$W/arch-test.log 2>&1"
  code=$?
  grep -E '^test |^test result|panicked' $W/arch-test.log | head -20
  grep -E 'pixel [0-9]|glslangValidator' $W/arch-test.log | head -8
  exit $code
  ;;
rm)
  docker rm -f $name >/dev/null && echo "removed $name"
  ;;
esac
