# Rubylight host and Android features

[Documentation](index.md) · [Building](building.md)

How the Android client works with each Rubylight host feature.

| Capability | What Rubylight Android does |
| --- | --- |
| Discovery, manual addresses, IPv6, custom ports | Finds PCs on your network, saves hosts, accepts manual addresses and follows the ports the host advertises. |
| Wake-on-LAN | Wakes your PC with its validated MAC address over every interface, broadcast and resolved destination. |
| PIN and one-time-PIN pairing | Certificate pairing with a cancellable flow and a persistent installation identity. |
| Device permissions | Separate list, view, launch, input, clipboard and command permissions, set by the host. |
| Library and artwork | Shows your apps in host order and caches artwork until the host publishes new art. |
| Launch, resume, disconnect, quit | Checks permissions first and confirms before ending a game. |
| Remote Monitor / Input-only | View-only video and audio, or controls and feedback without starting decoders. |
| H.264 / HEVC / AV1 | Hardware decoding with automatic codec selection and per-device low-latency decoder tuning ([decoder notes](../decoder-errata.txt)). |
| HDR10 | Main10 negotiation, BT.2020/PQ, static display metadata and smooth HDR transitions. |
| YUV 4:4:4 | Full-chroma conventional profiles and PyroWave chroma modes for sharp colored text. |
| PyroWave | 64-bit Vulkan decoder with record framing, diagnostics and automatic codec fallback. |
| Resolution / fractional refresh / virtual display | Per-PC resolution and refresh rate, host render scale and a matching host virtual display. |
| PC display mode | Per-PC Exclusive, Extended, primary, separated and physical display choices for normal streams. Separated displays share one Windows session. See [controls](controls.md#pc-display-mode-and-administration). |
| VRR / Android adaptive refresh | Follows the game's cadence on adaptive displays and runs at maximum refresh elsewhere. |
| Pacing and decoder queues | Lowest latency, balanced, capped and smooth pacing with bounded output queues. |
| Client upscaling | Lower-resolution SDR video can use bilinear, FSR 1.0 or SGSR 1. PyroWave uses SGSR for both FSR and SGSR selections; native/larger inputs and HDR/10-bit use direct output. |
| Snapdragon GPU drivers | Import a compatible Vulkan driver ZIP on ARM64 Adreno devices for PyroWave, with system-driver fallback. MediaCodec remains on the system driver. See [driver requirements](video-and-latency.md). |
| Automatic bitrate | Per-PC adaptation from loss, RTT variation and decode headroom, up to 500 Mbps at runtime. |
| Audio | Stereo, 5.1 and 7.1, with low-latency AAudio for stereo on Android 8.1 and later. |
| Controllers and mappings | Multiple players, per-model button mapping and built-in Xbox USB drivers. |
| DualSense / Edge | Adaptive triggers, rumble, lightbar and Edge paddles over the direct USB driver. |
| DualSense USB audio and haptics | Waveform feedback with compatible hosts, plus speaker, headset and microphone audio with matching updated Rubylight hosts. USB only. See [setup and limitations](dualsense-haptics.md). |
| Reconnect reliability | Three bounded retries after interrupted streams, resume after screen lock, and host controller preparation/identity retention with matching Rubylight hosts. |
| Gyro aiming / radial menus | Provided by Steam Input using forwarded controller motion and touchpads. Configure per game in Steam. |
| Touch, pen, mouse and keyboard | Trackpad, direct and multi-touch, native pen, pointer capture and keyboard input. Direct positions account for shifted stream layouts. Extra keyboard keys include Esc, F1–F12, modifiers and Task Manager/window shortcuts. Custom chords can be edited under Controls and imported/exported as JSON. |
| Clipboard / server commands | Foreground text transfer in both directions and the host's configured commands. |
| Performance overlay / CSV | Statistics and scrollable Debug views with stage timings, available host processing and CSV export. Choose the mode in the stream menu's Overlay section. |
| Host CPU / AMD GPU telemetry | Optional host CPU usage and AMD ADLX GPU readings in Statistics and Debug, over paired HTTPS during this device's video stream. Requires the host's hardware-metrics opt-in and advertised protocol support. See [host hardware metrics](video-and-latency.md#host-hardware-metrics). |
| Connection test / local benchmark | Connection and throughput test, plus a hostless timing benchmark. |
| Problem reports | Redacted local events, decoder details and the last crash record, shared when you choose. |
| PiP / shortcuts / frontends | Picture-in-picture, pinned app shortcuts and launcher entries for ES-DE, Daijisho and Pegasus. |
| Settings / accessibility | Five sections, search, presets, PC profiles, a control layout editor, TalkBack and D-pad navigation, in English and German. |
| Host administration | Open host administration from an online Rubylight PC's menu. Devices, app overrides, display layout and host services remain in the web console, with its own login and network access requirements. |

See [building](building.md) for reproducible checks.

## Remaining gaps and validation

The [October 2026 Android audit](android-audit-2026-10.md) records the reproducible defects, release tooling gaps and remaining hardware/integration checks. Feature implementation is not a claim that every device, host or document provider has been tested.
