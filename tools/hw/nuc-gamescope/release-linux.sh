#!/bin/bash
# f2-release-linux.sh [package|bundle|all]: the release job's Linux steps on the NUC for ~/deck/src:
# package: the decode-only FFmpeg and package-linux.sh in Ubuntu 22.04 (the runner's image; the
#   PyroWave SDK is the 4.4.0 archive's, the same pin), into ~/deck/rel/dist;
# bundle: build-bundle.sh in Ubuntu 22.04 and check-bundle.sh in a clean Fedora 42, both privileged,
#   as release.yml runs them. Logs in ~/deck/rel/*.log.
set -u
d=$HOME/deck V=${V:-4.5.0} what=${1:-all}
src=$d/src/client rel=$d/rel
mkdir -p $rel/dist
if [ $what = package ] || [ $what = all ]; then
  if [ ! -s $rel/sdk/libpyrowave-shared.so ]; then
    rm -rf $rel/sdk $rel/t && mkdir -p $rel/sdk $rel/t && tar xzf $d/dist/nightjar-desktop-4.4.0-linux-x86_64.tar.gz -C $rel/t
    cp $rel/t/*/libpyrowave-shared.so $rel/sdk/ && cp $rel/t/*/pyrowave-sdk-build-info.txt $rel/sdk/build-info.txt && cp -a $rel/t/*/licenses-pyrowave $rel/sdk/licenses && rm -rf $rel/t
  fi
  docker run --rm -v $HOME:$HOME -w $src -e HOME=/root -e rel=$rel -e V=$V -e home=$HOME ubuntu:22.04 bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends ca-certificates curl xz-utils gcc g++ make git cmake meson ninja-build \
      pkg-config libclang-dev clang nasm libopus-dev libva-dev libx11-dev libxext-dev libxrandr-dev \
      libxcursor-dev libxi-dev libxss-dev libxfixes-dev libxtst-dev libxkbcommon-dev libwayland-dev \
      wayland-protocols libegl-dev libgl-dev libgles-dev libdrm-dev libgbm-dev libpulse-dev \
      libasound2-dev libpipewire-0.3-dev libudev-dev libdbus-1-dev libdecor-0-dev libvulkan-dev file patchelf >/dev/null
    as_me() { setpriv --reuid=1000 --regid=1000 --clear-groups env HOME=$home PATH=$home/.cargo/bin:$PATH "$@"; }
    if [ ! -s $rel/ffprefix/licenses-ffmpeg/SOURCE.txt ]; then
      as_me bash -euo pipefail -c "
        cd $rel && rm -rf ffbuild && mkdir ffbuild && cd ffbuild
        curl -sSLf --retry 5 --retry-all-errors -o ffmpeg.tar.xz https://ffmpeg.org/releases/ffmpeg-8.1.2.tar.xz
        echo \"464beb5e7bf0c311e68b45ae2f04e9cc2af88851abb4082231742a74d97b524c  ffmpeg.tar.xz\" | sha256sum -c -
        tar -xf ffmpeg.tar.xz
        bash $PWD/protocol/desktop/ffmpeg-deps-linux.sh $rel/ffdeps
        PKG_CONFIG_PATH=$rel/ffdeps/lib/pkgconfig bash $PWD/protocol/desktop/ffmpeg-decode-only.sh $rel/ffbuild/ffmpeg-8.1.2 $rel/ffprefix
      " >$rel/ffmpeg.log 2>&1 || { tail -30 $rel/ffmpeg.log; echo "ffmpeg FAILED"; exit 1; }
      echo "ffmpeg built"
    fi
    as_me bash protocol/desktop/package-linux.sh -f $rel/ffprefix -s $rel/sdk -v $V -o $rel/dist -t $rel/target >$rel/package.log 2>&1 ||
      { tail -40 $rel/package.log; echo "package FAILED"; exit 1; }
    echo "package built"; ls -l $rel/dist'
  echo "package exit $?"
fi
if [ $what = bundle ] || [ $what = all ]; then
  L=$src/protocol/desktop/packaging/linux
  rm -f $rel/dist/*.flatpak
  docker run --rm --privileged -v $rel/dist:/dist -v $L:/linux:ro ubuntu:22.04 bash -euo pipefail -c "
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq && apt-get install -y -qq --no-install-recommends flatpak flatpak-builder ca-certificates >/dev/null
    flatpak-builder --version
    bash /linux/flatpak/build-bundle.sh /dist/nightjar-desktop-$V-linux-x86_64.tar.gz
    chmod a+r /dist/*.flatpak" >$rel/bundle-build.log 2>&1
  echo "bundle build exit $?"; tail -n 5 $rel/bundle-build.log
  docker run --rm --privileged -v $rel/dist:/dist:ro -v $L:/linux:ro fedora:42 \
    bash /linux/flatpak/check-bundle.sh /dist/nightjar-desktop-$V-x86_64.flatpak $V /linux/check-start.sh >$rel/bundle-check.log 2>&1
  echo "bundle check exit $?"; tail -n 15 $rel/bundle-check.log
fi
