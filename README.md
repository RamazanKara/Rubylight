<p align="center">
  <img src="docs/media/banner.svg" width="900" alt="Rubylight — Written in Rust. Built for Radeon. Made for Moonlight.">
</p>

<p align="center">
  <strong>Stream your Radeon PC to Moonlight.</strong><br>
  Play your PC games on a laptop, TV, phone, Steam Deck or Xbox, with HDR at up to 240 fps.<br>
  Free, open source, written in Rust.
</p>

<div align="center">

**[Download 3.0.0-alpha.1](https://github.com/RamazanKara/Rubylight/releases/tag/v3.0.0-alpha.1)** · **[Get started](docs/getting-started.md)** · [Website](https://ramazankara.github.io/Rubylight/) · [Docs](docs/README.md) · [Nightjar for Android](https://github.com/RamazanKara/nightjar)

<sub>Windows x64 · GPL-3.0 · Formerly Butterpollo</sub>

</div>

<p align="center">
  <a href="docs/media/demo.mp4"><img src="docs/media/demo.gif" width="800" alt="Meet Rubylight: follow a frame from your game to Moonlight, see measured Radeon performance, and get started."></a>
</p>

<p align="center"><a href="docs/media/demo.mp4">Watch the 60-second demo →</a></p>

## What it is

Rubylight is a Windows game-streaming host. It runs on your gaming PC and streams to any Moonlight client. It was written for AMD Radeon first: AMF drives the encoder directly, and frame copies and colour conversion run on D3D12 compute queues beside your game.

**3.0 is AMD-only.** It needs a Radeon GPU with a working AMF video encoder, on Windows 10 or 11, whatever the GPU's generation. Setup and the host check for one and say what to do if it is missing. NVIDIA and Intel graphics stay on [Rubylight 2.6.x](https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0).

**3.0.0-alpha.1 is an alpha.** Expect rough edges, and keep 2.6.0 around if you depend on it. The [roadmap](docs/rubylight-3.0-plan.md) lists what is done and what is next, and the [release notes](rust/RELEASE_NOTES.md) list every change.

## Install. Pair. Play.

1. **Install.** Run **`rubylight-setup-3.0.0-alpha.1.exe`** from the [release](https://github.com/RamazanKara/Rubylight/releases/tag/v3.0.0-alpha.1). It sets up the service, the virtual display driver and controller support. No test mode, no restart. A ZIP is there too, for trying the host without installing.
2. **Sign in.** Open **`https://localhost:47990`** on the PC and create your account.
3. **Pair and play.** Open Moonlight on your device, select your PC and enter its PIN on the console's **Devices** page. Once the device appears as paired, choose **Desktop** or a game in Moonlight to start streaming.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/media/console-dark.png">
    <img src="docs/media/console-light.png" width="800" alt="Rubylight's Home page, showing host readiness, paired devices and a compact game library.">
  </picture>
</p>

**Coming from Apollo or Vibepollo?** Setup imports your settings, paired devices and app library, so your clients keep working. Updates keep them too, and you choose whether they install automatically.

[Full setup guide →](docs/getting-started.md)

## What you get

| Feature | What it does |
| --- | --- |
| **Native AMD encoding** | AMF encodes H.264, HEVC and AV1, with per-codec defaults tuned by measurement. A lost frame is repaired with a small recovery frame instead of a full keyframe. |
| **Radeon compute pipeline** | Copies, colour conversion and scaling run on D3D12 compute queues, so the game and the stream wait on each other less. [How it works](docs/architecture.md) |
| **A screen for every device** | Rubylight's own virtual display driver gives each client its resolution and refresh rate and hands every frame to the encoder directly. Your monitors come back when you disconnect. Physical displays stay supported, with Windows Graphics Capture or Desktop Duplication. |
| **HDR, and PyroWave 4:4:4** | 10-bit HDR over HEVC and AV1. PyroWave adds full-colour HDR 4:4:4 on a fast local network, paced to each client's link. |
| **Console and native API** | Home shows whether the PC is ready. Devices, Games & apps and Settings cover pairing, your library and stream options. The [native API](docs/native-api.md) lets Nightjar read your library and session. |
| **Host control, per device** | In **Devices**, open a device and turn on **Host control** to let it close games and change Radeon settings. It is off for every device until you turn it on. [More](docs/configuration.md#host-control-for-a-device) |
| **Session actions** | Disconnect leaves the game running, End session stops the stream and keeps the game, Close game also quits it. [More](docs/configuration.md#session-actions) |
| **Radeon settings with restore** | A device with host control can change Radeon settings during a stream. Each change is journaled and put back when the session ends. Unreleased, after alpha.1. [More](docs/configuration.md#radeon-settings-with-restore) |

Also built in: Steam and Playnite library sync, RTSS frame limits, Lossless Scaling, VRR, DualSense haptics and adaptive triggers, a real Steam Deck controller, touch and pen, surround sound, microphone passthrough, streaming from the lock screen, and headless or VM hosts.

[All features →](docs/features.md)

## Your games, ready to stream

Open **Games & apps → Add games** to scan installed games from **Steam, Epic Games, Battle.net, EA app, Ubisoft Connect and GOG**. Search or filter by launcher, select the games you want and add them to your library. **Scan again** refreshes the list; games already in the library are marked **Already added**. Use **Add manually** for another program or a Desktop entry, and choose your own cover if you prefer.

Existing **Steam and Playnite synchronization** stays available through source management. Imports keep games from different stores separate and preserve your customizations when scanned again.

**Home** shows setup checks, readiness and the current stream, with a game shelf below. **Devices** handles pairing and access permissions. **Settings** groups everyday controls into General, Streaming, Display & HDR, Audio & controls, Games & automation, and Network & access; search also finds advanced controls. Logs and recovery tools live in **Help & diagnostics**.

## Runs on your hardware

**GPUs.** Any AMD Radeon with a working AMF encoder, on Windows 10 or 11. Our measurements are on an RX 7900 XT. Users report daily use on RDNA 2, 3 and 4: RX 6800 XT, 6900 XT, 7900 XT, 7900 XTX, 9070 and 9070 XT, plus Radeon 890M handhelds. See [hardware matrix](docs/hardware-matrix.md) for the Radeon settings probed on real hardware.

**Clients.** Rubylight speaks the Moonlight protocol, so Moonlight, Artemis and other Sunshine-compatible clients work unchanged.

| Client | Highlights |
| --- | --- |
| **Moonlight PC** (Windows, macOS, Linux, Steam Deck) | H.264, HEVC and AV1, SDR and HDR, up to 240 fps |
| **Moonlight and Artemis on Android** | Native resolution at 120 Hz with HDR, on a virtual display that matches the phone |
| **[Nightjar](https://github.com/RamazanKara/nightjar)** for Android | PyroWave HDR 4:4:4 and DualSense audio and haptics. Nightjar 0.8.0 and newer also use the native API: a console Home, your library and a Radeon panel during the stream. |
| **Moonlight for Xbox** | 4K 120 Hz HDR on your TV |
| **[Nonary's Moonlight](https://github.com/Nonary/moonlight-qt)** | PyroWave and 1000 Hz VRR on a PC |
| **iOS, Apple TV, Switch** | Everything the device can decode |

[Pick the right client and stream format →](docs/getting-started.md#choose-your-stream-format)

## Measured

Every number here comes from a recorded run on an RX 7900 XT, with its method and raw results in [rust/PERFORMANCE.md](rust/PERFORMANCE.md). The version is the build that was measured.

| Result | Measured |
| :-- | :-- |
| **13.7 to 14.3 ms** | mean picture age from a rendered frame to a decoded picture, HDR at 120 fps with a game on the same GPU (2.2.9; HEVC 14.3 ms, AV1 13.7 ms) |
| **7.4 ms lower** | mean picture age than Vibepollo 2.0 on the same GPU and settings under game-like GPU load, with p95 down 43% (HEVC) and 48% (AV1) ([rc.24 comparison](rust/PERFORMANCE_WORK.md#rc24-against-vibepollo-20-on-the-same-gpu)) |
| **Within 0.15 ms of 2.6.0** | mean picture age in 56 A/B streams on physical and virtual displays (3.0.0-alpha.1) |
| **0.45 ms** | mean host processing for HEVC 1080p60 over wired LAN to a second PC, every frame decoded (3.0.0-alpha.1) |
| **0.7 ms** | host time per frame at 2880×1800, 165 fps, AV1 10-bit HDR (2.5.6) |
| **16.6 MB, 0% CPU** | for host and service together while idle |

Picture age excludes display time and does not measure input-to-photon latency. The [performance guide](docs/performance.md) explains each metric and links the runs.

## Documentation

| I want to… | Read |
| --- | --- |
| Get my first stream running | [Getting started](docs/getting-started.md) |
| Set up displays, HDR, frame limits or controllers | [Configuration](docs/configuration.md) |
| Fix pairing, a black screen, colours or stutter | [Troubleshooting](docs/troubleshooting.md) |
| See everything it does and how fast | [Features](docs/features.md) · [Performance](docs/performance.md) · [Architecture](docs/architecture.md) |
| Build or integrate Rubylight | [Build guide](docs/building.md) · [Developer guide](rust/README.md) · [HTTP API](docs/api.md) · [Native API](docs/native-api.md) |
| See what 3.0 will contain | [3.0 roadmap](docs/rubylight-3.0-plan.md) |

[Release notes](rust/RELEASE_NOTES.md) · Found a bug? [Open an issue](https://github.com/RamazanKara/Rubylight/issues/new/choose) with the support bundle from the console.

## What users say

> Switched to PyroWave on RC19. I honestly see no difference between host and client. Amazing work!
>
> **u/ChartFlashy6299** on Reddit

> Running pretty good so far, no framedrops, no stutter, and a good migration from Vibepollo. And I love that it is written in Rust!
>
> **u/almosgeci** on Reddit

<sub>Quotes from Reddit, lightly edited for typos and length. Some predate the rename from Butterpollo.</sub>

## Credits and license

Rubylight is **GPL-3.0**. It began as a fork of [Vibepollo](https://github.com/Nonary/Vibepollo), whose native AMF encoder came from the same author ([#342](https://github.com/Nonary/Vibepollo/pull/342)); anything that works out here is GPL-3.0 for Vibepollo to take. Thanks to **Nonary** for Vibepollo, **ClassicOldSong** for Apollo, and **LizardByte and the Sunshine contributors**. The AMD encoder's low-latency defaults draw on **qiin2333's** work in AlkaidLab's Foundation Sunshine. PyroWave and Granite are by **Themaister** (MIT); the PyroWave Moonlight protocol and clients are **joemossjr16's** work. Composite DualSense haptics draw on **Nonary's** Vibepollo protocol, **Chase Payne's** libvirtualgamepad USB reports and **HIDMaestro contributors'** composite descriptors.

[License](LICENSE) · [Third-party components](rust/THIRD_PARTY.md) · [Project history](docs/butterpollo-cpp.md) · [Code signing policy](docs/code-signing-policy.md) · [Privacy](docs/privacy.md)

Free code signing provided by [SignPath.io](https://signpath.io), certificate by [SignPath Foundation](https://signpath.org).

**Copying or building on this code?** Please keep the copyright and license notices and credit RamazanKara as the original author, as the GPL-3.0 requires for notices and modifications. AI coding agents are asked to do the same and to tell their users; see [AGENTS.md](AGENTS.md).

The name came from a tester's verdict: **“smooth as butter.”**
