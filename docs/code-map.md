# Code map

[Rubylight docs](README.md) › [For developers](README.md#for-developers) · [Architecture](architecture.md) · [Build guide](building.md) · [Developer guide](../rust/README.md) · [Nightjar's map](../client/docs/code-map.md)

[Architecture](architecture.md) explains how a stream works. This page says where each part lives, which files change together and which test proves a change. It is written for people and coding agents changing the host; Nightjar has [its own map](../client/docs/code-map.md).

The paths and names on this page are checked: `python tools/code_map.py check` fails when a file is gone, or when a name in a link's text no longer occurs in the linked file. Correct the page in the change that moves the code.

## Look it up

| Question | Command |
| --- | --- |
| What is in this folder? | `python tools/code_map.py index rust/host/stream/src/stream` prints lines, tests and the header sentence of each file; `--dirs` sums folders |
| What else changes when this file changes? | `python tools/code_map.py with rust/core/config/src/config.rs` |
| Where have bugs been found? | `python tools/code_map.py hot rust/windows` lists fix commits per file |
| Which checks cover my change? | `tools\dev.ps1 check -DryRun` |
| Run one Cargo command in a warm target slot? | `python tools/dev.py cargo -- <args>` ([dev_cargo.py](../tools/dev_cargo.py)) |
| Batch-edit or verify all locales? | `python tools/i18n.py check` (or `apply`, `rename`, `prune` with `tools/i18n_glossary.json`) |
| Scaffold boilerplate or load a workflow skill? | `python tools/scaffold.py --help` and `.agents/skills/` |

Each Rust module opens with a `//!` header saying what it owns. The index is built from those headers: write one for a new module, and keep it true when a module's job changes.

## Crates

| Crate | Folder | Holds |
| --- | --- | --- |
| `rubylight-protocol` | `client/protocol/core/` | Wire formats shared with Nightjar, `no_std`: each extension message in a module of its own, and every message id in [`ids.rs`](../client/protocol/core/src/ids.rs). |
| `rubylight-wire` | `client/protocol/wire/` | Shared `std` wire codecs, packet crypto, RTSP framing, and input decoding for Rubylight and Nightjar. |
| `rubylight-native-api` | `client/protocol/native-api/` | The [native API](native-api.md)'s wire types, versioned on their own. |
| `rubylight-traverse` | `client/protocol/traverse/` | Internet play shared with Nightjar: reaching a paired host from outside its network without forwarded ports (DHT records, STUN, PCP and NAT-PMP, hole punching, the tunnel). The host uses it from `rust/host/stream/src/internet_play.rs`. |
| `butterpollo-{crypto, config, media-policy, input-policy, session}` | `rust/core/` | Protocol, state and policy without a Windows call: crypto/pairing (`crypto/`), host configuration (`config/`), media/encoder/network/RTSP policy (`media-policy/`), input/couch-menu policy (`input-policy/`), and state/session/launchers (`session/`). Decisions are plain functions over plain values, tested on any OS. |
| `butterpollo-win-{abi, graphics, display, io}` | `rust/windows/` | Domain-split Windows platform crates: C FFI bindings and compiled HLSL shaders (`abi/`), Direct3D capture/conversion/encoding (`graphics/`), physical/virtual display topology, arrangement and recovery (`display/`), and audio/input/process/network/launcher/Nucleus I/O (`io/`). |
| `butterpollo-{stream, webrtc, web}`, `butterpollo` | `rust/host/` | Host runtime split into stream (`rust/host/stream/`), WebRTC (`rust/host/webrtc/`), web/console/native API (`rust/host/web/`), and thin binary assembly (`rust/host/`): it asks core what to do and windows to do it. [The host integration tests](../tests/host/tests/reconnect_tests.rs) live in `tests/host/` (`butterpollo-host-tests`) and [the interop tests](../tests/interop/tests/stream_control.rs) live in `tests/interop/` (`rubylight-interop`), both enabling `test-support` as dev-dependencies. |
| console | `rust/web/` | The Svelte console, built with Vite and served by the host from files. |
| service, setup, drivers | `rust/service/`, `rust/setup/`, `rust/drivers/` | The Windows service and the display and virtual gamepad drivers are workspaces of their own. |
| NVIDIA NGX adapter | `rust/truehdr-runtime/` | An MSVC DLL between NVIDIA's NGX and the host, shipped in the NVIDIA component's archive (`build.ps1 -Nvidia`); outside the workspace. |

A behaviour usually has three homes. `rust/core/media-policy/src/encoder_policy.rs` decides, `rust/windows/graphics/src/amf.rs` applies it and `rust/host/stream/src/stream.rs` calls both; displays (`display_policy.rs`, `display.rs`, `display_session.rs`) and input (`input_policy.rs`, `input.rs`, `stream/control.rs`) follow the same split. Put the decision in core, where its test needs no hardware.

## A stream, hop by hop

| Step | Where |
| --- | --- |
| Launch or resume | [`launch`, `resume`, `start`, `launch_back_half`](../rust/host/stream/src/nvhttp/launch.rs), whose steps are in [`launch_steps.rs`](../rust/host/stream/src/nvhttp/launch_steps.rs); the settings in force for this app and device come from `rust/host/stream/src/stream/config.rs` |
| RTSP | `rust/host/stream/src/rtsp_server.rs` negotiates and calls [`Media`](../rust/host/stream/src/stream.rs)'s `start`; the extension lines it offers are [`describe_lines`](../rust/core/media-policy/src/extensions.rs) |
| Session thread | [`spawn_session`](../rust/host/stream/src/stream/video.rs), then [`start_media`, `spawn_audio`](../rust/host/stream/src/stream/video/setup.rs), the loop [`run`](../rust/host/stream/src/stream/video/run.rs) and `rust/host/stream/src/stream/video/teardown.rs` |
| Capture | The capture thread in `rust/host/stream/src/stream.rs` publishes the newest frame through `rust/host/stream/src/stream/capture.rs`; backends and fallback are [`Capture`, `open_stream_capture`](../rust/windows/graphics/src/capture.rs) |
| Convert | `rust/windows/graphics/src/compute.rs` (D3D12 compute), `rust/windows/graphics/src/gpu_color.rs` (D3D11), both from `rust/windows/abi/src/shaders/color.hlsl` |
| Encode | [`configure_encoder`](../rust/host/stream/src/stream.rs) builds an [`Encoder`](../rust/windows/graphics/src/encoder.rs), an enum over AMF, NVENC and PyroWave; which one is [`backend`](../rust/core/media-policy/src/encoder_policy.rs) |
| Packetize, send | [`spawn`, `submit`](../rust/host/stream/src/video_send.rs), `rust/host/stream/src/pyrowave_send.rs` for PyroWave, `rust/host/stream/src/stream/slices.rs` for the inline path; formats in `rust/core/media-policy/src/packet.rs` |
| Control, input | `rust/host/stream/src/stream/control.rs` and `stream/control/` run ENet and dispatch each message by its id; injection is `rust/windows/io/src/input.rs` and `input/` |
| Audio | `rust/host/stream/src/stream/audio.rs`, `audio_encoder.rs` and `audio_sink.rs` beside it; capture is `rust/windows/io/src/audio.rs` |
| End | [`session_begin`, `session_teardown`](../rust/host/stream/src/session_control.rs) are the two points every start and end pass; states are [`Machine`](../rust/core/session/src/session_state.rs); [`stop_streams`](../rust/host/stream/src/nvhttp/quit.rs) ends from HTTP |

Threads carry these names in a debugger and in `rust/host/stream/src/stall_watch.rs` lines: `session`, `control`, `capture`, `gpu-recovery`, `video-packetize`, `video-send`, `pyrowave-packetize`, `pyrowave-send`, `audio`, `audio route`, `server commands`, `gamepads`, `session-journal`.

Deadlines, by name: [`ENCODER_RECOVERY`](../rust/host/stream/src/stream/video.rs), [`RECOVERY_RETRY`, `DISPLAY_RETURN_WAIT`, `CAPTURE_START_WAIT`](../rust/host/stream/src/stream.rs), [`TIMEOUT`](../rust/host/stream/src/stream/device_recovery.rs) for a lost GPU, [`ping_timeout`](../rust/host/stream/src/network.rs), [`UNATTENDED_SWEEP`](../rust/host/stream/src/stream/control.rs), [`TEARDOWN_WAIT`](../rust/host/stream/src/session_control.rs).

## Where to start

| Area | Decides: `rust/core/` | Does: `rust/windows/{abi,graphics,display,io}/src/` | Wires: `rust/host/{stream,webrtc,web}/src/` |
| --- | --- | --- | --- |
| Encoder | `media-policy/src/encoder_policy.rs`, `media-policy/src/encoder_gpu.rs`, `media-policy/src/encoder_backlog.rs`, `media-policy/src/nvenc.rs`, `media-policy/src/codec_support.rs`, `media-policy/src/amf_vpe.rs`, `media-policy/src/image_quality.rs` | `graphics/src/encoder.rs`, `amf.rs`, `amf/` (`amf/vpe.rs`), `nvenc.rs`, `nvenc/`, `pyrowave.rs`, `pyrowave/` | `stream.rs`, `stream/video/run.rs`, `stream/video/run/` |
| Capture | `media-policy/src/capture_policy.rs`, `media-policy/src/app_window.rs` (one app's window) | `graphics/src/capture.rs`, `capture/` (`capture/amd_direct.rs`, `capture/app_window.rs`) | `stream/capture.rs`, `stream/app_window.rs` |
| Colour, HDR | `media-policy/src/hdr.rs`, `media-policy/src/hdr_dynamic.rs`, `media-policy/src/dovi.rs` | `graphics/src/compute.rs`, `compute/`, `gpu_color.rs`, `gpu_color/`, `color.rs`, `compute_hdr_stats.rs` | `stream.rs` |
| NVIDIA RTX HDR, RTX Video | `media-policy/src/nvidia_enhance.rs` | `graphics/src/nvidia_enhance.rs`, `rtx_video.rs`, `rtx_profiles.rs`, `truehdr.rs`, `abi/src/shaders/rtx_video.hlsl` | `nvidia_enhance.rs` |
| NVIDIA component (download, checks, install) | `media-policy/src/nvidia_component.rs` | `graphics/src/nvidia_enhance.rs` (loads the adapter) | `nvidia_component.rs`, `web/nvidia_component.rs` |
| Displays | `media-policy/src/display_policy.rs`, `media-policy/src/display_session_machine.rs`, `media-policy/src/display_caps.rs`, `media-policy/src/edid.rs`, `media-policy/src/topology.rs` | `display/src/display.rs`, `display/`, `display_arrangement.rs`, `display_arrangement/`, `display_recovery.rs`, `display_recovery/` | `display_session.rs`, `display_session/`, `remote_display.rs` |
| Pacing, bitrate, loss | `media-policy/src/stream_policy.rs`, `media-policy/src/network_pacing.rs`, `media-policy/src/pacing_high_hz.rs` (above 120 fps), `media-policy/src/adaptive_bitrate.rs`, `media-policy/src/adaptive_fec.rs`, `media-policy/src/link_rate.rs`, `media-policy/src/ltr.rs`, `media-policy/src/congestion.rs` | `graphics/src/timing.rs`, `io/src/limiter.rs` | `video_send.rs`, `video_send/`, `stream/qos.rs` |
| Input, controllers | `input-policy/src/input.rs`, `input-policy/src/input_policy.rs`, `input-policy/src/steam_pad.rs`, `input-policy/src/dualsense.rs`, `input-policy/src/switch2.rs`, `input-policy/src/virtual_hid.rs` (virtual mouse and touchpad), `input-policy/src/usbip.rs`, `input-policy/src/dsu.rs` | `io/src/input.rs`, `input/` (`input/virtual_hid.rs`, `input/gamepad_backend.rs`, `input/gamepad_backend/recovery.rs`, `input/switch2_pad.rs`), `local_input.rs` | `stream/control.rs`, `stream/control/`, `controller_sessions.rs`, `dsu.rs` |
| Audio, microphone | `media-policy/src/audio.rs`, `media-policy/src/audio_dsp/`, `media-policy/src/audio_passthrough.rs`, `session/src/mic.rs` | `io/src/audio.rs`, `audio_app_route.rs`, `mic.rs` | `stream/audio.rs`, `stream/audio_sink.rs`, `mic.rs` |
| Sessions, pairing | `session/src/session.rs`, `session/src/session/`, `session/src/session_state.rs`, `session/src/session_history.rs` (the ended streams the console lists), `crypto/src/pairing.rs`, `crypto/src/auth.rs`, `crypto/src/crypto.rs`, `session/src/state.rs`, `session/src/state/`, `session/src/multiclient.rs` (guests), `session/src/auto_lock.rs` | | `nvhttp.rs`, `nvhttp/`, `rtsp_server.rs`, `session_control.rs`, `session_history.rs`, `state.rs`, `state/`, `multiclient.rs`, `web/session_guests.rs`, `native_api/guests.rs`, `auto_lock.rs` |
| Settings | `config/settings.toml` (catalog), `config/src/config.rs`, `config/src/config/` (`config/src/config/retired/`, `config/src/config/tests/`), `session/src/device_overrides.rs`, `media-policy/src/stream_preset.rs`, `session/src/settings_backup.rs`, `session/src/settings_backup/` | | `web/config.rs`, `console/settings.rs`, `stream/config.rs`, `settings_backup.rs`, `web/backup.rs` |
| Library, launch | `session/src/library.rs`, `session/src/library/`, `session/src/steam.rs`, `session/src/steam/`, `session/src/playnite.rs`, `session/src/playnite/`, `session/src/catalog.rs`, `input-policy/src/menu_commands.rs`, `session/src/connection_commands.rs`, `session/src/steamgriddb.rs` (SteamGridDB covers) | `io/src/process.rs`, `process/`, `launcher.rs`, `steam.rs` | `process.rs`, `process/`, `library.rs`, `launcher_watch.rs`, `steamgriddb.rs` |
| Neural Rendering | `session/src/neural.rs`, `session/src/neural/` (pure archive, manifest, eligibility, GPU and ini policy) | `io/src/hip.rs`, `io/src/process.rs` | `neural.rs`, `neural/`, `neural/folder.rs`, `neural/folder/`, `web/neural.rs`, `native_api/neural.rs` |
| Co-op, background | `session/src/nucleus.rs`, `session/src/nucleus/`, `session/src/background.rs` | `io/src/nucleus.rs`, `nucleus/` | `nucleus.rs`, `stream/instance_audio.rs`, `stream/background.rs` |
| Frame Warp 2 | `media-policy/src/frame_warp/` (`media-policy/src/frame_warp/v2.rs` composes what a client gets from `media-policy/src/frame_warp/rls.rs`, `media-policy/src/frame_warp/lens_fast.rs`, `media-policy/src/frame_warp/gyro.rs` and `media-policy/src/frame_warp/confidence.rs`), `session/src/session/frame_warp.rs`, `session/src/session/frame_warp/` | `graphics/src/compute_frame_warp.rs` (no second pass for Frame Warp 2) | `stream/frame_warp.rs`, `stream/frame_warp_prior.rs` (`frame_warp.json`), `stream/control/extensions.rs`, `stream/control/input.rs` |
| Client extensions | `media-policy/src/extensions.rs`, `session/src/native.rs`, `session/src/native/`, `session/src/roaming.rs` | `io/src/clipboard.rs`, `files.rs`, `shared_folder.rs` | `extensions.rs`, `native_api.rs`, `native_api/`, `stream/roaming.rs`, `usb_forward.rs` |
| Internet play | (`rubylight-traverse`) | | `internet_play.rs`, `internet_play/`, `network.rs`, `network/` (`network/portmap.rs`) |
| Invites to play over the internet | `session/src/invites.rs` (links, invited devices) | | `invites.rs`, `nvhttp/pairing.rs`, `multiclient.rs`, `internet_play.rs`, `native_api/guests.rs`, `web/session_guests.rs` |
| Console, API | `crypto/src/auth.rs`, `session/src/log_analysis.rs` | | `web.rs`, `web/`, `console/`, `web_sessions.rs`, `webhooks.rs` |
| Browser play | `session/src/webrtc/` | | `webrtc/` |
| Update, service | `src/update_files.rs`, `src/update_recovery.rs` | `io/src/service.rs` | `updater.rs`, `updater/`, `maintenance.rs`, `maintenance/`, `service_tasks.rs` |
| Tray | `session/src/tray.rs` (app status, notice queue) | `io/src/tray.rs`, `tray/status.rs` (status, 7.0 badge colours and compositing, menu), `tray/icon.rs`, `tray/notices.rs` | `runtime.rs` (`user_action`, `show_app`) |

Outside those crates: the installer in `rust/setup/src/`, the service in `rust/service/src/`, the display and virtual gamepad drivers and their protocols in `rust/drivers/`, capture and encoder probes in `rust/probes/src/` (run with `python tools/dev.py cargo -- run -p butterpollo-probes --bin <name>`; several change real settings and say so in their header).

## Checklists

Each list names every home of one kind of change. `python tools/code_map.py with <file>` shows what recent changes of that kind touched.

### A host setting

1. Define the setting in `rust/core/config/settings.toml` (key, type, default, control, tier, `override_allowed`, `restart`, `visible_when`/`caution_when` rules, fallback console entry, optional `rust_accessor`/`validate`, and English copy) and run `python tools/generated_docs.py`. That regenerates `rust/core/config/src/config/{catalog_gen,retired_gen,console_global_gen,console_layers_gen}.rs`, `rust/web/src/lib/schema/*.gen.ts`, and the `## All settings` tables in `docs/configuration.md` (`python tools/generated_docs.py --check` enforces parity).
2. Complex accessors or parsers live on [`Config`](../rust/core/config/src/config.rs) with [`parse_boolean`, `parse_integer`](../rust/core/config/src/config/values.rs) reporting bad values. Tests go in `rust/core/config/src/config/tests/` (`rust/core/config/src/config/tests.rs` hub).
3. A 3.0 extension setting sets `settings_30 = true` in `rust/core/config/settings.toml` (generated into [`SETTINGS_30`](../rust/core/config/src/config/settings30.rs) and `rust/web/src/lib/schema/extensions.gen.ts`).
4. A renamed, folded, or removed key goes under `[retired]` in `rust/core/config/settings.toml` (generated into [`RENAMED_KEYS`, `REMOVED_KEYS`](../rust/core/config/src/config/retired.rs), with multi-key folding in `rust/core/config/src/config/retired/fold.rs`). Settings are renamed or retired, not dropped silently.
5. A per-app or per-device setting sets `override_allowed = true` in `rust/core/config/settings.toml` (checked by [`override_allowed`](../rust/core/config/src/config/overrides.rs)). The order is app and device, app, device, host ([`for_device`](../rust/core/session/src/device_overrides.rs)); a value set in the configuration beats a preset (`rust/core/media-policy/src/stream_preset.rs`).
6. The console thin wrappers in `rust/web/src/lib/schema/` re-export the generated `*.gen.ts` modules; `rust/web/src/lib/schema/tiers.ts` and `main.ts` wrap the generated tier and group tables (`tiers.gen.ts`). `overrides.gen.ts` lists the `override_allowed` keys for the console's override cascade cards (`rust/web/src/components/library/override-cascade.ts`, shown on a game's Per device panel and a device's Stream settings; `rust/web/OverrideCascade.test.mjs`). `rust/web/SettingsSchema.test.mjs`, `SettingsHome.test.mjs` and `SettingsTiers.test.mjs` hold the rules.
7. The console without scripts (`rust/host/web/src/console/settings.rs`) renders directly from the generated [`console_catalog`](../rust/core/config/src/config.rs); add translations in every file of `rust/assets/locale/`.
8. Add topic prose in `docs/configuration.md` if needed (the `## All settings` key tables regenerate from `settings.toml`) and a note in `rust/release/notes.d/`.

A setting's visible text says, in this order, what the user gets, what it costs (delay, picture quality, network, load, or what stops working) and when to change it ("Leave it on unless ..."); a value that can break streaming says so first. Facts from the code, the docs or our measurements only. Well-known terms (VRR, HDR, bitrate, HEVC) stay as they are, a lesser-known one gets a few words of explanation, and source-level internals belong in the console's details.

Saving goes through [`save_setting`](../rust/host/web/src/web/config.rs), which validates with `check_saved` before the atomic write.

### A control message

1. Reserve the id in [`ids.rs`](../client/protocol/core/src/ids.rs) and give the message a module in `client/protocol/core/src/`, as `clipboard_sync.rs` has.
2. Offer it: a field of [`Offered`](../rust/core/media-policy/src/extensions.rs) with its serverinfo field and DESCRIBE line, set per stream in [`stream_offered`](../rust/host/stream/src/extensions.rs). A client that asks for nothing new must see nothing new (`tests/host/tests/reconnect_tests/extension_tests.rs`).
3. Dispatch it in `rust/host/stream/src/stream/control.rs`; give it a worker beside that file when it keeps state, as `rust/host/stream/src/stream/clipboard_sync.rs` does.
4. Nightjar in the same change: `client/protocol/client/src/control/` (`messages.rs`, `events.rs`, `session.rs`), then Android and desktop ([Nightjar's map](../client/docs/code-map.md#a-control-message)).
5. `docs/control-messages.md`, and the tests `client/protocol/client/tests/control_messages_contract.rs` and a file under `tests/host/tests/reconnect_tests/`.

### A capability a client can see

The host says what it offers in three places: serverinfo fields ([`serverinfo_fields`](../rust/core/media-policy/src/extensions.rs)), DESCRIBE lines and the native API's capabilities (`client/protocol/native-api/src/capabilities.rs`). Nightjar reads them into [`Feature`, `HostCapabilities`](../client/protocol/policy/src/http/capabilities.rs) and gates on that, never on a host's name. `client/protocol/client/tests/capabilities_contract.rs` checks the reading against recorded serverinfo replies.

### A native API endpoint

1. Types in `client/protocol/native-api/src/`. Within v1, changes only add: a response may gain fields, a request refuses unknown ones.
2. The route in [`router`](../rust/host/web/src/native_api.rs), the handler in `rust/host/web/src/native_api/`, what it reads from host state in `rust/core/session/src/native/`. The caller is the paired device of the TLS certificate; its permissions are read on every request.
3. Tests in `tests/host/tests/nvhttp_parity/native_tests/`; the contract in `docs/native-api.md`.
4. Nightjar: `client/protocol/client/src/http/native.rs`, then both apps.

### A console page or API route

1. The route and its methods in [`token_catalog`](../rust/host/web/src/web.rs) when an API token may call it, then the arm in [`api`](../rust/host/web/src/web.rs). [`guard`](../rust/host/web/src/web.rs) does sign-in, CSRF and token scope for every route.
2. The handler in `rust/host/web/src/web/`, with tests beside it. `docs/api.md` lists the route.
3. A typed call in `rust/web/src/lib/api.ts`, a component under `rust/web/src/components/`, a page in `rust/web/src/pages/` routed from `rust/web/src/App.svelte`.
4. A `*.test.mjs` beside `rust/web/package.json`; a reply in `rust/web/scripts/screenshots.cjs` when a screenshot shows the page.

## The narrowest test

| Change | Test |
| --- | --- |
| A decision in core | The module's own tests: `python tools/dev.py cargo -- test -p butterpollo-<crate> --locked <module>` (`crypto`, `config`, `media-policy`, `input-policy`, `session`). No GPU, any OS. |
| Launch, resume, reconnect, control messages, roaming, session end | `tests/host/tests/reconnect_tests.rs` and the files under `tests/host/tests/reconnect_tests/`: the real HTTP, RTSP, ENet and session workers on loopback with a fake capture, encoder and input. Start here for a session bug: it needs no GPU, driver or service. |
| Anything Nightjar and the host must agree on | `tests/interop/` (`python tools/dev.py cargo -- test -p rubylight-interop --locked`) runs Nightjar's engine against this host in one process. |
| Moonlight HTTP replies | `tests/host/tests/nvhttp_parity/parity_tests.rs`, `tests/host/tests/nvhttp_parity/native_tests/` |
| Bytes from the network | The property tests in `rust/core/session/src/fuzz.rs` and `client/protocol/core/src/fuzz.rs`: no input panics, and what decodes encodes back. `PROPTEST_CASES` raises the count. |
| Capture, conversion, encoders, displays, input devices | Tests marked `#[ignore]` in `rust/windows/{abi,graphics,display,io}/src/`, each with the reason it needs hardware; the [developer guide](../rust/README.md#running-the-native-checks) has the command. They read `BUTTERPOLLO_TEST_*` variables for decoders and report files. |
| Console | `node --test "*.test.mjs"` in `rust/web/` renders components on the server through `rust/web/test-server.mjs`, without a browser. The `*.browser.cjs` files need Playwright. |
| A packaged host | `rust/tests/interop.py`, `rust/tests/web_api.py` and their neighbours start an isolated host; `rust/release/e2e.py` streams one end to end. Exit code 3 is an AMD encoder hang: stop, do not retry. |
| A real device or a second machine | A case in `tools/hw/catalog.yaml`, run by `tools/hw/run.py` (`tools\hw\run.ps1 -Batch <device>`). `tools/hw/laptop-host.ps1` makes the laptop the host and Nightjar on this PC the client; its laptop side is `tools/hw/laptop-host/start.ps1` (over ssh: locks, refusals, the Interactive task) and `tools/hw/laptop-host/host.ps1` (the test host and `motion_probe` in the console session). |

`tools\dev.ps1 check` maps changed paths to these (`plan()` in `tools/dev.py`): host crates and the crates that depend on them to fmt, clippy and tests; `client/protocol/` to `cargo test` in that workspace; `client/app/` sources to the Gradle tests of the touched classes; `rust/web/` to `npm run check`, the node tests and the build; `rust/release/` and `tools/` to their Python suites; a generated doc's sources to `tools/generated_docs.py --check`. Markdown maps to nothing. Each build stage takes its own lane (`tools/build_lock.py`) and the Cargo stages a warm target slot; `tools/ci.py` (`tools\dev.ps1 ci`) watches GitHub CI for a `wip/**` branch and `tools/remote-check.ps1` runs the check on the laptop.

## Evidence

| Evidence | Where |
| --- | --- |
| Host log | `logs\butterpollo.log` in the profile, 32 MiB a file with four older ones kept (`rust/core/session/src/logfile.rs`). `RUST_LOG` overrides the saved level (`rust/host/stream/src/logging.rs`). |
| Known failures in words | [`RULES`](../rust/core/session/src/log_analysis.rs) maps warnings and errors to the explanations the console shows. Read it to learn what a log line means; add a rule when a new failure has a known cause. |
| A stalled stream thread | `rust/host/stream/src/stall_watch.rs` logs where it is. |
| Console warnings | `rust/host/stream/src/host_warnings.rs`, kept in `host-warnings.json` until dismissed |
| Support bundle | `rust/host/stream/src/maintenance/support_bundle.rs` writes it with the logs redacted by `rust/core/session/src/log_redact.rs`; the crash memory dump only on request (`include_dump`). |
| Per-frame timings | `BUTTERPOLLO_TIMING_RECORD` writes the [timing record](timing-record.md); `rust/release/timing_report.py` turns host and receiver records into a stage table. |
| Which stage lost the stream's time | The console's **Stream details & performance**, the end-of-stream `stream stutter report` line, and `rust/core/media-policy/src/stutter_report.rs` over the stage samples `rust/core/media-policy/src/performance.rs` keeps. |
| A stream run | `rust/release/e2e.py` leaves `result.json`, `frames.csv` and the host's record in the run's folder. |
| Faults on purpose | `rust/host/stream/src/soak_fault.rs` (debug hosts only) and the send switches `BUTTERPOLLO_TEST_SEND_LOSS`, `BUTTERPOLLO_TEST_SEND_JITTER` and `BUTTERPOLLO_TEST_SEND_OUTAGE` |

## What has gone wrong before

The fix history repeats a few shapes. Check a change against them before calling it done.

- **State across a reconnect.** A resume, a second device, a roam or a host restart meets state the first launch set: HDR mode, display layout, overrides, audio route. Add the case to the reconnect matrix.
- **Which setting wins.** A value can come from the host, a preset, the device, the app, or the app for one device. Read it through `rust/host/stream/src/stream/config.rs`, never from the raw configuration.
- **Restore.** Whatever a stream changes on the PC (displays, Radeon settings, the frame limiter, audio routes, game files) is journaled before the change and restored on every end, a crash included. A new change needs both halves and a session-end test.
- **Work on the wrong thread.** A blocking call on the control loop, the packet sender or the audio thread shows as stutter far from its cause. Slow work gets its own worker, as `rust/host/stream/src/stream/qos.rs` and `rust/host/stream/src/stream/game_audio.rs` have.
- **One of two homes.** A setting missing from the console schema, a message the host offers and Nightjar never asks for, a text in one language. `with` shows the homes; the checklists above name them.
- **Offered by what one PC can do.** What the host offers follows the encoder's probe and the codec level, not the machine a change was written on.
