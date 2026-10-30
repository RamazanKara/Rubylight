# Android and desktop

[Documentation](index.md) · [Everything Nightjar does](features.md) · [Host feature support](PARITY-MATRIX.md)

Every Nightjar feature on every Nightjar client: Android (phones, handhelds, tablets, Android TV and Fire TV) and the desktop client for Windows and Linux ([protocol/desktop](../protocol/desktop/README.md)). Both run the same streaming engine, written in Rust, and much of their behaviour comes from the shared `nightjar-policy` crate. Host support is the same for both clients: see [host feature support](PARITY-MATRIX.md).

Status words:

- **Tested**: in the client and run on our own hardware; the evidence column names the device and date.
- **Done**: in the client and covered by its automated tests; not yet run on our hardware, or run only in part.
- **Partial**: in the client with a named gap.
- **Missing**: not in this client yet.
- **N/A**: does not apply to this client.

Our hardware: a Retroid Pocket 6 (Snapdragon, Android 13), a TCL C6K TV (MediaTek, Android 14), a Windows laptop with a Radeon 780M, and a NUC that runs the engine's command-line client on Linux. The Linux desktop client is built and tested by CI; it has not streamed on our hardware. "rc.4 *step*" names the step of the Rubylight 3.0.0-rc.4 hardware pass that a cell refers to.

## Library and PCs

| Feature | Android | Windows | Linux | Evidence and notes |
| --- | --- | --- | --- | --- |
| Discovery, manual addresses, IPv6, Wake-on-LAN | Done | Done | Done | |
| PIN and one-time PIN pairing | Done | Done | Done | |
| Pair with a QR code (Android) or a pasted pairing link (desktop) | Tested | Done | Done | Android, rc.4 R1, 2026-10-28: a pairing link adds and pairs with no PIN, and a forged certificate is refused. Replaying a used code while no other code is live showed a network error instead of "code used"; after rc.4 the host's 400 refusal shows "show a new code" (unit-tested, Retroid retest pending). Desktop: rc.4 L15. |
| Home, library, artwork and the game page (Rubylight 3.0) | Done | Done | Done | |
| Store shelves, All games and the TV layout | Tested | Partial | Partial | TCL C6K, 2026-10-28: TV layout items 1 to 4 (rows, presets, dark theme, edges); the rest is rc.4 T1. Desktop: Home merges the library by title, without store shelves or All games filters. |
| Launch, resume, quit, quit and start another | Done | Done | Done | |
| Confirm before a running game closes | Tested | Done | Done | Retroid, 2026-10-28: Cancel keeps the running game, confirming starts the new one. Desktop: rc.4 L6. |
| Join co-op (Nucleus Co-op) | Done | Done | Done | Android join with a desktop second player is queued for the end batch. |
| Quick resume, quit on leave per game | Done | Done | Done | Off by default (Settings → App). Desktop: rc.4 L5. |
| Remote Monitor and Input-only tiles | Done | Done | Done | Remote Monitor sends no input, microphone, clipboard or commands. |
| Input-only on any PC | Done | Done | Done | Off by default (Settings → Streaming on Android, App on desktop): a 640x360, 30 fps, 500 kbps stream that is neither shown nor played. Desktop: rc.4 L5. |
| Settings per PC and per game, profiles by network, mobile data cap | Done | Done | Done | |
| Settings that open on the main rows, with "More … settings" | Tested | Done | Done | Retroid, rc.4 R2, 2026-10-28: each section's main rows, then "More … settings" opens the rest in place. |
| Settings search and presets | Done | Missing | Missing | |
| PIN lock | Done | Missing | Missing | |
| Launching from a launcher or frontend | Done | Done | Done | Android: TV home screen, Beacon, ES-DE, Daijisho, Pegasus and shortcuts. Desktop: `nightjar --host … [--app …] [--quit-on-exit]`. |
| Host family and version in the PC's details, host web UI button | Done | Missing | Missing | |
| Interface size, high contrast, reduced motion | Done | Done | Done | On Linux, Follow system leaves motion on (no setting to read). |
| Screen reader | Done | Missing | Missing | TalkBack and D-pad navigation on Android. |
| Languages | Done | Partial | Partial | Android: 15 languages. Desktop: English and German. |
| Update check | Done | Missing | N/A | Android, outside Google Play. Linux uses distribution packages. |
| Problem reports and crash records | Done | Partial | Partial | Desktop: the logs folder. |

