# Android and desktop

[Documentation](index.md) · [Everything Nightjar does](features.md) · [Host feature support](PARITY-MATRIX.md)

Which Nightjar features each client provides: Android (phones, handhelds, tablets, Android TV and Fire TV) and the [desktop client](../protocol/desktop/README.md) for Windows, Linux and macOS. Both clients run the same Rust streaming engine and share much of their behavior through the `nightjar-policy` crate. Host requirements are identical for both and are listed in [host feature support](PARITY-MATRIX.md).

Downloads: an APK for Android; an installer and a zip for Windows (x86_64); `.deb`, `.rpm`, AppImage, Flatpak and `tar.gz` for Linux on x86_64 and aarch64; a zip with `Nightjar.app` for macOS 13 or later on Apple Silicon and Intel ([installing](../protocol/desktop/README.md#installing)).

The tables below have no macOS column. The Mac client shares the desktop library, protocol and controls, and decodes with VideoToolbox and draws with Metal; its differences and hardware checks are in the [macOS section](../protocol/desktop/README.md#macos) of the desktop guide.

| Status | Meaning |
| --- | --- |
| **Yes** | Provided by this client. |
| **Partial** | Provided, with the limitation in the notes. |
| **No** | Not provided by this client. |
| **N/A** | Does not apply to this platform. |

[Verification on our hardware](#verification-on-our-hardware) lists the features we have run on real devices, with dates and results.

## Library and PCs

| Feature | Android | Windows | Linux | Notes |
| --- | --- | --- | --- | --- |
| Discovery, manual addresses, IPv6, Wake-on-LAN | Yes | Yes | Yes | |
| When Nightjar opens: PCs, last game or a chosen game, with countdown and guard | Yes | Yes | Yes | The decision is shared (`nightjar-policy`'s `open_play`). |
| Start Nightjar when I sign in | No | Yes | Yes | Run value on Windows; XDG autostart on Linux (a removed package leaves the per-user entry, which `TryExec` makes the desktop skip), the Background portal in a Flatpak. A start at sign-in waits up to a minute for the network. Android has no sign-in start. |
| "Continue" with the last game on the app icon's long press | Yes | No | No | An Android launcher feature. |
| PIN and one-time PIN pairing | Yes | Yes | Yes | |
| Approve a new device from a paired one | Yes | Yes | Yes | On a Rubylight host, for a device with Approve new devices or Host control. Home shows who waits; Pair another device is always in the PC's options, dimmed with the reason without the permission. |
| QR pairing | Yes | Yes | Yes | Android scans the code; the desktop pastes the host's pairing link. |
| Open a pairing link from a browser, a QR reader or the host console | Yes | Yes | Yes | Asks with the PC and its address first. On the desktop a link goes to the open window; the Windows installer and the Linux packages register the `rubylight:` scheme. |
| Home, library, artwork and game pages | Yes | Yes | Yes | Both show Home once a paired PC has supplied its app list or its native library (Rubylight 3.0), and keep the last app list so Home works while the PC sleeps; app lists alone leave the list of PCs as the start page. Store shelves and the richer game pages need the native library on both. |
| Group Home by collection | Yes | Yes | Yes | Needs a host that sends `collections` (Rubylight 3.7). Shared grouping in `nightjar-policy`. |
| Hide a game on this device | Yes | Yes | Yes | Per device; a **Hidden** chip in All games shows them again. |
| Store shelves, All games and the TV layout | Yes | Partial | Partial | The desktop Home has a row per store with an **All N** tile, and All games filters by All, Installed, each store and Other apps, as on Android. It has no **Not added** chip and no per-PC last-played row, because the desktop API does not provide that data, and no separate TV layout. |
| Launch, resume, quit, and quit and start another | Yes | Yes | Yes | |
| Confirmation before a running game closes | Yes | Yes | Yes | |
| Continue here: move the running game's stream between devices | Yes | Yes | Yes | Hosts with `session.continue_here`, for a device with View and Launch |
| Join co-op (Nucleus Co-op) | Yes | Yes | Yes | |
| Microphone only | Yes | Yes | Yes | A session that sends only this device's microphone, on hosts that offer it, for a device with Launch. |
| Restart, shut down or sleep the PC; update its host | Yes | Yes | Yes | In the PC's options, where the host offers it and allows it for this device. See [the PC's power and host updates](console.md#the-pcs-power-and-host-updates). |
| Quick resume and quit on leave, per game | Yes | Yes | Yes | Off by default (**Settings → App**). |
| Remote Monitor and Input-only entries | Yes | Yes | Yes | Remote Monitor sends no input, microphone, clipboard or commands. |
| Input-only on any PC | Yes | Yes | Yes | Off by default. A 640×360, 30 fps, 500 kbps stream that is neither shown nor played. |
| Settings per PC and per game, profiles by network, mobile data cap | Yes | Yes | Yes | |
| Settings that open on the main rows, with "More … settings" | Yes | Yes | Yes | The same sections (Video, Display & image, Controls, Audio, Streaming, App, Advanced, Debug), groups and labels on both; rarely needed settings sit in Debug at the end. |
| Settings search | Yes | Yes | Yes | Desktop: the search field or Ctrl+F. |
| Settings presets (Native, Battery saver) | Yes | No | No | |
| PIN lock | Yes | Yes | Yes | Android can also lock individual PCs. |
| Launching from a launcher or frontend | Yes | Yes | Yes | Android: TV home screen, Beacon, ES-DE, Daijisho, Pegasus and shortcuts. Desktop: `nightjar --host … [--app …] [--quit-on-exit]`. |
| Host software in the PC's details, host web UI button | Yes | Yes | Yes | |
| Text and button size, high contrast, reduced motion | Yes | Yes | Yes | On Linux, **Follow system** leaves motion on, since there is no system setting to read. |
| Screen reader | Yes | Yes | Yes | Android: TalkBack. Desktop: the platform accessibility API (UI Automation on Windows, AT-SPI on Linux). |
| Languages | Yes | Yes | Yes | The same 15 languages everywhere. Desktop CJK, Cyrillic, Polish, Turkish and Vietnamese text uses a system font ([Translations](TRANSLATIONS.md#desktop)). |
| Theme (System, Dark, OLED black) | Yes | No | No | The desktop has one dark theme. |
| Update check | Yes | Yes | Partial | Android outside Google Play, automatically or with **Check for updates now**; Windows and the AppImage once per launch with **Check for updates automatically**, and no check on request. deb, rpm and Flatpak update through their package manager. |
| One-click update | Yes | Yes | Partial | Android outside Google Play; the installed Windows copy runs the new setup with its progress window and restarts, the AppImage replaces its file and restarts. Size and SHA-256 are checked first. A portable Windows copy keeps the link to the release page; deb, rpm and Flatpak update through their package manager. |
| Problem reports and crash records | Yes | Partial | Partial | The desktop writes redacted logs to its logs folder, and on Windows a minidump for each crash; it has no problem-report export. |

## Picture

| Feature | Android | Windows | Linux | Notes |
| --- | --- | --- | --- | --- |
| H.264, HEVC and AV1 hardware decoding | Yes | Yes | Yes | Before a stream, the desktop decodes a test picture of each format and offers the PC only what the hardware decodes. Windows: D3D11VA, without CPU copies. Linux: VA-API, then Vulkan video, then V4L2 M2M (ARM handhelds and boards), then NVDEC; NVIDIA with nvidia-vaapi-driver decodes with Vulkan video, then NVDEC. VA-API and Vulkan video pictures stay on the GPU on both renderers, and NVDEC's are copied into the GPU renderer by CUDA; where a mapping cannot be made, the log says why and the pictures go through memory. A decoder that gives up mid-stream hands over to the next one, with a notice. |
| HDR10 | Yes | Yes | Yes | Linux presents HDR10 PQ (BT.2020) where the compositor offers it, with the host's mastering metadata, else scRGB; under gamescope it asks the swapchain. |
| HDR brightness follows the desktop (setting) | N/A | N/A | Yes | Off by default: the stream keeps the PC's absolute HDR brightness. Android hands HDR to the system and Windows maps it itself, so only a Wayland desktop has the choice. |
| HDR10+ | Yes | No | No | |
| HLG | Yes | Yes | Yes | The desktop draws HLG through the BT.2100 inverse OETF and OOTF for the display's peak; on Linux for hardware-decoded pictures. |
| Dolby Vision | Yes | Yes | No | Profiles 8.1 (HEVC) and 10.1 (AV1). Windows draws them on the HDR10 base layer, with the tone map's peak from the Dolby Vision metadata. |
| Less color banding (10-bit SDR) | Yes | Yes | Yes | Offered only when this device's decoder outputs 10-bit pictures. |
| Sharper colored text (YUV 4:4:4) | Yes | Yes | Yes | See [Sharper colored text](#sharper-colored-text-yuv-444). |
| PyroWave | Yes | Yes | Yes | Decoding, the bandwidth test, pace calibration and PyroWave's own automatic bitrate (`nightjar-policy`). When the decoder falls behind, the desktop skips to the newest frame. |
| Decode PyroWave on a compute queue (setting) | N/A | Yes | Yes | Off by default. Android's renderer has no separate compute queue ([PyroWave decoding](pyrowave-decode.md)). |
| Upscaling (FSR 1, SGSR 1) | Yes | Yes | Yes | Windows also offers the GPU's video upscaler (**AI enhancement**). |
| Render through Vulkan | Yes | N/A | N/A | |
| Renderer choice (**Settings → Advanced**) | N/A | N/A | Yes | Automatic starts on OpenGL and switches to the GPU (Vulkan) renderer when a stream needs it; OpenGL and GPU force one. |
| Snapdragon GPU drivers | Yes | N/A | N/A | |
| Resolution, fractional refresh, virtual display, display arrangement | Yes | Yes | Yes | |
| Dual screen mode | Yes | Yes | Yes | Off by default. A second stream of the same game on a second screen (the desktop: full screen on another monitor), with sound, controllers and the microphone on the first. Needs a host with `SecondScreenVersion` (Rubylight 3.11). |
| Ready the decoder for folding | Yes | N/A | N/A | Foldables: a fold or turn needs no decoder restart. Off by default. |
| Variable refresh rate | Yes | Yes | Yes | See [Variable refresh rate](#variable-refresh-rate). |
| Frame pacing modes | Yes | Yes | Yes | Lowest latency, Balanced, Balanced with FPS limit, Smoothest video. |
| Take back display queue delay | Yes | N/A | N/A | Lowest latency and Balanced. Desktop has no compositor display queue to watch. |
| Phase-lock frame timing | Yes | Yes | Partial | Linux: X11 with the OpenGL renderer only. |
| Beam racing and early decode (PyroWave, wired) | N/A | Yes | Yes | Both off by default; see [beam racing](beam-racing.md). |
| Rotation, picture-in-picture, external displays | Yes | N/A | N/A | |

### Sharper colored text (YUV 4:4:4)

Both clients have one setting, **Sharper colored text** (Off or Automatic), decided the same way in `nightjar-core`: **Automatic** asks for true 4:4:4 where this device decodes it and the PC encodes it, else full chroma packed into a stream that any HEVC or AV1 hardware decoder handles (on Android with the New streaming engine). True 4:4:4 needs a 4:4:4 decoder; the software decoder has one, few hardware decoders do. PyroWave has its own chroma modes. Before a stream starts, the desktop checks the PC's modes and each codec against this device's decoder and offers only those whose pictures come out as 4:4:4; otherwise the stream starts in 4:2:0 and states the reason, and a PC that refuses full chroma after the launch gets the stream again without it.

- **Windows.** GPUs whose decoder supports HEVC 4:4:4 (8- and 10-bit; NVIDIA and Intel, as AYUV, Y410 and Y416 surfaces) decode it in hardware, drawn by the plane shader. AMD decoders have no 4:4:4.
- **Linux.** Packed full chroma from VA-API and Vulkan video is rebuilt on the GPU. On the GPU renderer, Intel's VA-API decodes true 4:4:4 in hardware, and NVDEC's 10-bit 4:4:4 is drawn by the plane shader.
- **With Less color banding.** With **Sharper colored text** on **Automatic** and **Less color banding** on, a PC that encodes HEVC 4:4:4 10-bit streams both at once where the decoder check passes it; otherwise the PC provides either 4:4:4 or 10-bit SDR, not both. When HDR takes priority over YUV 4:4:4, the notice says so.
- **Android** asks only for 8-bit 4:4:4: its media API lists no 10-bit 4:4:4 profile.

### Variable refresh rate

- **Desktop.** Frames are presented on decode with tearing for VRR, with host-time pacing, or with V-Sync. On a display that keeps its own refresh rate, the stream waits for each refresh: on Windows in fullscreen when it runs below about 70% of that rate (a faster stream there still tears), on Linux when the graphics driver reports that the display is not running VRR.
- **Android.** **Variable refresh rate** can be turned on only on Android 16 with a screen that supports adaptive refresh rate (ARR); elsewhere the setting is greyed out, also in PC and game settings, and a saved choice is kept for later. The stream votes for its own frame rate instead of 0, which would leave a game at Android's 60 Hz default. The overlay flags a screen that presents only half the rendered frames for 3 seconds.

Android needs no scan-line check: SurfaceFlinger never flips mid-scan.

## Stream quality and the network

| Feature | Android | Windows | Linux | Notes |
| --- | --- | --- | --- | --- |
| Automatic bitrate (follows resolution, frame rate and codec) | Yes | Yes | Yes | On until the bitrate is set by hand. |
| Lower bitrate when the network is busy | Yes | Yes | Yes | The same controllers, PyroWave's included: Rust `nightjar-policy` on the desktop, the Java originals it is kept equal to on Android. |
| Automatic resolution and frame rate | Yes | Yes | Yes | |
| Change bitrate / frame rate during a stream | Yes | Yes | Yes | |
| Lower bitrate when frames are lost | Yes | Yes | Yes | Off by default. |
| Error correction choice and the loss report | Yes | Yes | Yes | Under **Settings → Debug** on both; the PC's setting by default. |
| Test the link before starting | Yes | Yes | Yes | Off by default. |
| Recover interrupted streams | Yes | Yes | Yes | On by default. |
| Wait through network dropouts | Yes | Yes | Yes | |
| Wi-Fi and link speed reported to the PC | Yes | Yes | Yes | |
| Stream health report and bitrate trial | Yes | Yes | Yes | |
| Speed test and PyroWave frame pace | Yes | Yes | Yes | **Settings → Streaming → Connection test** and **Settings → Advanced → PyroWave frame pace**, and both in each PC's settings; with several paired PCs you choose the PC. |
| Connection test | Yes | Partial | Partial | The desktop's **Connection test** measures the download speed only, on PCs that offer the test; it has no ping, jitter, loss, network path or delay test. Its **Remote access** check verifies the paired PC over HTTPS. |
| Test the network after a failed start | Yes | Yes | Yes | Moonlight's port-test server checks whether a firewall blocks the streaming ports. Android also runs the check by itself after some failed connections; the desktop runs it only when you choose it. |
| Play away from home | Yes | Partial | Partial | Android has the Tailscale setup guide; the desktop has the **Remote access** check only. |
| Stream traffic priority | Yes | Yes | Yes | The engine sets socket and thread priorities on Android and Linux, and thread priorities on Windows. |

## Sound

| Feature | Android | Windows | Linux | Notes |
| --- | --- | --- | --- | --- |
| Stereo, 5.1, 7.1 and loss concealment | Yes | Yes | Yes | |
| 7.1.4 (12 channels) | Yes | Yes | No | Offered when the output has 12 channels and the PC reports 7.1.4. Windows plays it through WASAPI; SDL, used on Linux, opens at most 8 channels. |
| Audio passthrough | Yes | Partial | Partial | Android: PCM, Dolby Digital and Dolby Digital Plus. Desktop: PCM. |
| Microphone | Yes | Yes | Yes | |
| Echo cancellation | Yes | Yes | Yes | **Automatic**, **On** or **Off**. Automatic turns it off with headphones. Linux needs the PipeWire or PulseAudio echo-cancel module. |
| Microphone gain | Yes | Yes | Yes | |
| Noise suppression, automatic gain, microphone button | Yes | No | No | |
| The PC's night mode and headphone surround | Yes | Yes | Yes | In the stream menu, on hosts that offer them. |

## Controls

| Feature | Android | Windows | Linux | Notes |
| --- | --- | --- | --- | --- |
| Controllers and per-model mapping | Yes | Yes | Yes | The desktop drives rumble, motion, touchpads, LEDs, battery and DualSense features through SDL's gamepad API. |
| Controller test, stick and trigger dead zones, Back button as Guide | Yes | Yes | Yes | The desktop's **Controller buttons** page can also set dead zones per controller. |
| Steam Deck Game Mode (gamescope) | N/A | N/A | Yes | Under gamescope the Flatpak opens fullscreen without a border; the pointer hides while the controller is used, and Start+Back opens the stream menu since Steam keeps the Guide button. [Steam Deck](steam-deck.md) |
| Extra buttons: touchpad click, Share / Capture, paddles | Yes | Yes | Yes | The desktop maps every SDL button. |
| DualSense, Xbox and Switch Pro USB drivers | Yes | N/A | N/A | SDL handles these controllers on the desktop. |
| On-screen controls, Classic and Modern | Yes | No | No | |
| Per-game touch controls | Yes | No | No | |
| Touch modes (trackpad, direct, multi-touch, local cursor) | Yes | Partial | Partial | The desktop sends native touch only. |
| Host cursor shapes | Yes | Yes | Yes | |
| Frame Warp | Yes | Yes | Yes | Android: New streaming engine; Vulkan, or OpenGL ES without upscaling, and PyroWave's own renderer with its upscaling and HDR; not with Full chroma. Desktop: every presenter, macOS through Metal; with beam racing one shift per frame. |
| Keyboard, mouse and pen | Yes | Yes | Yes | The desktop also sends pen rotation. |
| System keys only in full screen | Yes | Yes | Yes | With **Send system keys** on, Alt+Tab, the Windows key and similar stay with this device while the stream is in a window (Android: split screen or a window). |
| On-screen PC keyboard | Yes | N/A | N/A | |
| Keyboard follows text fields | Yes | Yes | Yes | The desktop opens a keyboard only where the system provides an on-screen one. |
| Keyboard layout reported to the PC | Yes | Yes | Yes | |
| Saved key chords, import and export | Yes | Yes | Yes | Same file format on both clients. |
| Stream hotkeys, changeable | Yes | Yes | Yes | One table for both clients. Mouse mode, clipboard, fullscreen, minimize and the shortcut list are desktop-only; the PC keyboard and local cursor are Android-only. |

## Sharing with your PC

Each sharing feature appears only when the host reports it and allows it for this device.

| Feature | Android | Windows | Linux | Notes |
| --- | --- | --- | --- | --- |
| Clipboard text from the menu, host commands | Yes | Yes | Yes | |
| Clipboard sync | Yes | Yes | Yes | |
| Clipboard images | Yes | Yes | Yes | |
| Files | Yes | Yes | Yes | |
| Shared folders | Yes | Yes | Yes | |
| USB forwarding | Yes | Yes | Yes | |

## Diagnostics

| Feature | Android | Windows | Linux | Notes |
| --- | --- | --- | --- | --- |
| Performance overlay: Compact, Statistics, Debug, host CPU and GPU | Yes | Yes | Yes | The desktop also shows the host's pipeline frame rates. |
| Input latency test, decoder benchmark, timing CSV | Yes | Partial | Partial | The desktop exports statistics and the timing CSV; it has no latency test or benchmark. |

## Verification on our hardware

Our test devices are a Retroid Pocket 6 (Snapdragon, Android 13), a Galaxy Z Fold7, a TCL C6K TV (MediaTek, Android 14), a Windows laptop with a Radeon 780M, a Windows PC with a Radeon RX 7900 XT, and an Intel NUC that runs Nightjar for Linux. CI builds and tests the desktop client on Windows, Linux and macOS. Everything above is covered by automated tests; these runs add measurements on real hardware. The release notes record later device runs.

| Feature | Device and date | Result |
| --- | --- | --- |
| QR pairing | Retroid, 2026-10-28 | A pairing link adds and pairs the PC without a PIN; a forged certificate is refused. |
| TV layout | TCL C6K, 2026-10-28 | Rows, presets, dark theme and screen edges. |
| Confirmation before a running game closes | Retroid, 2026-10-28 | Cancel keeps the running game; confirming starts the new one. |
| Settings that open on the main rows | Retroid, 2026-10-28 | Each section shows its main rows; "More … settings" opens the rest in place. |
| Hardware decoding | Retroid and TCL C6K, 2026-10-28 | Snapdragon low-latency HEVC and H.264 at 120 fps; MediaTek's short queues on the first configure. |
| HDR10, HDR10+ | TCL C6K, 2026-10-28 | AV1 HDR10; HEVC and AV1 shown as HDR10+, also through **Render through Vulkan**. |
| HLG | Retroid, 2026-10-28 | PyroWave HLG on screen through the per-device window probe. |
| Dolby Vision | TCL C6K, 2026-10-28 | Profile 8.1 over HEVC and 10.1 over AV1 on the TV's Dolby Vision decoders, on a pre-release build. |
| 10-bit SDR | Windows laptop, 2026-10-28 | 2305 HEVC Main10 and 2436 AV1 frames decoded with HDR off; 8-bit unchanged. |
| YUV 4:4:4 | Windows laptop, 2026-10-28 | The decoder refused the 4:4:4 test pictures; the stream used 4:2:0 and stated the reason. |
| Render through Vulkan | TCL C6K, 2026-10-28 | HDR10 and HDR10+ shown. The TV presents fewer frames than the stream carries, and the pace message reports it. |
| VRR on the desktop | Windows PC and laptop, 2026-10-29 | On a 240 Hz FreeSync panel the stream stays on decode, and 12% of a 60 fps stream's presents land mid-scan. The laptop's fixed 120 Hz panel switches to V-Sync 2.2 s into the stream ([measurement](https://github.com/RamazanKara/Rubylight/blob/main/rust/PERFORMANCE.md#october-29-nightjar-desktop-on-a-vrr-panel-and-a-fixed-one)). |
| Refresh rate on Android | Retroid, 2026-10-29 | HEVC 1080p120 with a 60 Hz test pattern: the game's rate (59.959 Hz) was detected 7 s after the stream started, with one vote in 70 s. |
| Error correction and the loss report | Retroid, 2026-10-28; NUC, 2026-10-27 | Automatic asked the PC to follow, one loss report a second, none refused. NUC on shaped links ([measurement](https://github.com/RamazanKara/Rubylight/blob/main/rust/PERFORMANCE.md#october-27-client-error-correction-and-a-link-test-before-the-first-bitrate-nuc)). |
| Test the link before starting | NUC, 2026-10-27 | Measured worse than without it, so it stays off by default (same measurement). |
| Audio passthrough | Retroid, 2026-10-28; Windows, 2026-10-27 and 2026-10-28 | PCM stereo to the AAudio output; PCM stereo and 5.1 on Windows. |
| Microphone, echo cancellation | Retroid, 2026-10-28 | The PC received the voice; **Automatic** follows a headphone plug. |
| Microphone gain and mute | Retroid, 2026-10-28 | 400% and 25% gain differ by 24 dB at the PC without clipping; a missing effect is named; the mute button mutes and keeps its position. |
| Extra controller buttons | Retroid, 2026-10-28 | Mapped buttons reached the PC as the DualSense touchpad click and as Capture on a Switch Pro Controller. |
| On-screen controls | Retroid, 2026-10-28 | The Modern pad reached the PC, also in portrait. |
| Keyboard follows text fields | Retroid and Windows, 2026-10-28 | Text, number and password fields open the matching keyboard; a closed keyboard stays closed for its field; the stream moves up above it. On Windows, the PC's text-field notices reach the client. |
| Keyboard layout reported to the PC | Windows, 2026-10-28 | The PC logged the reported layout. |
| Clipboard sync and images | Retroid, 2026-10-28 | Text both ways, including accents and emoji; a PNG with alpha both ways. |
| Shared folders | Retroid, 2026-10-28 | Browse, byte-exact download, upload, and cancelling a large upload; 36 of 36 host-side checks. |
| USB forwarding | Retroid and Windows laptop, 2026-10-28 | The stream menu row appears only when the PC allows USB for this device. On Windows a mouse attached, enumerated on the PC and detached. |
| Performance overlay | Retroid, 2026-10-28 | Debug shows frame pacing, vsyncs per frame and the timeline. |
