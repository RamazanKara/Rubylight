# Host feature support

[Documentation](index.md) · [Building](building.md)

What Nightjar does for each capability and which hosts provide it. **Any host** means any Moonlight-compatible host: Apollo, NVIDIA GameStream, Rubylight, Sunshine, Vibepollo and others. Named hosts are the ones known to provide a feature; forks that implement the same extension can work too.

| Capability | What Nightjar does | Host support |
| --- | --- | --- |
| Discovery, manual addresses, IPv6, custom ports | Finds PCs on your network, saves hosts, accepts manual addresses and follows the ports the host advertises. | Any host |
| Wake-on-LAN | Wakes your PC with its validated MAC address over every interface, broadcast and resolved destination. | Any host |
| PIN and one-time-PIN pairing | Certificate pairing with a cancellable flow and a persistent installation identity. | PIN: any host. One-time PIN: Apollo, Rubylight |
| Device permissions | Separate list, view, launch, input, clipboard and command permissions, set by the host. Hosts that report no permissions keep their usual behavior. | Apollo, Rubylight |
| Library and artwork | Shows your apps in host order and caches artwork until the host publishes new art. | Any host |
| Launch, resume, disconnect, quit | Checks permissions first and confirms before ending a game. | Any host |
| Automatic recovery | Bounded retries resume the same running app after transient transport loss. Enabled by default; retries cannot launch, quit or replace an app. | Any host |
| Remote Monitor / Input-only | View-only video and audio, or controls and feedback without starting decoders. | Rubylight |
| H.264 / HEVC / AV1 | Hardware decoding with automatic codec selection and per-device low-latency decoder tuning ([decoder notes](../decoder-errata.txt)). | Any host (AV1 where the host encodes it) |
| HDR10 | Main10 negotiation, BT.2020/PQ, static display metadata and smooth HDR transitions. | Any host with HDR support |
| HDR display luminance | Reports the Android display's HDR luminance at stream start and when the output changes, for the host's output diagnostics. | Rubylight |
| YUV 4:4:4 | Full-chroma conventional profiles and PyroWave chroma modes for sharp colored text. | Hosts that advertise 4:4:4 (e.g. Apollo, Rubylight, Sunshine). PyroWave chroma modes: Rubylight |
| PyroWave | 64-bit Vulkan decoder with record framing, diagnostics and automatic codec fallback. | Rubylight |
| Resolution / fractional refresh / virtual display | Per-PC resolution and refresh rate, host render scale and a matching host virtual display. | Resolution: any host. Fractional refresh: Rubylight. Virtual display and render scale: hosts that advertise a virtual display (e.g. Apollo, Rubylight) |
| PC display mode | Per-PC Exclusive, Extended, primary, separated and physical display choices for normal streams. Separated displays share one Windows session. See [controls](controls.md#pc-display-mode-and-administration). | Rubylight |
| VRR / Android adaptive refresh | Follows the game's cadence on adaptive displays and runs at maximum refresh elsewhere. | Any host. Host-side VRR: Rubylight, Vibepollo |
| Phase-lock frame timing | Reports how long frames wait for the next display latch so the host can time frames to land just before it. Not sent for external displays. | Rubylight |
| Pacing and decoder queues | Lowest latency, balanced, capped and smooth pacing with bounded output queues. | Any host |
| Client upscaling | Lower-resolution SDR video can use bilinear, FSR 1.0 or SGSR 1. PyroWave uses SGSR for both FSR and SGSR selections; native/larger inputs and HDR/10-bit use direct output. | Any host |
| Snapdragon GPU drivers | Import a compatible Vulkan driver ZIP on ARM64 Adreno devices for PyroWave, with system-driver fallback. MediaCodec remains on the system driver. See [driver requirements](video-and-latency.md). | Rubylight (PyroWave only) |
| Automatic bitrate | Per-PC adaptation from loss, RTT variation and decode headroom, up to 500 Mbps at runtime. | Rubylight |
| Automatic resolution/FPS | Opt-in motion, detail and battery modes with user floors and a session ceiling, on decoders supporting stream reconfiguration. Unsupported hosts and PyroWave retain their negotiated size. | Hosts that advertise control message `0x5532` (currently Rubylight) |
| Stream health | [Recent evidence](stream-health.md) for network, decoder, host, presentation and heat, with an optional reversible bitrate trial. | Any host. Bitrate trial: Rubylight |
| Audio | Stereo, 5.1 and 7.1, with low-latency AAudio for stereo on Android 8.1 and later. | Any host |
| Controllers and mappings | Multiple players, per-model button, stick/trigger and D-pad mapping, and built-in Xbox USB drivers. | Any host |
| Per-game touch controls | Held/toggle keyboard and mouse buttons, layout editing, and validated JSON import/export/sharing. | Any host |
| Desktop tools | Pinch zoom/pan with matching absolute input, plus external-display video and a phone touchpad with reconnect fallback. | Any host |
| Remote access guide | [Paired HTTPS checks](remote-setup.md) and direct/DERP/peer-relay status for the current Tailscale peer. | Any host. Tailscale path status: Rubylight |
| DualSense / Edge | Adaptive triggers, rumble, lightbar and Edge paddles over the direct USB driver. | Rumble: any host. Adaptive triggers and lightbar: hosts that send them (Sunshine protocol extension) |
| DualSense USB audio and haptics | Waveform feedback, plus speaker, headset and microphone audio. USB only. See [setup and limitations](dualsense-haptics.md). | Waveform: Rubylight, Vibepollo. Audio: Rubylight |
| Controller reconnects | Host controller preparation and identity/audio-endpoint retention, alongside the guarded stream recovery policy. | Rubylight (recovery policy: any host) |
| Gyro aiming / radial menus | Provided by Steam Input using forwarded controller motion and touchpads. Configure per game in Steam. | Hosts that support controller motion (Sunshine protocol extension) |
| Touch, pen, mouse and keyboard | Trackpad, direct and multi-touch, native pen, pointer capture and keyboard input. Direct positions account for shifted stream layouts. Extra keyboard keys include Esc, F1–F12, modifiers and Task Manager/window shortcuts. Custom chords can be edited under Controls and imported/exported as JSON. | Any host. Native touch and pen: hosts that support them (Sunshine protocol extension) |
| Clipboard / server commands | Foreground text transfer in both directions and the host's configured commands. | Apollo, Rubylight |
| Performance overlay / CSV | Statistics and scrollable Debug views with stage timings, available host processing and CSV export. Choose the mode in the stream menu's Overlay section. | Any host (host processing where the host reports it) |
| Host CPU / AMD GPU telemetry | Optional host CPU usage and AMD ADLX GPU readings in Statistics and Debug, over paired HTTPS during this device's video stream. Requires the host's hardware-metrics opt-in and advertised protocol support. See [host hardware metrics](video-and-latency.md#host-hardware-metrics). | Rubylight |
| Connection test / local benchmark | Connection and throughput test, plus a hostless timing benchmark. | Any host. Throughput download: Rubylight |
| Problem reports | Redacted local events, decoder details and the last crash record, shared when you choose. | Any host |
| PiP / shortcuts / frontends | Picture-in-picture, pinned app shortcuts and launcher entries for ES-DE, Daijisho and Pegasus. | Any host |
| Settings / accessibility | Five sections, search, presets, PC profiles, a control layout editor, TalkBack and D-pad navigation, in English and German. | Any host |
| Host administration | Open host administration from an online PC's menu. Devices, app overrides, display layout and host services remain in the host's web UI, with its own login and network access requirements. | Rubylight |

See [building](building.md) for reproducible checks.

## Remaining gaps and validation

The [October 2026 Android audit](android-audit-2026-10.md) records the reproducible defects, release tooling gaps and remaining hardware/integration checks. Feature implementation is not a claim that every device, host or document provider has been tested.
