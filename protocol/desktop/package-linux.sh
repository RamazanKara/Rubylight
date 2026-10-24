#!/usr/bin/env bash
# Builds a release nightjar and packs it as a tar.gz, with the PyroWave SDK the GPU decoder
# loads at run time when one is given (the Linux counterpart of package-windows.ps1):
#
#   package-linux.sh [-s <PyroWave SDK folder from scripts/build-pyrowave-sdk.sh>] [-o <folder>] [-t <target dir>] [-v <version>]
#
# Needs the packages from README.md ("Building", Linux). SDL3 is linked statically (see
# Cargo.toml); FFmpeg's libavcodec and libavutil and libopus come from the system, so the
# package lists what `ldd` finds for them (and, on Debian and Ubuntu, the packages that own
# them, in LINUX-RUNTIME.txt) and the target machine needs the same sonames. -v names the
# package after a release version instead of the crate's (the release workflow passes the
# app version); the archive is nightjar-desktop-<version>-linux-<arch>.tar.gz.
#
# With -s the package carries libpyrowave-shared.so next to the executable (nothing links it, the
# client loads it by path at run time), its build-info.txt as pyrowave-sdk-build-info.txt and
# its licences. The script refuses a library that is not the one pyrowave-sdk.pin names
# (commit, Granite, API, patches) or whose SHA-256 is not the one the build recorded, before the
# long build. With it the unpacked archive is also run as `nightjar --check-pyrowave` (see the end of
# the script). Without it the package has no GPU decoder and the client says so when asked for it.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
workspace=$(cd "$here/.." && pwd)
sdk=
out=$workspace/../dist
target=$workspace/target
version=
while getopts "s:o:t:v:" opt; do
	case $opt in
		s) sdk=$OPTARG ;;
		o) out=$OPTARG ;;
		t) target=$OPTARG ;;
		v) version=$OPTARG ;;
		*) exit 2 ;;
	esac
done
mkdir -p "$out"
out=$(cd "$out" && pwd)

# key=value lines (the pin, build-info.txt): the value of one key.
value() { sed -n "s/^[[:space:]]*$2=//p" "$1" | head -n1 | tr -d '\r' | sed 's/[[:space:]]*$//'; }

# The SDK is checked before the long build, so a wrong one fails in a second.
if [[ -n $sdk ]]; then
	sdk=$(cd "$sdk" && pwd)
	for path in "$sdk/libpyrowave-shared.so" "$sdk/build-info.txt" "$sdk/licenses"; do
		[[ -e $path ]] || { echo "error: $path is missing; build the SDK with scripts/build-pyrowave-sdk.sh" >&2; exit 1; }
	done
	pin=$here/pyrowave-sdk.pin
	for key in pyrowave_commit api_version patches; do
		[[ $(value "$sdk/build-info.txt" $key) == "$(value "$pin" $key)" ]] ||
			{ echo "error: the PyroWave SDK in $sdk has $key=$(value "$sdk/build-info.txt" $key), pyrowave-sdk.pin says $(value "$pin" $key)" >&2; exit 1; }
	done
	[[ $(value "$sdk/build-info.txt" granite_commit) == "$(value "$pin" granite_commit)"* ]] ||
		{ echo "error: the PyroWave SDK in $sdk has granite_commit=$(value "$sdk/build-info.txt" granite_commit), pyrowave-sdk.pin says $(value "$pin" granite_commit)" >&2; exit 1; }
	sha=$(sha256sum "$sdk/libpyrowave-shared.so" | cut -d' ' -f1)
	[[ $(value "$sdk/build-info.txt" so_sha256) == "$sha" ]] ||
		{ echo "error: libpyrowave-shared.so is $sha, build-info.txt recorded $(value "$sdk/build-info.txt" so_sha256): it is not the library that was built" >&2; exit 1; }
fi

(cd "$workspace" && cargo build --release -p nightjar-desktop --locked --target-dir "$target")
exe=$target/release/nightjar

[[ -n $version ]] || version=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$workspace/Cargo.toml" | head -n1)
arch=$(uname -m)
name=nightjar-desktop-$version-linux-$arch
stage=$out/$name
rm -rf "$stage"
mkdir -p "$stage"
cp "$exe" "$stage/nightjar"
strip "$stage/nightjar"
if [[ -n $sdk ]]; then
	cp "$sdk/libpyrowave-shared.so" "$stage/"
	cp "$sdk/build-info.txt" "$stage/pyrowave-sdk-build-info.txt"
	cp -r "$sdk/licenses" "$stage/licenses-pyrowave"
fi
cp "$here/README.md" "$stage/"
# What the machine running it needs besides the C library.
ldd "$stage/nightjar" | sed -n 's/^[[:space:]]*\(lib[^ ]*\) => .*/\1/p' | sort >"$stage/system-libraries.txt"
{
	echo "Nightjar for Linux links these shared libraries directly (SDL3 is built in); the"
	echo "packages that provide them bring the rest. system-libraries.txt has the full ldd list."
	echo
	while read -r lib; do
		owner=
		if command -v dpkg >/dev/null; then
			path=$(ldd "$stage/nightjar" | sed -n "s/^[[:space:]]*$lib => \([^ ]*\) .*/\1/p")
			owner=$(dpkg -S "$(readlink -f "$path")" 2>/dev/null | head -n1 | cut -d: -f1) || true
		fi
		echo "  $lib${owner:+  (Debian/Ubuntu package $owner)}"
	done < <(readelf -d "$stage/nightjar" | sed -n 's/.*(NEEDED).*Shared library: \[\(lib[^]]*\)\].*/\1/p' | sort | grep -vE '^lib(c|m|dl|pthread|rt|gcc_s|stdc\+\+)\.so')
	echo
	echo "SDL3 loads the X11 or Wayland, PulseAudio/PipeWire/ALSA and udev libraries of the desktop"
	echo "at run time; a desktop installation has them."
	echo
	if [[ -n $sdk ]]; then
		echo "PyroWave GPU decoding also needs a Vulkan 1.3 driver and the Vulkan loader (libvulkan1)."
	else
		echo "This package does not carry the PyroWave SDK: PyroWave GPU decoding needs"
		echo "libpyrowave-shared.so beside the executable (see README.md), a Vulkan 1.3 driver and"
		echo "the Vulkan loader (libvulkan1)."
	fi
} >"$stage/LINUX-RUNTIME.txt"
tar -C "$out" -czf "$out/$name.tar.gz" "$name"

# With the SDK, the unpacked archive must find and load the library (`nightjar --check-pyrowave`,
# no window needed). Exit 2 means it loaded but this machine has no usable Vulkan device (a CI
# runner); NIGHTJAR_REQUIRE_GPU_PROBE=1 asks for the device too.
if [[ -n $sdk ]]; then
	check=$(mktemp -d)
	tar -C "$check" -xzf "$out/$name.tar.gz"
	code=0
	"$check/$name/nightjar" --check-pyrowave "$check/report.txt" || code=$?
	echo "pyrowave check (exit $code): $(tr '\n' ';' <"$check/report.txt" 2>/dev/null)"
	if [[ $code != 0 && ( $code != 2 || -n ${NIGHTJAR_REQUIRE_GPU_PROBE:-} ) ]]; then
		echo "error: nightjar from the archive cannot use the PyroWave SDK it carries" >&2
		exit 1
	fi
	rm -rf "$check"
fi

(cd "$stage" && ls -l | tail -n +2 | awk '{printf "%12d  %s\n", $5, $9}')
echo "$out/$name.tar.gz: $(du -h "$out/$name.tar.gz" | cut -f1)"
