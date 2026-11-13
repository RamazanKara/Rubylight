# Code map

[Documentation](README.md) · [Architecture](architecture.md) · [Build guide](building.md) · [Developer guide](../rust/README.md) · [Nightjar's map](../client/docs/code-map.md)

[Architecture](architecture.md) explains how a stream works. This page says where each part lives, which files change together and which test proves a change. It is written for people and coding agents changing the host; Nightjar has [its own map](../client/docs/code-map.md).

The paths and names on this page are checked: `python tools/code_map.py check` fails when a file is gone, or when a name in a link's text no longer occurs in the linked file. Correct the page in the change that moves the code.

## Look it up

| Question | Command |
| --- | --- |
| What is in this folder? | `python tools/code_map.py index rust/host/src/stream` prints lines, tests and the header sentence of each file; `--dirs` sums folders |
| What else changes when this file changes? | `python tools/code_map.py with rust/core/src/config.rs` |
| Where have bugs been found? | `python tools/code_map.py hot rust/windows` lists fix commits per file |
| Which checks cover my change? | `tools\dev.ps1 check -DryRun` |

Each Rust module opens with a `//!` header saying what it owns. The index is built from those headers: write one for a new module, and keep it true when a module's job changes.

## Crates

| Crate | Folder | Holds |
| --- | --- | --- |
| `rubylight-protocol` | `client/protocol/core/` | Wire formats shared with Nightjar, `no_std`: each extension message in a module of its own, and every message id in [`ids.rs`](../client/protocol/core/src/ids.rs). |
| `rubylight-native-api` | `rust/core/native-api/` | The [native API](native-api.md)'s wire types, versioned on their own. |
| `butterpollo-core` | `rust/core/` | Protocol, state and policy without a Windows call. Decisions are plain functions over plain values, tested on any OS. |
| `butterpollo-windows` | `rust/windows/` | Everything that calls Windows, a GPU or a driver. |
| `butterpollo` | `rust/host/` | Servers, sessions and threads: it asks core what to do and windows to do it. `nightjar-core` is a test dependency, for [the interop tests](../rust/host/src/stream/reconnect_tests/interop_tests.rs). |
| console | `rust/web/` | The Svelte console, built with Vite and served by the host from files. |
| service, setup, driver | `rust/service/`, `rust/setup/`, `rust/drivers/` | The Windows service and the display driver are workspaces of their own. |

A behaviour usually has three homes. `rust/core/src/encoder_policy.rs` decides, `rust/windows/src/amf.rs` applies it and `rust/host/src/stream.rs` calls both; displays (`display_policy.rs`, `display.rs`, `display_session.rs`) and input (`input_policy.rs`, `input.rs`, `stream/control.rs`) follow the same split. Put the decision in core, where its test needs no hardware.

## A stream, hop by hop

| Step | Where |
| --- | --- |
| Launch or resume | [`launch`, `resume`, `start`, `launch_back_half`](../rust/host/src/nvhttp.rs); the settings in force for this app and device come from `rust/host/src/stream/config.rs` |
| RTSP | `rust/host/src/rtsp_server.rs` negotiates and calls [`Media`](../rust/host/src/stream.rs)'s `start`; the extension lines it offers are [`describe_lines`](../rust/core/src/extensions.rs) |
| Session thread | [`spawn_session`](../rust/host/src/stream/video.rs), then [`start_media`, `spawn_audio`](../rust/host/src/stream/video/setup.rs), the loop [`run`](../rust/host/src/stream/video/run.rs) and `rust/host/src/stream/video/teardown.rs` |
| Capture | The capture thread in `rust/host/src/stream.rs` publishes the newest frame through `rust/host/src/stream/capture.rs`; backends and fallback are [`Capture`, `open_stream_capture`](../rust/windows/src/capture.rs) |
| Convert | `rust/windows/src/compute.rs` (D3D12 compute), `rust/windows/src/gpu_color.rs` (D3D11), both from `rust/windows/src/shaders/color.hlsl` |
| Encode | [`configure_encoder`](../rust/host/src/stream.rs) builds an [`Encoder`](../rust/windows/src/encoder.rs), an enum over AMF, NVENC and PyroWave; which one is [`backend`](../rust/core/src/encoder_policy.rs) |
| Packetize, send | [`spawn`, `submit`](../rust/host/src/video_send.rs), `rust/host/src/pyrowave_send.rs` for PyroWave, `rust/host/src/stream/slices.rs` for the inline path; formats in `rust/core/src/packet.rs` |
| Control, input | `rust/host/src/stream/control.rs` runs ENet and dispatches each message by its id; injection is `rust/windows/src/input.rs` |
| Audio | `rust/host/src/stream/audio.rs`, `audio_encoder.rs` and `audio_sink.rs` beside it; capture is `rust/windows/src/audio.rs` |
| End | [`session_begin`, `session_teardown`](../rust/host/src/session_control.rs) are the two points every start and end pass; states are [`Machine`](../rust/core/src/session_state.rs); [`stop_streams`](../rust/host/src/nvhttp.rs) ends from HTTP |

Threads carry these names in a debugger and in `rust/host/src/stall_watch.rs` lines: `session`, `control`, `capture`, `gpu-recovery`, `video-packetize`, `video-send`, `pyrowave-packetize`, `pyrowave-send`, `audio`, `audio route`, `server commands`, `gamepads`, `session-journal`.

Deadlines, by name: [`ENCODER_RECOVERY`](../rust/host/src/stream/video.rs), [`RECOVERY_RETRY`, `DISPLAY_RETURN_WAIT`, `CAPTURE_START_WAIT`](../rust/host/src/stream.rs), [`TIMEOUT`](../rust/host/src/stream/device_recovery.rs) for a lost GPU, [`ping_timeout`](../rust/host/src/network.rs), [`UNATTENDED_SWEEP`](../rust/host/src/stream/control.rs), [`TEARDOWN_WAIT`](../rust/host/src/session_control.rs).

## Where to start

| Area | Decides: `rust/core/src/` | Does: `rust/windows/src/` | Wires: `rust/host/src/` |
| --- | --- | --- | --- |
| Encoder | `encoder_policy.rs`, `encoder_gpu.rs`, `encoder_backlog.rs`, `nvenc.rs`, `codec_support.rs` | `encoder.rs`, `amf.rs`, `amf/`, `nvenc.rs`, `nvenc/`, `pyrowave.rs` | `stream.rs`, `stream/video/run.rs` |
| Capture | `capture_policy.rs` | `capture.rs`, `capture/` | `stream/capture.rs` |
| Colour, HDR | `hdr.rs`, `hdr_dynamic.rs`, `dovi.rs` | `compute.rs`, `gpu_color.rs`, `color.rs`, `compute_hdr_stats.rs` | `stream.rs` |
| Displays | `display_policy.rs`, `display_caps.rs`, `edid.rs`, `topology.rs` | `display.rs`, `display/`, `display_arrangement.rs`, `display_recovery.rs` | `display_session.rs`, `remote_display.rs` |
| Pacing, bitrate, loss | `stream_policy.rs`, `network_pacing.rs`, `adaptive_bitrate.rs`, `adaptive_fec.rs`, `link_rate.rs`, `ltr.rs` | `timing.rs`, `limiter.rs` | `video_send.rs`, `video_send/`, `stream/qos.rs` |
| Input, controllers | `input.rs`, `input_policy.rs`, `steam_pad.rs`, `dualsense.rs` | `input.rs`, `input/` | `stream/control.rs`, `controller_sessions.rs` |
| Audio, microphone | `audio.rs`, `audio_dsp/`, `audio_passthrough.rs`, `mic.rs` | `audio.rs`, `audio_app_route.rs`, `mic.rs` | `stream/audio.rs`, `stream/audio_sink.rs`, `mic.rs` |
| Sessions, pairing | `session.rs`, `session/`, `session_state.rs`, `pairing.rs`, `auth.rs`, `crypto.rs`, `state.rs` | | `nvhttp.rs`, `nvhttp/`, `rtsp_server.rs`, `session_control.rs`, `state.rs` |
| Settings | `config.rs`, `config/`, `device_overrides.rs`, `stream_preset.rs` | | `web/config.rs`, `console/settings.rs`, `stream/config.rs` |
| Library, launch | `library.rs`, `steam.rs`, `playnite.rs`, `catalog.rs` | `process.rs`, `process/`, `launcher.rs`, `steam.rs` | `process.rs`, `library.rs`, `launcher_watch.rs` |
| Co-op, background | `nucleus.rs`, `nucleus/`, `background.rs` | `nucleus.rs`, `nucleus/` | `nucleus.rs`, `stream/instance_audio.rs`, `stream/background.rs` |
| Client extensions | `extensions.rs`, `native.rs`, `native/`, `roaming.rs` | `clipboard.rs`, `files.rs`, `shared_folder.rs` | `extensions.rs`, `native_api.rs`, `native_api/`, `stream/roaming.rs`, `usb_forward.rs` |
| Console, API | `auth.rs`, `log_analysis.rs` | | `web.rs`, `web/`, `console/`, `web_sessions.rs`, `webhooks.rs` |
| Browser play | `webrtc/` | | `webrtc/` |
| Update, service | `update_files.rs`, `update_recovery.rs` | `service.rs` | `updater.rs`, `updater/`, `maintenance.rs`, `service_tasks.rs` |

Outside the three crates: the installer in `rust/setup/src/`, the service in `rust/service/src/`, the display driver and its ring protocol in `rust/drivers/`, capture and encoder probes in `rust/windows/examples/` (run with `cargo run -p butterpollo-windows --example <name>`; several change real settings and say so in their header).

## Checklists

Each list names every home of one kind of change. `python tools/code_map.py with <file>` shows what recent changes of that kind touched.

### A host setting

1. An accessor on [`Config`](../rust/core/src/config.rs) with the default; parsers such as [`parse_boolean`, `parse_integer`](../rust/core/src/config/values.rs) report a bad value. Tests go in `rust/core/src/config/tests.rs`.
2. A setting of a 3.0 extension is also a row of [`SETTINGS_30`](../rust/core/src/config/settings30.rs), and a test there compares the rows with `rust/web/src/lib/schema/extensions.ts`.
3. A renamed or removed key: [`RENAMED_KEYS`, `REMOVED_KEYS`](../rust/core/src/config/retired.rs). Settings are renamed or retired, not dropped silently.
4. A per-app or per-device setting: [`override_allowed`](../rust/core/src/config/overrides.rs). The order is app and device, app, device, host ([`for_device`](../rust/core/src/device_overrides.rs)); a value set in the configuration beats a preset (`rust/core/src/stream_preset.rs`).
5. The console: an entry with label, description, default and control in its category file under `rust/web/src/lib/schema/`. The schema is written by hand and its defaults must equal the accessor's. `rust/web/SettingsSchema.test.mjs` and `rust/web/SettingsHome.test.mjs` hold the rules.
6. The console without scripts: a row in `rust/host/src/console/settings.rs`, with its label and description in every file of `rust/assets/locale/`.
7. `docs/configuration.md`: the topic section and the key table. No test checks this one.
8. A note in `rust/release/notes.d/`.

Saving goes through [`save_setting`](../rust/host/src/web/config.rs), which validates with `check_saved` before the atomic write.

### A control message

1. Reserve the id in [`ids.rs`](../client/protocol/core/src/ids.rs) and give the message a module in `client/protocol/core/src/`, as `clipboard_sync.rs` has.
2. Offer it: a field of [`Offered`](../rust/core/src/extensions.rs) with its serverinfo field and DESCRIBE line, set per stream in [`stream_offered`](../rust/host/src/extensions.rs). A client that asks for nothing new must see nothing new (`rust/host/src/stream/reconnect_tests/extension_tests.rs`).
3. Dispatch it in `rust/host/src/stream/control.rs`; give it a worker beside that file when it keeps state, as `rust/host/src/stream/clipboard_sync.rs` does.
4. Nightjar in the same change: `client/protocol/client/src/control/` (`messages.rs`, `events.rs`, `session.rs`), then Android and desktop ([Nightjar's map](../client/docs/code-map.md#a-control-message)).
5. `docs/control-messages.md`, and the tests `client/protocol/client/tests/control_messages_contract.rs` and a file under `rust/host/src/stream/reconnect_tests/`.

### A capability a client can see

The host says what it offers in three places: serverinfo fields ([`serverinfo_fields`](../rust/core/src/extensions.rs)), DESCRIBE lines and the native API's capabilities (`rust/core/native-api/src/capabilities.rs`). Nightjar reads them into [`Feature`, `HostCapabilities`](../client/protocol/client/src/http/capabilities.rs) and gates on that, never on a host's name. `client/protocol/client/tests/capabilities_contract.rs` checks the reading against recorded serverinfo replies.

### A native API endpoint

1. Types in `rust/core/native-api/src/`. Within v1, changes only add: a response may gain fields, a request refuses unknown ones.
2. The route in [`router`](../rust/host/src/native_api.rs), the handler in `rust/host/src/native_api/`, what it reads from host state in `rust/core/src/native/`. The caller is the paired device of the TLS certificate; its permissions are read on every request.
3. Tests in `rust/host/src/nvhttp/native_tests/`; the contract in `docs/native-api.md`.
4. Nightjar: `client/protocol/client/src/http/native.rs`, then both apps.

### A console page or API route

1. The route and its methods in [`token_catalog`](../rust/host/src/web.rs) when an API token may call it, then the arm in [`api`](../rust/host/src/web.rs). [`guard`](../rust/host/src/web.rs) does sign-in, CSRF and token scope for every route.
2. The handler in `rust/host/src/web/`, with tests beside it. `docs/api.md` lists the route.
3. A typed call in `rust/web/src/lib/api.ts`, a component under `rust/web/src/components/`, a page in `rust/web/src/pages/` routed from `rust/web/src/App.svelte`.
4. A `*.test.mjs` beside `rust/web/package.json`; a reply in `rust/web/scripts/screenshots.cjs` when a screenshot shows the page.

## The narrowest test

| Change | Test |
| --- | --- |
| A decision in core | The module's own tests: `cargo test -p butterpollo-core --locked <module>`. No GPU, any OS. |
| Launch, resume, reconnect, control messages, roaming, session end | `rust/host/src/stream/reconnect_tests.rs` and the files under `rust/host/src/stream/reconnect_tests/`: the real HTTP, RTSP, ENet and session workers on loopback with a fake capture, encoder and input. Start here for a session bug: it needs no GPU, driver or service. |
| Anything Nightjar and the host must agree on | `rust/host/src/stream/reconnect_tests/interop_tests.rs` runs Nightjar's engine against this host in one process. |
| Moonlight HTTP replies | `rust/host/src/nvhttp/parity_tests.rs`, `rust/host/src/nvhttp/native_tests/` |
| Bytes from the network | The property tests in `rust/core/src/fuzz.rs` and `client/protocol/core/src/fuzz.rs`: no input panics, and what decodes encodes back. `PROPTEST_CASES` raises the count. |
| Capture, conversion, encoders, displays, input devices | Tests marked `#[ignore]` in `rust/windows/src/`, each with the reason it needs hardware; the [developer guide](../rust/README.md#running-the-native-checks) has the command. They read `BUTTERPOLLO_TEST_*` variables for decoders and report files. |
| Console | `node --test "*.test.mjs"` in `rust/web/` renders components on the server through `rust/web/test-server.mjs`, without a browser. The `*.browser.cjs` files need Playwright. |
| A packaged host | `rust/tests/interop.py`, `rust/tests/web_api.py` and their neighbours start an isolated host; `rust/release/e2e.py` streams one end to end. Exit code 3 is an AMD encoder hang: stop, do not retry. |

`tools\dev.ps1 check` maps changed paths to these (`plan()` in `tools/dev.py`): host crates and the crates that depend on them to fmt, clippy and tests; `client/protocol/` to `cargo test` in that workspace; `client/app/` sources to the Gradle tests of the touched classes; `rust/web/` to `npm run check`, the node tests and the build; `rust/release/` and `tools/` to their Python suites. Markdown maps to nothing.

## Evidence

| Evidence | Where |
| --- | --- |
| Host log | `logs\butterpollo.log` in the profile, 32 MiB a file with four older ones kept (`rust/core/src/logfile.rs`). `RUST_LOG` overrides the saved level (`rust/host/src/logging.rs`). |
| Known failures in words | [`RULES`](../rust/core/src/log_analysis.rs) maps warnings and errors to the explanations the console shows. Read it to learn what a log line means; add a rule when a new failure has a known cause. |
| A stalled stream thread | `rust/host/src/stall_watch.rs` logs where it is. |
| Console warnings | `rust/host/src/host_warnings.rs`, kept in `host-warnings.json` until dismissed |
| Per-frame timings | `BUTTERPOLLO_TIMING_RECORD` writes the [timing record](timing-record.md); `rust/release/timing_report.py` turns host and receiver records into a stage table. |
| A stream run | `rust/release/e2e.py` leaves `result.json`, `frames.csv` and the host's record in the run's folder. |
| Faults on purpose | `rust/host/src/soak_fault.rs` (debug hosts only) and the send switches `BUTTERPOLLO_TEST_SEND_LOSS`, `BUTTERPOLLO_TEST_SEND_JITTER` and `BUTTERPOLLO_TEST_SEND_OUTAGE` |

## What has gone wrong before

The fix history repeats a few shapes. Check a change against them before calling it done.

- **State across a reconnect.** A resume, a second device, a roam or a host restart meets state the first launch set: HDR mode, display layout, overrides, audio route. Add the case to the reconnect matrix.
- **Which setting wins.** A value can come from the host, a preset, the device, the app, or the app for one device. Read it through `rust/host/src/stream/config.rs`, never from the raw configuration.
- **Restore.** Whatever a stream changes on the PC (displays, Radeon settings, the frame limiter, audio routes, game files) is journaled before the change and restored on every end, a crash included. A new change needs both halves and a session-end test.
- **Work on the wrong thread.** A blocking call on the control loop, the packet sender or the audio thread shows as stutter far from its cause. Slow work gets its own worker, as `rust/host/src/stream/qos.rs` and `rust/host/src/stream/game_audio.rs` have.
- **One of two homes.** A setting missing from the console schema, a message the host offers and Nightjar never asks for, a text in one language. `with` shows the homes; the checklists above name them.
- **Offered by what one PC can do.** What the host offers follows the encoder's probe and the codec level, not the machine a change was written on.
