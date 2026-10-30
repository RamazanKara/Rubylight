# Android and desktop

[Documentation](index.md) · [Everything Nightjar does](features.md) · [Host feature support](PARITY-MATRIX.md)

Which Nightjar features each client provides: Android (phones, handhelds, tablets, Android TV and Fire TV) and [Nightjar for Windows and Linux](../protocol/desktop/README.md). Both clients run the same Rust streaming engine and share much of their behavior through the `nightjar-policy` crate. Host requirements are identical for both and are listed in [host feature support](PARITY-MATRIX.md).

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
| PIN and one-time PIN pairing | Yes | Yes | Yes | |
| QR pairing | Yes | Yes | Yes | Android scans the code; the desktop pastes the host's pairing link. |
| Home, library, artwork and game pages | Yes | Yes | Yes | Needs a host with the native library (Rubylight 3.0). |
| Store shelves, All games and the TV layout | Yes | Partial | Partial | The desktop Home merges the library by title, without store shelves or All games filters. |
| Launch, resume, quit, and quit and start another | Yes | Yes | Yes | |
| Confirmation before a running game closes | Yes | Yes | Yes | |
| Join co-op (Nucleus Co-op) | Yes | Yes | Yes | |
| Quick resume and quit on leave, per game | Yes | Yes | Yes | Off by default (**Settings → App**). |
| Remote Monitor and Input-only entries | Yes | Yes | Yes | Remote Monitor sends no input, microphone, clipboard or commands. |
| Input-only on any PC | Yes | Yes | Yes | Off by default. A 640×360, 30 fps, 500 kbps stream that is neither shown nor played. |
| Settings per PC and per game, profiles by network, mobile data cap | Yes | Yes | Yes | |
| Settings that open on the main rows, with "More … settings" | Yes | Yes | Yes | |
| Settings search and presets | Yes | No | No | |
| PIN lock | Yes | Yes | Yes | Android can also lock individual PCs. |
| Launching from a launcher or frontend | Yes | Yes | Yes | Android: TV home screen, Beacon, ES-DE, Daijisho, Pegasus and shortcuts. Desktop: `nightjar --host … [--app …] [--quit-on-exit]`. |
| Host software in the PC's details, host web UI button | Yes | Yes | Yes | |
| Interface size, high contrast, reduced motion | Yes | Yes | Yes | On Linux, **Follow system** leaves motion on, since there is no system setting to read. |
| Screen reader | Yes | Yes | Yes | Android: TalkBack. Desktop: the platform accessibility API (UI Automation on Windows, AT-SPI on Linux). |
| Languages | Yes | Partial | Partial | Android: 15 languages. Desktop: English and German. |
| Update check | Yes | Yes | N/A | Android outside Google Play; Windows once per launch. Linux installs updates through its package manager. |
| Problem reports and crash records | Yes | Partial | Partial | The desktop writes redacted logs to its logs folder, and on Windows a minidump for each crash; it has no problem-report export. |

## Picture