## Picture

| Feature | Android | Windows | Linux | Evidence and notes |
| --- | --- | --- | --- | --- |
| H.264, HEVC and AV1 hardware decoding | Tested | Done | Partial | Retroid and TCL C6K, 2026-10-28: Snapdragon low-latency HEVC and H.264 at 120 fps, MediaTek's short queues first. Windows: D3D11VA zero-copy. Linux: VAAPI pictures are read back to memory (no dmabuf import). A fallback to software decoding is announced. |
| HDR10 | Tested | Done | Partial | TCL C6K, 2026-10-28: AV1 HDR10 control. Linux asks the compositor for an HDR swap chain; not checked on an HDR display. |
| HDR10+ | Tested | Missing | Missing | TCL C6K, 2026-10-28: HEVC and AV1 show as HDR10+, also through Render through Vulkan. |
| HLG | Tested | Missing | Missing | Retroid, 2026-10-28: PyroWave HLG on screen through the per-device window probe. Render through Vulkan HLG on Android 13: rc.4 R10. |
| Dolby Vision | Tested | Missing | Missing | TCL C6K, 2026-10-28: profile 8.1 over HEVC and 10.1 over AV1 on the TV's Dolby Vision decoders. On the merged code: rc.4 T2 and T3 (TV lane on hold). |
| 10-bit SDR | Done | Tested | Done | Windows laptop, 2026-10-28: after the fix that tells the PC the stream is SDR, it decoded 2305 HEVC Main10 and 2436 AV1 frames with HDR off; 8-bit unchanged. Desktop: Settings → Video → **10-bit color (SDR)**, offered only when this device's decoder gives 10-bit pictures. |
| YUV 4:4:4 | Done | Done | Done | Android: packed 4:4:4 on the Rust engine and PyroWave's chroma modes. Desktop: **Full chroma** packs 4:4:4 into a stream any HEVC or AV1 hardware decoder handles. **Prefer YUV 4:4:4** sends true 4:4:4, which needs a 4:4:4 decoder: the software decoder has one, few hardware decoders do. It checks each codec against this device's decoder when the stream starts and offers only those whose pictures come out as 4:4:4; a decoder that can't gets 4:2:0 and the stream names the reason. A hardware decoder that returns packed 4:4:4 surfaces (AYUV, Y410) is not drawn yet and counts as one that can't. The PC takes 4:4:4 or 10-bit SDR, not both. On the Windows laptop the decoder refused the 4:4:4 test pictures, so it streamed 4:2:0 and named the reason (2026-10-28). |
| PyroWave | Done | Partial | Partial | Desktop: decoding, bandwidth test and pace calibration; no PyroWave automatic bitrate. |
| Upscaling (FSR 1, SGSR 1) | Done | Done | Done | |
| Render through Vulkan | Tested | N/A | N/A | TCL C6K, 2026-10-28: HDR10 and HDR10+ shown; the TV shows fewer frames than the stream carries there, which the pace message reports. |
| Snapdragon GPU drivers | Done | N/A | N/A | |
| Resolution, fractional refresh, virtual display, PC display mode | Done | Done | Done | |
| Adaptive refresh and VRR | Done | Tested | Done | Desktop: presents on decode with tearing for VRR, host-time pacing and V-Sync. On Windows in fullscreen, a display that keeps its own refresh gets V-Sync when the stream runs below about 70% of its rate (measured at 60 fps); a faster stream there still tears. Windows, 2026-10-29: on this PC's 240 Hz FreeSync panel the stream stays on decode, and 12% of a 60 fps stream's presents land mid-scan. The laptop's fixed 120 Hz panel turns to V-Sync 2.2 s in ([measurement](https://github.com/RamazanKara/Rubylight/blob/main/rust/PERFORMANCE.md#october-29-nightjar-desktop-on-a-vrr-panel-and-a-fixed-one)). Android without Android 16's adaptive refresh (ARR), on Android before 16 or a screen without it: VRR runs at the panel's top rate; on a Rubylight host, a game whose rate would judder there gets the highest whole multiple of it the current mode reaches seamlessly (90 Hz for a 90 fps game on a 90/120 Hz panel; 60 fps on 120 Hz stays at 120). Retroid, 2026-10-29 (Android 13, rc.5 host, HEVC 1080p at 120 fps, a 60 Hz test pattern, an earlier build that voted the game's rate itself): the game's rate (59.959 Hz) was found 7 s after the stream started, one vote in 70 s; the current rule keeps that panel at 120 Hz. The lower-rate switch is unit-tested only, since its panel offers one rate. Android under ARR: the stream votes its own rate instead of 0, which would leave a game at Android's 60 Hz default, and the overlay flags a screen presenting half the frames rendered for 3 s while the display reports about half the stream rate. Unit-tested only, no ARR device. Android needs no scan-line check: SurfaceFlinger never flips mid-scan, and a panel without ARR reports only its fixed modes. Linux: not streamed on our hardware. |
| Frame pacing modes (Balanced, Smoothest, drop late) | Done | Missing | Missing | |
| Phase-lock frame timing | Done | Done | Partial | Linux: X11 with the OpenGL renderer only. |
| Beam racing and early decode (PyroWave, wired) | N/A | Done | Done | Both off by default; see [beam racing](beam-racing.md). |
| Rotation, picture-in-picture, external displays | Done | N/A | N/A | |

