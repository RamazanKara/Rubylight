#!/usr/bin/env bash
# Builds the decode-only FFmpeg that the Windows package ships (README: "Building").
#
#   protocol/desktop/ffmpeg-decode-only.sh <ffmpeg source dir> <prefix>
#
# Run it in an MSYS2 UCRT64 shell (gcc, make, nasm, pkgconf, mingw-w64-ucrt-x86_64-dav1d).
# The result is libavcodec and libavutil only, with the H.264, HEVC and AV1 decoders, their
# D3D11VA hardware paths and dav1d: no encoders, muxers, filters, swscale, swresample, or
# programs. It has the same sonames as MSYS2's FFmpeg 8, so `NIGHTJAR_FFMPEG_DIR=<prefix>`
# builds nightjar against it. Opus (and its header) is copied into the prefix next to it.
set -euo pipefail

source_dir=${1:?FFmpeg source directory}
prefix=${2:?install prefix}
jobs=${JOBS:-$(nproc)}

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
  --enable-d3d11va --enable-dxva2 \
  --enable-hwaccel=h264_d3d11va,h264_d3d11va2,hevc_d3d11va,hevc_d3d11va2,av1_d3d11va,av1_d3d11va2 \
  --enable-hwaccel=h264_dxva2,hevc_dxva2,av1_dxva2
make -j"$jobs"
make install

# build.rs takes the Opus header and import library from the same prefix.
msys=$(cygpath -u "${MSYSTEM_PREFIX:-/ucrt64}")
cp -r "$msys/include/opus" "$prefix/include/"
cp "$msys/lib/libopus.dll.a" "$prefix/lib/"
cp "$msys/bin/libopus-0.dll" "$prefix/bin/"
cp "$msys/bin/libdav1d-7.dll" "$prefix/bin/"
rm -rf "$build"
