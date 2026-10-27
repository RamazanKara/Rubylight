#!/usr/bin/env bash
# Build FFmpeg's AC3 and E-AC3 codecs (avcodec-62.dll, avutil-60.dll) as an LGPL
# shared library with MSYS2 UCRT64 MinGW-w64, for the host's audio passthrough.
#
# Usage (MSYS2 UCRT64 shell):
#   rust/tools/build_ffmpeg.sh tarball prefix
#     tarball  the official ffmpeg-8.1.2.tar.xz from https://ffmpeg.org/releases/,
#              already checked against its SHA-256 by build.ps1
#     prefix   install prefix
#
# Environment:
#   FFMPEG_WORKDIR  sources + build tree (default: $PWD/ffmpeg-work)
#   FFMPEG_JOBS     parallel jobs (default: 4)
#
# Needs gcc, make and nasm from UCRT64. The configuration enables nothing but
# the AC3 and E-AC3 encoders and decoders and the AC3 parser, and never
# --enable-gpl or --enable-nonfree, so the DLLs stay LGPL 2.1 or later. The
# prefix gets bin/avcodec-62.dll and bin/avutil-60.dll, include/, and
# share/licenses/ffmpeg (COPYING.LGPLv2.1, LICENSE.md and the configure line).
# A prefix that already holds the same source and configuration is kept.

set -euo pipefail

VERSION=8.1.2
TARBALL=${1:?usage: build_ffmpeg.sh tarball prefix}
PREFIX=${2:?usage: build_ffmpeg.sh tarball prefix}
WORKDIR=${FFMPEG_WORKDIR:-$PWD/ffmpeg-work}

if [[ "${MSYSTEM:-}" != "UCRT64" ]]; then
	echo "error: run this from an MSYS2 UCRT64 shell (MSYSTEM=UCRT64), got '${MSYSTEM:-unset}'" >&2
	exit 1
fi
# PowerShell and Visual Studio developer shells can put Windows find.exe and
# link.exe ahead of MSYS utilities. Use this MSYS installation's tools first.
export PATH="/ucrt64/bin:/usr/bin:$PATH"

mkdir -p "$WORKDIR" "$PREFIX"
WORKDIR=$(cd "$WORKDIR" && pwd)
PREFIX=$(cd "$PREFIX" && pwd)
SRC=$WORKDIR/ffmpeg-$VERSION
BUILD=$WORKDIR/build
LICENSE_DIR=$PREFIX/share/licenses/ffmpeg
SOURCE_SHA256=$(sha256sum "$TARBALL" | cut -d' ' -f1)

# The two codecs only, shared, no external libraries.
CONFIGURE=(
	--prefix="$PREFIX"
	--disable-everything
	--disable-autodetect
	--disable-programs
	--disable-doc
	--disable-network
	--disable-avdevice
	--disable-avformat
	--disable-avfilter
	--disable-swscale
	--disable-swresample
	--disable-debug
	--enable-shared
	--disable-static
	--enable-w32threads
	# MinGW-w64 keeps clock_gettime and nanosleep in winpthreads: link its
	# static library into avutil instead of shipping libwinpthread-1.dll.
	# (-static would also take MSYS2's own libavutil.a for avcodec.)
	--extra-libs=-l:libwinpthread.a
	--enable-encoder=ac3,eac3
	--enable-decoder=ac3,eac3
	--enable-parser=ac3
)

# The DLLs may only depend on Windows system DLLs, the UCRT and each other.
check_imports() {
	for dll in avcodec-62.dll avutil-60.dll; do
		if [[ ! -f "$PREFIX/bin/$dll" ]]; then
			echo "error: $PREFIX/bin/$dll is missing; the FFmpeg library versions changed" >&2
			exit 1
		fi
		imports=$(objdump -p "$PREFIX/bin/$dll" | sed -n 's/^[[:space:]]*DLL Name: //p')
		echo "Imports of $dll:"
		sed 's/^/  /' <<<"$imports"
		unexpected=$(grep -Eiv '^(kernel32|bcrypt|ucrtbase|api-ms-win-crt-[a-z0-9-]+|avutil-60)\.dll$' <<<"$imports" || true)
		if [[ -n "$unexpected" ]]; then
			echo "error: $dll imports DLLs that are neither Windows' nor shipped: $unexpected" >&2
			exit 1
		fi
	done
}

# 0. A prefix that already holds this source built with this configuration
# (a later gate, or CI's dependency cache) is kept once its imports check out.
if [[ -f "$LICENSE_DIR/build-info.txt" ]] &&
	grep -qxF "source_sha256=$SOURCE_SHA256" "$LICENSE_DIR/build-info.txt" &&
	grep -qxF "configure=${CONFIGURE[*]:1}" "$LICENSE_DIR/build-info.txt"; then
	check_imports
	echo "FFmpeg $VERSION with this configuration is already in $PREFIX"
	cat "$LICENSE_DIR/build-info.txt"
	exit 0
fi

# 1. A clean source tree from the checked tarball.
rm -rf "$SRC" "$BUILD" "$LICENSE_DIR/build-info.txt"
tar -xJf "$TARBALL" -C "$WORKDIR"
if [[ ! -x "$SRC/configure" ]]; then
	echo "error: $TARBALL did not unpack to ffmpeg-$VERSION" >&2
	exit 1
fi

# 2. Configure out of tree.
mkdir -p "$BUILD"
(cd "$BUILD" && "$SRC/configure" "${CONFIGURE[@]}")
if grep -Eq '^(CONFIG_GPL|CONFIG_NONFREE)=yes' "$BUILD/ffbuild/config.mak"; then
	echo "error: the configuration is not LGPL" >&2
	exit 1
fi

# 3. Build and install.
make -C "$BUILD" -j "${FFMPEG_JOBS:-4}"
make -C "$BUILD" install

# 4. Check the imports and strip the DLLs.
check_imports
for dll in avcodec-62.dll avutil-60.dll; do
	strip --strip-unneeded "$PREFIX/bin/$dll"
done

# 5. The license and how this build was made, which the LGPL asks to ship.
# build-info.txt is written last: step 0 reuses only a finished build.
mkdir -p "$LICENSE_DIR"
cp "$SRC/COPYING.LGPLv2.1" "$LICENSE_DIR/COPYING.LGPLv2.1"
cp "$SRC/LICENSE.md" "$LICENSE_DIR/LICENSE.md"
cat > "$LICENSE_DIR/build-info.txt" <<EOF
ffmpeg_version=$VERSION
source=https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz
source_sha256=$SOURCE_SHA256
configure=${CONFIGURE[*]:1}
license=$(sed -n 's/^#define FFMPEG_LICENSE "\(.*\)"$/\1/p' "$BUILD/config.h")
compiler=$(gcc --version | head -n1)
EOF
cat "$LICENSE_DIR/build-info.txt"
# Step 0 needs only the prefix: drop the trees (about 90 MB) from the cache.
rm -rf "$SRC" "$BUILD"
