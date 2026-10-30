#!/usr/bin/env bash
# Builds the two FFmpeg dependencies that ffmpeg-decode-only.sh needs on Linux where the
# distribution's are older than FFmpeg 8 accepts (dav1d 1.0, ffnvcodec 11.1.5.3): Ubuntu 22.04,
# which the release workflow builds on so the packages need only glibc 2.35 (the AppImage catalog
# tests on the oldest Ubuntu LTS still in standard support).
#
#   protocol/desktop/ffmpeg-deps-linux.sh <prefix>
#   PKG_CONFIG_PATH=<prefix>/lib/pkgconfig protocol/desktop/ffmpeg-decode-only.sh <ffmpeg source> <ffmpeg prefix>
#
# <prefix> gets dav1d as a shared library (ffmpeg-decode-only.sh ships it next to libavcodec)
# with its licence in share/licenses/dav1d/COPYING, and the NVIDIA codec headers
# (nv-codec-headers: headers only, NVDEC loads the driver's libraries at run time), both at the
# versions and SHA-256 below. Needs curl, gcc, meson, ninja, nasm 2.14 or newer and make.
set -euo pipefail

prefix=${1:?install prefix}
dav1d_version=1.5.4
dav1d_sha256=686616b7c69eb88d44459391ab25cac13b6647a3b288835c5784e71c1514a5c5
# The 12.1 headers, as Ubuntu 24.04's libffmpeg-nvenc-dev: NVDEC needs driver 530 or newer.
nvcodec_version=12.1.14.1
nvcodec_sha256=043e46265517855f13bcb98b3a225b2761b2a83983b1645fa1d05876f1c3a781

mkdir -p "$prefix"
prefix=$(cd "$prefix" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
curl -sSLf --retry 5 --retry-all-errors -o dav1d.tar.xz "https://downloads.videolan.org/pub/videolan/dav1d/$dav1d_version/dav1d-$dav1d_version.tar.xz"
curl -sSLf --retry 5 --retry-all-errors -o nv-codec-headers.tar.gz "https://github.com/FFmpeg/nv-codec-headers/releases/download/n$nvcodec_version/nv-codec-headers-$nvcodec_version.tar.gz"
printf '%s  %s\n' "$dav1d_sha256" dav1d.tar.xz "$nvcodec_sha256" nv-codec-headers.tar.gz | sha256sum -c -
tar -xf dav1d.tar.xz
tar -xf nv-codec-headers.tar.gz

# --libdir=lib, not the multiarch lib/x86_64-linux-gnu meson picks on Debian: one known path.
meson setup dav1d-build "dav1d-$dav1d_version" --prefix="$prefix" --libdir=lib --buildtype=release \
	--default-library=shared -Denable_tools=false -Denable_examples=false -Denable_tests=false -Denable_docs=false
meson compile -C dav1d-build ${JOBS:+-j "$JOBS"}
meson install -C dav1d-build
install -Dm644 "dav1d-$dav1d_version/COPYING" "$prefix/share/licenses/dav1d/COPYING"

make -C "nv-codec-headers-$nvcodec_version" PREFIX="$prefix" install
echo "$prefix: dav1d $dav1d_version, nv-codec-headers $nvcodec_version"
