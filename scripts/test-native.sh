#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
core=app/src/main/jni/moonlight-core
common="$core/moonlight-common-c"
tests=app/src/test/native
out=app/build/native-tests
mkdir -p "$out"

for name in bughunt-stream.patch bughunt-input.patch controller-haptics.patch host-termination.patch; do
    patch="$core/patches/$name"
    patch="$(pwd)/$patch"
    if ! git -C "$common" apply --reverse --check "$patch" 2>/dev/null; then
        git -C "$common" apply --check "$patch"
        git -C "$common" apply "$patch"
    fi
done

flags=(-std=gnu11 -O2 -ffunction-sections -fdata-sections -DHAS_SOCKLEN_T=1 -DHAVE_CLOCK_GETTIME=1
    -I "$core" -I "$common/src" -I "$common/enet/include" -I "$common/nanors"
    -I "$common/nanors/deps/obl" -Wl,--gc-sections)
sanitize=(-fsanitize=address,undefined -fno-sanitize-recover=all -fno-omit-frame-pointer)

for mode in plain sanitized; do
    extra=()
    if [ "$mode" = sanitized ]; then extra=("${sanitize[@]}"); fi
    for test in aaudio_queue control_negotiation controller_haptics dualsense_usb_audio networking rtsp_parser rtsp_receive evdev_ipc stun; do
        cc "${flags[@]}" "${extra[@]}" -fno-builtin "$tests/${test}_test.c" -o "$out/$test-$mode"
    done
    cc "${flags[@]}" "${extra[@]}" -c "$tests/control_packet_types_test.c" -o "$out/control_packet_types-$mode.o"
    cc "${flags[@]}" "${extra[@]}" "$tests/rtsp_duplicate_test.c" -o "$out/rtsp_duplicate-$mode"
    cc "${flags[@]}" "${extra[@]}" "$tests/audio_reorder_test.c" "$common/nanors/rs.c" \
        "$common/nanors/deps/obl/oblas_common.c" "$common/nanors/deps/obl/oblas_lite.c" -o "$out/audio_reorder-$mode"
    cc "${flags[@]}" "${extra[@]}" "$tests/video_shutdown_test.c" "$common/src/ByteBuffer.c" -o "$out/video_shutdown-$mode"
    cc "${flags[@]}" "${extra[@]}" "$tests/control_termination_test.c" "$common/src/ByteBuffer.c" \
        -lpthread -o "$out/control_termination-$mode"
    cc "${flags[@]}" "${extra[@]}" "$tests/microphone_packet_test.c" "$common/src/PlatformCrypto.c" \
        -lcrypto -o "$out/microphone_packet-$mode"
    cc "${flags[@]}" "${extra[@]}" "$tests/input_queue_test.c" "$common/src/LinkedBlockingQueue.c" \
        "$common/src/Platform.c" "$common/src/PlatformCrypto.c" -lcrypto -lpthread -o "$out/input_queue-$mode"
    for test in aaudio_queue control_negotiation controller_haptics dualsense_usb_audio networking rtsp_parser rtsp_receive evdev_ipc stun rtsp_duplicate audio_reorder video_shutdown control_termination microphone_packet input_queue; do
        "$out/$test-$mode"
        echo "$test ($mode): passed"
    done
done

cargo test --locked --manifest-path protocol/Cargo.toml --workspace
cargo build --locked --manifest-path protocol/Cargo.toml -p rubylight-protocol-ffi --release
c++ -std=c++17 -O2 "${sanitize[@]}" "$tests/pyrowave_frame_test.cpp" \
    protocol/target/release/librubylight_protocol.a -o "$out/pyrowave_frame"
"$out/pyrowave_frame"
c++ -std=c++17 -O2 "${sanitize[@]}" "$tests/pyrowave_presentation_test.cpp" -o "$out/pyrowave_presentation"
"$out/pyrowave_presentation"
bash protocol/tests/differential.sh
