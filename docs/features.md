# Rubylight features

[Documentation](README.md) · [Getting started](getting-started.md) · [Configuration](configuration.md) · [Performance](performance.md)

Everything Rubylight does, grouped by what you will notice while streaming. For the numbers behind the speed, see [Performance](performance.md).

## Fast video on Radeon

- **Native AMF encoding** for H.264, HEVC and AV1, driven directly instead of through a generic wrapper, with defaults tuned per codec from VMAF measurements.
- **Adaptive AMF backlog.** The encoder learns the driver's required depth and times the next input to preserve conversion and network overlap. The [Radeon measurements](../rust/PERFORMANCE_WORK.md#october-13-adaptive-amf-backlog-candidate) show lower picture age under saturation; normal-load tails and broader compatibility remain unresolved.
- **Radeon compute.** Frame copies, colour conversion and letterboxing run on D3D12 compute queues beside the game's graphics work, so your game and your stream stop waiting on each other.
- **HDR from capture to screen.** Ten-bit BT.2020/PQ through HEVC and AV1, with HDR metadata in the keyframes, and ten-bit SDR where you want it.
- **PyroWave HDR 4:4:4.** A colour sample for every pixel, so coloured text and fine edges stay crisp, on [Nightjar](https://github.com/RamazanKara/nightjar) for Android or any other PyroWave-capable Moonlight client over a fast local network.
- **Loss recovery that skips the keyframe.** Lost frames are repaired with reference frame invalidation, and forward error correction covers short bursts of loss. When the host's own Wi-Fi briefly stops taking packets, the host sends a keyframe at once instead of waiting for the client to ask.
- **Experimental host bitrate adaptation.** Opt-in adaptation for conventional Moonlight streams uses observed feedback, accounts for FEC and audio, and yields to client bitrate control. [Policy and limits](configuration.md#host-bitrate-adaptation).
- **Optional adaptive packet pacing.** The existing fast route policy remains the default. Experimental adaptation starts conservatively and responds to reported loss without lowering picture bitrate, at the cost of slower startup and possible probe losses. A suitable fixed limit remains the simpler option for a known constrained path. [Pacing policy](configuration.md#capture-and-video).
- **Aspect ratio kept.** A source of another shape sits between black bars.
- **Antialiased GPU downscaling.** Streams below the desktop's resolution average the covered source pixels to reduce thin-line shimmer. Native resolution, enlargement and aligned 2× reduction retain their existing pixels; reduction above 8× retains the previous bounded filter.

Rubylight encodes on AMD GPUs only: a Radeon RX 6000 (RDNA 2) or newer. H.264 and HEVC work on all of them; AV1 is offered where the GPU's encoder has it.

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
- **Back grips.** Steam Deck L4/R4/L5/R5 and Xbox Elite paddles press the buttons you choose, under **Back grips**.
- **A real Steam Deck controller.** With usbip-win2 installed, a Steam Deck client shows up on the host as a Steam Deck with trackpads, gyro and back grips.
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

- **Pairing** with a PIN or a one-time PIN, with per-device permissions and an enable switch. Devices paired after the first start view-only until you grant more.
- **A rebuilt console** for overview, library, devices, settings, logs, maintenance and API tokens, with frame rate, bitrate and encoder timing together, and a stream card that explains what the host did and why.
- **A tray icon** with notifications for pairing requests, new versions and apps launching, pausing and stopping.
- **Logs and support.** Rotating logs, a live tail in the console, crash dumps and a support bundle.
- **Hardware readings for client overlays.** Opt-in CPU usage and available AMD GPU readings, collected on request during a paired client's video stream. Compatible clients draw their own overlay; see the [hardware-metrics protocol](hardware-metrics.md).
- **Private connection-path diagnosis.** An authenticated client can check whether its current Tailscale peer route is direct, relayed or unknown, without exposing other peers. [Protocol and limits](network-path.md).

## Install and update

- **One installer** that sets up the service, drivers, firewall rule and shortcuts, and upgrades in place. The installer bundles optional usbip-win2 for native Steam Deck controllers and DualSense USB audio and waveform haptics. It imports an existing Vibepollo or Apollo profile and keeps your settings, paired devices, apps and covers.
- **Updates notify you first.** Automatic installation is opt-in; the host downloads the exact installer from the official release, verifies its size and SHA-256 digest, waits for an idle minute and upgrades through the service. A failed copy or start restores the previous files.
- **A service that restarts the host** after a crash.
- **Separate instances** with independent profile folders and base ports. Copied portable packages can use an adjacent `config` folder from 2.2.6; older packages support explicit `--config-dir` paths. See [multiple instances](configuration.md#multiple-instances).

## Clients

Rubylight speaks the Moonlight protocol, so **Moonlight and every other Sunshine-compatible client works with it**: on Windows, macOS, Linux, Android, iOS, Apple TV, Android TV, game consoles and handhelds. H.264, HEVC and AV1 work in SDR and HDR wherever the client's device can decode them. PyroWave needs a PyroWave-capable client such as [Nightjar](https://github.com/RamazanKara/nightjar) on Android.

See [Getting started](getting-started.md#choose-your-stream-format) to pick a stream format.
