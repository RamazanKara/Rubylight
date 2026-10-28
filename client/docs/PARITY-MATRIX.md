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
| Client address change | Keeping a session when the device's address changes needs the host to accept the client from its new address. The [roaming protocol](protocol/client-roaming.md) (Experimental, version 1) proves the new address to the host with an authenticated challenge. Its wire format is fixed; client behaviour is in progress. | Hosts that advertise `a=x-rl-roaming:1` (Rubylight: in progress) |
| Remote Monitor / Input-only | View-only video and audio, or controls and feedback without starting decoders. Their reserved app tiles count as tiles only on hosts that publish them; elsewhere they are ordinary apps. Apollo's Remote Input and Terminate apps work as Input-only and Terminate on Apollo and Vibepollo. | Rubylight, Vibepollo. Input-only: also Apollo with its input-only mode on; any other host with **Input-only on any PC** on, as a small stream that still uses about 0.7 Mbit/s ([details](controls.md#input-only-on-any-pc)) |
| H.264 / HEVC / AV1 | Hardware decoding with automatic codec selection and per-device low-latency decoder tuning ([decoder notes](../decoder-errata.txt)). | Any host (AV1 where the host encodes it) |
| HDR10 | Main10 negotiation, BT.2020/PQ, static display metadata and smooth HDR transitions. | Any host with HDR support |
| HLG and Dolby Vision | HLG for a screen with HLG and no HDR10, and Dolby Vision profile 8.1, then 8.4, through a hardware Dolby Vision decoder on a screen that reports it, falling back to HEVC with the same stream when that decoder fails. HLG also on the Vulkan renderer; neither on packed full chroma or PyroWave, and Dolby Vision never on the Vulkan renderer. Dolby Vision is Experimental. | Hosts that list them in `HdrFormats` (Rubylight 3.0) |
| HDR display luminance | Reports the Android display's HDR luminance at stream start and when the output changes, for the host's output diagnostics. | Rubylight |
| YUV 4:4:4 | Full-chroma conventional profiles and PyroWave chroma modes for sharp colored text. | Hosts that advertise 4:4:4 (e.g. Apollo, Rubylight, Sunshine). PyroWave chroma modes: Rubylight |
| PyroWave | 64-bit Vulkan decoder with record framing, diagnostics and automatic codec fallback. PyroWave codec bits count only from a host that also reports PyroWave. A PC whose host has none falls back to the codec for all PCs (or Automatic), says so once, and keeps TextureView and HDR as for any other codec. | Rubylight; Vibepollo once paired with PyroWave available |
| Resolution / fractional refresh / virtual display | Per-PC resolution and refresh rate, PC render resolution and a matching host virtual display, as an on/off switch per PC. | Resolution: any host. Fractional refresh: Apollo, Rubylight, Vibepollo (Sunshine reads whole numbers). Virtual display and PC render resolution: hosts that advertise a virtual display (Apollo, Rubylight, Vibepollo) |
| PC display mode | Per-PC Exclusive, Extended, primary, separated and physical display choices for normal streams. Separated displays share one Windows session. Only offered for hosts with display modes; a choice saved earlier is not sent to other hosts. See [controls](controls.md#pc-display-mode-and-administration). | Rubylight 2.2.1 or later |
| VRR / Android adaptive refresh | Follows the game's cadence on adaptive displays and runs at maximum refresh elsewhere. The host-side VRR request goes only to hosts that read it. | Any host. Host-side VRR: Rubylight, Vibepollo |
| Phase-lock frame timing | Reports how long frames wait for the next display latch so the host can time frames to land just before it. Not sent for external displays. | Rubylight |
| Pacing and decoder queues | Lowest latency, balanced, capped and smooth pacing with bounded output queues. | Any host |
| Portrait and rotation | Off by default. Locks phones to landscape or portrait, or follows the device. The stream starts in the screen's shape and, with Rotate with device, turns during the stream; elsewhere it keeps its size and video scaling fits it. See [orientation](video-and-latency.md#orientation). | Screen lock and scaled rotation: any host. Portrait-shaped stream: hosts with a virtual display (Apollo, Rubylight, Vibepollo). Turning a running stream: hosts that advertise control message `0x5532` (currently Rubylight) |
| Client upscaling | Lower-resolution SDR video can use Smooth (bilinear), Sharp (FSR 1.0) or Sharp, Snapdragon (SGSR 1). PyroWave uses SGSR for both FSR and SGSR selections; native/larger inputs and HDR/10-bit use direct output. | Any host |
| Snapdragon GPU drivers | Import a compatible Vulkan driver ZIP on ARM64 Adreno devices for PyroWave, with system-driver fallback. MediaCodec remains on the system driver. See [driver requirements](video-and-latency.md). | Rubylight (PyroWave only) |
| Automatic bitrate | Per-PC adaptation from loss, RTT variation and decode headroom, up to 2000 Mbps at runtime, within the host's own ceiling. Manual bitrate changes during a stream use the same host request. | Automatic: Rubylight 2.0 or later (it reports its encoder bitrate). Manual: Rubylight, Vibepollo |
| Automatic resolution/FPS | Opt-in motion, detail and battery modes with user floors and a session ceiling, on decoders supporting stream reconfiguration. Unsupported hosts and PyroWave retain their negotiated size; the message about it appears once per PC. | Hosts that advertise control message `0x5532` (currently Rubylight) |
| Stream health | [Recent evidence](stream-health.md) for network, decoder, host, presentation and heat, with an optional reversible bitrate trial. | Any host. Bitrate trial: Rubylight |
| Audio | Stereo, 5.1 and 7.1, with low-latency AAudio for stereo on Android 8.1 and later. | Any host |
| Audio passthrough (Experimental) | Off by default. PCM, Dolby Digital (AC3) or Dolby Digital Plus (E-AC3, up to 5.1) for an output that plays it as is, picked from what the route reports (Android 10 and later, Rust engine); Opus otherwise, with a message. Dolby frames are 32 ms, so it arrives later than Opus. | Rubylight |
| Controllers and mappings | Multiple players, per-model button, stick/trigger and D-pad mapping, and built-in Xbox USB drivers. | Any host |
| Host cursor shapes | In Local cursor mode, draws the PC's own cursor image (I-beam, resize arrows, hidden while a game captures the mouse) instead of a plain arrow, while the host leaves it out of the video. Images are checked and decoded off the stream threads; anything malformed falls back to the arrow. See the [protocol proposal](protocol/cursor-shape.md). | None yet. Hosts that advertise `CursorShapeVersion` and control message `0x5540` |
| Per-game touch controls | Held/toggle keyboard, mouse and gamepad buttons, analog sticks (fixed or floating), triggers, a D-pad and WASD sticks; layout editing with grid snapping; validated JSON import/export/sharing; import of Artemis keyboard layouts and Custom Keys files. | Any host |
| Desktop tools | Pinch zoom/pan with matching absolute input, plus external-display video and a phone touchpad with reconnect fallback. | Any host |
| Remote access guide | [Paired HTTPS checks](remote-setup.md) and direct/DERP/peer-relay status for the current Tailscale peer. | Any host. Tailscale path status: Rubylight |
| DualSense / Edge | Adaptive triggers, rumble, lightbar and Edge paddles over the direct USB driver. | Rumble: any host. Adaptive triggers and lightbar: hosts that send them (Sunshine protocol extension) |
| DualSense USB audio and haptics | Waveform feedback, plus speaker, headset and microphone audio. USB only. See [setup and limitations](dualsense-haptics.md). | Waveform: Rubylight, Vibepollo. Audio: Rubylight |
| Switch Pro Controller (USB) | Opt-in direct USB driver: calibrated sticks, Home and Capture, motion, rumble, player lights and battery. Experimental. Joy-Con stay on Android's input path. See [Switch Pro over USB](controls.md#switch-pro-over-usb). | Any host. Motion: hosts that support controller motion (Sunshine protocol extension) |
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

## Desktop client (Windows and Linux)

Nightjar for Windows and Linux (`protocol/desktop`) uses the same Rust engine and host checks as the Android app. This table lists how far it matches the capabilities above. **Done** means as on Android, **Partial** names what is left, **Experimental** is written but not checked on our hardware, **Missing** is not there yet. The [desktop README](../protocol/desktop/README.md#what-works) has the details.

| Capability | Windows | Linux | Notes |
| --- | --- | --- | --- |
| Discovery, Wake-on-LAN, PIN and one-time-PIN pairing | Done | Done | |
| Library, launch, resume, quit, Home | Done | Done | |
| Remote Monitor / Input-only | Done | Done | Input-only only for tiles the host publishes; **Input-only on any PC** is missing. |
| Quick resume, quit on leave per game | Missing | Missing | |
| Settings per PC and per game, profiles | Done | Done | Profiles also by network (mobile, home Wi-Fi, other Wi-Fi). |
| H.264 / HEVC / AV1 | Done | Partial | Linux reads VAAPI pictures back to memory (no dmabuf import yet). A fallback to software decoding is announced. |
| PyroWave, bandwidth test and frame pace calibration | Done | Done | |
| HDR10 | Done | Experimental | Windows also sends the display's luminance. Linux asks the compositor for an HDR swap chain; not checked on an HDR display yet. |
| YUV 4:4:4, 10-bit SDR | Partial | Partial | Only the experimental packed full chroma mode and PyroWave's chroma modes. No 10-bit SDR. |
| Resolution, fractional refresh, virtual display, PC display mode | Done | Done | |
| Phase-lock frame timing | Done | Experimental | Linux: X11 with the OpenGL renderer only; Wayland sends no phase reports yet. |
| Pacing | Partial | Partial | VRR with host times and V-Sync; no Balanced, Smoothest or drop-late modes. |
| Client upscaling | Done | Done | |
| Automatic bitrate | Partial | Partial | An older port of Android's controller. No PyroWave bitrate control. |
| Automatic resolution/FPS | Done | Done | |
| Automatic recovery, network drop tolerance | Done | Done | |
| Instant mouse and pen input | Done | Done | |
| Prioritize stream traffic | Not applicable | Done | The engine sets socket and thread priorities on Linux and Android only, so Windows does not show the setting. |
| Audio, microphone | Done | Done | |
| Audio passthrough | Experimental | Experimental | PCM only, played through SDL like decoded Opus; no Dolby (no AC3 decoder or IEC 61937 output). |
| Controllers, DualSense, controller audio | Partial | Partial | Wired to SDL's gamepad API; not tried with physical controllers yet. |
| Touch, pen, mouse and keyboard | Done | Done | Native touch and pen. No touch trackpad mode or on-screen controls. |
| Clipboard / server commands, host cursor shapes | Done | Done | Gated by the host's features and device permissions. |
| Performance overlay / CSV | Partial | Partial | Overlay on or off with host processing, statistics export and the timing CSV; no Compact, Statistics and Debug modes. |
| Stream health, host telemetry, remote access guide | Missing | Missing | The connection test has no delay test. |
| Host identification, host web UI | Missing | Missing | |
| Problem reports | Partial | Partial | The logs folder only. |
| Settings / accessibility | Partial | Partial | English and German; no search, presets, PIN lock or screen reader. |
| Command line for frontends, updater | Missing | Missing | The updater is Windows only; Linux uses distribution packages. |
| PiP, rotation, Snapdragon drivers, USB drivers | Not applicable | Not applicable | SDL covers the controllers the Android USB drivers handle. |

## Rust streaming engine

Nightjar ships two streaming engines. The C engine (moonlight-common-c with Nightjar's patches) is the default. The Rust engine (`nightjar-core` through `libnightjar_core.so`) is behind Settings › Advanced › Diagnostics › New streaming engine. Each stream picks one: the Rust engine runs only when the setting is on, the library loads and the host passes `RustBridge.fallbackReason`. Otherwise the stream runs on the C engine, and the stream overlays and the problem report say why ("Streaming engine: C (moonlight-common-c), Rust engine requested but unavailable: ..."). The Rust column below is what the code and its tests show; "device" means checked on a phone, which is the open part. Details of the Java and Rust split are in [internal/RUST_CORE.md](../internal/RUST_CORE.md).

Status words: **same** (both engines do it and tests cover the Rust side), **C only** (a deliberate gap, with the reason), **neither** (no engine does it), **device pending** (written and tested in Rust, not yet seen on a phone).

| Area | C engine | Rust engine | Notes |
| --- | --- | --- | --- |
| H.264, HEVC, AV1, 10-bit, HDR metadata | Yes | Same | Difftested against the patched C; emulator streams of H.264 and HEVC at 60 FPS, 0% loss (2026-10-20). |
| YUV 4:4:4 (packed) | Yes | Same | `packed444` and its modes are passed through; loopback test in `session_loopback.rs`. Device pending. |
| PyroWave records, link and pace hints | Yes | Same, device pending | The fallback was removed. Records, SDP/RTSP negotiation and the queue are difftested and covered by `host_interop.rs`; no phone has streamed PyroWave through the Rust engine yet. |
| Audio: stereo, 5.1, 7.1, high-quality surround, concealment | Yes | Same | Opus multistream decoded in Rust against the app's own `libopus.a`. |
| Audio passthrough: PCM, AC3, E-AC3 | No | Yes, device pending | Rust only, on purpose: the C engine keeps Opus and says so. Rust asks with `x-rl-audio` and hands AC3 and E-AC3 frames to a passthrough AudioTrack; no TV or receiver has played it yet. |
| Microphone | Yes | Same | Encrypted stream only, in both. |
| Mouse, keyboard, scroll, text | Yes | Same | Return codes match the C API. |
| Unbatched input (no 1 ms batching of mouse and pen motion) | Yes | Same | `unbatched_input`; tests `unbatched_motion_does_not_wait` and `batched_motion_still_waits_by_default`. |
| Touch, pen, controller touch, motion, battery | Yes | Same | |
| Gamepad: buttons, triggers, sticks, rumble, trigger rumble, LED, adaptive triggers, haptics, controller audio | Yes | Same | Callbacks are the same `bridge*` methods; a JVM test (`RustEngineJniParityTest`) checks every callback and every native against the Rust sources. |
| Clipboard, server commands | Yes | Same | Server commands (0x3000) through `sendServerCommand`. Clipboard transfer is Java and HTTP, so engine-neutral. |
| Radeon and native-API extensions, session actions | Yes | Same | HTTP and Java side; the Rust crate has its own `native_api` client with its own tests. |
| Host timing (0x5536), clock sync (0x5537), present clock, phase lock (0x5530), display caps (0x5531), reconfigure (0x5532), cursor shapes (0x5540) | Yes | Same | Used by phase-lock, automatic resolution, cursor shapes and the host-join columns of the timing CSV. |
| Link notice (0x5538): the host says when it lowers the bitrate for the network | No | Yes | The Rust engine asks and the overlay (all three modes) shows "Host lowered the bitrate to X Mbps: network" until the host is back at the requested rate; the C engine does not support it and never asks. Tests `link_notices_are_requested_and_heard`, `a_link_notice_reaches_java_as_state_and_bitrates`, `GamePerformanceOverlayTest`. Device pending. |
| Steam Controller haptics and settings (0x5504, 0x5505) | No | Experimental | Rust engine only: decoded and handed to `bridgeClSteamHaptic` and `bridgeClSteamSettings`, which do nothing yet. Nightjar has no Steam Controller driver, so no controller advertises `LI_CCAP_STEAM_HAPTIC` or `LI_CCAP_STEAM_SETTINGS` and a host never sends them. Tests `steam_controller_messages_become_events_only_from_sunshine_hosts`, `noControllerCapabilityAsksForSteamControllerHapticsOrSettings`. |
| Input-only sessions | Yes | Same | |
| Remote monitor sessions | Yes | Same | The fallback was removed: no input is sent and the start wiggle is skipped. Server commands and controller audio messages still go out on the control stream, the same as in the C engine, which only skips the input stream. Test `a_remote_monitor_connection_sends_no_input_at_all`. |
| Network grace period | Yes | Same | |
| Socket QoS and Prioritize stream traffic | Yes | Same | Always on: `SO_PRIORITY` 6 on the audio and microphone sockets, 5 on video, and 6 plus DSCP EF with ECT(1) (`IP_TOS` / `IPV6_TCLASS`) on the control socket, which takes the marking off again when the first three connect datagrams get no answer, as ENet does. With the setting on: priority 6 on the RTSP socket too, and nice -4 on VideoRecv, AudioRecv, AudioDec and the control thread (which does ControlRecv and InputSend). VideoDec is always raised to nice -8 (-4 when refused) in the Rust engine, independent of the setting; the C engine leaves it at the default. Linux and Android only; refused options are logged at warn. Tests `media_sockets_are_bound_with_their_priority`, `the_marking_comes_off_after_three_unanswered_connects` (Linux). |
| Encryption: control, video, audio, microphone | Yes | Same | `ENCFLG_ALL` with hardware AES, `ENCFLG_AUDIO` without, in both. |
| IPv6, NAT64, local and remote packet sizes | Yes | Same | |
| Timing CSV and the client timing record (A8) | Yes | Same | The CSV is written in Java from `bridgeDrSubmitDecodeUnit` (receive, enqueue, host processing, present times) and the present-clock offset, which both engines fill the same way; `monotonic_ns_of` maps Rust session time to the same clock the C engine reports. The Statistics overlay and the problem report name the engine, so a CSV can be matched to its engine. |
| Problem report fields | Yes | Same | The report has a "Streaming engine for the last stream" section, the decoder and audio sections are filled by the renderers, and native crash frames from `libnightjar_core.so` are kept. |
| Automatic bitrate, manual bitrate, reconnect, automatic recovery | Yes | Same | Java policy and host requests above the engine. |
| Host streamed-frame receive (B2) | Not built | Rust receive tested, no Java input | The Rust client receives and reassembles slice-delivered frames (`slice_delivery.rs`). The Java side has no partial-frame MediaCodec input, so neither engine uses it on a phone. B2 is a measurement probe. |
| Client address change (roaming) | Neither | Neither | In progress for the Rust engine only, behind the host's `a=x-rl-roaming:1`; see the [roaming protocol](protocol/client-roaming.md). |
| GFE before 3.22 (plaintext control, RTSP over ENet) | Yes | C only | Not ported on purpose; these hosts stay on the C engine. The fallback reason names the host version. |
| Hosts that report no codec support | Yes | C only | Falls back with a reason. |
| Pull renderer, STUN | Yes | C only | The Java side never requests the pull renderer. STUN and the connectivity test stay in the C library. |
| Stage names, port flags, `testClientConnectivity`, controller type guessing, USB audio natives | Yes | C only | Stay in the C library and Java. These must move out of `libmoonlight-core.so` before the C engine can be deleted. |

### Switching the default

Done on 2026-10-23. `checkbox_rust_engine` is on by default (`PreferenceConfiguration`, `res/xml/preferences.xml`) and its label reads "New streaming engine" without "beta"; turning it off selects the classic C engine, which stays for that and for hosts older than GFE 3.22. The device measurements are in [October 23: Rust engine against the C engine on Android (B8)](#october-23-rust-engine-against-the-c-engine-on-android-b8). Installs that stored the old default cannot be told from an explicit off, so `migrateRustEngineDefault` switches a stored off to on once (flag `rust_engine_default_on`); an off chosen afterwards is kept, as is an explicit on. Tests: `RustEngineDefaultTest`.

### Comparing the engines on a phone

Use the same phone, host, codec, resolution, refresh rate and bitrate for both runs, with the phone on the same Wi-Fi spot. For each engine:

1. Set Settings › Advanced › Diagnostics › New streaming engine to off (C) or on (Rust). Turn on the Debug overlay and the latency CSV.
2. Start a stream and confirm the engine: the Statistics line ends with `· C` or `· Rust`, and the Debug view shows `Streaming engine: ...`. If it says "Rust fell back", read the reason; that run measures the C engine.
3. Play the same content for 5 minutes, then a 30-minute soak. Read picture age (`receive_to_present_ms`, `receive_to_render_ms`), decode time (`input_to_output_ms`), loss percent, frame drops and RTT.
4. Export the CSV with Export latency CSV, and share a problem report to keep the engine line with it. With a Rubylight host, join the CSV to the host's timing record with `timing_report.py`.
5. Flip the setting and repeat. The Rust engine is no worse when its median and 99th-percentile picture age and decode time are within the C run's noise, loss is equal or lower, and the 30-minute soak has no disconnects or growing latency.

### October 23: Rust engine against the C engine on Android (B8)

Nightjar's new streaming engine (`checkbox_rust_engine`, Rust) against the current engine (C, moonlight-common-c), on the same phone, host, codec and picture. By measurement the Rust engine is no worse than the C engine on picture age, decode time, loss recovery and a 30-minute soak. The two follow-ups this measurement found (the empty clock columns and the high overlay RTT) are fixed, and the default is flipped (see "Caveats" and "After the fixes").

**Fixture.** Retroid Pocket 6 (Snapdragon, adb `e24bb88e`), Nightjar `dev-retroid` debug build with the B8 gap fixes (Nightjar main `6456da5d` and later), Wi-Fi at -41 to -43 dBm, link 1080 to 1200 Mbps. Host: isolated profile on 192.168.4.69:48123 (RX 7900 XT, AMF, WGC, `virtual_display_layout = extended`, audio off, `video_slice_delivery = off`) running the package from the B2 phone work. Picture: `motion_probe` with 1024 noise on the captured display. Compact overlay on. The engine was confirmed on every run from the logcat line `Streaming engine: C (moonlight-common-c)` or `Rust (nightjar-core)` and the overlay. Runs took the GPU-LOCK one group at a time and the main host was idle before each.

**Method.** The client latency CSV joined to the host timing record (`BUTTERPOLLO_TIMING_RECORD`) with `timing_report.py`, first 2 s of every run dropped. The C engine writes `clock_offset_us` and `clock_rtt_us`, so its join is `clock_sync` (bound about 1 ms). The Rust engine writes neither, which would drop its join to a `min_owd` lower bound that cannot be compared with the C one. So the phone-to-PC clock (phone clock minus PC QPC, which drifts at a steady 34.9 us/s, checked over 14 runs) was fitted on the C runs and written into copies of the Rust CSVs. Leave-one-out error of that fit: median -115 to +64 us, p5 to p95 about +-0.5 ms. Differences under about 0.5 ms in `end_to_end` therefore need the clock-free rows to confirm them: `assemble`, `client_total` and `decode` are measured on the phone only.

#### HEVC 1080p60, 40 Mbps, 4 runs of 75 s per engine (ABBA)

| ms, median / p95 / p99 | C | Rust | Rust minus C (median) |
| --- | --- | --- | ---: |
| end_to_end (joined, picture age) | 7.04 / 10.42 / 15.98 | 6.92 / 10.23 / 15.61 | -0.12 |
| network_last (host first packet to client last packet) | 1.72 / 2.91 / 4.33 | 1.50 / 2.69 / 4.10 | -0.22 |
| decode (`input_to_output`) | 0.92 / 1.26 / 1.47 | 0.92 / 1.27 / 1.46 | 0.00 |
| host_total | 3.49 / 6.87 / 12.49 | 3.56 / 6.87 / 12.41 | +0.07 |
| assemble (client, clock-free) | 0.33 / 1.08 / 2.00 | 0.12 / 0.82 / 1.76 | -0.21 |
| client_total (client, clock-free) | 1.72 / 2.64 / 3.62 | 1.53 / 2.39 / 3.37 | -0.19 |

- Per-run `end_to_end` medians: C 7.24, 7.06, 7.00, 6.89; Rust 6.88, 7.01, 6.85, 6.91 (the unfitted lower-bound join put Rust 0.46 ms behind, which was the join, not the engine).
- Frames: 17,523 (C) and 17,520 (Rust) joined of 18,003 and 18,000 sent. Dropped by the client: 2 (C, 0.011%) and 4 (Rust, 0.022%). CSV rows lost: 0 in both. Host `idr_requests` 1 and `reference_invalidations` 0 in every run, so no recovery keyframes.
- Host fps median 59.35 (C) and 59.32 (Rust), the same capture rate in both.

#### Loss recovery

`BUTTERPOLLO_TEST_SEND_LOSS=<percent>:42` on the host (random drop of video datagrams, same seed), one 75 s run per engine and loss rate, HEVC 1080p60 at 40 Mbps.

| Run | Engine | Packets FEC had to rebuild | Frames FEC rebuilt | Host `idr_requests` / `reference_invalidations` | Host frames the client never showed | `end_to_end` median / p95 / p99 (ms) | Longest gap between received frames (ms) |
| --- | --- | ---: | ---: | --- | ---: | --- | ---: |
| 1% | C | 1972 | 1561 | 1 / 0 | 0 | 7.28 / 10.94 / 16.89 | 23.7 |
| 1% | Rust | 1950 | 1573 | 1 / 0 | 0 | 6.98 / 10.66 / 16.58 | 29.2 |
| 5% | C | 10551 | 3813 | 4 / 3 | 3 | 7.56 / 10.65 / 15.97 | 49.8 |
| 5% | Rust | 10363 | 3830 | 1 / 0 | 0 | 7.30 / 10.62 / 16.16 | 25.4 |

- At 1% both engines recovered every frame from FEC. No recovery keyframe, no skipped frame.
- At 5% the C run lost three frames (client CSV frame numbers 1507 to 1509, 1809 to 1811, 3892 to 3894) and asked for a reference invalidation each time, which the host answered without a keyframe. Each loss cost one skipped frame, no stall: the next frame decoded and displayed normally. The Rust run lost none. One run per engine, so 3 against 0 lost frames out of 4,490 is not a significant difference; it is a "no worse".
- `fec_unrecoverable_frames` was 0 in all four runs and `send_loss_recoveries` 0. No visible corruption on the mid and end screenshots of any of the four runs (noise and checker picture stayed clean), no decode errors in logcat.
- Recovery time: the longest gap between received frames never exceeded 50 ms (one C outlier at 49.8 ms), so every loss healed within about three frame intervals.

#### PyroWave, 1080p60, 400 Mbps, 75 s per engine

Nightjar has no PyroWave-specific default bitrate (the stored bitrate follows resolution and frame rate, 20 Mbps at 1080p60, below PyroWave's floor), so the value is the one `docs/video-and-latency.md` gives as the start for 1080p/60. The host streamed `codec=3` at 398.7 Mbps (configured 400,000 kbps) in both runs. Host `pyrowave = true`, adaptive FEC not negotiated by the client in either run (host warning, same in both).

| ms, median / p95 / p99 | C | Rust | Rust minus C (median) |
| --- | --- | --- | ---: |
| end_to_end | 36.19 / 43.76 / 62.55 | 34.99 / 41.79 / 57.44 | -1.20 |
| to_decode (end_to_end minus display) | 16.05 / 25.07 / 44.02 | 15.29 / 20.20 / 36.75 | -0.76 |
| decode | 3.05 / 3.45 / 3.77 | 3.04 / 3.37 / 3.64 | -0.01 |
| display | 19.72 / 25.10 / 26.62 | 19.23 / 25.47 / 26.66 | -0.49 |
| assemble | 8.75 / 12.46 / 17.13 | 7.99 / 11.00 / 14.13 | -0.76 |

Frames dropped by the client: 0 in both (4 rows each end with `stream_ended`, from leaving the stream). Host frames the client never showed: 16 (C) and 10 (Rust), counted at the end of the run, includes the frames in flight when the stream ended. One run per engine.

#### Soak: Rust engine, HEVC 1080p60, 40 Mbps, 30 minutes

- Frames: 107,513 sent, 14,915 joined. The client latency CSV holds the last 14,915 frames only (about the last 4 minutes), so the picture-age trend covers the last four minutes, not all 30. Host `stream timings` cover all 30 minutes (357 five-second lines).
- Fps: host median 59.33 per 5 s window (C 75 s runs 59.35), minimum window 57.34. Mean per fifth of the run: 59.37, 59.29, 59.27, 59.35, 59.34, 59.36, 59.29, so steady. Client drops 0, CSV rows lost 0, `present_drops` 0.
- Picture age over the last 4 minutes, per minute (median, p95, p99 ms): 7.57 / 18.41 / 20.22, then 7.39 / 10.55 / 16.28, 7.34 / 10.75 / 16.79, 7.38 / 10.65 / 16.10, and 7.34 / 11.64 / 19.94 for the 515 frames of the partial last minute. The median is flat and equals the 75 s runs. The first minute's p95 of 18 ms is one burst; I did not trace it, but the host logged one FEC-unrecoverable frame and three reference invalidations over the 30 minutes (host `idr_requests` 2, `fec_unrecoverable_frames` 1, no injected loss), so it is plausibly that.
- Memory (`dumpsys meminfo`, total PSS): 153.3 MB at the start, then 148.3, 157.1, 154.5, 155.0, 154.6 MB at 5-minute steps; no growth. A last sample 22 s after the stream ended read 156.1 MB. Java heap 37.7 MB at the start and 38.2 MB at the last in-stream sample; native heap 21.5 and 21.2 MB.
- Errors: no error-level lines in the 30-minute logcat, no ANR or crash, no warning or error in the host log. The stream ended only when the run was stopped.
- Thermal: the thermal service reported status 0 throughout. Battery temperature 33.0 C at the start, 34.0 C from the 25-minute sample on; battery level 85 percent to 79 percent.

#### Caveats

- Fixed. The Rust engine left `clock_offset_us` and `clock_rtt_us` empty in the CSV, so the `end_to_end` figures above used a clock fitted on the C runs (leave-one-out error up to +-0.5 ms); the 0.12 ms median gap is inside that error, and the clock-free `assemble`, `client_total` and `decode` rows are the firmer evidence. Cause: the renderer asks whether the host supports control messages when it starts, but the Rust session was stored only after `Session::start` returned, so the answer was always no and no clock probes ran. The session now reports the host's control-message support through a listener callback before the first frame.
- Fixed. The compact overlay RTT read 2 ms on the C engine and 7 to 8 ms on the Rust engine while the host measured 2.1 ms. Cause: the ENet round-trip smoothing. moonlight-common-c's ENet fork rounds the smoothing step up (`(diff + 7) / 8`), while upstream ENet and `rusty_enet` truncate it (`diff / 8`); with truncation the mean stops moving once a sample is within 7 ms of it, so after a slow first exchange a stream on a 2 ms link stayed at 7 to 8 ms. The Rust engine now builds on a patched, vendored `rusty_enet` 0.4.0 (`protocol/vendor/rusty_enet`) that rounds like the fork, for the mean and the variance; a regression test (`protocol/client/tests/enet_rtt.rs`) pins the value.

#### Verdict

Rust is no worse than C on picture age (median, p95 and p99 equal or lower in HEVC and PyroWave), decode time (equal), loss recovery (no more skipped frames, no corruption at 1% and 5%) and the 30-minute soak (steady fps, flat memory and latency, no errors). By the plan's rule the default can flip, and with both caveats fixed (below) it did, on 2026-10-23.

#### After the fixes (2026-10-23, Retroid Pocket 6)

One 60 s run per engine in the order C, Rust, C, on the same phone and an isolated host: HEVC, 1080p60, 40 Mbps, motion probe noise picture, Wi-Fi RSSI -41 dBm, compact overlay, host with the arm off. Release host build of the B2 package, debug root Nightjar with both fixes.

| | C (1) | Rust | C (2) |
|---|---|---|---|
| CSV rows with a clock offset | 3462 of 3598 | 3469 of 3603 | 3463 of 3600 |
| `clock_rtt_us` median | 2.11 ms | 2.15 ms | 2.10 ms |
| Overlay RTT (mid-run screenshot) | 3 ms | 3 ms | 2 ms |
| Host skipped or dropped frames, byte-cap hits | 0 | 0 | 0 |

The phone-to-PC clock K (the CSV offset minus the host's media epoch) fits a line over receive time at 35.0 us/s on the two C runs. The Rust run's K sits a median of 28 us from that line (p5 -565 us, p95 +47 us), inside the 0.5 ms the brief asked for and inside the C runs' own scatter (p5 to p95 -292 to +462 us). The Rust run's own slope is 36.7 us/s. The Rust engine's overlay RTT now matches the C engine's (2 to 3 ms on every screenshot, against 7 to 8 ms before; the CSV round trip is 2.1 ms on both). One phone, one run per engine, so these confirm the two fixes and say nothing new about picture age.

## Remaining gaps and validation

The [October 2026 Android audit](android-audit-2026-10.md) records the reproducible defects, release tooling gaps and remaining hardware/integration checks. Feature implementation is not a claim that every device, host or document provider has been tested.
