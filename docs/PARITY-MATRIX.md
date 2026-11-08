# Rubylight Android and host parity matrix

[Documentation](index.md) · [Device checklist](device-checklist.md)

This matrix describes paths implemented in the Android source. Device checks remain required for hardware behavior, latency, image quality and host policy; implementation does not certify a phone or controller.

| Capability | Android implementation | Boundary / device check |
| --- | --- | --- |
| Discovery, manual addresses, IPv6, custom ports | Discovery and saved hosts; manual addressing; host-advertised service ports | Network routing and discovery isolation. |
| Wake-on-LAN | Validated MAC; interface/global broadcasts and resolved destinations | PC firmware, NIC power state and routers. |
| PIN and one-time-PIN pairing | Certificate pairing, cancellable flow and persistent installation identity | Expiry, cancellation and backup/restore. |
| Device permissions | Separate list/view/launch/input/clipboard/command gates | Host is authoritative; confirm enabled/denied devices. |
| Library and artwork | App UUIDs, host ordering, artwork-version caching | Host must change its artwork token to signal replacement. |
| Launch, resume, disconnect, quit | Permission checks and explicit destructive-action confirmation | Ownership changes, two-client sessions and reconnect. |
| Remote Monitor / Input-only | View-only media or input/feedback without media decoders | Role-scoped exit, host support and confirmation expiry. |
| H.264 / HEVC / AV1 | Hardware MediaCodec, capability selection and decoder hints | Exact codec/firmware performance; [decoder evidence](../decoder-errata.txt). |
| HDR10 | Main10 negotiation, BT.2020/PQ, static display metadata and transitions | HDR screen/decoder/host; no dynamic HDR10+ metadata path. |
| YUV 4:4:4 | Capability-gated conventional profiles; PyroWave chroma modes | Fallback to 4:2:0, HDR priority and actual hardware profiles. |
| PyroWave | 64-bit Vulkan decoder, codec fallback, record framing on compatible hosts, diagnostics | GPU readiness, compatible bitstream, loss concealment and sustained bandwidth. |
| Resolution / fractional refresh / virtual display | Per-PC geometry/rate, render scale and advertised virtual-display request | Host driver and device/app display policy; render scale is not desktop DPI. |
| VRR / Android adaptive refresh | Cadence on confirmed adaptive displays; maximum-refresh fallback | Android/OEM mode policy and observed presentation cadence. |
| Pacing and decoder queues | Latency, balanced, capped and smooth modes; bounded output queues | Mixed refresh ratios, starvation, callback availability and thermal behavior. |
| Client upscaling | Bilinear, FSR 1.0 and SGSR 1 with sharpening and fallback | Lower-resolution MediaCodec SDR only; image quality and GPU cost. |
| Automatic/runtime bitrate | Per-PC opt-in adaptation using loss, RTT variation and decode headroom | Host caps; PyroWave queue signal; 500 Mbps runtime ceiling. No negotiated runtime FEC-percentage control. |
| Audio | Stereo, 5.1/7.1; eligible API 27+ stereo tries low-latency AAudio with AudioTrack fallback | Routes, effects, rate mismatch, buffer recovery and A/V sync. |
| Controllers and mappings | Multiple slots, model-specific digital mappings, Xbox USB drivers | Physical mappings, detach/reconnect and host game limits. |
| DualSense / Edge | Android controller extensions and opt-in direct USB driver with triggers/paddles | Real pad firmware/host feedback; no Bluetooth trigger or USB audio-haptics implementation. |
| Touch, pen, mouse and keyboard | Relative/direct/native touch, native pen, pointer capture and text/key input | Host permissions, OS-reserved keys, layouts and gesture cancellation. |
| Clipboard / server commands | Explicit foreground text transfer and configured host commands | Independent permissions; no command execution-result acknowledgement. |
| Performance overlay / CSV | Compact and Advanced views; stage timings, host processing and export | Missing metrics remain unavailable; callbacks are not physical scanout. |
| Connection test / local benchmark | HTTP probe test, optional throughput download; hostless timing benchmark | Neither measures full end-to-end stream latency. |
| Problem reports | Local redacted events, decoder details and last crash record; manual sharing | Review receiving apps and any separately attached host logs. |
| PiP / shortcuts / frontends | Local PiP disconnect, pinned apps and exported launcher entries | Android lifecycle, URI access, aspect ratios and permission changes. |
| Settings / accessibility | Five sections, search, presets, PC profiles and control layout editor | Small screens, large fonts, TalkBack, D-pad and English/German labels. |
| Host administration | Uses host policies and configured commands | Edit devices, app overrides, display layout and host services in the host web console. |

See [building](building.md) for reproducible checks. Record hardware results with the tested Rubylight Android and host versions in the [device checklist](device-checklist.md).
