# Code map

[Documentation](index.md) · [Building](building.md) · [Features](features.md)

Where each part of Nightjar lives, which files change together and which test proves a change. It is written for people and coding agents changing the client.

Nightjar is developed inside the [Rubylight](https://github.com/RamazanKara/Rubylight) repository, in its `client` folder, and mirrored from there. That repository's `tools/code_map.py` answers three questions from the tree and its history: `index <folder>` prints lines, tests and the header sentence of each file, `with <file>` lists the files that changed in the same commits, and `hot <folder>` lists fix commits per file. Its `check` verifies the paths and names on this page, so correct the page in the change that moves the code.

## Crates

All Rust is one Cargo workspace in `protocol/`.

| Crate | Folder | Holds |
| --- | --- | --- |
| `rubylight-protocol` | `protocol/core/` | Wire formats shared with the host, `no_std`: each extension message in a module of its own, every message id in [`ids.rs`](../protocol/core/src/ids.rs). |
| `nightjar-core` | `protocol/client/` | The streaming engine: pairing and HTTPS, RTSP, control, video and audio receive, input, USB forwarding. |
| `nightjar-policy` | `protocol/policy/` | Decisions without I/O or a clock: bitrate, the quality ladder, the recovery budget, the overlay, the stream menu. Both apps call it, so they decide alike. |
| `nightjar-desktop` | `protocol/desktop/` | The Windows, Linux and macOS app. |
| `nightjar-android` | `protocol/android/` | `libnightjar_core.so`, the JNI side of the Android app's Rust engine. |
| `rubylight-protocol-ffi` | `protocol/ffi/` | A C interface to `rubylight-protocol` for the Android app's C code. |
| `nightjar-cli` | `protocol/cli/` | A headless client for tests. |
| `nightjar-difftest` | `protocol/difftest/` | Runs the engine and the patched moonlight-common-c on the same packets and compares their output byte for byte. |

A feature lands in both apps in one change. What they share goes into `nightjar-policy` or `nightjar-core`; an app only draws it and binds it to its platform.

## A stream in the engine

| Step | Where |
| --- | --- |
| Find and pair a PC, list apps, launch | `protocol/client/src/http/`; what the host offers is [`Feature`, `HostCapabilities`](../protocol/client/src/http/capabilities.rs) |
| RTSP | `protocol/client/src/rtsp/`, `protocol/client/src/sdp.rs` |
| Control | `protocol/client/src/control/`: `session.rs` is the state machine without a socket, `driver.rs` runs it on ENet, `messages.rs` and `events.rs` are what is sent and received |
| Start | `protocol/client/src/session/mod.rs`, which also defines the [`VideoDecoder`](../protocol/client/src/session/mod.rs) an app implements |
| Video | [`receive_thread`, `receive_batch`](../protocol/client/src/session/video.rs) read the socket; [`VideoReceiver`](../protocol/client/src/video/mod.rs) decrypts; [`RtpQueue`, `add_packet`, `reconstruct_frame`](../protocol/client/src/video/queue.rs) order packets and repair loss with [`ReedSolomon`](../protocol/client/src/fec.rs); [`Depacketizer`](../protocol/client/src/video/depacketizer.rs) builds frames; [`FrameQueue`, `decoder_thread`, `decode_loop`](../protocol/client/src/session/video.rs) hand them to the app's decoder |
| Audio | `protocol/client/src/audio/`, `protocol/client/src/session/audio.rs` |
| Input, microphone | `protocol/client/src/input/` |
| Native API, files, folders | `protocol/client/src/http/native.rs`, `files.rs` and `folders.rs` beside it |

## The desktop app

| Part | `protocol/desktop/src/` |
| --- | --- |
| Entry, command line | `main.rs` owns the loop and carries out what the UI returns; `cli.rs` |
| UI | `gui.rs` (egui on SDL3), `ui.rs` and `ui/`: pages only draw and return commands. Settings page: `ui/settings.rs` |
| PCs and apps | `hosts.rs`: discovery, pairing and app lists on worker threads, results as events |
| Stream | `stream/mod.rs` (session worker), `stream/connect.rs` (launch), `stream/negotiate.rs` (mode, bitrate, codecs, HDR), `stream/pipeline.rs` (picture loop, decoder order), `stream/present.rs` (when a picture is shown), `stream/input.rs`, `stream/recovery.rs` |
| Decode | `media.rs` (FFmpeg, Opus), `vulkan_video.rs`, `amd_decode.rs`, `linux_video.rs`, `dmabuf.rs`, `stream/pyrowave.rs`, `stream/pyrowave_gpu.rs` |
| Windows only | `stream/direct.rs`, `stream/d3d11.rs`, `stream/amf_scaler.rs`, `stream/wasapi.rs`, `stream/microphone.rs` |
| Linux only | `dmabuf.rs`, `vulkan_video.rs`, `stream/wl_scanout.rs`, `stream/wl_presentation.rs`, `stream/drm_vrr.rs`, `stream/planes_gpu.rs` |
| macOS only | `metal/`, `stream/macos_video_check.rs` |
| Settings, text | `config.rs`, `strings.rs`, `strings/` |

`cfg(unix)` includes macOS: Linux-only code takes `target_os = "linux"`. A changed shader `.spv` needs its Metal translation from `protocol/desktop/src/metal/generate.py`.

## The Android app

Gradle builds one module, `app/`, in the flavors `nonRoot` and `root`. Its tasks [`buildRustEngine` and `prepareNativePatches`](../app/build.gradle) build `nightjar-android` for each ABI and apply `app/src/main/jni/moonlight-core/patches/` to moonlight-common-c.

A stream runs on one of two engines. The Rust engine is `nightjar-core` behind [`RustBridge`](../app/src/main/java/com/limelight/nvstream/jni/RustBridge.java) and `protocol/android/src/lib.rs`; the C engine is moonlight-common-c behind [`MoonBridge`](../app/src/main/java/com/limelight/nvstream/jni/MoonBridge.java) and `app/src/main/jni/moonlight-core/`. The Rust side calls the same `MoonBridge` callbacks the C side does (`protocol/android/src/bridge.rs`, `listener.rs`, `renderers.rs`), so decoders and input see one interface. [`StreamEngine`](../app/src/main/java/com/limelight/nvstream/jni/StreamEngine.java) records which one ran. A protocol change must hold on both.

| Part | `app/src/main/java/com/limelight/` |
| --- | --- |
| Stream activity | `Game.java`, with `GameConnection.java`, `GameRecovery.java`, `GameVideoOutput.java`, `GameTouchInput.java`, `GameOverlays.java` |
| Connection | `nvstream/NvConnection.java`, `nvstream/http/` (serverinfo, pairing, [`HostCapabilities`](../app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java)), `nvstream/jni/` |
| Video | `binding/video/`, led by `binding/video/MediaCodecDecoderRenderer.java` |
| Input, audio | `binding/input/ControllerHandler.java`, `binding/input/driver/`, `binding/input/touch/`, `binding/input/virtual_controller/`, `binding/audio/` |
| Settings | `preferences/PreferenceConfiguration.java`, `preferences/StreamSettings.java` |
| PCs, library, UI | `computers/`, `console/`, `ui/`, `grid/` |
| Native API, files | `nativeapi/`, `files/` |

Native code beside the engines, in `app/src/main/jni/`: `pyrowave-renderer/`, `vulkan/`, `evdev_reader/`, `window-dataspace/`.

## Checklists

### A setting

Desktop:

1. A field of [`Settings`](../protocol/desktop/src/config.rs) with a doc comment and its default. A stored name is never renamed: an old value is read with a serde alias or in [`migrate`](../protocol/desktop/src/config.rs). Per-PC and per-game values are [`Overrides`](../protocol/desktop/src/config.rs).
2. A row in [`settings_rows`](../protocol/desktop/src/ui/settings.rs), in the section the setting belongs to.
3. Its words: the English literal inside [`tr`, `trf`, `en`](../protocol/desktop/src/strings.rs) is the key, and every table in `protocol/desktop/src/strings/` needs an entry for it. A test fails for a missing one.
4. Tests in `protocol/desktop/src/ui/settings_tests.rs`.

Android:

1. The preference in `app/src/main/res/xml/preferences.xml`, its texts in `app/src/main/res/values/strings.xml` and each translated `strings.xml`.
2. The key and its reading in [`PreferenceConfiguration`](../app/src/main/java/com/limelight/preferences/PreferenceConfiguration.java). Keys are never renamed and retired keys never return: [`SettingsKeysTest`](../app/src/test/java/com/limelight/preferences/SettingsKeysTest.java).
3. Its help text in `scripts/generate-settings.py`, then run the script. It writes `docs/settings.md`, and the documentation check fails when that file is out of date.

A decision both apps make from the setting goes into `protocol/policy/src/`.

### A control message

1. The id in [`ids.rs`](../protocol/core/src/ids.rs) and a module in `protocol/core/src/`.
2. The engine: `protocol/client/src/control/messages.rs` to send, `events.rs` to receive. It is sent only to a host that announced it.
3. Android: `protocol/android/src/lib.rs` and its [`RustBridge`](../app/src/main/java/com/limelight/nvstream/jni/RustBridge.java) method; on the C engine the bridge in `app/src/main/jni/moonlight-core/`, with a patch in its `patches/` folder where moonlight-common-c itself must change.
4. Desktop: the handler in `protocol/desktop/src/stream/`.
5. Tests: `protocol/client/tests/control_messages_contract.rs` and [`MoonBridgeControlMessagesTest`](../app/src/test/java/com/limelight/nvstream/jni/MoonBridgeControlMessagesTest.java).

### A feature that needs the host

Add it to [`Feature`](../protocol/client/src/http/capabilities.rs) with the evidence that proves it (a serverinfo field, a DESCRIBE line, a native API capability merged by [`with_native`](../protocol/client/src/http/capabilities.rs)), and to the Java [`HostCapabilities`](../app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java). Both read the recorded replies in `app/src/test/resources/protocol/capabilities/`, checked by `protocol/client/tests/capabilities_contract.rs`. Gate on the feature, never on a host's name.

## The narrowest test

| Change | Test |
| --- | --- |
| Engine | `cargo test -p nightjar-core` in `protocol/`. `protocol/client/tests/` runs whole sessions on loopback (`session_loopback.rs`, `control_loopback.rs`, `host_interop.rs`) and holds the contracts. |
| Bytes from the network | The property tests `protocol/core/src/fuzz.rs`, `protocol/client/src/fuzz_tests.rs` and the `fuzz_tests.rs` under `http/` and `rtsp/`. `PROPTEST_CASES` raises the count. |
| Same result as moonlight-common-c | `cargo test -p nightjar-difftest` |
| A policy | Its module tests, and recorded traces under `protocol/policy/tests/traces/` replayed by `protocol/policy/tests/traces.rs` |
| Desktop UI | `protocol/desktop/src/ui/tests.rs` draws pages and dialogs without a window; `protocol/desktop/src/shot.rs` rasterises them for screenshots. |
| Android | `gradlew :app:testNonRootDebugUnitTest --tests <class>` (JUnit and Robolectric, under `app/src/test/java/`). The unit tests do not compile the JNI code: `gradlew :app:externalNativeBuildNonRootDebug` does. `scripts/test-native.sh` runs the C tests, `scripts/emulator-smoke.py` drives the app on an emulator. |
| Documentation | `node scripts/check-doc-links.mjs`, and `scripts/generate-settings.py` leaving `docs/settings.md` unchanged |

[Building](building.md) has the toolchains each of these needs.

## What has gone wrong before

The fix history repeats a few shapes. Check a change against them before calling it done.

- **One app only.** A setting, a text or a behaviour that reached Android or desktop and not the other. `with` on the file you changed shows what the other side touched last time.
- **One language only.** A new text needs every table in `protocol/desktop/src/strings/` and every translated `strings.xml`.
- **Gated by name or model.** A feature follows what the host announced and what the device's decoder reports at run time, not a host brand, a vendor or an Android version.
- **Recovery.** A reconnect, a network change or a stopped stream meets state the first launch set. The cases live in `protocol/desktop/src/stream/recovery.rs`, `protocol/policy/src/recovery.rs` and [`GameRecoveryTest`](../app/src/test/java/com/limelight/GameRecoveryTest.java).
- **One engine only** on Android: a change in the Rust engine that the C engine's patches do not have, or the reverse.
- **Linux code compiled for macOS.** `cfg(unix)` is both; the macOS build runs only in CI.
