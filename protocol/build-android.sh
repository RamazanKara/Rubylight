#!/usr/bin/env bash
# Builds rubylight-protocol's C ABI for the Android client and refreshes the prebuilt static
# libraries the app links (app/src/main/jni/protocol/prebuilt/<abi>/librubylight_protocol.a).
#
# Needs rustup with the aarch64-linux-android, armv7-linux-androideabi, i686-linux-android and
# x86_64-linux-android targets, and llvm-ar
# and llvm-strip (any LLVM, including the NDK's). The library is no_std and depends only on
# libc, so no Android linker is involved: only the crate's own object is kept, which makes
# the committed archives a few tens of kilobytes.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
out="$here/../app/src/main/jni/protocol/prebuilt"
ar="${LLVM_AR:-llvm-ar}"
strip="${LLVM_STRIP:-llvm-strip}"

# The desktop client needs SDL3 and FFmpeg; it has its own CI job.
cargo test --manifest-path "$here/Cargo.toml" --workspace --exclude nightjar-desktop --quiet

for pair in arm64-v8a:aarch64-linux-android armeabi-v7a:armv7-linux-androideabi x86:i686-linux-android \
        x86_64:x86_64-linux-android; do
    abi="${pair%%:*}"
    target="${pair#*:}"
    cargo build --manifest-path "$here/Cargo.toml" -p rubylight-protocol-ffi --release --target "$target" --quiet
    archive="$here/target/$target/release/librubylight_protocol.a"
    work="$(mktemp -d)"
    (cd "$work" && "$ar" x "$archive")
    objects=("$work"/rubylight_protocol-*.o)
    if [ "${#objects[@]}" -ne 1 ]; then
        echo "expected one rubylight_protocol object in $archive, found ${#objects[@]}" >&2
        exit 1
    fi
    "$strip" --strip-debug "${objects[0]}"
    mkdir -p "$out/$abi"
    rm -f "$out/$abi/librubylight_protocol.a"
    "$ar" rcs --format=gnu "$out/$abi/librubylight_protocol.a" "${objects[0]}"
    rm -rf "$work"
    echo "$abi: $(wc -c < "$out/$abi/librubylight_protocol.a") bytes"
done
