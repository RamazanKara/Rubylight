#!/usr/bin/env bash
# Builds rubylight-protocol for this machine and compares it with the old C++ framing on
# random and corrupted frames. Usage: tests/differential.sh [seeds] [streams per seed]
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
seeds="${1:-8}"
streams="${2:-2000}"
cargo build --manifest-path "$here/Cargo.toml" -p rubylight-protocol-ffi --release --quiet
bin="$here/target/differential"
"${CXX:-c++}" -std=c++17 -O2 -fsanitize=address,undefined -fno-sanitize-recover=all \
    -I "$here/ffi/include" "$here/tests/differential.cpp" "$here/target/release/librubylight_protocol.a" -o "$bin"
for seed in $(seq 1 "$seeds"); do
    "$bin" "$seed" "$streams"
done
