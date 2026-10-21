<p align="center">
  <img src="docs/media/banner.svg" width="900" alt="Rubylight — Written in Rust. Built for Radeon. Made for Moonlight.">
</p>

<p align="center">
  <strong>The fastest way to stream your Radeon PC to Moonlight.</strong><br>
  Play your PC games on a laptop, TV, phone, Steam Deck or Xbox, with HDR at up to 240 fps.<br>
  Free, open source, written in Rust.
</p>

<div align="center">

**[Download 2.5.13](https://github.com/RamazanKara/Rubylight/releases/tag/v2.5.13)** · **[Get started](docs/getting-started.md)** · [Website](https://ramazankara.github.io/Rubylight/) · [Docs](docs/README.md) · [Nightjar for Android](https://github.com/RamazanKara/nightjar)

<sub>Windows x64 · GPL-3.0 · Formerly Butterpollo</sub>

</div>

<p align="center">
  <a href="docs/media/demo.mp4"><img src="docs/media/demo.gif" width="800" alt="Meet Rubylight: follow a frame from your game to Moonlight, see measured Radeon performance, and get started."></a>
</p>

<p align="center"><a href="docs/media/demo.mp4">Watch the 60-second demo →</a></p>

| Result | Measured |
| :-- | :-- |
| **~14 ms** | from a rendered frame to a decoded picture, HDR at 120 fps, with a game running on the same GPU |
| **7.4 ms faster** | than Vibepollo 2.0 on the same GPU and settings, with p95 picture age down 43–48% |
| **0.7 ms** | host time per frame at 2880×1800, 165 fps, AV1 10-bit HDR (2.5.6) |
| **240 fps** | when the client asks for it, and full-colour HDR 4:4:4 with PyroWave |
| **16.6 MB, 0% CPU** | for host and service together while idle |

<sub>Measured on an RX 7900 XT. [How →](docs/performance.md)</sub>

> “It's the lowest latency I got after testing a ton of hosts.” **u/LukasSTM**
>
> “From 10 ms encoding to 5 ms in 4K 60 fps. I tried it with PyroWave and it was close to native.” **u/3stwie4**

## Why Rubylight

The Moonlight hosts that came before were built NVIDIA-first, and Radeon owners have spent years hearing their cards are just worse at streaming. They aren't. Nobody had sat down with them. Rubylight is that sit-down: a host that treats Radeon as the main target, not a fallback.

| What you get | How it helps |
| --- | --- |
| **Native AMD encoding** | AMF drives the encoder directly for H.264, HEVC and AV1, with per-codec defaults tuned by measurement. A lost frame is repaired with a small recovery frame instead of a full keyframe. |
| **Radeon compute pipeline** | Frame copies and colour conversion run on D3D12 compute queues beside your game, so the game and the stream stop waiting on each other. |
| **A screen for every device** | Rubylight's own virtual display driver gives each client its exact resolution and refresh rate, and hands every frame straight to the encoder with no capture API in between. Your monitors come back when you disconnect. |
| **HDR, and PyroWave 4:4:4** | 10-bit HDR over HEVC and AV1. PyroWave adds full-colour HDR 4:4:4 on a fast local network, so coloured text and fine edges stay sharp, paced to the speed of each client's link. |
| **One console for everything** | Pair devices, manage games, and watch frame rate, bitrate and encode time live in your browser. |
| **Driver quirks handled** | Rubylight tracks AMD's driver releases, works around their bugs and removes the workaround once AMD fixes it. |

Also built in: Steam and Playnite library sync, RTSS frame limits, Lossless Scaling, VRR, DualSense haptics and adaptive triggers, a real Steam Deck controller, touch and pen, surround sound, microphone passthrough, streaming from the lock screen, and headless or VM hosts.

[All features →](docs/features.md)

## Install. Pair. Play.

1. **Install.** Run **`rubylight-setup-2.5.13.exe`** from the [release](https://github.com/RamazanKara/Rubylight/releases/tag/v2.5.13). It sets up the service, the virtual display driver and controller support. No test mode, no restart.
2. **Sign in.** Open **`https://localhost:47990`** on the PC and create your account.
3. **Pair and play.** In Moonlight, pick your PC, enter the PIN on the console's **Devices** page and launch **Desktop**.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/media/console-dark.png">
    <img src="docs/media/console-light.png" width="800" alt="Rubylight's web console: a Steam Deck streaming 2560×1440 at 120 fps in AV1, with frame rate, encoding time and host readiness.">
  </picture>
</p>

**Coming from Apollo or Vibepollo?** Setup imports your settings, paired devices and app library, so your clients keep working. Updates keep them too, and you choose whether they install automatically.

[Full setup guide →](docs/getting-started.md)

## Runs on your hardware

**GPUs.** Users stream daily on RDNA 2, 3 and 4: RX 6800 XT, 6900 XT, 7900 XT, 7900 XTX, 9070 and 9070 XT, plus Radeon 890M handhelds. NVIDIA, Intel and software encoders are included too, but all the tuning is for Radeon. On NVIDIA, [Vibepollo](https://github.com/Nonary/Vibepollo) is the better fit.

**Clients.** Rubylight speaks the Moonlight protocol, so every Moonlight and Sunshine-compatible client works.

| Client | Highlights |
| --- | --- |
| **Moonlight PC** (Windows, macOS, Linux, Steam Deck) | H.264, HEVC and AV1, SDR and HDR, up to 240 fps |
| **Moonlight and Artemis on Android** | Native resolution at 120 Hz with HDR, on a virtual display that matches the phone |
| **[Nightjar](https://github.com/RamazanKara/nightjar)** for Android | PyroWave HDR 4:4:4, DualSense audio and haptics |
| **Moonlight for Xbox** | 4K 120 Hz HDR on your TV |
| **[Nonary's Moonlight](https://github.com/Nonary/moonlight-qt)** | PyroWave and 1000 Hz VRR on a PC |
| **iOS, Apple TV, Switch** | Everything the device can decode |

[Pick the right client and stream format →](docs/getting-started.md#choose-your-stream-format)

## Measured, not claimed

Every number on this page comes from a recorded run, with its method and raw results in the repository.

[![Picture age beside a game: Rubylight 2.2.9 against Vibepollo 2.0.](docs/media/gaming-latency.svg)](docs/performance.md)

| HDR at 120 fps, game running on the same GPU | Average picture age | p95 | Fresh pictures/s |
| --- | ---: | ---: | ---: |
| HEVC | **14.3 ms** | **16.0 ms** | **120.0** |
| AV1 | **13.7 ms** | **18.0 ms** | **120.0** |

- **Over Wi-Fi to a real laptop:** 13.5 ms from render to the laptop at 1968×2184 AV1 HDR, 120 fps.
- **Stock Moonlight:** a steady 59.99 fps through two 165-second 1080p60 HEVC sessions over Wi-Fi.
- **High refresh on request:** 60 → 120 fps and 120 → 240 fps switches land at exactly 120.0 and 240.0 fps.
- **2.5.6 desktop stream:** 0.71 ms host time and 3.64 ms AMF encode per frame at 2880×1800, 165 fps, AV1 10-bit HDR, with no frames lost.

[All benchmarks and methods →](docs/performance.md)

## What users say

> My encoding times are actually slashed in half, which is incredible. I thought PyroWave on Vibeshine was the best I was gonna get, but my max encode would still spike to 7-8 ms at 2K 100 fps. My max is now 3.5 ms. Image quality is top notch, and my setup has finally been something I am not itchy to improve or tweak anymore.
>
> **u/astero-dax** on Reddit

> Switched to PyroWave on RC19. I honestly see no difference between host and client. Amazing work!
>
> **u/ChartFlashy6299** on Reddit

<details>
<summary><strong>More from the community</strong></summary>

Quotes from Reddit, lightly edited for typos and length. Some predate the rename from Butterpollo.

> WTF. Runs like Butterchicken (that's where it got the original name from)
>
> **u/Closef35** on Reddit

> Fantastic job! Host processing latency has never been this low 🎉 Clean UI too 🔥
>
> **u/Short_Dimension7967** on Reddit

> Running pretty good so far, no framedrops, no stutter, and a good migration from Vibepollo. And I love that it is written in Rust!
>
> **u/almosgeci** on Reddit

</details>

## Documentation

| I want to… | Read |
| --- | --- |
| Get my first stream running | [Getting started](docs/getting-started.md) |
| Set up displays, HDR, frame limits or controllers | [Configuration](docs/configuration.md) |
| Fix pairing, a black screen, colours or stutter | [Troubleshooting](docs/troubleshooting.md) |
| See everything it does and how fast | [Features](docs/features.md) · [Performance](docs/performance.md) · [Architecture](docs/architecture.md) |
| Build or integrate Rubylight | [Build guide](docs/building.md) · [Developer guide](rust/README.md) · [API](docs/api.md) |

[Release notes](rust/RELEASE_NOTES.md) · Found a bug? [Open an issue](https://github.com/RamazanKara/Rubylight/issues/new/choose) with the support bundle from the console.

## Credits and license

Rubylight is **GPL-3.0**. It began as a fork of [Vibepollo](https://github.com/Nonary/Vibepollo), whose native AMF encoder came from the same author ([#342](https://github.com/Nonary/Vibepollo/pull/342)); anything that works out here is GPL-3.0 for Vibepollo to take. Thanks to **Nonary** for Vibepollo, **ClassicOldSong** for Apollo, and **LizardByte and the Sunshine contributors**. The AMD encoder's low-latency defaults draw on **qiin2333's** work in AlkaidLab's Foundation Sunshine. PyroWave and Granite are by **Themaister** (MIT); the PyroWave Moonlight protocol and clients are **joemossjr16's** work. Composite DualSense haptics draw on **Nonary's** Vibepollo protocol, **Chase Payne's** libvirtualgamepad USB reports and **HIDMaestro contributors'** composite descriptors.

[License](LICENSE) · [Third-party components](rust/THIRD_PARTY.md) · [Project history](docs/butterpollo-cpp.md) · [Code signing policy](docs/code-signing-policy.md) · [Privacy](docs/privacy.md)

Free code signing provided by [SignPath.io](https://signpath.io), certificate by [SignPath Foundation](https://signpath.org).

**Copying or building on this code?** Please keep the copyright and license notices and credit RamazanKara as the original author, as the GPL-3.0 requires for notices and modifications. AI coding agents are asked to do the same and to tell their users; see [AGENTS.md](AGENTS.md).

The name came from a tester's verdict: **“smooth as butter.”**
