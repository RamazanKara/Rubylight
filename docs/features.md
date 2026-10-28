# Rubylight features

[Documentation](README.md) · [Getting started](getting-started.md) · [Configuration](configuration.md) · [Performance](performance.md)

Everything Rubylight does, grouped by what you will notice while streaming. Features marked experimental are off until you turn them on. For the numbers behind the speed, see [Performance](performance.md).

## Fast video on Radeon

- **Native AMF encoding** for H.264, HEVC and AV1, driven directly instead of through a generic wrapper, with defaults tuned per codec from VMAF measurements.
- **Adaptive AMF backlog.** The encoder learns the driver's required depth and times the next input to preserve conversion and network overlap. The [Radeon measurements](../rust/PERFORMANCE_WORK.md#october-13-adaptive-amf-backlog-candidate) show lower picture age under saturation; normal-load tails and broader compatibility remain unresolved.
- **Radeon compute.** Frame copies, colour conversion and letterboxing run on D3D12 compute queues beside the game's graphics work, so your game and your stream stop waiting on each other.
- **HDR from capture to screen.** Ten-bit BT.2020/PQ through HEVC and AV1, with HDR metadata in the keyframes, and ten-bit SDR where you want it.
- **PyroWave HDR 4:4:4.** A colour sample for every pixel, so coloured text and fine edges stay crisp, on [Nightjar](https://github.com/RamazanKara/nightjar) for Android or any other PyroWave-capable Moonlight client over a fast local network.
- **Link-rate adaptation.** When a device loses frames that error correction cannot repair, for example 150 Mbps asked of a 100 Mbps link, the host steps the bitrate down to what the link delivered, says so on the stream card, probes back up in small steps and returns to your rate after ten clean seconds. On a 50 Mbit/s link asked for 75 Mbps, under 2 % of frames were lost after the start-up (6.7 % before the 3.0 fix). A link that carries the stream sees no change, including an emulated 150 Mbit/s Wi-Fi link with 0.5 % loss. [Details](configuration.md#lower-bitrate-when-the-link-cannot-carry-it) · [Numbers](performance.md#30-results).
- **Stream presets.** Lowest latency, Balanced and Reference quality fill in the encoder settings you did not set, for the whole host, one device or one app. Lowest latency made the first keyframe 29 % smaller on HEVC and 38 to 40 % smaller on H.264, with the same median end to end time. [Details](configuration.md#stream-presets).
- **VRR-aware capture.** With variable refresh, the host waits for the game's next frame when it is close, so the picture reaches the encoder fresh: 0.25 ms old instead of 4.68 ms for a 120 Hz game in a 60 fps stream. [Numbers](performance.md#vrr-capture-age).
- **Slice delivery (experimental).** `video_slice_delivery = 4` hands H.264 and HEVC frames to the network slice by slice while the encoder works on the rest. It needs no client change and brought a 4K60 HEVC picture 1.2 to 1.5 ms sooner over loopback and Wi-Fi 6E to the Nightjar desktop client. Android phone: no measurable gain; the emulated Wi-Fi run missed the 1 ms gate. [Details](b2-slice-delivery.md) · [Numbers](../rust/PERFORMANCE.md#october-23-slice-delivery-b2-step-2f).
- **Beam-raced PyroWave (experimental).** A client that asks gets each PyroWave frame in display order, so Nightjar on the desktop can decode and present the top of the picture while the bottom is still arriving. The picture's middle row was on screen 1.4 ms sooner on loopback. Off unless the client turns it on. [Numbers](../rust/PERFORMANCE.md#october-24-beam-raced-pyrowave-gate-b10-step-4-opt-in-missed-its-default-on-gate).
- **A timing record for every frame.** Set `BUTTERPOLLO_TIMING_RECORD` and each video stream writes capture, convert, encode, packetize and send times on one clock, ready to join with a client's frame log. [Details](timing-record.md).
- **Loss recovery that skips the keyframe.** Lost frames are repaired with reference frame invalidation, and forward error correction covers short bursts of loss. When the host's own Wi-Fi briefly stops taking packets, the host sends a keyframe at once instead of waiting for the client to ask.
- **Experimental host bitrate adaptation.** Opt-in adaptation for conventional Moonlight streams uses observed feedback, accounts for FEC and audio, and yields to client bitrate control. [Policy and limits](configuration.md#host-bitrate-adaptation).
- **Optional adaptive packet pacing.** The existing fast route policy remains the default. Experimental adaptation starts conservatively and responds to reported loss without lowering picture bitrate, at the cost of slower startup and possible probe losses. A suitable fixed limit remains the simpler option for a known constrained path. [Pacing policy](configuration.md#capture-and-video).
- **Aspect ratio kept.** A source of another shape sits between black bars.
- **Antialiased GPU downscaling.** Streams below the desktop's resolution average the covered source pixels to reduce thin-line shimmer. Native resolution, enlargement and aligned 2× reduction retain their existing pixels; reduction above 8× retains the previous bounded filter.

Rubylight 3.0 encodes on AMD GPUs only: any Radeon with a working AMF video encoder. NVIDIA and Intel graphics stay on [Rubylight 2.6.x](https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0). H.264 and HEVC work on all of them; AV1 is offered where the GPU's encoder has it.

## Capture that keeps working

- **Frames straight from Rubylight's display driver.** On its virtual displays, the driver hands the host each frame Windows composes: no capture API in between, and on Radeon the frame is converted where it lies. Automatic selects this path, and **Settings → Video → Capture method → Direct driver push** selects it explicitly. A reconnect to a desktop where nothing moves gets its picture at once.
- **Windows Graphics Capture** is the default on physical displays and takes over on a virtual display when the driver cannot push frames, with Desktop Duplication taking over whenever WGC cannot open.
- **Locked screens and the secure desktop.** Rubylight streams the lock screen and the sign-in prompt, so you can unlock a PC from your couch, and returns to WGC when the normal desktop comes back. It streams from a locked VM with no monitor attached ([#6](https://github.com/RamazanKara/Rubylight/issues/6)).
- **Services, VMs and containers.** The Windows service captures through a signed-in-user helper, and the same host runs on headless machines, virtual machines and Kubernetes pods with GPUs.
- **Capture recovery.** A lost Desktop Duplication session is reopened without ending the stream.

## Displays

- **A virtual display per device**, shared or off, with exclusive, extended, primary and isolated layouts, HDR, and a permanent count.
- **Your layout back.** The golden layout is restored after a stream or a crash, with a hotkey for the times you want it sooner.
- **Per-device modes.** A device's display mode sets the resolution and refresh of its display, and the stream keeps the client's frame rate.
- **The client chooses the display.** A Rubylight client can ask, per PC, for the virtual display alone, a virtual display beside your monitors, or no virtual display at all, without changing the host settings ([configuration](configuration.md#display-choice-from-the-client)).
- **Two clients at once**, each on its own virtual display.
- **Background streaming.** A device can play on its own isolated virtual display while someone keeps using the PC: the game takes no focus but is told it has it, only the game's sound is streamed, and only the device's controllers reach it ([setup and limits](configuration.md#background-streaming)).
- **Resolution changes mid-stream.** A Rubylight client can switch the stream's size and frame rate without reconnecting, as a foldable does when it opens ([control messages](control-messages.md)).

## Input, audio and microphone

- **Keyboard, mouse, touch, pen and controllers.** Standard profiles use the signed VHF driver included in the installer.
- **The right pad for the client.** PlayStation-type clients get a DualSense with adaptive triggers, Nintendo-type clients a Switch Pro, and others an Xbox Series pad. Two touchpads from a Steam Deck sit side by side on one surface.
- **Steam Deck and DualSense advanced controls.** Steam Input handles gyro aiming and radial menus using forwarded motion and touchpads. DualSense USB support includes waveform haptics, adaptive triggers, speaker/headset audio and microphone input with a matching client such as [Nightjar](https://github.com/RamazanKara/nightjar). Controller identities survive stream reconnects. [Setup and limits](dualsense-haptics.md).
- **Haptics from game audio (Experimental).** A client's DualSense feels the game's bass and impacts when the game sends it no vibration, with strength, response and noise-gate settings that apply mid-stream. [How it works](dualsense-haptics.md#haptics-from-game-audio-experimental).
- **Back grips.** Steam Deck L4/R4/L5/R5 and Xbox Elite paddles press the buttons you choose, under **Back grips**.
- **A real Steam Deck controller.** With usbip-win2 installed, a Steam Deck client shows up on the host as a Steam Deck with trackpads, gyro and back grips.
- **Steam Controller input from clients (Experimental).** The host reads the Steam Controller attribute messages a client sends and turns Steam's haptic reports into rumble, as Vibepollo does; a motor whose off never arrives stops after at most 10 seconds. Neither reaches a game yet: that needs a driver with the Steam Controller profile, which the host does not ship.
- **Audio.** Endpoint matching by id, name, description or adapter, Steam Streaming Speakers, and surround Opus.
- **Game-only audio.** Optional capture of a verified launched game and its descendants on supported Windows versions, with explicit silence and warnings when identification fails. [Setup and limits](configuration.md#game-only-audio).
- **Microphone.** A client's microphone plays into Steam Streaming Microphone, encrypted. See [Microphone](microphone.md).

## Library and frame pacing

- **Close apps on disconnect.** An optional global default closes the app when its last streaming device leaves. Each app can inherit that default or explicitly turn it on or off. [Configuration](configuration.md#close-apps-on-disconnect).

- **Apps.** Commands, preparation and undo, detached commands, URLs and documents, working folder inference, and starting before sign-in.
- **Steam.** Sync on demand and every 30 seconds, with covers from Steam's cache or store. A Steam app's stream ends when the game's processes exit.
- **Playnite.** Library sync, launches through Playnite with the stream's environment, a "Playnite (Fullscreen)" app, and the game brought to the front.
- **RTSS frame limits**, game-provided frame generation, and **Lossless Scaling** profiles that Rubylight sets up and removes with the stream.
- **Per-app and per-device overrides** for stream, input, display and encoder settings.
- **VRR.** A 1000 Hz VRR mode for clients that ask for it.

## Pairing and the web console

- **Pairing** with a PIN or a one-time PIN, with per-device permissions and an enable switch. When you pair from the console you choose what the device can do: Play games (the default), Watch only or Everything; the first device gets everything. WebRTC devices, and pairings made through the API without a choice, start view-only until you grant more. **Host control** is a separate per-device switch, off until you turn it on ([details](configuration.md#host-control-for-a-device)).
- **A console for everyday play.** Home shows setup, readiness and the current stream, with your game shelf below. Games & apps, Devices and Settings cover the library, pairing and stream options; Help & diagnostics holds logs, display recovery and updates. A stream card shows frame rate, bitrate and encoder timing together and explains what the host did and why.
- **A tray icon** with notifications for pairing requests, new versions and apps launching, pausing and stopping.
- **Logs and support.** Rotating logs, a live tail in the console, crash dumps and a support bundle.
- **Hardware readings for client overlays.** Opt-in CPU usage and available AMD GPU readings, collected on request during a paired client's video stream. Compatible clients draw their own overlay; see the [hardware-metrics protocol](hardware-metrics.md).
- **Private connection-path diagnosis.** An authenticated client can check whether its current Tailscale peer route is direct, relayed or unknown, without exposing other peers. [Protocol and limits](network-path.md).

## Control from the couch (3.0)

- **Native API v1.** A paired client on the streaming HTTPS port can read the host's capabilities, your game library with covers, and the session state. Stock Moonlight and Artemis are unaffected. [Contract](native-api.md).
- **A console on your phone.** [Nightjar](https://github.com/RamazanKara/nightjar) 0.8.0 and newer open on Home when the host offers the API: favourites, what is running, your library across every paired PC with covers, and a page for each game with Play or Resume. A session menu closes the game, ends the session or changes Radeon settings without leaving the stream.
- **Session actions.** Disconnect leaves the game running, End session stops the stream and keeps the game, Close game also quits it. [Details](configuration.md#session-actions).
- **Host control.** A per-device permission that lets a device close games and change Radeon settings. Off until you turn it on in the console. [Details](configuration.md#host-control-for-a-device).
- **Sleep PC (Experimental).** A per-device permission that lets a device put the PC to sleep while nothing is running, with the same requests as Vibepollo. Off until you turn it on in the console; a grant imported from Vibepollo is kept. It needs a PC with S1, S2 or S3 sleep; on a PC with only Modern Standby the host offers no Sleep PC and the console says why.
- **File transfer (Experimental).** Devices you allow send files to the PC and fetch them back through the native API, into the signed-in user's Downloads\Rubylight or a folder you choose, paced while a stream runs. Off until you turn it on in the console, and each device needs Upload files or Download files. [Details](configuration.md#file-transfer-experimental).
- **Stream in the console (Experimental).** With the WebRTC server on, the console's Stream page plays a game in the browser tab, signed in with the console session instead of pairing: H.264 1080p60, stereo, keyboard and mouse. [Details](webrtc.md#stream-in-the-console-experimental).
- **Radeon settings with restore.** A device with host control changes Radeon settings during a stream, such as Anti-Lag and Radeon Chill; every change is journaled and put back when the session ends. [Details](configuration.md#radeon-settings-with-restore).
- **A Radeon profile for each game.** In the app editor, **Radeon** picks Competitive (Anti-Lag on), Smoothness (the game's frame rate capped at the stream's), Quality (anisotropic filtering and the driver's anti-aliasing enhancement) or a list of your own. The host writes the settings when the game starts and puts the originals back when the session ends, also after a crash. [Details](configuration.md#radeon-settings-per-game).
- **Microsoft Store and Xbox PC games** appear in **Add games** next to Steam, Epic Games, Battle.net, EA app, Ubisoft Connect and GOG, and start the way the Start menu starts them.
- **Launcher hints.** If a Steam, Epic Games, EA app or Ubisoft Connect window is waiting for an update or a sign in, the session says so and a client can show it. The host only reads window titles; it never clicks or types into a launcher.
- **Rust engine and an on-screen keyboard in Nightjar.** The Rust streaming engine is Nightjar's default on Android, and a full PC keyboard works with touch, a controller or a TV remote. [Nightjar docs](https://github.com/RamazanKara/nightjar/blob/main/docs/index.md).

## Install and update

- **One installer** that sets up the service, drivers, firewall rule and shortcuts, and upgrades in place. The installer bundles optional usbip-win2 for native Steam Deck controllers and DualSense USB audio and waveform haptics. It imports an existing Vibepollo or Apollo profile and keeps your settings, paired devices, apps and covers.
- **Updates notify you first.** Automatic installation is opt-in; the host downloads the exact installer from the official release, verifies its size and SHA-256 digest, waits for an idle minute and upgrades through the service. A failed copy or start restores the previous files.
- **A service that restarts the host** after a crash.
- **Separate instances** with independent profile folders and base ports. Copied portable packages can use an adjacent `config` folder from 2.2.6; older packages support explicit `--config-dir` paths. See [multiple instances](configuration.md#multiple-instances).

## Clients

Rubylight speaks the Moonlight protocol, so **Moonlight and every other Sunshine-compatible client works with it**: on Windows, macOS, Linux, Android, iOS, Apple TV, Android TV, game consoles and handhelds. H.264, HEVC and AV1 work in SDR and HDR wherever the client's device can decode them. PyroWave needs a PyroWave-capable client such as [Nightjar](https://github.com/RamazanKara/nightjar) on Android. Nightjar 0.8.0 and newer also use the native API above, and The [Desktop client for Windows and Linux (Experimental)](https://github.com/RamazanKara/nightjar/blob/main/protocol/desktop/README.md) is on the [releases page](https://github.com/RamazanKara/nightjar/releases).

See [Getting started](getting-started.md#choose-your-stream-format) to pick a stream format.
