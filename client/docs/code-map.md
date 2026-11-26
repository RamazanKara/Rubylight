# Code map

[Nightjar docs](index.md) › [For developers](index.md#for-developers) · [Building](building.md) · [Features](features.md)

Where each part of Nightjar lives, which files change together and which test proves a change. It is written for people and coding agents changing the client.

Nightjar is developed inside the [Rubylight](https://github.com/RamazanKara/Rubylight) repository, in its `client` folder, and mirrored from there. That repository's code map tool (tools/code_map.py there) answers three questions from the tree and its history: `index <folder>` prints lines, tests and the header sentence of each file, `with <file>` lists the files that changed in the same commits, and `hot <folder>` lists fix commits per file. Its `check` verifies the paths and names on this page, so correct the page in the change that moves the code.

## Crates

All Rust is one Cargo workspace in `protocol/`.

| Crate | Folder | Holds |
| --- | --- | --- |
| `rubylight-protocol` | `protocol/core/` | Wire formats shared with the host, `no_std`: each extension message in a module of its own, every message id in [`ids.rs`](../protocol/core/src/ids.rs). |
| `rubylight-wire` | `protocol/wire/` | Wire codecs and control crypto shared with the host (`crypto`, `packet`, `haptics`, `controller_audio`, `fec_status`, `mic`, `input`, `rtsp`). |
| `rubylight-native-api` | `protocol/native-api/` | Wire types of the Rubylight native API v1, versioned on their own and shared with the host. |
| `rubylight-traverse` | `protocol/traverse/` | Internet play, shared with the host: DHT records, STUN, PCP and NAT-PMP, hole punching and the tunnel that reach a paired PC from outside its network. |
| `nightjar-core` | `protocol/client/` | The streaming engine: pairing and HTTPS, RTSP, control, video and audio receive, input, USB forwarding, mDNS/WoL/STUN/host-db/identity stores (`mdns.rs`, `wol/`, `stun.rs`, `hosts/`), unified [`PinStore`](../protocol/client/src/pin_lock.rs), cross-host `console/` stores/repository, and TLS release/APK/GameInfo HTTP (`WebPkiTransport` in `update/net.rs`, `update/exchange_tests.rs`, [`fetch_latest`](../protocol/client/src/update.rs)). |
| `nightjar-policy` | `protocol/policy/` | Decisions without I/O or a clock: declarative Nightjar settings catalog (`settings.toml`, `settings_catalog_gen.rs`), bitrate, the quality ladder, the recovery budget, the overlay, the stream menu, video pacing/vsync/display-rate/queue/codec (`vsync.rs`, `video_pacing.rs`, `video_queue.rs`, `video_codec.rs`), controller input & rumble policies (`controller/`, `rumble.rs`), connection stage/failure/mic/permission rules (`connect_text.rs`, `mic_route.rs`, `permissions.rs`), host capabilities (`http/capabilities.rs`), GameInfo (`game_info/`), cross-host library (`console/`), per-display profile recall (`display_profile.rs`), mDNS & WoL planning (`mdns.rs`, `wol.rs`), shared host schemas/notices/names (`hosts/`), [`PinGate`](../protocol/policy/src/pin.rs), and [`UpdatePolicy`](../protocol/policy/src/update.rs). Both apps call it, so they decide alike. |
| `nightjar-desktop-config` | `protocol/desktop-config/` | Desktop settings, shortcuts, string tables (`strings/`), pure state DTOs (`hosts_types`, `stream_types`, `gamepad_types`, `power_types`, `wol_types`, `pin_lock`). |
| `nightjar-desktop-ui` | `protocol/desktop-ui/` | Pure `egui` desktop pages (`ui/`, `host_page`, `folders_page`, `coop_page`, `neural_page`, `session_page`, `console`, `stream_menu`, `radeon`, `theme`) and CPU test rasterizer (`shot.rs`). |
| `nightjar-desktop-media` | `protocol/desktop-media/` | SDL3, FFmpeg, Opus, Vulkan/D3D11/Metal video decode and presentation (`media.rs`, `stream/`, `vulkan_video`, `kms/`, `metal/`), gamepads, and USB forwarding. |
| `nightjar-desktop` | `protocol/desktop/` | Thin Windows, Linux and macOS binary entry, SDL3+egui host (`gui.rs`), launcher/session loop (`app/`), worker threads (`hosts/`), and OS hooks. |
| `nightjar-android` | `protocol/android/` | `libnightjar_core.so`, the JNI side of the Android app's Rust engine. |
| `rubylight-protocol-ffi` | `protocol/ffi/` | A C interface to `rubylight-protocol`. |
| `nightjar-cli` | `protocol/cli/` | A headless client for tests. |

A feature lands in both apps in one change. What they share goes into `nightjar-policy` or `nightjar-core`; an app only draws it and binds it to its platform.

## A stream in the engine

| Step | Where |
| --- | --- |
| Find and pair a PC, list apps, launch | `protocol/client/src/http/` and shared orchestration `protocol/client/src/connect/`; what the host offers is [`Feature`, `HostCapabilities`](../protocol/policy/src/http/capabilities.rs) |
| Internet play, guests | `protocol/client/src/internet.rs` and `protocol/client/src/http/internet_play.rs` (over `rubylight-traverse`); joining another device's game in `protocol/client/src/http/session_guests.rs`; invite links in `protocol/core/src/invite_uri.rs`, opened by `protocol/client/src/invite.rs` (Android `computers/Invites.java`, desktop `hosts.rs`) |
| RTSP | `protocol/client/src/rtsp/`, `protocol/client/src/sdp.rs` |
| Control | `protocol/client/src/control/`: `session.rs` is the state machine without a socket, `driver.rs` runs it on ENet, `messages.rs` and `events.rs` are what is sent and received |
| Start | `protocol/client/src/session/mod.rs`, which also defines the [`VideoDecoder`](../protocol/client/src/session/mod.rs) an app implements |
| Video | [`receive_thread`, `receive_batch`](../protocol/client/src/session/video.rs) read the socket; [`VideoReceiver`](../protocol/client/src/video/mod.rs) decrypts; [`RtpQueue`, `add_packet`, `reconstruct_frame`](../protocol/client/src/video/queue.rs) order packets and repair loss with [`ReedSolomon`](../protocol/client/src/fec.rs); [`Depacketizer`](../protocol/client/src/video/depacketizer.rs) builds frames; [`FrameQueue`, `decoder_thread`, `decode_loop`](../protocol/client/src/session/video.rs) hand them to the app's decoder |
| Audio | `protocol/client/src/audio/`, `protocol/client/src/session/audio.rs` |
| Input, microphone | `protocol/client/src/input/` (a physical touchpad's packets in `input/touchpad.rs`) |
| Native API, files, folders | `protocol/client/src/http/native.rs`, `files.rs` and `folders.rs` beside it |

## The desktop app

| Part | Where (`protocol/desktop*/src/`) |
| --- | --- |
| Entry, command line | `desktop/src/main.rs` holds the app's state (`App`) and its loop; `desktop/src/cli.rs` |
| What the app does | `desktop/src/app.rs` (saving, shutdown), `desktop/src/app/launcher.rs` (the launcher's events and frames), `desktop/src/app/command.rs` (carries out what the UI returns), `desktop/src/app/launch.rs` (starting a stream), `desktop/src/app/launch_probe.rs` (the decoder checks before a start, on a worker thread), `desktop/src/app/session.rs` (a running stream and its end), `desktop/src/app/overlay.rs` (statistics, toasts and the stream menu over the picture), `desktop/src/app/renderer.rs` and `desktop-media/src/render_device.rs` (the window's renderer and device-loss recovery) |
| UI | `desktop/src/gui.rs` (egui on SDL3); `desktop-ui/src/ui.rs` and `desktop-ui/src/ui/` pages only draw and return commands. Settings page: `desktop-ui/src/ui/settings.rs` |
| PCs and apps | `desktop/src/hosts.rs`: discovery and app lists on worker threads, results as events (`desktop/src/hosts/events.rs` applies them); `desktop/src/hosts/pairing.rs`, `desktop/src/hosts/power.rs` (wake, sleep, updates), `desktop/src/hosts/native.rs` (the native API's reads), `desktop/src/hosts/covers.rs`, `desktop/src/hosts/game_info.rs` (SteamGridDB covers and IGDB facts for the games); UI models in `desktop-ui/src/hosts_model.rs` |
| Stream (`desktop-media/src/`) | `stream/mod.rs` (session worker), `stream/microphone_watch.rs`, `stream/connect.rs` and `stream/connect/` (launch, preflight, worker), `stream/negotiate.rs` (mode, bitrate, codecs, HDR), `stream/pipeline.rs` (picture loop, decoder order), `stream/present.rs` and `stream/present/` (the video texture; `stream/present/pacer.rs` and `stream/present/skipped_picture.rs` decide when a picture is shown), `stream/upscale.rs` (FSR and SGSR upscaling), `stream/input.rs`, `stream/recovery.rs`, `stream/guests.rs`, `stream/touchpad/` |
| Decode (`desktop-media/src/`) | `media.rs` and `media/` (FFmpeg, Opus, hardware selection, frames, `media/d3d11.rs` Windows D3D11VA decode-device isolation), `vulkan_video.rs` and `vulkan_video/` (FFmpeg Vulkan ABI, device sharing, VAAPI/CUDA maps, planes, sampling), `amd_decode.rs`, `linux_video.rs`, `dmabuf.rs` (`dmabuf/linux.rs`), `stream/pyrowave.rs` (`stream/pyrowave/`), `stream/pyrowave_sdk.rs` (`stream/pyrowave_sdk/`), `stream/pyrowave_gpu.rs`, `sdl.rs` (`sdl/`), `gamepad.rs` (`gamepad/`), `usb.rs` (`usb/`), `usb_arbitration.rs` (a controller forwarded over USB pauses as a gamepad until the forwarding stops, `gamepad/arbitration.rs`), `usb_windows.rs` (`usb_windows/`: usbipd-win forwarding on loopback with its ownership journal); `stream/decoder_cache.rs` keeps the last stream's decoder for the reconnect that follows it |
| Windows only (`desktop-media/src/`) | `stream/direct.rs`, `stream/d3d11.rs` (`stream/d3d11/`), `stream/amf_scaler.rs`, `stream/wasapi.rs`, `stream/microphone.rs` |
| Linux only (`desktop-media/src/`) | `dmabuf.rs` (`dmabuf/linux.rs`), `vulkan_video.rs` (`vulkan_video/`), `stream/wl_scanout.rs` (`stream/wl_scanout/`), `stream/wl_presentation.rs`, `stream/drm_vrr.rs`, `stream/planes_gpu.rs` (`stream/planes_gpu/`), `kms/` with `stream/kms_scanout.rs` (`stream/kms_scanout/`, DRM/KMS output without a desktop), `stream/touchpad/evdev.rs` |
| macOS only (`desktop-media/src/`) | `metal/`, `stream/macos_video_check.rs`, `stream/metalfx.rs` with its Objective-C bridge `stream/metalfx.m` (MetalFX upscaling, compiled by `desktop-media`'s build script), `stream/pyrowave_sdk/own_vulkan.rs` (PyroWave's GPU decoder on the bundled KosmicKrisp Vulkan driver; `desktop-media/macos-vulkan.sh` builds the driver and loader) |
| Settings, text (`desktop-config/src/`) | `config.rs` (`config/defaults_gen.rs`, `config/enums.rs`, `config/host.rs`, `config/migrate.rs`, `config/profiles.rs`, `config/store.rs`, `config/art.rs`), `strings.rs`, `strings/` |
| Artwork and game facts | `desktop/src/game_info.rs` (keys, HTTPS, the record and image cache) over `nightjar_policy::game_info`; the HTTPS is `desktop/src/update_net.rs`'s `exchange` |

`cfg(unix)` includes macOS: Linux-only code takes `target_os = "linux"`. A changed shader `.spv` needs its Metal translation from `protocol/desktop-media/src/metal/generate.py`.

## The Android app

Gradle builds one module, `app/`, in the flavors `nonRoot` and `root`. Its task [`buildRustEngine`](../app/build.gradle) builds `nightjar-android` for each Android ABI, and `buildHostNightjarCore` builds the host library (`hostNativeLibs`) on `java.library.path` so unit tests run directly against real `nightjar_core` without JNI shims.

A stream and the shared JSON models run on the Rust engine, `nightjar-core`, through the pure-JDK JNI holders [`NightjarJni`](../app/src/main/java/com/limelight/nvstream/jni/NightjarJni.java), [`NightjarPolicyJni`](../app/src/main/java/com/limelight/nvstream/jni/NightjarPolicyJni.java), [`NightjarInputJni`](../app/src/main/java/com/limelight/nvstream/jni/NightjarInputJni.java), [`NightjarHttpJni`](../app/src/main/java/com/limelight/nvstream/jni/NightjarHttpJni.java), [`NightjarPinJni`](../app/src/main/java/com/limelight/nvstream/jni/NightjarPinJni.java), [`NightjarConsoleJni`](../app/src/main/java/com/limelight/nvstream/jni/NightjarConsoleJni.java), [`NightjarDiscoveryJni`](../app/src/main/java/com/limelight/nvstream/jni/NightjarDiscoveryJni.java), [`NightjarHostsJni`](../app/src/main/java/com/limelight/nvstream/jni/NightjarHostsJni.java), and [`NightjarSessionJni`](../app/src/main/java/com/limelight/nvstream/jni/NightjarSessionJni.java) (excluded from Robolectric sandbox acquisition by `NightjarAndroidConfigurer`) backed by `protocol/android/src/` (`lib.rs`, `capabilities.rs`, `uri.rs`, `policy.rs`, `policy_connect.rs`, `policy_input/`, `policy_quality.rs`, `policy_recovery.rs`, `policy_roaming.rs`, `policy_video_codec.rs`, `policy_video_pacing.rs`, `policy_video_queue.rs`, `display_profile.rs`, `connect.rs`, `connect_launch.rs`, `session_actions.rs`, `http.rs`, `http_folders.rs`, `http_launch.rs`, `http_probes.rs`, `pin.rs`, `update.rs`, `console/`, `mdns.rs`, `wol.rs`, `stun.rs`, `usb.rs`, `hosts.rs`, `hosts_meta.rs`, `hosts_manager.rs`, `hosts_poller.rs`, `host_flows.rs`, `hosts_power_flows.rs`, `identity.rs`, `slab.rs`). [`RustBridge`](../app/src/main/java/com/limelight/nvstream/jni/RustBridge.java) delegates stream natives there and binds `MoonBridge` callbacks (`protocol/android/src/bridge.rs`, `listener.rs`, `renderers.rs`) lazily on stream start, so decoders and input see one interface. [`StreamEngine`](../app/src/main/java/com/limelight/nvstream/jni/StreamEngine.java) records the engine and why a stream could not start. `libmoonlight-core.so` is a slim native library (`app/src/main/jni/moonlight-core/`) for AAudio output, DualSense USB audio, haptics and microphone, and the controller-type tables; it does not stream.

| Part | `app/src/main/java/com/limelight/` |
| --- | --- |
| Stream activity | `Game.java`, with `GameConnection.java`, `GameStreamUiSetup.java`, `GameMoveFlow.java`, `GameTerminationFlow.java`, `GameRecovery.java`, `GameVideoOutput.java`, `GameVideoRefresh.java` (and [`CompositorWatch`](../app/src/main/java/com/limelight/binding/video/CompositorWatch.java) / [`CompositorHeartbeat`](../app/src/main/java/com/limelight/ui/CompositorHeartbeat.java)), `GameTouchInput.java`, `GameOverlays.java`, `StreamGuests.java` |
| Connection | `nvstream/NvConnection.java`, `nvstream/NvConnectionAudio.java`, `nvstream/NvConnectionActions.java`, `nvstream/NvConnectionInput.java`, `nvstream/NvConnectionLaunch.java`, `nvstream/NvConnectionPreflight.java`, `nvstream/http/` (serverinfo, pairing, [`HostCapabilities`](../app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java)), `nvstream/jni/` |
| Video | `binding/video/`, led by `binding/video/MediaCodecDecoderRenderer.java` |
| Input, audio | `binding/input/ControllerHandler.java`, `binding/input/driver/`, `binding/input/touch/`, `binding/input/pad/` (the on-screen controller and its layout editor), `binding/audio/` |
| Settings | `preferences/PreferenceConfiguration.java` (`preferences/SettingsCatalogGen.java`, `preferences/PreferenceResolution.java`, `preferences/PreferenceMigrations.java`, `preferences/PreferenceReader.java`), `preferences/HostStreamSettings.java` (`preferences/HostFrontendExporter.java`, `preferences/HostStreamProfileSummary.java`, `preferences/HostStreamSettingDialogs.java`), `preferences/StreamSettings.java` |
| PCs, library, UI | `computers/`, `console/`, `ui/`, `grid/` |
| Artwork and game facts | `console/GameInfo.java` (keys, `NightjarHttpJni` `WebPkiTransport`, cache), `console/GameInfoModel.java` (`nightjar_policy::game_info` through `protocol/android/src/game_info.rs`), `console/GameInfoFilter.java` |
| PIN lock, updates | `pin/` (over `nightjar_core::pin_lock`), `update/` (over `nightjar_policy::update` and `nightjar_core::update`) |
| Native API, files | `nativeapi/`, `files/` |

Native code beside the engines, in `app/src/main/jni/`: `pyrowave-renderer/`, `vulkan/`, `evdev_reader/`, `window-dataspace/`.

## Checklists

### A setting

Desktop:

1. Declare the setting in `protocol/policy/settings.toml` and run `python scripts/generate-settings.py` (which updates `protocol/policy/src/settings_catalog_gen.rs`, `protocol/desktop-config/src/config/defaults_gen.rs`, `app/src/main/java/com/limelight/preferences/SettingsCatalogGen.java`, and `docs/settings.md`), then add the field to [`Settings`](../protocol/desktop-config/src/config.rs) with a doc comment and its default. A stored name is never renamed: an old value is read with a serde alias or in [`migrate`](../protocol/desktop-config/src/config/migrate.rs). Per-PC and per-game values are [`Overrides`](../protocol/desktop-config/src/config/profiles.rs).
2. A row in [`settings_rows`](../protocol/desktop-ui/src/ui/settings.rs), in the section the setting belongs to.
3. Its words: the English literal inside [`tr`, `trf`, `en`](../protocol/desktop-config/src/strings.rs) is the key, and every table in `protocol/desktop-config/src/strings/` needs an entry for it. A test fails for a missing one.
4. Tests in `protocol/desktop-ui/src/ui/settings_tests.rs`.

Android:

1. Declare the key, default, choices/ranges, and help text in `protocol/policy/settings.toml`, then run `python scripts/generate-settings.py` to regenerate [`SettingsCatalogGen`](../app/src/main/java/com/limelight/preferences/SettingsCatalogGen.java) and `docs/settings.md` (the documentation check fails when any generated target is out of date).
2. The preference in `app/src/main/res/xml/preferences.xml`, its texts in `app/src/main/res/values/strings.xml` and each translated `strings.xml`.
3. Its reading in [`PreferenceConfiguration`](../app/src/main/java/com/limelight/preferences/PreferenceConfiguration.java) / [`PreferenceReader`](../app/src/main/java/com/limelight/preferences/PreferenceReader.java). A key is renamed only with a migration of its stored values ([`PreferenceMigrations`](../app/src/main/java/com/limelight/preferences/PreferenceMigrations.java)), and retired keys never return: [`SettingsKeysTest`](../app/src/test/java/com/limelight/preferences/SettingsKeysTest.java) and [`SettingsMigrationsAndChoicesTest`](../app/src/test/java/com/limelight/preferences/SettingsMigrationsAndChoicesTest.java).

A decision both apps make from the setting goes into `protocol/policy/src/`.

### A control message

1. The id in [`ids.rs`](../protocol/core/src/ids.rs) and a module in `protocol/core/src/`.
2. The engine: `protocol/client/src/control/messages.rs` to send, `events.rs` to receive. It is sent only to a host that announced it.
3. Android: `protocol/android/src/lib.rs` and its [`RustBridge`](../app/src/main/java/com/limelight/nvstream/jni/RustBridge.java) method.
4. Desktop: the handler in `protocol/desktop-media/src/stream/`.
5. Tests: `protocol/client/tests/control_messages_contract.rs` and [`MoonBridgeControlMessagesTest`](../app/src/test/java/com/limelight/nvstream/jni/MoonBridgeControlMessagesTest.java).

### A feature that needs the host

Add it to [`Feature`](../protocol/policy/src/http/capabilities.rs) with the evidence that proves it (a serverinfo field, a DESCRIBE line, a native API capability merged by [`with_native`](../protocol/policy/src/http/capabilities.rs)), and to the Java [`HostCapabilities`](../app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java). Both read the recorded replies in `app/src/test/resources/protocol/capabilities/`, checked by `protocol/client/tests/capabilities_contract.rs`. Gate on the feature, never on a host's name.

## The narrowest test

| Change | Test |
| --- | --- |
| Engine | `cargo test -p nightjar-core` in `protocol/`. `protocol/client/tests/` runs standalone loopback and native API tests (`session_loopback.rs`, `control_loopback.rs`, `native_api.rs`), while host interop tests run from `tests/interop/` in the root workspace. |
| Bytes from the network | The property tests `protocol/core/src/fuzz.rs`, `protocol/client/src/fuzz_tests.rs` and the `fuzz_tests.rs` under `http/` and `rtsp/`. `PROPTEST_CASES` raises the count. |
| Same bytes as the classic C engine for audio, video and FEC | `cargo test -p nightjar-core --test golden_audio --test golden_video --test golden_fec` (digests recorded from it in `protocol/client/tests/golden/golden.txt`) |
| A policy | Its module tests, and recorded traces under `protocol/policy/tests/traces/` replayed by `protocol/policy/tests/traces.rs` |
| Desktop UI | `protocol/desktop-ui/src/ui/tests.rs` draws pages and dialogs without a window; `protocol/desktop-ui/src/shot.rs` rasterises them for screenshots. |
| Android | `gradlew :app:testNonRootDebugUnitTest --tests <class>` (JUnit and Robolectric, under `app/src/test/java/`). The unit tests do not compile the JNI code: `gradlew :app:externalNativeBuildNonRootDebug` does. `scripts/test-native.sh` runs the native C tests, `scripts/emulator-smoke.py` drives the app on an emulator. |
| Documentation | `node scripts/check-doc-links.mjs`, and `scripts/generate-settings.py` leaving `docs/settings.md` unchanged |

[Building](building.md) has the toolchains each of these needs.

## What has gone wrong before

The fix history repeats a few shapes. Check a change against them before calling it done.

- **One app only.** A setting, a text or a behaviour that reached Android or desktop and not the other. `with` on the file you changed shows what the other side touched last time.
- **One language only.** A new text needs every table in `protocol/desktop-config/src/strings/` and every translated `strings.xml`.
- **Gated by name or model.** A feature follows what the host announced and what the device's decoder reports at run time, not a host brand, a vendor or an Android version.
- **Recovery.** A reconnect, a network change or a stopped stream meets state the first launch set. The cases live in `protocol/desktop-media/src/stream/recovery.rs`, `protocol/policy/src/recovery.rs` and [`GameRecoveryTest`](../app/src/test/java/com/limelight/GameRecoveryTest.java).
- **Linux code compiled for macOS.** `cfg(unix)` is both; the macOS build runs only in CI.
