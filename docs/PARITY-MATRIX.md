# Host feature support

[Documentation](index.md) · [Building](building.md)

What Nightjar does for each capability and which hosts provide it. **Any host** means any Moonlight-compatible host: Apollo, NVIDIA GameStream, Rubylight, Sunshine, Vibepollo and others. Named hosts are the ones known to provide a feature; forks that implement the same extension can work too.

Nightjar decides from what each host reports about itself, not from its name. A PC's settings hide what its host can never do, disable with a short reason what needs a host setting, permission or update, and keep everything for a PC never reached yet ("Applies if the host supports it"). Offline PCs keep the features they last reported. A message about something a PC lacks appears once for that PC, not on every stream.

| Capability | What Nightjar does | Host support |
| --- | --- | --- |
| Discovery, manual addresses, IPv6, custom ports | Finds PCs on your network, saves hosts, accepts manual addresses and follows the ports the host advertises. | Any host |
| Host identification | Names the host in its details (Rubylight with its version; Apollo-, Vibepollo- or Sunshine-compatible; NVIDIA GameStream) with the features it reports. | Any host |
| Wake-on-LAN | Wakes your PC with its validated MAC address over every interface, broadcast and resolved destination. | Any host |
| PIN and one-time-PIN pairing | Certificate pairing with a cancellable flow and a persistent installation identity. | PIN: any host. One-time PIN: hosts that report device permissions (Apollo, Rubylight, Vibepollo) |
| Device permissions | Separate list, view, launch, input, clipboard and command permissions, set by the host. Hosts that report no permissions keep their usual behavior. | Apollo, Rubylight, Vibepollo |
| Library and artwork | Shows your apps in host order and caches artwork until the host publishes new art. | Any host |
| Launch, resume, disconnect, quit | Checks permissions first and confirms before ending a game. Optional quick resume from the PC list and per-game quit when you disconnect. | Any host |
| Automatic recovery | Bounded retries resume the same running app after transient transport loss. Off by default (Settings › Streaming); retries cannot launch, quit or replace an app. | Any host |
| Network drop tolerance | Keeps the stream open for 20, 30 or 60 seconds of network silence (Wi-Fi roaming, short dropouts, a VPN changing paths) instead of 10. Off by default (Settings › Streaming); ends with an explanation when the wait runs out. See [network drops](video-and-latency.md#network-drops). | Any host; waits beyond 10 seconds need the host's own client timeout raised (Sunshine, Apollo, Vibepollo: `ping_timeout`) |
| Client address change | Keeping a session when the device's address changes needs the host to accept the client from its new address. Written up as a [proposal for hosts](protocol/client-roaming.md); no client behaviour yet. | No host yet |
| Remote Monitor / Input-only | View-only video and audio, or controls and feedback without starting decoders. Their reserved app tiles count as tiles only on hosts that publish them; elsewhere they are ordinary apps. Apollo's Remote Input and Terminate apps work as Input-only and Terminate on Apollo and Vibepollo. | Rubylight, Vibepollo. Input-only: also Apollo with its input-only mode on; any other host with **Input-only on any PC** on, as a small stream that still uses about 0.7 Mbit/s ([details](controls.md#input-only-on-any-pc)) |
| H.264 / HEVC / AV1 | Hardware decoding with automatic codec selection and per-device low-latency decoder tuning ([decoder notes](../decoder-errata.txt)). | Any host (AV1 where the host encodes it) |
| HDR10 | Main10 negotiation, BT.2020/PQ, static display metadata and smooth HDR transitions. | Any host with HDR support |
| HDR display luminance | Reports the Android display's HDR luminance at stream start and when the output changes, for the host's output diagnostics. | Rubylight |
| YUV 4:4:4 | Full-chroma conventional profiles and PyroWave chroma modes for sharp colored text. | Hosts that advertise 4:4:4 (e.g. Apollo, Rubylight, Sunshine). PyroWave chroma modes: Rubylight |
| PyroWave | 64-bit Vulkan decoder with record framing, diagnostics and automatic codec fallback. PyroWave codec bits count only from a host that also reports PyroWave. A PC whose host has none falls back to the codec for all PCs (or Automatic), says so once, and keeps TextureView and HDR as for any other codec. | Rubylight; Vibepollo once paired with PyroWave available |
| Resolution / fractional refresh / virtual display | Per-PC resolution and refresh rate, host render scale and a matching host virtual display, as an on/off switch per PC. | Resolution: any host. Fractional refresh: Apollo, Rubylight, Vibepollo (Sunshine reads whole numbers). Virtual display and render scale: hosts that advertise a virtual display (Apollo, Rubylight, Vibepollo) |
| PC display mode | Per-PC Exclusive, Extended, primary, separated and physical display choices for normal streams. Separated displays share one Windows session. Only offered for hosts with display modes; a choice saved earlier is not sent to other hosts. See [controls](controls.md#pc-display-mode-and-administration). | Rubylight 2.2.1 or later |
| VRR / Android adaptive refresh | Follows the game's cadence on adaptive displays and runs at maximum refresh elsewhere. The host-side VRR request goes only to hosts that read it. | Any host. Host-side VRR: Rubylight, Vibepollo |
| Phase-lock frame timing | Reports how long frames wait for the next display latch so the host can time frames to land just before it. Not sent for external displays. | Rubylight |
| Pacing and decoder queues | Lowest latency, balanced, capped and smooth pacing with bounded output queues. | Any host |
| Portrait and rotation | Off by default. Locks phones to landscape or portrait, or follows the device. The stream starts in the screen's shape and, with Rotate with device, turns during the stream; elsewhere it keeps its size and video scaling fits it. See [orientation](video-and-latency.md#orientation). | Screen lock and scaled rotation: any host. Portrait-shaped stream: hosts with a virtual display (Apollo, Rubylight, Vibepollo). Turning a running stream: hosts that advertise control message `0x5532` (currently Rubylight) |
| Client upscaling | Lower-resolution SDR video can use bilinear, FSR 1.0 or SGSR 1. PyroWave uses SGSR for both FSR and SGSR selections; native/larger inputs and HDR/10-bit use direct output. | Any host |
| Snapdragon GPU drivers | Import a compatible Vulkan driver ZIP on ARM64 Adreno devices for PyroWave, with system-driver fallback. MediaCodec remains on the system driver. See [driver requirements](video-and-latency.md). | Rubylight (PyroWave only) |
| Automatic bitrate | Per-PC adaptation from loss, RTT variation and decode headroom, up to 2000 Mbps at runtime, within the host's own ceiling. Manual bitrate changes during a stream use the same host request. | Automatic: Rubylight 2.0 or later (it reports its encoder bitrate). Manual: Rubylight, Vibepollo |
| Automatic resolution/FPS | Opt-in motion, detail and battery modes with user floors and a session ceiling, on decoders supporting stream reconfiguration. Unsupported hosts and PyroWave retain their negotiated size; the message about it appears once per PC. | Hosts that advertise control message `0x5532` (currently Rubylight) |
| Stream health | [Recent evidence](stream-health.md) for network, decoder, host, presentation and heat, with an optional reversible bitrate trial. | Any host. Bitrate trial: Rubylight |
| Audio | Stereo, 5.1 and 7.1, with low-latency AAudio for stereo on Android 8.1 and later. | Any host |
| Controllers and mappings | Multiple players, per-model button, stick/trigger and D-pad mapping, and built-in Xbox USB drivers. | Any host |
| Host cursor shapes | In Local cursor mode, draws the PC's own cursor image (I-beam, resize arrows, hidden while a game captures the mouse) instead of a plain arrow, while the host leaves it out of the video. Images are checked and decoded off the stream threads; anything malformed falls back to the arrow. See the [protocol proposal](protocol/cursor-shape.md). | None yet. Hosts that advertise `CursorShapeVersion` and control message `0x5540` |
| Per-game touch controls | Held/toggle keyboard, mouse and gamepad buttons, analog sticks (fixed or floating), triggers, a D-pad and WASD sticks; layout editing with grid snapping; validated JSON import/export/sharing; import of Artemis keyboard layouts and Custom Keys files. | Any host |
| Desktop tools | Pinch zoom/pan with matching absolute input, plus external-display video and a phone touchpad with reconnect fallback. | Any host |
| Remote access guide | [Paired HTTPS checks](remote-setup.md) and direct/DERP/peer-relay status for the current Tailscale peer. | Any host. Tailscale path status: Rubylight |
| DualSense / Edge | Adaptive triggers, rumble, lightbar and Edge paddles over the direct USB driver. | Rumble: any host. Adaptive triggers and lightbar: hosts that send them (Sunshine protocol extension) |
| DualSense USB audio and haptics | Waveform feedback, plus speaker, headset and microphone audio. USB only. See [setup and limitations](dualsense-haptics.md). | Waveform: Rubylight, Vibepollo. Audio: Rubylight |
| Switch Pro Controller (USB) | Opt-in direct USB driver: calibrated sticks, Home and Capture, motion, rumble, player lights and battery. Untested on hardware. Joy-Con stay on Android's input path. See [Switch Pro over USB](controls.md#switch-pro-over-usb). | Any host. Motion: hosts that support controller motion (Sunshine protocol extension) |
| Controller reconnects | Host controller preparation and identity/audio-endpoint retention, alongside the guarded stream recovery policy. | Rubylight (recovery policy: any host) |
| Gyro aiming / radial menus | Provided by Steam Input using forwarded controller motion and touchpads. Configure per game in Steam. | Hosts that support controller motion (Sunshine protocol extension) |
| Touch, pen, mouse and keyboard | Trackpad (optional acceleration, glide, active area), direct, multi-touch, local cursor and touch off modes, native pen, pointer capture and keyboard input. Direct positions account for shifted stream layouts. Extra keyboard keys include Esc, F1–F12, modifiers and Task Manager/window shortcuts. Custom chords can be edited under Controls and imported/exported as JSON. | Any host. Native touch and pen: hosts that support them (Sunshine protocol extension) |
| Clipboard / server commands | Foreground text transfer in both directions and the host's configured commands. | Apollo, Rubylight, Vibepollo |
| Performance overlay / CSV | One-line Compact (default), Statistics and scrollable Debug views with stage timings, available host processing and CSV export. Choose the mode under the stream menu's Display → Overlay and diagnostics. | Any host (host processing where the host reports it) |
| Host CPU / AMD GPU telemetry | Optional host CPU usage and AMD ADLX GPU readings in Statistics and Debug, over paired HTTPS during this device's video stream. Requires the host's hardware-metrics opt-in and advertised protocol support. See [host hardware metrics](video-and-latency.md#host-hardware-metrics). | Rubylight |
| Connection test / local benchmark | Delay test for any PC, plus throughput where the host offers the download test, and a hostless timing benchmark. The test says up front whether it measures speed. | Any host. Throughput download: Rubylight, Vibepollo (paired) |
| Problem reports | Redacted local events, decoder details and the last crash record, shared when you choose. | Any host |
| PiP / shortcuts / frontends | Picture-in-picture, pinned app shortcuts and launcher entries for ES-DE, Daijisho and Pegasus. | Any host |
| Settings / accessibility | Seven sections, with decoder controls and diagnostics directly in Advanced; search, presets, PC profiles, a control layout editor, TalkBack and D-pad navigation, in 15 languages ([translations](TRANSLATIONS.md)). | Any host |
| Host web UI | Open the host's web UI from an online PC's settings, at its default port (six above the HTTPS port). Devices, apps and host settings remain in the host's web UI, with its own login and network access requirements. | Any host except NVIDIA GameStream |

See [building](building.md) for reproducible checks.

## Rust streaming engine

Nightjar ships two streaming engines. The C engine (moonlight-common-c with Nightjar's patches) is the default. The Rust engine (`nightjar-core` through `libnightjar_core.so`) is behind Settings › Advanced › Diagnostics › New streaming engine. Each stream picks one: the Rust engine runs only when the setting is on, the library loads and the host passes `RustBridge.fallbackReason`. Otherwise the stream runs on the C engine, and the stream overlays and the problem report say why ("Streaming engine: C (moonlight-common-c), Rust engine requested but unavailable: ..."). The Rust column below is what the code and its tests show; "device" means checked on a phone, which is the open part. Details of the Java and Rust split are in [internal/RUST_CORE.md](../internal/RUST_CORE.md).

Status words: **same** (both engines do it and tests cover the Rust side), **C only** (a deliberate gap, with the reason), **neither** (no engine does it), **device pending** (written and tested in Rust, not yet seen on a phone).

| Area | C engine | Rust engine | Notes |
| --- | --- | --- | --- |
| H.264, HEVC, AV1, 10-bit, HDR metadata | Yes | Same | Difftested against the patched C; emulator streams of H.264 and HEVC at 60 FPS, 0% loss (2026-10-20). |
| YUV 4:4:4 (packed) | Yes | Same | `packed444` and its modes are passed through; loopback test in `session_loopback.rs`. Device pending. |
| PyroWave records, link and pace hints | Yes | Same, device pending | The fallback was removed. Records, SDP/RTSP negotiation and the queue are difftested and covered by `host_interop.rs`; no phone has streamed PyroWave through the Rust engine yet. |
| Audio: stereo, 5.1, 7.1, high-quality surround, concealment | Yes | Same | Opus multistream decoded in Rust against the app's own `libopus.a`. |
| Microphone | Yes | Same | Encrypted stream only, in both. |
| Mouse, keyboard, scroll, text | Yes | Same | Return codes match the C API. |
| Unbatched input (no 1 ms batching of mouse and pen motion) | Yes | Same | `unbatched_input`; tests `unbatched_motion_does_not_wait` and `batched_motion_still_waits_by_default`. |
| Touch, pen, controller touch, motion, battery | Yes | Same | |
| Gamepad: buttons, triggers, sticks, rumble, trigger rumble, LED, adaptive triggers, haptics, controller audio | Yes | Same | Callbacks are the same `bridge*` methods; a JVM test (`RustEngineJniParityTest`) checks every callback and every native against the Rust sources. |
| Clipboard, server commands | Yes | Same | Server commands (0x3000) through `sendServerCommand`. Clipboard transfer is Java and HTTP, so engine-neutral. |
| Radeon and native-API extensions, session actions | Yes | Same | HTTP and Java side; the Rust crate has its own `native_api` client with its own tests. |
| Host timing (0x5536), clock sync (0x5537), present clock, phase lock (0x5530), display caps (0x5531), reconfigure (0x5532), cursor shapes (0x5540) | Yes | Same | Used by phase-lock, automatic resolution, cursor shapes and the host-join columns of the timing CSV. |
| Input-only sessions | Yes | Same | |
| Remote monitor sessions | Yes | Same | The fallback was removed: no input is sent and the start wiggle is skipped. Test `a_remote_monitor_connection_sends_no_input_at_all`. |
| Network grace period | Yes | Same | |
| Socket QoS and Prioritize stream traffic | Yes | Same | `SO_PRIORITY` 6 for audio, 5 for video, 6 for the control and microphone sockets with the setting on, and nice -4 on the receive, decode and control threads. Linux and Android only; test `media_sockets_are_bound_with_their_priority`. |
| Encryption: control, video, audio, microphone | Yes | Same | `ENCFLG_ALL` with hardware AES, `ENCFLG_AUDIO` without, in both. |
| IPv6, NAT64, local and remote packet sizes | Yes | Same | |
| Timing CSV and the client timing record (A8) | Yes | Same | The CSV is written in Java from `bridgeDrSubmitDecodeUnit` (receive, enqueue, host processing, present times) and the present-clock offset, which both engines fill the same way; `monotonic_ns_of` maps Rust session time to the same clock the C engine reports. The Statistics overlay and the problem report name the engine, so a CSV can be matched to its engine. |
| Problem report fields | Yes | Same | The report has a "Streaming engine for the last stream" section, the decoder and audio sections are filled by the renderers, and native crash frames from `libnightjar_core.so` are kept. |
| Automatic bitrate, manual bitrate, reconnect, automatic recovery | Yes | Same | Java policy and host requests above the engine. |
| Host streamed-frame receive (B2) | Not built | Rust receive tested, no Java input | The Rust client receives and reassembles slice-delivered frames (`slice_delivery.rs`). The Java side has no partial-frame MediaCodec input, so neither engine uses it on a phone. B2 is a measurement probe. |
| Client address change (roaming) | Neither | Neither | Needs the host to accept the new address; see the [proposal](protocol/client-roaming.md). |
| GFE before 3.22 (plaintext control, RTSP over ENet) | Yes | C only | Not ported on purpose; these hosts stay on the C engine. The fallback reason names the host version. |
| Hosts that report no codec support | Yes | C only | Falls back with a reason. |
| Pull renderer, STUN | Yes | C only | The Java side never requests the pull renderer. STUN and the connectivity test stay in the C library. |
| Stage names, port flags, `testClientConnectivity`, controller type guessing, USB audio natives | Yes | C only | Stay in the C library and Java. These must move out of `libmoonlight-core.so` before the C engine can be deleted. |

### Switching the default

Not done yet. Flipping it is a decision after the device measurements below. The places that change are `PreferenceConfiguration.RUST_ENGINE_PREF_STRING`'s `false` default, `checkbox_rust_engine` in `res/xml/preferences.xml`, and the expectations in `SettingsKeysTest` and `AdvancedOptionsTest`. Existing installs keep their stored value, so a default flip also needs a one-time migration if they should follow it.

### Comparing the engines on a phone

Use the same phone, host, codec, resolution, refresh rate and bitrate for both runs, with the phone on the same Wi-Fi spot. For each engine:

1. Set Settings › Advanced › Diagnostics › New streaming engine (beta) to off (C) or on (Rust). Turn on the Debug overlay and the latency CSV.
2. Start a stream and confirm the engine: the Statistics line ends with `· C` or `· Rust`, and the Debug view shows `Streaming engine: ...`. If it says "Rust fell back", read the reason; that run measures the C engine.
3. Play the same content for 5 minutes, then a 30-minute soak. Read picture age (`receive_to_present_ms`, `receive_to_render_ms`), decode time (`input_to_output_ms`), loss percent, frame drops and RTT.
4. Export the CSV with Export latency CSV, and share a problem report to keep the engine line with it. With a Rubylight host, join the CSV to the host's timing record with `timing_report.py`.
5. Flip the setting and repeat. The Rust engine is no worse when its median and 99th-percentile picture age and decode time are within the C run's noise, loss is equal or lower, and the 30-minute soak has no disconnects or growing latency.

## Remaining gaps and validation

The [October 2026 Android audit](android-audit-2026-10.md) records the reproducible defects, release tooling gaps and remaining hardware/integration checks. Feature implementation is not a claim that every device, host or document provider has been tested.