## Stream quality and the network

| Feature | Android | Windows | Linux | Evidence and notes |
| --- | --- | --- | --- | --- |
| Device controls bitrate | Done | Partial | Partial | Desktop: an older port of the Android controller; rc.4 L1. |
| Automatic resolution and frame rate | Done | Done | Done | |
| Change bitrate / frame rate during a stream | Done | Done | Done | Android: rc.4 R8. Desktop: rc.4 L3. |
| Loss-based bitrate | Done | Done | Done | Off by default. |
| Error correction choice and the loss report | Tested | Done | Tested | Retroid, 2026-10-28: Automatic asked the PC to follow, one loss report a second, none refused. Linux: the NUC's command-line client on shaped links, 2026-10-27 ([measurement](https://github.com/RamazanKara/Rubylight/blob/main/rust/PERFORMANCE.md#october-27-client-error-correction-and-a-link-test-before-the-first-bitrate-nuc)). Loss and recovery parity: rc.4 R9. |
| Link test before the first bitrate | Done | Done | Tested | Off by default; it measured worse on the NUC, 2026-10-27 (same measurement). |
| Recover interrupted streams (on by default) | Done | Done | Done | Android: rc.4 R11. Desktop: rc.4 L4. |
| Ride out network drops | Done | Done | Done | |
| Tell the PC when on Wi-Fi, and the link speed | Done | Done | Done | |
| Stream health report and bitrate trial | Done | Done | Done | Desktop: rc.4 L2. |
| Connection test | Done | Partial | Partial | Desktop: no network path and remote access check, no delay test. |
| Play away from home (Tailscale guide) | Done | Missing | Missing | |
| Prioritize stream traffic | Done | N/A | Done | The engine sets socket and thread priorities on Linux and Android only. |

## Sound

| Feature | Android | Windows | Linux | Evidence and notes |
| --- | --- | --- | --- | --- |
| Stereo, 5.1, 7.1 and loss concealment | Done | Done | Done | |
| 7.1.4 (12 channels) | Done | Done | Missing | Offered where the output has 12 channels, asked of a Rubylight PC that reports 7.1.4. Windows plays it through WASAPI; SDL, and so Linux, opens at most 8 channels. |
| Audio passthrough | Tested | Tested | Done | Retroid, 2026-10-28: PCM stereo from the PC to the AAudio output. Windows, 2026-10-27 and 2026-10-28: PCM stereo and 5.1. Desktop asks for PCM only. Dolby Digital to a TV: rc.4 T8 (log part). |
| Microphone | Tested | Done | Done | Retroid, 2026-10-28: the PC received the voice. |
| Echo cancellation, noise suppression, gain, mute button | Tested | Missing | Missing | Retroid, 2026-10-28: automatic echo cancellation follows a headphone plug, 400 % and 25 % gain differ by 24 dB at the PC without clipping, a missing effect is named, and the mute button mutes and keeps its place. Remote Monitor without the microphone: rc.4 R12. |

