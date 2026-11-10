# Nightjar's Rust workspace

The Rust code of both Nightjar clients, and the wire formats Rubylight's host and its clients share. [internal/RUST_CORE.md](../internal/RUST_CORE.md) describes the streaming engine and its Android switch-over.

| Crate | Path | Role |
| --- | --- | --- |
| `rubylight-protocol` | `core/` | The wire formats shared with the Rubylight host: `no_std` with `alloc` and no dependencies, so the host takes it as an ordinary dependency. |
| `rubylight-protocol-ffi` | `ffi/` | Its C ABI for the Android app (`ffi/include/rubylight_protocol.h`). |
| `nightjar-core` | `client/` | The streaming engine: pairing and host HTTP, RTSP/SDP, the encrypted control stream, input, video and audio receive with FEC, timing. |
| `nightjar-policy` | `policy/` | Stream policies both clients need, with no I/O and no clock: bitrate, PyroWave bitrate, quality, health, recovery, launch, collections, open-play, stream menu, shortcuts and more. |
| `nightjar-android` | `android/` | `libnightjar_core.so`, the JNI bridge over `nightjar-core` behind the app's "New streaming engine" setting. |
| `nightjar-desktop` | `desktop/` | The desktop client for Windows, Linux and macOS ([README](desktop/README.md)). Not a default member: it needs SDL3 and FFmpeg. |
| `nightjar-cli` | `cli/` | Headless test client for the Rubylight host's end-to-end tests. |
| `nightjar-difftest` | `difftest/` | Test-only: compares `nightjar-core` with the patched moonlight-common-c it replaces. |

`vendor/` holds the patched `rusty_enet` and the SDL source the Linux desktop client builds ([vendor/sdl3-src/README.md](vendor/sdl3-src/README.md)).

## rubylight-protocol

These are the Rubylight host's extensions, so the crate, its C header and the static library keep their `rubylight` names: the host pins this crate, and Nightjar uses them only when the host advertises them. Among its modules:

- `pyrowave`: the client side of PyroWave framing, the packet container and the record reassembly that lets a frame with lost packets still decode.
- `phase_lock` and `present_clock`: the client's report of how early frames are ready before its display latch, the host controller that times frames to land just before it, and the client's presentation schedule from the host's frame times.
- `roaming`: the client roaming wire format v1 ([client roaming](../docs/protocol/client-roaming.md)), with known answers for the AES-GCM the host and client seal it with.
- `cursor_shape` ([cursor shape](../docs/protocol/cursor-shape.md)), `hdr`, `packed444`, `rate_change`, `fec_report`, `pipeline_rates`, `text_input`, `keyboard_layout`, `pen`, `clipboard_sync`, `pairing_uri`, `usb` and `usbip`: the other control messages and formats.

`tests/differential.sh` compares the framing with the C++ the client used before (`tests/frame_reference.h`) on random, lossy and corrupted frames.

## Building for Android

The app links prebuilt static libraries of `rubylight-protocol-ffi` from `app/src/main/jni/protocol/prebuilt/`. After changing the crate, refresh them:

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
protocol/build-android.sh      # runs the workspace tests (without the desktop client), then rebuilds all four ABIs
protocol/tests/differential.sh # optional: 8 seeds x 2000 streams against the old C++
```

Only the crate's own object goes into each archive (a few tens of kilobytes); it needs nothing but libc.

The streaming engine is different: `libnightjar_core.so` links Rust's std, ring and the app's libopus, so it is not committed. The Gradle task `buildRustEngine` (`app/build.gradle`) runs cargo for each ABI with the NDK's clang as compiler and linker and checks the 16 KiB segment alignment, so a normal app build needs rustup with the four targets above.

## nightjar-policy on Android

Android calls `nightjar-policy` through `libnightjar_core.so` for collections, open-play and the stream menu, each with a JSON contract beside its bridge in `android/src/`. For the other policies the Java originals stay; they are kept equal to the Rust through the traces in `policy/tests/traces/<module>/*.json`: `cargo test -p nightjar-policy` and the app's `PolicyTraceTest` replay the same files, and the format is described in `policy/src/trace.rs`. The desktop client calls `nightjar-policy` directly.
