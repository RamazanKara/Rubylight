# Features

[Documentation](index.md) · [Android and desktop](CLIENT-PARITY.md) · [Host feature support](PARITY-MATRIX.md)

Nightjar 4.0 streams a PC to Android phones, handhelds, tablets, Android TV and Fire TV, and to Windows and Linux. All clients share one streaming engine, written in Rust; the Android app also keeps the classic moonlight-common-c engine as a setting.

This page lists every feature with a link to its guide. Features available on only one client are marked, and [Android and desktop](CLIENT-PARITY.md) compares the clients row by row. Features that need support from the PC's host name the requirement; [host feature support](PARITY-MATRIX.md) lists which hosts provide each capability. Every Moonlight-compatible host gets the stream, the controls and the diagnostics.

## Library

- **Home.** Favorites, **Continue playing** and the games of every paired PC in one library. Cover art is cached on the device, so Home works while a PC sleeps. Needs a host with the native library API (Rubylight 3.0). [Home](console.md#home)
- **Store shelves on a TV.** Rows for Steam, Epic Games, GOG, EA app, Ubisoft Connect, Battle.net and Xbox under a large hero, plus Recently played, Other apps, and games the PC found but that were never added. Android. [On a TV](console.md#on-a-tv)
- **All games.** One poster grid, filtered by store, by installed, or by not yet added.
- **Game pages.** The game's store, whether it is running, installed or HDR-capable, the PC, and the exact settings the stream will use, followed by **Play** or **Resume**. [A game's page](console.md#a-games-page)
- **Wake and play.** Selecting a sleeping PC's game wakes the PC and starts the game.
- **Confirmation before a game closes.** Starting a game while another is running closes the running one only after confirmation, and the prompt names both games.
- **Join co-op.** Take a free player slot in a PC's Nucleus Co-op split-screen session, or return to your own seat. Needs a host with co-op sessions (Rubylight 3.0). [Join co-op](console.md#join-co-op)
- **Launcher integration.** Games start from the Android TV home screen, Beacon, ES-DE, Daijisho, Pegasus and home-screen shortcuts on Android ([launcher integrations](FRONTENDS.md)), and from `nightjar --host … --app …` on Windows and Linux.

## Pairing and PCs

- **Discovery and manual entry.** PCs are found on the local network or added by name, IPv4 or IPv6 address and port. Wake-on-LAN is sent on every network interface.
- **Pairing.** PIN, one-time PIN, and QR pairing: **Scan QR code** on Android, or a pasted pairing link on Windows and Linux. QR pairing needs a host that offers it (Rubylight 3.0). [Pairing](pairing.md)
- **Per-device permissions.** Permissions set on the host are checked before every protected action, and a refused action names the missing permission.
- **Settings per PC and per game.** Profiles selected by network (mobile data, home Wi-Fi, other Wi-Fi) and a mobile data cap. [Settings](settings.md)
- **Remote Monitor and Input-only.** Watch a PC without sending input (on hosts that list the role, such as Rubylight and Vibepollo), or control it without video on any PC. [Input-only](controls.md#input-only-on-any-pc)
- **PIN lock.** For the app or for every stream; on Android also for selected PCs.
- **Host web UI.** Opens the host's web UI from the PC's details.

## Picture

- **H.264, HEVC and AV1** on the hardware decoder: MediaCodec with per-chip low-latency settings for Snapdragon, MediaTek, Tegra, Exynos, Google Tensor and Amlogic on Android ([decoder flags](decoder-flags.md)), D3D11VA on Windows, and VA-API, V4L2 M2M or NVDEC on Linux.
- **HDR.** HDR10 with any HDR-capable host. HDR10+ (with scene statistics), HLG and Dolby Vision (profile 10.1 on AV1, 8.1 and 8.4 on HEVC) need a host that sends them (Rubylight 3.0). Android shows all four; Windows HDR10, HLG and Dolby Vision; Linux HDR10 and HLG where the compositor provides an HDR surface. [HDR formats](video-and-latency.md#hdr-formats)
- **10-bit SDR and 4:4:4.** 10-bit SDR reduces banding; full-chroma 4:4:4 keeps colored text sharp. **Full chroma** packs 4:4:4 into a stream that ordinary hardware decoders handle; true 4:4:4 needs a decoder that supports it, usually the software decoder. Each is offered only where this device's decoder handles it; otherwise the stream uses 8-bit or 4:2:0 and states the reason. [HDR and color](video-and-latency.md#hdr-and-color)
- **PyroWave.** A low-latency wavelet codec, decoded through Vulkan on Android and on the GPU on Windows and Linux, including 4:4:4 and HDR. On Snapdragon devices a newer Adreno driver can be loaded for it. Needs a host that offers PyroWave. [PyroWave GPU decoding](pyrowave-decode.md)
- **Upscaling.** FSR 1 or Snapdragon GSR on the device; on Windows also the GPU's video upscaler (**AI enhancement**) where the driver provides it. [Upscaling](video-and-latency.md#upscaling)
- **Render through Vulkan.** On Android, the stream can be drawn through Vulkan, so frame-generation overlays can hook the video, with HDR10, HDR10+ and HLG.
- **Virtual display.** The PC can create a display at this device's exact resolution and refresh rate, including fractional rates, and you choose what the PC's monitors do during the stream. Needs a host with virtual displays (Apollo, Vibepollo, Rubylight). [PC display mode](controls.md#pc-display-mode-and-administration)
- **Frame timing.** Adaptive refresh and VRR follow the game's frame rate. With hosts that support phase-lock timing (Rubylight 3.0), Nightjar reports how long frames wait for the next display refresh, so the host can time frames to land just before it. When a frame comes for every refresh (60 fps on a 60 Hz screen), it asks for more room, because a frame that misses its refresh there waits a whole refresh and costs the next frame its place. [Refresh rate and VRR](video-and-latency.md#refresh-rate-and-vrr)
- **Beam racing.** On Windows and Linux, for wired links: a PyroWave picture is decoded in slices while the rest is still arriving. [Beam racing](beam-racing.md)
- **Orientation and displays.** Portrait and rotation, picture-in-picture, and video on an external display with this device as the touchpad. Android. [Orientation](video-and-latency.md#orientation)

## Stream quality and network

- **Device controls bitrate.** Adjusts the bitrate to loss, round-trip variation and the decoder's headroom, up to 2000 Mbps. **Automatic quality** changes resolution and frame rate during the stream, favoring smooth motion, sharp detail or battery life. [Recovery and automatic quality](recovery-and-quality.md)
- **Changes during a stream.** **Change bitrate / frame rate** and **Loss-based bitrate** need a host that accepts them (Rubylight 3.0). **Error correction** can be left to the PC, follow measured loss or use a fixed share, with Rubylight and Sunshine-based PCs that accept it. A Sunshine-based PC with its own automatic bitrate follows Nightjar's network reports. [Bitrate and error correction](video-and-latency.md#bitrate-frame-rate-and-error-correction-with-a-rubylight-pc)
- **Recover interrupted streams.** On by default: up to five attempts within a minute return to the same running game, never a different one. [Recovering a connection](recovery-and-quality.md#recovering-a-connection)
- **Ride out network drops.** Holds the stream through 20, 30 or 60 seconds of network silence. [Network drops](video-and-latency.md#network-drops)
- **Link reporting.** Nightjar tells a supporting host whether the device is on Wi-Fi or a wired link and how fast the link is, and reports lost video once a second.
- **Play away from home.** On Android, a guide sets up a private Tailscale connection and tests the PC from where you are; the desktop checks the paired PC with **Remote access → Test this PC**. [Play away from home](remote-setup.md)

## Sound

- **Stereo, 5.1, 7.1 and 7.1.4.** Low-latency AAudio on Android; lost packets are concealed rather than played as gaps. 7.1.4 needs a host that sends it (Rubylight 3.0) and a 12-channel output (Android 12L or newer on Android).
- **Audio passthrough.** PCM, Dolby Digital or Dolby Digital Plus to a receiver, soundbar or TV on Android; PCM on Windows and Linux. Needs a host that sends it (Rubylight 3.0). [Sound](video-and-latency.md#sound)
- **Microphone.** Echo cancellation on Android and the desktop; noise suppression, automatic gain, a gain slider and a one-tap mute button on Android. [Microphone](controls.md#microphone)

## Controls

- **Controllers.** Up to 16 players, with button, stick, trigger and D-pad mapping per controller model. Extra buttons can press the touchpad, Share / Capture or paddles 1 to 4 on the PC. [Gamepads](controls.md#gamepads)
- **USB controller drivers.** DualSense over USB with adaptive triggers, the lightbar, Edge paddles, waveform haptics, and its speaker, headset jack and microphone; built-in drivers for Xbox and Switch Pro controllers. Android; the desktop uses SDL for the same controllers. [DualSense and USB](controls.md#dualsense-and-usb)
- **On-screen controls.** Classic and Modern styles with an in-stream editor, and per-game touch layouts with keys, buttons, sticks, triggers and a D-pad, shared as files. Android. [On-screen controls](controls.md#on-screen-controls)
- **Touch modes.** Trackpad with acceleration and glide, direct mouse, multi-touch, and a local cursor that takes the PC's own cursor shape from a host that sends it. Android. [Touch modes](controls.md#touch-modes)
- **PC keyboard.** A full on-screen keyboard with Esc, F1 to F12, arrows and sticky modifiers, usable by touch, controller or TV remote. Android. [PC keyboard](controls.md#pc-keyboard)
- **Keyboard follows text fields.** The device keyboard opens when a text field on the PC takes focus, keeps the field visible and closes afterwards; Nightjar also reports the keyboard layout to the PC. Needs a host that reports text fields (Rubylight 3.0). [Keyboard follows text fields](controls.md#keyboard-follows-text-fields)
- **Mouse, keyboard, pen and native touch.** Saved key chords and three- and four-finger shortcuts.

## Sharing

- **Clipboard sync.** Copied text in both directions, and images on request from the stream menu. [Clipboard](sharing.md#clipboard)
- **Files.** Transfers to and from the PC's file-transfer folder, including from Android's share sheet. [Files](sharing.md#files)
- **Shared folders.** Browse, download from and upload to folders the PC shares. [Shared folders](sharing.md#shared-folders)
- **USB devices.** Hand a device such as a drawing tablet or a security key to the PC. [USB devices](sharing.md#usb-devices)

Each needs a host that offers it (Rubylight 3.0) and allows it for this device.

## Diagnostics

- **Performance overlay.** Compact, Statistics or Debug, in any corner, with the PC's CPU and GPU load, temperatures, power and clocks from a host that sends them. [Performance overlay](video-and-latency.md#performance-overlay)
- **Why did my stream stutter?** Names the likely cause and can run a reversible bitrate trial. [Stream health](stream-health.md)
- **Measurement tools.** An input latency test during a stream, a decoder benchmark that needs no PC, and frame timings as CSV. Android; the desktop exports statistics and frame timings. [Device results](devices.md)
- **Connection test and problem reports.** A connection test before raising the bitrate, and redacted problem reports that stay on the device until you share them. Android. [Troubleshooting](TROUBLESHOOTING.md)

## Interface

- **Controller and remote first.** Every screen works with a controller or a TV remote, and with touch, mouse and keyboard. [Menu navigation](controls.md#menu-navigation)
- **Readable status.** Green for good, yellow for a warning and red for poor, always with a word and a shape as well as color.
- **Layered settings.** Each section opens on the settings most people change, with the rest under "More … settings". Search across all settings, presets, and help on every row on Android. [Settings](settings.md)
- **Accessibility.** Interface size, high contrast, reduced motion and screen reader support, and System, Dark and OLED black themes. [Accessibility](console.md#accessibility-on-android)
- **Languages.** 15 on Android; English and German on Windows and Linux. [Translations](TRANSLATIONS.md)
- **Help and updates.** Built-in help that works offline. Update checks on Android (outside Google Play) and on Windows.
