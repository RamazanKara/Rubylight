# Everything Nightjar does

[Documentation](index.md) · [Android and desktop](CLIENT-PARITY.md) · [Host feature support](PARITY-MATRIX.md)

Nightjar 4.0 streams your PC to Android phones, handhelds, tablets, Android TV and Fire TV, and to Windows and Linux PCs. Every one of them runs the same Rust streaming engine. This page lists every feature. A feature only one client has says so ([Android and desktop](CLIENT-PARITY.md) compares them row by row), and a feature that needs something from the PC names the host ([host feature support](PARITY-MATRIX.md)). Most extras come from a [Rubylight](https://github.com/RamazanKara/Rubylight) PC; every Moonlight-compatible host gets the stream, the controls and the diagnostics.

## Your library

- **Home**: favourites, Continue playing and the games of every PC in one library, with covers kept on this device so Home works while a PC sleeps. Rubylight 3.0. [Home](console.md#home)
- **A shelf for every store** on a TV: Steam, Epic Games, GOG, EA app, Ubisoft Connect, Battle.net and Xbox rows under a large hero, plus Recently played, Other apps and games your PC found but never added. Android. [On a TV](console.md#on-a-tv)
- **All games** in one poster grid, filtered by store, installed or not added yet.
- **A page for every game**: its store, whether it runs, is installed or has HDR, the PC and the exact stream settings it will use, then Play or Resume. [A game's page](console.md#a-games-page)
- **Wake PC and play**: pick a sleeping PC's game and Nightjar wakes the PC and starts it.
- **No game closed by surprise**: starting a game while another runs closes the running one only after you confirm, and the prompt names both.
- **Join co-op**: take a free player slot in a PC's Nucleus Co-op split-screen game, or return to your own seat. Rubylight. [Join co-op](console.md#join-co-op)
- **Launch from anywhere**: the Android TV home screen, Beacon, ES-DE, Daijisho and Pegasus, and home-screen shortcuts on Android ([frontends](FRONTENDS.md)); `nightjar --host … --app …` for frontends on Windows and Linux.

## Pairing and your PCs

- **Finds your PCs** on the network, or adds one by name, IPv4 or IPv6 address and port. Wake-on-LAN over every interface.
- **PIN pairing**, one-time PIN pairing, and **Scan QR code** in Add PC, which adds and pairs a Rubylight PC from the code on its Devices page; Windows and Linux paste the PC's copied pairing link. [Pairing](pairing.md)
- **Per-device permissions** the host sets, checked before every protected action, with the reason when one is missing.
- **Settings per PC and per game**, profiles picked by network (mobile data, home Wi-Fi, other Wi-Fi) and a mobile data cap. [Settings](settings.md)
- **Remote Monitor** to watch a PC without taking its input (Rubylight, Vibepollo), and **Input-only** to drive it without video, on any PC. [Input-only](controls.md#input-only-on-any-pc)
- **PIN lock** for the app, every stream or the PCs you pick. Android.
- **Open the host's web UI** from a PC's settings. Android.

## The picture

- **H.264, HEVC and AV1** hardware decoding, with low-latency settings per chip for Snapdragon, MediaTek, Tegra, Exynos, Google Tensor and Amlogic decoders on Android ([decoder flags](decoder-flags.md)), D3D11VA on Windows and VAAPI on Linux.
- **HDR10, HDR10+, HLG and Dolby Vision**, each where the screen and decoder show it, with HDR10+ scene statistics and Dolby Vision 10.1 on AV1 or 8.1 and 8.4 on HEVC from a Rubylight 3.0 PC. Dolby Vision, HDR10+ and HLG on Android; HDR10 on Windows and Linux. [HDR formats](video-and-latency.md#hdr-formats)
- **10-bit SDR** against banding and **full-colour 4:4:4** for sharp coloured text. [HDR and color](video-and-latency.md#hdr-and-color)
- **PyroWave**, the low-latency wavelet codec, decoded through Vulkan on Android and on the GPU on Windows and Linux, in 4:4:4 and HDR. On Snapdragon you can load a newer Adreno driver for it. Rubylight. [PyroWave decode](pyrowave-decode.md)
- **Upscaling** on this device with FSR 1 or Snapdragon GSR.
- **Render through Vulkan** on Android, so frame-generation overlays can hook the video, with HDR10, HDR10+ and HLG.
- **A display of its own on the PC**: a virtual display at this device's exact resolution and refresh, fractional rates included, and a choice of what the PC's monitors do during the stream. [PC display mode](controls.md#pc-display-mode-and-administration)
- **Frames timed to your display**: adaptive refresh and VRR follow the game, and phase-lock timing tells a Rubylight PC how long frames wait for your screen.
- **Beam racing** on Windows and Linux, a setting for wired links: a PyroWave picture is decoded in slices while the rest is still arriving. [Beam racing](beam-racing.md)
- **Portrait and rotation**, picture-in-picture and video on an external display with this device as the touchpad. Android. [Orientation](video-and-latency.md#orientation)

## Stream quality and the network

- **Device controls bitrate** follows loss, jitter and your decoder's headroom up to 2000 Mbps, and **automatic quality** changes resolution and frame rate mid-stream for smooth motion, sharp detail or battery life. [Recovery and automatic quality](recovery-and-quality.md)
- **Change bitrate / frame rate** during a stream, **Loss-based bitrate** (the desktop's Client-hint bitrate) and a choice of **error correction**, from leaving it to the PC to fixed shares. Rubylight. [Bitrate and error correction](video-and-latency.md#bitrate-frame-rate-and-error-correction-with-a-rubylight-pc)
- **Recover interrupted streams**, on by default: up to five tries within a minute bring back the same running game, never a different one. [Recovering a connection](recovery-and-quality.md#recovering-a-connection)
- **Ride out network drops** of 20, 30 or 60 seconds. [Network drops](video-and-latency.md#network-drops)
- **A stream made for your link**: Nightjar tells a Rubylight PC that you are on Wi-Fi or how fast your wired link is, and reports lost video once a second.
- **Play away from home** with a guide that sets up a private Tailscale connection and tests your PC from where you are. Android. [Remote play](remote-setup.md)

## Sound

- **Stereo, 5.1 and 7.1**, with low-latency AAudio on Android, and lost packets filled in instead of gaps.
- **Audio passthrough** to a receiver, soundbar or TV: PCM, Dolby Digital or Dolby Digital Plus on Android, PCM on Windows and Linux. Rubylight. [Sound](video-and-latency.md#sound)
- **Microphone** to the PC, with echo cancellation, noise suppression, automatic gain, a gain slider and a one-tap mute button on Android. [Microphone](controls.md#microphone)

## Controls

- **Controllers**: up to 16 players, button, stick, trigger and D-pad mapping per model, and extra buttons that press the touchpad click, Share / Capture or paddles 1 to 4 on the PC. [Gamepads](controls.md#gamepads)
- **DualSense over USB** with adaptive triggers, the lightbar, Edge paddles, waveform haptics and its speaker, headset and microphone, and built-in **Xbox and Switch Pro USB drivers**. Android. [DualSense and USB](controls.md#dualsense-and-usb)
- **On-screen controls** in two styles, Classic and Modern, with an in-stream editor, plus **per-game touch controls** with keys, buttons, sticks, triggers and a D-pad, shared as files. Android. [On-screen controls](controls.md#on-screen-controls)
- **Touch that feels like a laptop**: trackpad with acceleration and glide, direct mouse, multi-touch, and a cursor drawn on this device that takes the PC's own cursor shape from a Rubylight PC. Android. [Touch modes](controls.md#touch-modes)
- **A full PC keyboard** on screen with Esc, F1 to F12, arrows and sticky modifiers, driven by touch, a controller or a TV remote. Android. [PC keyboard](controls.md#pc-keyboard)
- **The keyboard follows text fields**: it opens for a text field in the stream, fits it and closes after; Nightjar also tells the PC your keyboard layout. Rubylight. [Keyboard follows text fields](controls.md#keyboard-follows-text-fields)
- **Mouse, keyboard, pen and native touch**, with saved key chords and three- and four-finger shortcuts.

## Sharing with your PC

- **Clipboard sync** of copied text both ways, and images by hand from the stream menu. [Clipboard](sharing.md#clipboard)
- **Files** to and from the PC's file transfer folder, also from Android's share sheet. [Files](sharing.md#files)
- **Shared folders** to browse, download from and upload to. [Shared folders](sharing.md#shared-folders)
- **USB devices** such as a drawing tablet or a security key, handed to the PC. [USB devices](sharing.md#usb-devices)

All four need a Rubylight PC that offers them and allows them for this device.

## See what's happening

- **Performance overlay** in any corner: Compact, Statistics or Debug, with your PC's CPU and GPU usage, temperatures, power and clocks from a Rubylight PC. [Performance overlay](video-and-latency.md#performance-overlay)
- **Why did my stream stutter?** names the likely cause and can run a reversible bitrate test. [Stream health](stream-health.md)
- **Measure input latency** during a stream, a **decoder benchmark** that needs no PC, and frame timings as CSV. [Device results](devices.md)
- **Connection test** before you raise the bitrate, and **problem reports** that stay on this device until you share them (Android). [Troubleshooting](TROUBLESHOOTING.md)

## Look and feel

- **Built for a controller and a TV remote first**, with touch, mouse and keyboard everywhere too. [Menu navigation](controls.md#menu-navigation)
- **Status you can read**: green for live, yellow for a warning and red for poor, always with a word and a shape, never colour alone.
- **Settings that start simple**: each section opens on the settings most people change, with the rest behind "More … settings". Search across every setting, presets, and help on every row on Android. [Settings](settings.md)
- **Interface size, high contrast and reduced motion**, and System, Dark and OLED black themes. [Accessibility](console.md#accessibility-on-android)
- **15 languages** on Android; English and German on Windows and Linux. [Translations](TRANSLATIONS.md)
- **Help that works offline**, and an update check that tells you when a new release is out (Android, outside Google Play).