| Feature | Android | Windows | Linux | Notes |
| --- | --- | --- | --- | --- |
| H.264, HEVC and AV1 hardware decoding | Yes | Yes | Partial | Windows: D3D11VA, without CPU copies. Linux: VA-API, then V4L2 M2M, then NVDEC; 8-bit 4:2:0 VA-API pictures reach the OpenGL renderer without copies, while 10-bit pictures and those drawn by the GPU renderer (HDR, upscaling, 4:4:4) are copied through memory. A fallback to software decoding is announced. |
| HDR10 | Yes | Yes | Yes | Linux requests an HDR surface from the compositor. |
| HDR10+ | Yes | No | No | |
| HLG | Yes | Yes | Yes | The desktop draws HLG through the BT.2100 inverse OETF and OOTF for the display's peak; on Linux for hardware-decoded pictures. |
| Dolby Vision | Yes | Yes | No | Profiles 8.1 (HEVC) and 10.1 (AV1). Windows draws them on the HDR10 base layer, with the tone map's peak from the Dolby Vision metadata. |
| 10-bit SDR | Yes | Yes | Yes | Offered only when this device's decoder outputs 10-bit pictures. |
| YUV 4:4:4 | Yes | Yes | Yes | See [YUV 4:4:4](#yuv-444). |
| PyroWave | Yes | Partial | Partial | The desktop has decoding, the bandwidth test and pace calibration, but no PyroWave automatic bitrate. |
| Upscaling (FSR 1, SGSR 1) | Yes | Yes | Yes | Windows also offers the GPU's video upscaler (**AI enhancement**). |
| Render through Vulkan | Yes | N/A | N/A | |
| Snapdragon GPU drivers | Yes | N/A | N/A | |
| Resolution, fractional refresh, virtual display, PC display mode | Yes | Yes | Yes | |
| Adaptive refresh and VRR | Yes | Yes | Yes | See [Adaptive refresh and VRR](#adaptive-refresh-and-vrr). |
| Frame pacing modes | Yes | Yes | Yes | Lowest latency, Balanced, Balanced with FPS limit, Smoothest video. |
| Phase-lock frame timing | Yes | Yes | Partial | Linux: X11 with the OpenGL renderer only. |
| Beam racing and early decode (PyroWave, wired) | N/A | Yes | Yes | Both off by default; see [beam racing](beam-racing.md). |
| Rotation, picture-in-picture, external displays | Yes | N/A | N/A | |

### YUV 4:4:4

Android supports packed 4:4:4 on the Rust engine and PyroWave's chroma modes. The desktop offers two settings: **Full chroma** packs 4:4:4 into a stream that any HEVC or AV1 hardware decoder handles, and **Prefer YUV 4:4:4** sends true 4:4:4, which needs a 4:4:4 decoder; the software decoder has one, few hardware decoders do. At stream start the desktop checks each codec against this device's decoder and offers only those whose pictures come out as 4:4:4; otherwise the stream uses 4:2:0 and states the reason. Hardware decoders that return packed 4:4:4 surfaces (AYUV, Y410) are treated as unsupported. The PC provides either 4:4:4 or 10-bit SDR, not both at once.

### Adaptive refresh and VRR

- **Desktop.** Frames are presented on decode with tearing for VRR, with host-time pacing, or with V-Sync. In Windows fullscreen on a display that keeps its own refresh rate, the stream switches to V-Sync when it runs below about 70% of that rate; a faster stream there still tears.
- **Android with adaptive refresh (ARR, Android 16).** The stream votes for its own frame rate instead of 0, which would leave a game at Android's 60 Hz default. The overlay flags a screen that presents only half the rendered frames for 3 seconds.
- **Android without ARR.** VRR runs at the panel's top rate. With a host that reports the game's frame rate (Rubylight 3.0), a rate that would judder there gets the highest whole multiple the current mode reaches seamlessly: 90 Hz for a 90 fps game on a 90/120 Hz panel, while a 60 fps game on 120 Hz stays at 120 Hz.

Android needs no scan-line check: SurfaceFlinger never flips mid-scan, and a panel without ARR reports only its fixed modes.

## Stream quality and the network

| Feature | Android | Windows | Linux | Notes |
| --- | --- | --- | --- | --- |
| Device controls bitrate | Yes | Partial | Partial | The desktop runs an earlier version of the Android controller. |
| Automatic resolution and frame rate | Yes | Yes | Yes | |
| Change bitrate / frame rate during a stream | Yes | Yes | Yes | |
| Loss-based bitrate | Yes | Yes | Yes | Off by default. |
| Error correction choice and the loss report | Yes | Yes | Yes | |
| Link test before the first bitrate | Yes | Yes | Yes | Off by default. |
| Recover interrupted streams | Yes | Yes | Yes | On by default. |
| Ride out network drops | Yes | Yes | Yes | |
| Wi-Fi and link speed reported to the PC | Yes | Yes | Yes | |
| Stream health report and bitrate trial | Yes | Yes | Yes | |
| Connection test | Yes | Partial | Partial | The desktop's **Remote access** check verifies the paired PC over HTTPS; it has no network path or delay test. |
| Play away from home | Yes | Partial | Partial | Android has the Tailscale setup guide; the desktop has the **Remote access** check only. |
| Stream traffic priority | Yes | N/A | Yes | The engine sets socket and thread priorities on Android and Linux. |

## Sound

| Feature | Android | Windows | Linux | Notes |
| --- | --- | --- | --- | --- |
| Stereo, 5.1, 7.1 and loss concealment | Yes | Yes | Yes | |
| 7.1.4 (12 channels) | Yes | Yes | No | Offered when the output has 12 channels and the PC reports 7.1.4. Windows plays it through WASAPI; SDL, used on Linux, opens at most 8 channels. |
| Audio passthrough | Yes | Partial | Partial | Android: PCM, Dolby Digital and Dolby Digital Plus. Desktop: PCM. |
| Microphone | Yes | Yes | Yes | |
| Echo cancellation | Yes | Yes | Yes | **Automatic**, **On** or **Off**. Automatic turns it off with headphones. Linux needs the PipeWire or PulseAudio echo-cancel module. |
| Noise suppression, gain, mute button | Yes | No | No | |

## Controls

| Feature | Android | Windows | Linux | Notes |
| --- | --- | --- | --- | --- |
| Controllers and per-model mapping | Yes | Yes | Yes | The desktop drives rumble, motion, touchpads, LEDs, battery and DualSense features through SDL's gamepad API. |
| Extra buttons: touchpad click, Share / Capture, paddles | Yes | Yes | Yes | The desktop maps every SDL button. |
| DualSense, Xbox and Switch Pro USB drivers | Yes | N/A | N/A | SDL handles these controllers on the desktop. |
| On-screen controls, Classic and Modern | Yes | No | No | |
| Per-game touch controls | Yes | No | No | |
| Touch modes (trackpad, direct, multi-touch, local cursor) | Yes | Partial | Partial | The desktop sends native touch only. |
| Host cursor shapes | Yes | Yes | Yes | |
| Keyboard, mouse and pen | Yes | Yes | Yes | The desktop also sends pen rotation. |
| On-screen PC keyboard | Yes | N/A | N/A | |
| Keyboard follows text fields | Yes | Yes | Yes | The desktop opens a keyboard only where the system provides an on-screen one. |
| Keyboard layout reported to the PC | Yes | Yes | Yes | |
| Saved key chords, import and export | Yes | Yes | Yes | Same file format on both clients. |

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

Our test devices are a Retroid Pocket 6 (Snapdragon, Android 13), a TCL C6K TV (MediaTek, Android 14), a Windows laptop with a Radeon 780M, a Windows PC with a Radeon RX 7900 XT, and a NUC that runs the engine's command-line client on Linux. CI builds and tests the Linux desktop client. Everything above is covered by automated tests; these runs add measurements on real hardware.

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
| Link test before the first bitrate | NUC, 2026-10-27 | Measured worse than without it, so it stays off by default (same measurement). |
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
