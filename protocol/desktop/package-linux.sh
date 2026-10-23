#!/usr/bin/env bash
# Builds a release nightjar and packs it as a tar.gz, with the PyroWave SDK the GPU decoder
# loads at run time when one is given (the Linux counterpart of package-windows.ps1):
#
#   package-linux.sh [-s <PyroWave SDK folder from scripts/build-pyrowave-sdk.sh>] [-o <folder>] [-t <target dir>]
#
# Needs the packages from README.md ("Building", Linux). SDL3 is linked statically (see
# Cargo.toml); FFmpeg's libavcodec and libavutil and libopus come from the system, so the
# package lists what `ldd` finds for them and the target machine needs the same sonames.
#
# With -s the package carries libpyrowave-shared.so next to the executable (nothing links it, the
# client loads it by path at run time), its build-info.txt as pyrowave-sdk-build-info.txt and
# its licences. The script refuses a library that is not the one pyrowave-sdk.pin names
# (commit, Granite, API, patches) or whose SHA-256 is not the one the build recorded, before the
# long build. Without it the package has no GPU decoder and the client says so when asked for it.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
workspace=$(cd "$here/.." && pwd)
sdk=
out=$workspace/../dist
target=$workspace/target
while getopts "s:o:t:" opt; do
	case $opt in
		s) sdk=$OPTARG ;;
		o) out=$OPTARG ;;
		t) target=$OPTARG ;;
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

version=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$workspace/Cargo.toml" | head -n1)
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
tar -C "$out" -czf "$out/$name.tar.gz" "$name"

(cd "$stage" && ls -l | tail -n +2 | awk '{printf "%12d  %s\n", $5, $9}')
echo "$out/$name.tar.gz: $(du -h "$out/$name.tar.gz" | cut -f1)"
