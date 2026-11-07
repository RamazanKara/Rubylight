#!/usr/bin/env bash
# Runs v4l2-p010-test.c against a decode-only FFmpeg prefix (ffmpeg-decode-only.sh): the picture
# format and planes for an 8-bit and a 10-bit stream from a fake V4L2 decoder. Needs gcc.
#
#   test-v4l2-p010.sh <FFmpeg prefix>
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
prefix=$(cd "${1:?FFmpeg prefix}" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
gcc -std=gnu11 -O1 -Wall -Wno-deprecated-declarations -Wno-maybe-uninitialized -I"$prefix/include" "$here/v4l2-p010-test.c" -o "$work/test" \
	-L"$prefix/lib" -lavcodec -lavutil -ldl -Wl,--export-dynamic -Wl,-rpath,"$prefix/lib"
timeout 30 "$work/test" 8
timeout 30 "$work/test" 10