## Controls

| Feature | Android | Windows | Linux | Evidence and notes |
| --- | --- | --- | --- | --- |
| Controllers and per-model mapping | Done | Partial | Partial | Desktop: rumble, motion, touchpad, LEDs, battery and DualSense features are wired to SDL's gamepad API, not yet tried with physical controllers. |
| Extra buttons: touchpad click, Share / Capture, paddles | Tested | Done | Done | Retroid, 2026-10-28: mapped buttons reached the PC as the DualSense touchpad click and as Capture on a Switch Pro Controller; Share on an Xbox pad could not be observed. Desktop maps every SDL button already. |
| DualSense, Xbox and Switch Pro USB drivers | Done | N/A | N/A | SDL covers these controllers on the desktop. |
| On-screen controls, Classic and Modern | Tested | Missing | Missing | Retroid, 2026-10-28: the Modern pad reached the PC, also in portrait. |
| Per-game touch controls | Done | Missing | Missing | |
| Touch modes (trackpad, direct, multi-touch, local cursor) | Done | Partial | Partial | Desktop: native touch only. |
| Host cursor shapes | Done | Done | Done | Android: rc.4 R7. Desktop: rc.4 L7. |
| Keyboard, mouse and pen | Done | Done | Done | Desktop also sends a pen's turn in the hand. |
| On-screen PC keyboard | Done | N/A | N/A | |
| Keyboard follows text fields | Tested | Tested | Done | Retroid, 2026-10-28: text, number and password fields open the matching keyboard, a closed keyboard stays closed for its field, and the stream moves up above it. Windows, 2026-10-28: the PC's text field notices reach the client. Desktop opens a keyboard only where the system has an on-screen one. |
| Keyboard layout told to the PC | Done | Tested | Done | Windows, 2026-10-28: the PC logged the reported layout. |
| Saved key chords, import and export | Done | Done | Done | Same file format. |

## Sharing with your PC

| Feature | Android | Windows | Linux | Evidence and notes |
| --- | --- | --- | --- | --- |
| Clipboard text from the menu, host commands | Done | Done | Done | Each only when the host reports it and allows it. |
| Clipboard sync | Tested | Done | Done | Retroid, 2026-10-28: text both ways, accents and emoji included. The refusal cases were fixed after that run; recheck rc.4 R4b. Desktop: rc.4 L8a. |
| Clipboard images | Tested | Done | Done | Retroid, 2026-10-28: a PNG with alpha both ways. Desktop: rc.4 L8a. |
| Files | Done | Done | Done | Android: rc.4 R3 and R5. Desktop: rc.4 L8b. |
| Shared folders | Tested | Done | Done | Retroid, 2026-10-28: browse, download byte for byte, upload, and cancel a large upload. Host side as SYSTEM: 36 of 36 checks. Desktop: rc.4 L8b. |
| USB forwarding | Partial | Tested | Done | Retroid, 2026-10-28: the stream menu row shows only when the PC allows USB for this device; a device's data path needs a USB stick on the Retroid. Windows laptop, 2026-10-28: a mouse attached, enumerated on the PC and detached; recheck rc.4 L10. |

## Diagnostics

| Feature | Android | Windows | Linux | Evidence and notes |
| --- | --- | --- | --- | --- |
| Performance overlay: Compact, Statistics, Debug, host CPU and GPU | Tested | Done | Done | Retroid, 2026-10-28: Debug shows frame pacing, vsyncs per frame and the timeline. Desktop: also the host's pipeline frame rates; rc.4 L1. |
| Input latency test, decoder benchmark, timing CSV | Done | Partial | Partial | Desktop: statistics export and the timing CSV. |
