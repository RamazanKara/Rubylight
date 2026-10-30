#!/usr/bin/env bash
# Builds the decode-only FFmpeg that the Windows and Linux packages ship (README: "Building").
#
#   protocol/desktop/ffmpeg-decode-only.sh <ffmpeg source dir> <prefix>
#
# Windows: run it in an MSYS2 UCRT64 shell (gcc, make, nasm, pkgconf, mingw-w64-ucrt-x86_64-dav1d).
# Linux: gcc, make, nasm, pkg-config, libdav1d-dev, libva-dev and libffmpeg-nvenc-dev (Debian and
# Ubuntu names; nv-codec-headers elsewhere).
# The result is libavcodec and libavutil only, with the H.264, HEVC and AV1 decoders, their
# hardware paths (D3D11VA and DXVA2 on Windows; VAAPI, NVDEC and the V4L2 memory-to-memory
# decoders on Linux) and dav1d: no encoders, muxers,
# filters, swscale, swresample, or programs. It has the same sonames as the distributions'
# FFmpeg 8, so `NIGHTJAR_FFMPEG_DIR=<prefix>` builds nightjar against it. On Windows, Opus (and its
# header) is copied into the prefix next to it. On Linux the libraries get the run path $ORIGIN, so
# libavcodec finds libavutil and libdav1d (copied into the prefix) in the folder it is in, and the
# prefix's lib/ is what the package ships.
set -euo pipefail

source_dir=${1:?FFmpeg source directory}
prefix=${2:?install prefix}
jobs=${JOBS:-$(nproc)}

if [[ $(uname -s) == Linux ]]; then
  linux=1
  # The libraries get the run path $ORIGIN from LD_RUN_PATH (the linker uses it when no -rpath is
  # given; configure and make would expand a $ORIGIN in the flags).
  export LD_RUN_PATH='$ORIGIN'
  # The hardware decoders moonlight-qt and Moonlight V+ try on Linux: VAAPI, then the V4L2
  # memory-to-memory decoders of ARM handhelds and boards (H.264 and HEVC), then NVDEC. NVDEC
  # needs only the ffnvcodec headers at build time and loads the NVIDIA driver's libraries when it
  # runs, so nothing nonfree is linked and machines without NVIDIA lose nothing.
  platform=(
    --enable-vaapi
    --enable-hwaccel=h264_vaapi,hevc_vaapi,av1_vaapi
    --enable-v4l2-m2m
    --enable-decoder=h264_v4l2m2m,hevc_v4l2m2m
    --enable-ffnvcodec --enable-cuda --enable-nvdec
    --enable-hwaccel=h264_nvdec,hevc_nvdec,av1_nvdec
  )
else
  linux=
  platform=(
    --enable-d3d11va --enable-dxva2
    --enable-hwaccel=h264_d3d11va,h264_d3d11va2,hevc_d3d11va,hevc_d3d11va2,av1_d3d11va,av1_d3d11va2
    --enable-hwaccel=h264_dxva2,hevc_dxva2,av1_dxva2
  )
fi

build=$(mktemp -d)
cd "$build"
"$source_dir/configure" \
  --prefix="$prefix" \
  --enable-shared --disable-static \
  --disable-everything --disable-autodetect \
  --disable-programs --disable-doc --disable-debug \
  --disable-avdevice --disable-avformat --disable-avfilter \
  --disable-swscale --disable-swresample --disable-network \
  --enable-avcodec --enable-avutil \
  --enable-decoder=h264,hevc,av1 --enable-parser=h264,hevc,av1 \
  --enable-libdav1d --enable-decoder=libdav1d \
  "${platform[@]}"
make -j"$jobs"
make install

# The licences the package carries (FFmpeg is LGPL, dynamically linked; dav1d is BSD) and where
# the FFmpeg source is (LGPL source offer): the prefix's licenses-ffmpeg/ and licenses-dav1d/.
mkdir -p "$prefix/licenses-ffmpeg" "$prefix/licenses-dav1d"
cp "$source_dir/COPYING.LGPLv2.1" "$source_dir/LICENSE.md" "$prefix/licenses-ffmpeg/"
ffmpeg_release=$(cat "$source_dir/RELEASE")
cat >"$prefix/licenses-ffmpeg/SOURCE.txt" <<EOF
FFmpeg $ffmpeg_release, LGPL v2.1 or later, linked dynamically (libavcodec and libavutil).
Source: https://ffmpeg.org/releases/ffmpeg-$ffmpeg_release.tar.xz
SHA-256: ${FFMPEG_SHA256:-not recorded}
Built unmodified by protocol/desktop/ffmpeg-decode-only.sh of the Nightjar repository
(https://github.com/RamazanKara/nightjar), which has the configure options.
EOF

if [[ -n $linux ]]; then
  for lib in "$prefix"/lib/lib*.so.*.*; do
    readelf -d "$lib" | grep -Eq '(RPATH|RUNPATH).*\[\$ORIGIN\]' || { echo "${lib##*/} has no run path \$ORIGIN" >&2; exit 1; }
  done
  # libavcodec needs dav1d at run time: ship the one it was built against, next to it (a copy
  # from an earlier run would hide the system's from ldd).
  rm -f "$prefix"/lib/libdav1d.so*
  dav1d=$(ldd "$prefix/lib/libavcodec.so" | sed -n 's/^[[:space:]]*libdav1d\.so[^ ]* => \([^ ]*\) .*/\1/p')
  [ -n "$dav1d" ] || { echo "libavcodec.so does not link libdav1d (apt install libdav1d-dev)" >&2; exit 1; }
  cp -L "$dav1d" "$prefix/lib/"
  # dav1d's licence: the copyright file of the Debian package that owns the library, or Arch's.
  owner=$(dpkg -S "$(readlink -f "$dav1d")" 2>/dev/null | head -n1 | cut -d: -f1) || true
  if [[ -n $owner && -f /usr/share/doc/$owner/copyright ]]; then
    cp "/usr/share/doc/$owner/copyright" "$prefix/licenses-dav1d/COPYING"
  elif [[ -f /usr/share/licenses/dav1d/COPYING ]]; then
    cp /usr/share/licenses/dav1d/COPYING "$prefix/licenses-dav1d/COPYING"
  else
    echo "no licence text for $dav1d (/usr/share/doc/<package>/copyright or /usr/share/licenses/dav1d/COPYING)" >&2
    exit 1
  fi
  cd /
  rm -rf "$build"
  exit 0
fi

# build.rs takes the Opus header and import library from the same prefix.
msys=$(cygpath -u "${MSYSTEM_PREFIX:-/ucrt64}")
cp -r "$msys/include/opus" "$prefix/include/"
cp "$msys/lib/libopus.dll.a" "$prefix/lib/"
cp "$msys/bin/libopus-0.dll" "$prefix/bin/"
# The DLL's soname number changes with dav1d's ABI (libdav1d-7.dll today), so find it.
dav1d=$(ls "$msys"/bin/libdav1d-*.dll 2>/dev/null | head -n 1)
[ -n "$dav1d" ] || { echo "no libdav1d-*.dll in $msys/bin (pacman -S mingw-w64-ucrt-x86_64-dav1d)" >&2; exit 1; }
cp "$dav1d" "$prefix/bin/"
[ -f "$msys/share/licenses/dav1d/COPYING" ] || { echo "no $msys/share/licenses/dav1d/COPYING" >&2; exit 1; }
cp "$msys/share/licenses/dav1d/COPYING" "$prefix/licenses-dav1d/"
rm -rf "$build"
