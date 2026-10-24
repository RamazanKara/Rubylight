#!/usr/bin/env bash
# Builds the PyroWave SDK the Linux desktop client's GPU decoder loads, at the pin in
# protocol/desktop/pyrowave-sdk.pin with its patches (the Linux counterpart of
# build-pyrowave-sdk.ps1):
#
#   scripts/build-pyrowave-sdk.sh [-o <folder>] [-H <Rubylight checkout>] [-j <jobs>]
#
# <folder> gets libpyrowave-shared.so, build-info.txt (the pin, the patches, the library's SHA-256
# and the toolchain) and licenses/. The sources and the build tree stay in <folder>/work, so a
# second run only rebuilds what changed. Patch 0002 comes from the Rubylight host's
# rust/codec-patches/pyrowave, 0003 to 0005 from this repo's app/src/main/jni/pyrowave-renderer/patches
# (a patch both carry must be the same file). Needs git, gcc/g++, cmake 3.27 or newer, ninja and
# the Vulkan headers (libvulkan-dev); the library loads libvulkan.so.1 at run time through volk.
# package-linux.sh reads <folder> through its first argument.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$here/.." && pwd)
out=${PYROWAVE_SDK_OUT:-$HOME/nj-pyrowave-sdk}
host=${RUBYLIGHT_HOST_REPO:-$HOME/rubylight}
jobs=
while getopts "o:H:j:" opt; do
	case $opt in
		o) out=$OPTARG ;;
		H) host=$OPTARG ;;
		j) jobs=$OPTARG ;;
		*) exit 2 ;;
	esac
done

pin=$repo/protocol/desktop/pyrowave-sdk.pin
pin_value() { sed -n "s/^$1=//p" "$pin" | head -n1; }
commit=$(pin_value pyrowave_commit)
granite_pin=$(pin_value granite_commit)
api_pin=$(pin_value api_version)
patches_pin=$(pin_value patches)
for value in "$commit" "$granite_pin" "$api_pin" "$patches_pin"; do
	[[ -n $value ]] || { echo "error: pyrowave-sdk.pin is incomplete" >&2; exit 1; }
done
[[ $commit =~ ^[0-9a-f]{40}$ ]] || { echo "error: pyrowave_commit must be a full SHA" >&2; exit 1; }

host_patches=$host/rust/codec-patches/pyrowave
own_patches=$repo/app/src/main/jni/pyrowave-renderer/patches
mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$out/work
rm -rf "$work/stage" "$work/install"
mkdir -p "$work/stage" "$work/install"

# The patch set, in the pin's order. The stage holds the pin's patches only.
IFS=, read -r -a names <<<"$patches_pin"
for name in "${names[@]}"; do
	found=()
	for dir in "$host_patches" "$own_patches"; do
		[[ -f $dir/$name.patch ]] && found+=("$dir/$name.patch")
	done
	((${#found[@]})) || { echo "error: patch $name is in neither $host_patches nor $own_patches" >&2; exit 1; }
	if ((${#found[@]} == 2)) && ! cmp -s "${found[0]}" "${found[1]}"; then
		echo "error: patch $name differs between $host_patches and $own_patches" >&2
		exit 1
	fi
	cp "${found[0]}" "$work/stage/$name.patch"
done

src=$work/pyrowave
if [[ ! -d $src/.git ]]; then
	git init -q "$src"
	git -C "$src" remote add origin https://github.com/Themaister/pyrowave.git
fi
git -C "$src" fetch -q --depth 1 origin "$commit"
git -C "$src" -c advice.detachedHead=false checkout -q --force FETCH_HEAD
git -C "$src" clean -qfdx -e build
sha=$(git -C "$src" rev-parse HEAD)
[[ $sha == "$commit" ]] || { echo "error: checked out $sha, expected $commit" >&2; exit 1; }

granite_commit=$(sed -n 's/^GRANITE_COMMIT=//p' "$src/checkout_granite.sh")
[[ $granite_commit =~ ^[0-9a-f]{40}$ ]] || { echo "error: could not read GRANITE_COMMIT" >&2; exit 1; }
(cd "$src" && bash ./checkout_granite.sh)
granite_sha=$(git -C "$src/Granite" rev-parse HEAD)
[[ $granite_sha == "$granite_commit" ]] || { echo "error: Granite is at $granite_sha, expected $granite_commit" >&2; exit 1; }
[[ $granite_sha == "$granite_pin"* ]] || { echo "error: built Granite $granite_sha, the pin is $granite_pin" >&2; exit 1; }
for name in "${names[@]}"; do
	git -C "$src" apply --check "$work/stage/$name.patch"
	git -C "$src" apply "$work/stage/$name.patch"
done

build=$work/build
cmake -S "$src" -B "$build" -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$work/install" \
	-DPYROWAVE_DEVEL=OFF -DPYROWAVE_UTILS=OFF
cmake --build "$build" ${jobs:+--parallel "$jobs"}
cmake --install "$build" --strip

libs=("$work"/install/lib*/libpyrowave-shared.so "$work"/install/lib*/*/libpyrowave-shared.so)
lib=
for candidate in "${libs[@]}"; do [[ -e $candidate ]] && lib=$candidate && break; done
[[ -n $lib ]] || { echo "error: the build did not install libpyrowave-shared.so" >&2; find "$work/install" -name '*pyrowave*' >&2; exit 1; }
lib=$(readlink -f "$lib")

rm -rf "$out/licenses"
mkdir -p "$out/licenses"
cp "$src/LICENSE" "$out/licenses/LICENSE"
cp "$src/Granite/LICENSE" "$out/licenses/LICENSE.Granite"
cp "$src/Granite/third_party/volk/LICENSE.md" "$out/licenses/LICENSE.volk.md"
rm -f "$out/libpyrowave-shared.so"
cp "$lib" "$out/libpyrowave-shared.so"

api=$(sed -n 's/^#define PYROWAVE_API_VERSION_\(MAJOR\|MINOR\|PATCH\) \([0-9]*\)$/\2/p' "$src/pyrowave.h" | paste -sd.)
[[ $api == "$api_pin" ]] || { echo "error: built API $api, the pin is $api_pin" >&2; exit 1; }

# The library may link libstdc++ and libgcc statically or dynamically, but never the Vulkan loader
# (volk loads it) and nothing outside the C library and the C++ runtime.
needed=$(readelf -d "$out/libpyrowave-shared.so" | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p')
echo "Needed by libpyrowave-shared.so:"
sed 's/^/  /' <<<"$needed"
if grep -Eq '^libvulkan' <<<"$needed"; then
	echo "error: libpyrowave-shared.so links the Vulkan loader" >&2
	exit 1
fi

{
	echo "pyrowave_commit=$sha"
	echo "granite_commit=$granite_sha"
	echo "api_version=$api"
	echo "patches=$patches_pin"
	echo "compiler=$(gcc --version | head -n1)"
	echo "cmake=$(cmake --version | head -n1)"
	echo "needed=$(paste -sd, <<<"$needed")"
	echo "so_sha256=$(sha256sum "$out/libpyrowave-shared.so" | cut -d' ' -f1)"
	echo "built=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
} >"$out/build-info.txt"
echo "PyroWave SDK $sha with $patches_pin built into $out"
cat "$out/build-info.txt"
