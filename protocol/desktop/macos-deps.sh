#!/usr/bin/env bash
# Build the five dylibs shipped inside the Mac app. The last stdout line is their prefix.
set -euo pipefail
cache=${1:?usage: macos-deps.sh <absolute build cache> [arm64|x86_64]}
export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-13.0}
host=$(uname -m)
arch=${2:-$host}
case "$arch" in
  arm64) cpu=aarch64 ;;
  x86_64) cpu=x86_64 ;;
  *) echo "Unsupported macOS architecture: $arch" >&2; exit 1 ;;
esac
stamp=$(shasum -a 256 "$0" | cut -c1-12)
dav1d_asm=()
ffmpeg_asm=()
if [[ "$arch" == x86_64 ]] && ! command -v nasm >/dev/null; then
  # Installing NASM later must select a new prefix with the SIMD decoders.
  stamp="$stamp-noasm"
  dav1d_asm=(-Denable_asm=false)
  ffmpeg_asm=(--disable-x86asm)
fi
work="$cache/$arch-$MACOSX_DEPLOYMENT_TARGET-$stamp"
prefix="$work/prefix"
if [[ -f "$prefix/.complete" ]]; then
  echo "$prefix"
  exit 0
fi
jobs=${JOBS:-$(getconf _NPROCESSORS_ONLN)}
for tool in cmake meson ninja pkg-config; do
  command -v "$tool" >/dev/null || { echo "$tool is required to build Nightjar.app" >&2; exit 1; }
done
mkdir -p "$cache/downloads" "$work" "$prefix"
fetch() {
  local name=$1 url=$2 sha=$3 file
  file="$cache/downloads/${url##*/}"
  if ! echo "$sha  $file" | shasum -a 256 --check --status 2>/dev/null; then
    # A reset connection is retried too (plain --retry only retries timeouts and 5xx answers).
    curl -fLsS --retry 4 --retry-all-errors --retry-delay 3 "$url" -o "$file.part.$$"
    echo "$sha  $file.part.$$" | shasum -a 256 --check --status
    mv "$file.part.$$" "$file"
  fi
  mkdir -p "$work/$name"
  tar -xf "$file" -C "$work/$name" --strip-components=1
}
fetch SDL https://github.com/libsdl-org/SDL/releases/download/release-3.4.20/SDL3-3.4.20.tar.gz f3bb06f55bed9557227a700af2ea62e49497ebdf58037732a731a5fb3ef8581f
fetch ffmpeg https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz 8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
fetch opus https://ftp.osuosl.org/pub/xiph/releases/opus/opus-1.6.1.tar.gz 6ffcb593207be92584df15b32466ed64bbec99109f007c82205f0194572411a1
fetch dav1d https://code.videolan.org/videolan/dav1d/-/archive/1.5.4/dav1d-1.5.4.tar.bz2 2abfb0c89212e6e4733a54e0ae509ec00a5b845a6360946f918806e14aedb011

cmake_cross=()
meson_cross=()
ffmpeg_cross=()
if [[ "$arch" != "$host" ]]; then
  cmake_cross=(-DCMAKE_SYSTEM_NAME=Darwin "-DCMAKE_SYSTEM_PROCESSOR=$arch")
  # Meson needs a target machine for dav1d's CPUID and assembly selection.
  cat > "$work/darwin.ini" <<EOF
[binaries]
c = ['clang', '-arch', '$arch', '-mmacosx-version-min=$MACOSX_DEPLOYMENT_TARGET']
ar = 'ar'
strip = 'strip'
[host_machine]
system = 'darwin'
cpu_family = '$cpu'
cpu = '$cpu'
endian = 'little'
[properties]
needs_exe_wrapper = true
EOF
  meson_cross=(--cross-file "$work/darwin.ini")
  ffmpeg_cross=(--enable-cross-compile --target-os=darwin "--arch=$cpu" --cc=clang)
fi
cmake_library() {
  local name=$1
  shift
  cmake -S "$work/$name" -B "$work/$name-build" -G Ninja -DCMAKE_BUILD_TYPE=Release \
    "-DCMAKE_C_FLAGS=-ffile-prefix-map=\"$work\"=/nightjar-media" \
    -DCMAKE_OSX_ARCHITECTURES="$arch" ${cmake_cross[@]+"${cmake_cross[@]}"} \
    -DCMAKE_INSTALL_PREFIX="$prefix" -DCMAKE_INSTALL_LIBDIR=lib \
    -DCMAKE_OSX_DEPLOYMENT_TARGET="$MACOSX_DEPLOYMENT_TARGET" -DCMAKE_INSTALL_NAME_DIR="$prefix/lib" "$@"
  cmake --build "$work/$name-build" --parallel "$jobs"
  cmake --install "$work/$name-build"
}
cmake_library SDL -DSDL_SHARED=ON -DSDL_STATIC=OFF -DSDL_FRAMEWORK=OFF -DSDL_TEST_LIBRARY=OFF \
  -DSDL_TESTS=OFF -DSDL_EXAMPLES=OFF
cmake_library opus -DBUILD_SHARED_LIBS=ON -DOPUS_BUILD_PROGRAMS=OFF -DOPUS_BUILD_TESTING=OFF

export CFLAGS="-arch $arch -mmacosx-version-min=$MACOSX_DEPLOYMENT_TARGET"
export LDFLAGS="$CFLAGS"
meson setup "$work/dav1d-build" "$work/dav1d" --prefix "$prefix" --libdir lib \
  --buildtype release -Denable_tools=false -Denable_tests=false ${meson_cross[@]+"${meson_cross[@]}"} ${dav1d_asm[@]+"${dav1d_asm[@]}"}
meson install -C "$work/dav1d-build"

export PKG_CONFIG_LIBDIR="$prefix/lib/pkgconfig"
unset PKG_CONFIG_PATH
mkdir -p "$work/ffmpeg-build"
(
  cd "$work/ffmpeg-build"
  # FFmpeg includes its configure arguments in the dylib. Its prefix environment
  # variable selects the install directory without embedding the build machine's path.
  export prefix
  "$work/ffmpeg/configure" --enable-shared --disable-static --disable-everything \
    --disable-autodetect --disable-programs --disable-doc --disable-avdevice --disable-avfilter \
    --disable-avformat --disable-swscale --disable-swresample --disable-network --enable-pic \
    --enable-avcodec --enable-avutil --enable-videotoolbox --enable-libdav1d \
    --enable-decoder=h264,hevc,av1,libdav1d --enable-parser=h264,hevc,av1 \
    --enable-hwaccel=h264_videotoolbox,hevc_videotoolbox,av1_videotoolbox \
    --extra-cflags="$CFLAGS" --extra-ldflags="$LDFLAGS" ${ffmpeg_cross[@]+"${ffmpeg_cross[@]}"} ${ffmpeg_asm[@]+"${ffmpeg_asm[@]}"}
  make -j"$jobs"
  make install
)
mkdir -p "$prefix/licenses"/{SDL,FFmpeg,Opus,dav1d}
cp "$work/SDL/LICENSE.txt" "$prefix/licenses/SDL/"
cp "$work/ffmpeg/COPYING.LGPLv2.1" "$work/ffmpeg/LICENSE.md" "$prefix/licenses/FFmpeg/"
cp "$work/opus/COPYING" "$prefix/licenses/Opus/"
cp "$work/dav1d/COPYING" "$prefix/licenses/dav1d/"
touch "$prefix/.complete"
echo "$prefix"
