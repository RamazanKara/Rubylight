# Features

[Documentation](index.md) · [Android and desktop](CLIENT-PARITY.md) · [Host feature support](PARITY-MATRIX.md)

Nightjar streams a PC to Android phones, handhelds, tablets, Android TV and Fire TV, and to Windows, Linux and macOS. All clients share one streaming engine, written in Rust; the Android app also keeps the classic moonlight-common-c engine as a setting.

This page lists every feature with a link to its guide. Features available on only one client are marked, and [Android and desktop](CLIENT-PARITY.md) compares the clients row by row. Features that need support from the PC's host name the requirement; [host feature support](PARITY-MATRIX.md) lists which hosts provide each capability. Every Moonlight-compatible host gets the stream, the controls and the diagnostics. Setting names and defaults are in the [settings reference](settings.md).

## Library

- **Home.** Favourites, **Continue playing** and the games of every paired PC in one library. Cover art is cached on the device, so Home works while a PC sleeps, and an offline PC offers **Wake up**. Store shelves, Steam collections and Playnite categories, and the richer game pages need a host with the native library API (Rubylight 3.0); Home also shows the app lists of other paired hosts, such as Sunshine, Apollo or Vibepollo. [Home](console.md#home)
- **View.** Grid, rows or a list with each game's store and last-played time; grouped by store, by type, by collection or not at all; small, medium or large cards. [View](console.md#view)
- **Store shelves on a TV.** Rows for Steam, Epic Games, GOG, EA app, Ubisoft Connect, Battle.net and Xbox under a large hero, plus Recently played, Other apps, and games the PC found but that were never added. Android. [On a TV](console.md#on-a-tv)
- **All games.** One poster grid, filtered by store, by installed, by hidden or by not yet added.
- **Hide a game** on this device only; All games brings it back. [Home](console.md#home)
- **Game pages.** The game's store, whether it is running, installed or HDR-capable, the PC, and the exact settings the stream will use, followed by **Play** or **Resume**. [A game's page](console.md#a-games-page)
- **Wake and play.** Selecting a sleeping PC's game wakes the PC and starts the game.
- **Confirmation before a game closes.** Starting a game while another is running closes the running one only after confirmation, and the prompt names both games.
- **Continue here.** Moves a game another device is playing to this one in one press, or **Stream here too** beside it. Needs a host that offers it (Rubylight). [A game's page](console.md#a-games-page)
- **Join co-op.** Take a free player slot in a PC's Nucleus Co-op split-screen session, or return to your own seat. Needs a host with co-op sessions (Rubylight 3.0). [Join co-op](console.md#join-co-op)
- **Launcher integration.** Games start from the Android TV home screen, Beacon, ES-DE, Daijisho, Pegasus and home-screen shortcuts on Android ([launcher integrations](FRONTENDS.md)), and from `nightjar --host … --app …` on Windows and Linux.

## Pairing and PCs

- **Discovery and manual entry.** PCs are found on the local network or added by name, IPv4 or IPv6 address and port. Wake-on-LAN is sent on every network interface, also to the MAC addresses of other adapters added under **Other wake addresses**. A PC can be pinned to one address. [Wake a sleeping PC](pairing.md#wake-a-sleeping-pc)
- **Pairing.** PIN, one-time PIN, and QR pairing: **Scan QR code** on Android, or a pasted pairing link on Windows and Linux. A pairing link opened from any other app opens Nightjar, which asks before it pairs. QR pairing needs a host that offers it (Rubylight 3.0). [Pairing](pairing.md)
- **Approve from a paired device.** A new TV or console shows a PIN, and a paired device enters it, with a controller too. Needs a host with pairing approval (Rubylight 3.7). [Approve from a paired device](pairing.md#approve-from-a-paired-device)
- **Per-device permissions.** Permissions set on the host are checked before every protected action, and a refused action names the missing permission.
- **The PC's power and updates.** Sleep, restart or shut down the PC, and update its host, from the PC's options, on hosts that allow it for this device. Windows and Linux can also shut down, restart or put to sleep the computer Nightjar runs on. [The PC's power](console.md#the-pcs-power-and-host-updates)
- **Settings per PC and per game.** Profiles selected by network (mobile data, home Wi-Fi, other Wi-Fi) and a mobile data cap. [Settings](settings.md)
- **Remote Monitor and Input-only.** Watch a PC without sending input (on hosts that list the role, such as Rubylight and Vibepollo), or control it without video on any PC. [Input-only](controls.md#input-only-on-any-pc)
- **Microphone only.** Send this device's microphone to the PC with no picture. Needs Rubylight 3.4 or newer. [Microphone only](console.md#microphone-only)
- **PIN lock.** For the app, for every stream, or for streams on selected PCs.
- **Host web UI.** Opens the host's web UI from the PC's details.

## Picture

- **H.264, HEVC and AV1** on the hardware decoder: MediaCodec with per-chip low-latency settings for Snapdragon, MediaTek, Tegra, Exynos, Google Tensor and Amlogic on Android ([decoder flags](decoder-flags.md)), D3D11VA on Windows, VA-API, Vulkan video, V4L2 M2M or NVDEC on Linux, and VideoToolbox on macOS. Nightjar checks what the decoder handles before a stream and offers the PC only that. [Hardware decoding](video-and-latency.md#hardware-decoding-on-windows-and-linux)
- **HDR.** HDR10 with any HDR-capable host. HDR10+ (with scene statistics), HLG and Dolby Vision (profile 10.1 on AV1, 8.1 and 8.4 on HEVC) need a host that sends them (Rubylight 3.0). Android shows all four; Windows HDR10, HLG and Dolby Vision; Linux HDR10 and HLG where the compositor offers HDR output. [HDR formats](video-and-latency.md#hdr-formats)
- **Less color banding and sharper colored text.** **Less color banding** streams SDR in 10 bits to reduce banding. **Sharper colored text** streams 4:4:4 to keep colored text sharp: decoded in hardware where the decoder supports it (HEVC 4:4:4 on Windows, in 8-bit and 10-bit), else packed as full chroma into a stream that ordinary hardware decoders handle. Each is offered only where this device's decoder handles it; otherwise the stream uses 8-bit or 4:2:0 and states the reason. [HDR and color](video-and-latency.md#hdr-and-color)
- **PyroWave.** A low-latency wavelet codec, decoded through Vulkan on Android and on the GPU on Windows and Linux, including 4:4:4 and HDR. On Snapdragon devices a newer Adreno driver can be loaded for it. Needs a host that offers PyroWave. [PyroWave GPU decoding](pyrowave-decode.md)
- **Upscaling.** FSR 1 or Snapdragon GSR on the device; on Windows also the GPU's video upscaler (**AI enhancement**) where the driver provides it. [Upscaling](video-and-latency.md#upscaling)
- **Frame Warp.** A mouse or right-stick camera turn shows on the next refresh of this screen: each picture is shifted by the turn the PC has not drawn yet, and the game's HUD stays in place. Off by default; needs a host that offers it (Rubylight), which learns each game as you play. [Frame Warp](video-and-latency.md#frame-warp)
- **Render through Vulkan.** On Android, the stream can be drawn through Vulkan, so frame-generation overlays can hook the video, with HDR10, HDR10+ and HLG.
- **Virtual display.** The PC can create a display at this device's exact resolution and refresh rate, including fractional rates, and you choose what the PC's monitors do during the stream. Needs a host with virtual displays (Apollo, Vibepollo, Rubylight). [Display arrangement](controls.md#display-arrangement-and-administration)
- **Frame timing.** **Variable refresh rate** (Android 16 screens with adaptive refresh rate, and VRR displays on the desktop) follows the game's frame rate. With hosts that support phase-lock timing (Rubylight 3.0), Nightjar reports how long frames wait for the next display refresh, so the host can time frames to land just before it. On Android, **Take back display queue delay** drops a frame when the display holds frames longer than it needs to. [Refresh rate and VRR](video-and-latency.md#refresh-rate-and-vrr)
- **Beam racing.** On Windows and Linux, for wired links: a PyroWave picture is decoded in slices while the rest is still arriving. [Beam racing](beam-racing.md)
- **Dual screen mode.** A device with a second screen streams the same game on both, each at its own resolution, with sound, controllers and the microphone on the first. Needs a host that supports it (Rubylight 3.11). [Settings](settings.md#streaming)
- **Orientation and displays.** Portrait and rotation, picture-in-picture, video on an external display with this device as the touchpad, and a decoder readied for folding on foldables. Android. [Orientation](video-and-latency.md#orientation)

## Stream quality and network

- **Automatic bitrate.** The bitrate follows the resolution, frame rate and codec until you choose one. **Lower bitrate when the network is busy** then adjusts it during a stream to loss, round-trip variation and the decoder's headroom, up to 2000 Mbps. **Automatic quality** changes resolution and frame rate during the stream, favoring smooth motion, sharp detail or battery life. [Recovery and automatic quality](recovery-and-quality.md)
- **Changes during a stream.** **Change bitrate / frame rate** and **Lower bitrate when frames are lost** need a host that accepts them (Rubylight 3.0). **Error correction** can be left to the PC, follow measured loss or use a fixed share, with Rubylight and Sunshine-based PCs that accept it. A Sunshine-based PC with its own automatic bitrate follows Nightjar's network reports. [Bitrate and error correction](video-and-latency.md#bitrate-frame-rate-and-error-correction-with-a-rubylight-pc)
- **Packet arrival reports.** With a Rubylight PC that has **Adapt to network congestion** on, Nightjar tells it when each video packet arrived, about 20 times a second in a few hundred bytes, so the PC lowers the bitrate as delay builds up on a busy network, before pictures are lost. It applies to a bitrate you choose; **Automatic bitrate** keeps its own control. [Bitrate and error correction](video-and-latency.md#bitrate-frame-rate-and-error-correction-with-a-rubylight-pc)
- **Recover interrupted streams.** On by default: up to five attempts within a minute return to the same running game, never a different one. [Recovering a connection](recovery-and-quality.md#recovering-a-connection)
- **Wait through network dropouts.** Holds the stream through 15, 20, 30 or 60 seconds of network silence. [Network drops](video-and-latency.md#network-drops)
- **Link reporting.** Nightjar tells a supporting host whether the device is on Wi-Fi or a wired link and how fast the link is, and reports lost video once a second.
- **Connection test and PyroWave frame pace.** A speed test (**Settings → Streaming → Connection test**) and, for PyroWave, a frame pace calibration (**Settings → Advanced → PyroWave frame pace**), also in a PC's own settings, with a host that offers them. [Connection test](TROUBLESHOOTING.md#connection-test)
- **Play away from home.** On Android, a guide sets up a private Tailscale connection and tests the PC from where you are; the desktop checks the paired PC with **Remote access → Test this PC**. [Play away from home](remote-setup.md)

## Sound

- **Stereo, 5.1, 7.1 and 7.1.4.** Low-latency AAudio on Android; lost packets are concealed rather than played as gaps. 7.1.4 needs a host that sends it (Rubylight 3.0) and a 12-channel output (Android 12L or newer on Android). **Mute audio** streams without sound on this device. [Sound](video-and-latency.md#sound)
- **Audio passthrough.** PCM, Dolby Digital or Dolby Digital Plus to a receiver, soundbar or TV on Android; PCM on Windows and Linux. Needs a host that sends it (Rubylight 3.0). [Sound](video-and-latency.md#sound)
- **Microphone.** Echo cancellation on Android and the desktop; noise suppression, automatic gain, a gain slider and a one-tap mute button on Android. [Microphone](controls.md#microphone)
- **Night mode and headphone surround** in the stream menu, when the PC's host offers them.

## Controls

- **Controllers.** Up to 16 players, with button, stick, trigger and D-pad mapping per controller model, and **Connect as** to pick the type the PC sees. Extra buttons can press the touchpad, Share / Capture or paddles 1 to 4 on the PC. Nightjar tells a Rubylight host which controller it is, so a Switch 2 Pro Controller arrives as one, with its C button (desktop), grip buttons, motion and rumble, when the PC has usbip-win2 and a Rubylight host that supports it. [Gamepads](controls.md#gamepads)
- **USB controller drivers.** DualSense over USB with adaptive triggers, the lightbar, Edge paddles, waveform haptics, and its speaker, headset jack and microphone; built-in drivers for Xbox and Switch Pro controllers. Android; the desktop uses SDL for the same controllers and has **USB DualSense audio and haptics**. [DualSense and USB](controls.md#dualsense-and-usb)
- **Stream menu.** The same tiles on Android and desktop, opened by Back, a hotkey, a controller chord or a held Guide; **Y**, **X**, **LB** and **RB** run the tile you pick, and your keyboard shortcuts, Game Bar and Record that are tiles too. [The stream menu](controls.md#the-stream-menu)
- **Stream hotkeys.** One set on Android and desktop, each changeable or off. [Stream hotkeys](controls.md#stream-hotkeys)
- **On-screen controls.** Classic and Modern styles with an in-stream editor, a WASD stick, a Touchpad button, a Steam Controller's trackpads, back buttons and grip touch, the option to be their own player, and per-game touch layouts with keys, buttons, sticks, triggers and a D-pad, shared as files. Android. [On-screen controls](controls.md#on-screen-controls)
- **Touchpad gestures on the PC.** A physical touchpad's fingers go to a PC that takes them, so the PC's own touchpad gestures work: a keyboard cover's touchpad on Android, a precision touchpad on Windows, the touchpad on Linux and the trackpad on macOS. Other PCs keep the touchpad as a mouse. [Mouse and keyboard](controls.md#mouse-and-keyboard)
- **Touch modes.** Trackpad with acceleration and glide, direct mouse, multi-touch, and a local cursor that takes the PC's own cursor shape from a host that sends it. Android. [Touch modes](controls.md#touch-modes)
- **PC keyboard.** A full on-screen keyboard with Esc, F1 to F12, arrows and sticky modifiers, usable by touch, controller or TV remote. Android. [PC keyboard](controls.md#pc-keyboard)
- **Keyboard follows text fields.** The device keyboard opens when a text field on the PC takes focus, keeps the field visible and closes afterwards; Nightjar also reports the keyboard layout to the PC. Needs a host that reports text fields (Rubylight 3.0). [Keyboard follows text fields](controls.md#keyboard-follows-text-fields)
- **System keys.** Alt+Tab, the Windows key and similar go to the PC, or only while the stream is in full screen. [Keyboard options](controls.md#keyboard-options)
- **Mouse, keyboard, pen and native touch.** Saved key chords and three- and four-finger shortcuts.

## Sharing

- **Clipboard sync.** Copied text in both directions, and images on request from the stream menu. [Clipboard](sharing.md#clipboard)
- **Files.** Transfers to and from the PC's file-transfer folder, including from Android's share sheet. [Files](sharing.md#files)
- **Shared folders.** Browse, download from and upload to folders the PC shares. [Shared folders](sharing.md#shared-folders)
- **USB devices.** Hand a device such as a drawing tablet or a security key to the PC. [USB devices](sharing.md#usb-devices)

Each needs a host that offers it (Rubylight 3.0) and allows it for this device.

## Diagnostics

- **Performance overlay.** Compact, Statistics or Debug, in any corner, with decoder drops counted apart from network loss, and the PC's CPU and GPU load, temperatures, power and clocks from a host that sends them. [Performance overlay](video-and-latency.md#performance-overlay)
- **Why did my stream stutter?** Names the likely cause and can run a reversible bitrate trial. [Stream health](stream-health.md)
- **Measurement tools.** An input latency test during a stream, a decoder benchmark that needs no PC, and frame timings as CSV. Android; the desktop exports statistics and frame timings. [Device results](devices.md)
- **A start that fails.** The connecting screen turns into a card with the reason, **Try again**, the stream's settings and **Test the network**, which asks Moonlight's port-test server whether a firewall blocks the streaming ports. [Troubleshooting](TROUBLESHOOTING.md)
- **Problem reports.** Redacted reports that stay on the device until you share them or send them to a PC. Android; the desktop writes a redacted log. [Troubleshooting](TROUBLESHOOTING.md)

## Interface

- **Controller and remote first.** Every screen works with a controller or a TV remote, and with touch, mouse and keyboard; L1 and R1 switch tabs. [Menu navigation](controls.md#menu-navigation)
- **Open straight into a game.** "When Nightjar opens" shows your PCs, continues your last game, or starts a game chosen from its menu, after a 3-second countdown you can cancel, and only on a cold start. A start that failed skips the next one, and a running game is never replaced without asking. Windows and Linux can start Nightjar at sign-in (on Linux the entry stays when a package is removed, and the desktop skips it once the program is gone), and on Android long-pressing the app icon offers "Continue" with your last game. [Settings](settings.md#app)
- **Readable status.** Green for good, yellow for a warning and red for poor, always with a word and a shape as well as color.
- **Layered settings.** Each section opens on the settings most people change, with the rest under "More … settings" on Android and in labelled groups on the desktop. Search across all settings, presets, and help on every row. [Settings](settings.md)
- **Accessibility.** Text and button size, high contrast, reduced motion and screen reader support, and System, Dark and OLED black themes on Android. [Accessibility](console.md#accessibility-on-android)
- **Languages.** The same 15 on Android, Windows and Linux. [Translations](TRANSLATIONS.md)
- **Without a desktop.** On Linux, Nightjar runs on a text console or kiosk box with no Wayland or X11 session: it draws straight to the display through DRM/KMS, reads input from evdev, and puts the stream on a display plane with HDR10, VRR and content type Game. The whole app works there, as does starting a stream from the command line. [Without a desktop](FRONTENDS.md#without-a-desktop-linux)
- **Steam Deck and Linux handhelds.** A Flatpak with every release, for x86-64 and ARM64; under gamescope it opens fullscreen without a border, is driven by a controller alone, with Start+Back for the stream menu, and shows HDR through gamescope. Linux. [Steam Deck](steam-deck.md)
- **Help and updates.** Built-in help that works offline. Update checks on Android (outside Google Play), on Windows and in the AppImage, with a one-click update that checks the download's size and SHA-256 before installing; a portable Windows copy gets a link to the release page.
