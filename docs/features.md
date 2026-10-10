# Rubylight features

[Documentation](README.md) · [Getting started](getting-started.md) · [Configuration](configuration.md) · [Performance](performance.md)

Everything Rubylight does, grouped by what you will notice while streaming. For the numbers behind the speed, see [Performance](performance.md).

## Fast video on Radeon

- **Native AMF encoding** for H.264, HEVC and AV1, driven directly instead of through a generic wrapper, with defaults tuned per codec from VMAF measurements.
- **Radeon compute.** Frame copies, colour conversion and letterboxing run on D3D12 compute queues beside the game's graphics work, so your game and your stream stop waiting on each other.
- **HDR from capture to screen.** Ten-bit BT.2020/PQ through HEVC and AV1, with HDR metadata in the keyframes, and ten-bit SDR where you want it.
- **PyroWave HDR 4:4:4.** A colour sample for every pixel, so coloured text and fine edges stay crisp, on [Rubylight Android](https://github.com/RamazanKara/rubylight-android) or any PyroWave-capable Moonlight client over a fast local network.
- **Loss recovery that skips the keyframe.** Lost frames are repaired with reference frame invalidation, and forward error correction covers short bursts of loss. When the host's own Wi-Fi briefly stops taking packets, the host sends a keyframe at once instead of waiting for the client to ask.
- **Aspect ratio kept.** A source of another shape sits between black bars, on the GPU and in the software encoders.

NVIDIA NVENC, Intel Quick Sync and software encoders are included for other hardware, but Rubylight's tuning is for Radeon.

## Capture that keeps working

- **Windows Graphics Capture** is the default on physical and virtual displays, with Desktop Duplication taking over whenever WGC cannot open.
- **Locked screens and the secure desktop.** Rubylight streams the lock screen and the sign-in prompt, so you can unlock a PC from your couch, and returns to WGC when the normal desktop comes back. It streams from a locked VM with no monitor attached ([#6](https://github.com/RamazanKara/Rubylight/issues/6)).
- **Services, VMs and containers.** The Windows service captures through a signed-in-user helper, and the same host runs on headless machines, virtual machines and Kubernetes pods with GPUs.
- **Capture recovery.** A lost Desktop Duplication session is reopened without ending the stream.

## Displays

- **A virtual display per device**, shared or off, with exclusive, extended, primary and isolated layouts, HDR, and a permanent count.
- **Your layout back.** The golden layout is restored after a stream or a crash, with a hotkey for the times you want it sooner.
- **Per-device modes.** A device's display mode sets the resolution and refresh of its display, and the stream keeps the client's frame rate.
- **Two clients at once**, each on its own virtual display.

## Input, audio and microphone

- **Keyboard, mouse, touch, pen and controllers.** Every pad comes from one signed VHF driver that the installer brings.
- **The right pad for the client.** PlayStation-type clients get a DualSense with adaptive triggers, Nintendo-type clients a Switch Pro, and others an Xbox Series pad. Two touchpads from a Steam Deck sit side by side on one surface.
- **Back grips.** Steam Deck L4/R4/L5/R5 and Xbox Elite paddles press the buttons you choose, under **Back grips**.
- **A real Steam Deck controller.** With usbip-win2 installed, a Steam Deck client shows up on the host as a Steam Deck with trackpads, gyro and back grips.
- **Audio.** Endpoint matching by id, name, description or adapter, Steam Streaming Speakers, and surround Opus.
- **Microphone.** A client's microphone plays into Steam Streaming Microphone, encrypted. See [Microphone](microphone.md).

## Library and frame pacing

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

## Install and update

- **One installer** that sets up the service, drivers, firewall rule and shortcuts, and upgrades in place. It imports an existing Vibepollo or Apollo profile and keeps your settings, paired devices, apps and covers.
- **Updates notify you first.** Automatic installation is opt-in; the host downloads the exact installer from the official release, verifies its size and SHA-256 digest, waits for an idle minute and upgrades through the service. A failed copy or start restores the previous files.
- **A service that restarts the host** after a crash.

## Clients

Rubylight speaks the Moonlight protocol, so **Moonlight and every other Sunshine-compatible client works with it**: on Windows, macOS, Linux, Android, iOS, Apple TV, Android TV, game consoles and handhelds. H.264, HEVC and AV1 work in SDR and HDR wherever the client's device can decode them. PyroWave needs a PyroWave-capable client such as [Rubylight Android](https://github.com/RamazanKara/rubylight-android).

See [Getting started](getting-started.md#choose-your-stream-format) to pick a stream format.
