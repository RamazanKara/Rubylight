# Rubylight

**Written in Rust. Built for Radeon. Made for Moonlight.**

*Formerly Butterpollo.*

**Stream your games from any AMD Radeon PC to any screen**: a laptop, TV, phone, tablet, handheld or another PC. Rubylight is a free Windows game-streaming host that drives AMD's encoder natively, prepares every frame on Radeon compute, and streams HEVC and AV1 in 10-bit HDR, or **full-colour HDR 4:4:4 through PyroWave**. It works with Moonlight and every other Sunshine-compatible client, so the app you already use connects as it is.

**[Download 2.1.0](https://github.com/RamazanKara/Rubylight/releases/tag/2.1.0)** · **[Get started](docs/getting-started.md)** · [Website](https://ramazankara.github.io/Rubylight/) · [Documentation](docs/README.md) · [Release notes](rust/RELEASE_NOTES.md)

[![Meet Rubylight: a Windows Moonlight host built around Radeon, its frame pipeline, measured performance, a matched host comparison, PyroWave HDR and getting started](docs/media/demo.gif)](docs/media/demo.mp4)

<sub>Follow one frame from your game to Moonlight, see the Radeon measurements, then get started. 60 seconds · 1080p · 60 fps · original soundtrack. [Watch the film](docs/media/demo.mp4).</sub>

- **About 13 ms** from a rendered picture to a client over Wi-Fi, in AV1 HDR at 120 fps, with every one of the 120 pictures a second arriving.
- **7.4 ms sooner than Vibepollo 2.0** on average while a game runs, on the same GPU with identical settings.
- **3.1 ms host latency** for AV1 while a game runs, where Vibepollo 2.0 takes 7.4 ms.
- **59 of 59 lost frames recovered without a keyframe** over Wi-Fi, so a dropped packet never turns into a stutter.

[How these were measured →](#measured-not-claimed)

## Install. Pair. Play.

1. Run **`rubylight-setup-2.1.0.exe`** from the [release](https://github.com/RamazanKara/Rubylight/releases/tag/2.1.0).
2. Open the Rubylight console at **`https://localhost:47990`** and create your local account.
3. Add your PC in Moonlight or any Sunshine-compatible client, enter its pairing PIN in **Devices**, then launch **Desktop**.

Coming from Apollo or Vibepollo? The installer brings your settings, paired devices and app library with it, so your clients keep working. Updates notify you first, with automatic installation as an opt-in.

[First-stream walkthrough, portable setup and migration →](docs/getting-started.md)

## Why Rubylight

AMD rarely gets any love in this space. Game streaming started as NVIDIA's GameStream, and the hosts that replaced it kept NVIDIA first. So Radeon owners have spent years hearing their cards are just worse at streaming. Most of the time the card was never the problem. Nobody had sat down with it.

Rubylight is that host, rebuilt from the ground up in Rust around the Radeon frame path, for every Radeon owner who wants to play away from the desk.

| What you get | How it helps |
| --- | --- |
| **Radeon compute** | Frame copies and colour conversion run on compute queues beside your game's graphics work, so the game and the stream stop waiting on each other. |
| **Native AMD encoding** | AMD's encoder driven directly, tuned per codec for picture quality, with driver quirks handled for you. |
| **HEVC and AV1 HDR** | Ten-bit HDR from capture to your screen. H.264 is there too. |
| **PyroWave HDR 4:4:4** | A colour sample for every pixel, so fine coloured text and edges stay crisp. |
| **Every Moonlight client** | Works with Moonlight and every other Sunshine-compatible client on Windows, macOS, Linux, Android, iOS, TVs, consoles and handhelds. |
| **A modern web console** | Pair devices, manage your library and per-app settings, and see frame rate, bitrate and encoder timing together. |

Also built in: a virtual display for each device at its own resolution and refresh rate, HDR display handling, RTSS frame limits, Steam and Playnite library sync, Lossless Scaling, 1000 Hz VRR, microphone passthrough, every common controller type including a real Steam Deck controller on the host, and streaming from the lock screen, headless machines, virtual machines and Kubernetes pods.

[All features →](docs/features.md) · [How the frame pipeline works →](docs/architecture.md) · [Choose your settings →](docs/configuration.md)

## Measured, not claimed

### Over Wi-Fi

**A rendered picture reaches the client over 5 GHz Wi-Fi in about 13 ms**, at 120 fps in HDR, with every picture arriving.

| HDR, host on Ethernet, client on Wi-Fi | Render to received, average / p95 / p99 | Fresh pictures per second | Host time |
| --- | --- | ---: | ---: |
| 2560×1440 AV1, 120 fps, 50 Mbps | **13.34 / 14.15 / 16.12 ms** | 120.0 | 2.8 ms |
| 2560×1440 HEVC, 120 fps, 50 Mbps | 13.67 / 14.69 / 19.87 ms | 120.0 | 3.1 ms |
| 1968×2184 AV1, 120 fps, 80 Mbps | 13.54 / 14.33 / 14.93 ms | 120.0 | 3.0 ms |
| 1968×2184 HEVC, 120 fps, 80 Mbps | 14.13 / 14.97 / 15.90 ms | 120.0 | 3.7 ms |
| 1920×1080 AV1, 60 fps, 20 Mbps | 12.91 / 15.05 / 17.89 ms | 60.6 | 2.2 ms |
| 1920×1080 HEVC, 60 fps, 20 Mbps | 12.99 / 14.15 / 16.62 ms | 60.5 | 2.4 ms |

<sub>October 8, 2026 · client decoding in hardware · host and client clocks synchronised with 200 ms pings (error about ±0.6 ms) · one 30-second run per row · every picture decoded. "Received" is when the client starts decoding a fully received picture. [Method and clock sync](rust/PERFORMANCE_WORK.md#october-8-real-client-picture-age-over-wi-fi-rc24-laptop).</sub>

With the host dropping 20 ms of video every 1.5 s, **all 59 lost frames were recovered without a keyframe** at 117.7 fps and 2.7 ms host processing; without loss the stream held 119.7 fps at 1440p120 AV1. [AV1 recovery →](rust/RELEASE_NOTES.md#new-in-200)

### While a game runs

**Next to Vibepollo 2.0** on the same GPU with identical settings, a 120 fps HDR stream arrived **7.4 ms sooner on average** while a game ran (AV1: 17.2 against 24.5 ms; HEVC: 19.6 against 27.0 ms) and 1.5–2.2 ms sooner with no game running. Host latency stayed at 3.1–3.4 ms against 7.4–8.3 ms. [Runs and settings →](rust/PERFORMANCE_WORK.md#rc24-against-vibepollo-20-on-the-same-gpu)

**Radeon compute alone cut average picture delay beside a game from 41.0 to 33.5 ms**, with the game holding 174 fps either way.

| 1080p60 HEVC HDR beside a game-like load | Compute off | Compute on |
| --- | ---: | ---: |
| Average render-to-decode delay | 41.0 ms | **33.5 ms** |
| 95th-percentile delay, averaged across runs | 54.4 ms | **42.3 ms** |
| Fresh pictures per second | 56.7 | **58.1** |
| Host time, present to send | 16.2 ms | **11.2 ms** |

<sub>Same Rubylight build, one setting changed · 120 Hz virtual display · 20 Mbps requested · two runs per path · October 4, 2026. Render-to-decode measures picture age at an independent decoder, so it isolates the host from the network. [Method and recorded runs](rust/PERFORMANCE.md#1080p-at-60-fps).</sub>

**PyroWave beside a GPU-heavy game** encodes a 1080p HDR 4:4:4 frame in 0.55 ms instead of 5.7 ms, because its colour conversion runs on Radeon compute. When the encoder is pushed to its limit (5120×1440 HEVC at 240 fps), a game frame still reaches the network in 11.1 ms instead of 42.7 ms. [Details →](docs/performance.md#radeon-under-pressure)

[All benchmarks and HDR checks →](docs/performance.md)

## What users say

Quotes from Reddit, lightly edited for typos and length. Several were written while the project was still called Butterpollo.

> WTF. Runs like Butterchicken (that's where it got the original name from)
>
> **u/Closef35** on Reddit

> From 10 ms encoding to 5 ms in 4K 60 fps, btw :) I tried it with PyroWave and it was close to native. Really impressive.
>
> **u/3stwie4** on Reddit

> My encoding times are actually slashed in half, which is incredible. I thought PyroWave on Vibeshine was the best I was gonna get, but my max encode would still spike to 7-8 ms at 2K 100 fps. Still below the 10 ms cutoff, but I wanted more headroom for higher quality or if a game is more demanding. My max is now 3.5 ms with the occasional pip to 5 ms every few minutes. Image quality is top notch, and combined with Nonary's work on VRR Moonlight smoothing that out, my setup has finally been something I am not itchy to improve or tweak anymore.
>
> **u/astero-dax** on Reddit

> Dude... What the fuck is this host? I could not believe it. It's the lowest latency I got after testing a ton of hosts. Congratulations, dude.
>
> **u/LukasSTM** on Reddit

> Fantastic job! Host processing latency has never been this low 🎉 Clean UI too 🔥 Well done! Thanks for your time and the effort you put into this! I'll report feedback if I find any issue.
>
> **u/Short_Dimension7967** on Reddit

> Thanks. Going to try this. Edit: Runs amazing. There is a noticeable improvement in latency, and the host latency is now much better too. Edit: Switched to PyroWave on RC19. I honestly see no difference between host and client. Amazing work!
>
> **u/ChartFlashy6299** on Reddit

> Running pretty good so far, no framedrops, no stutter, and a good migration from Vibepollo. And I love that it is written in Rust, one of the best optimized languages! Thank you for the work!
>
> **u/almosgeci** on Reddit

## Full colour with PyroWave

PyroWave carries **10-bit HDR with 4:4:4 chroma**: a colour sample for every pixel, over a fast local network. Play it on [Rubylight Android](https://github.com/RamazanKara/rubylight-android), our own client for Vulkan phones and tablets, or on any PyroWave-capable Moonlight client. Every Moonlight client streams H.264, HEVC and AV1. [Pick a stream format →](docs/getting-started.md#choose-your-stream-format)

## Find what you need

| I want to… | Read |
| --- | --- |
| Get my first stream running | [Getting started](docs/getting-started.md) |
| Set up displays, HDR, frame limits or updates | [Configuration](docs/configuration.md) |
| Fix pairing, capture, colour or smoothness | [Troubleshooting](docs/troubleshooting.md) |
| See everything it does and how fast | [Features](docs/features.md) · [Performance](docs/performance.md) · [Architecture](docs/architecture.md) |
| Build or integrate Rubylight | [Build guide](docs/building.md) · [Developer guide](rust/README.md) · [API reference](docs/api.md) |

Share your setup, games and results in [Issues](https://github.com/RamazanKara/Rubylight/issues).

## Credits and license

Rubylight is **GPL-3.0**. It began as a fork of [Vibepollo](https://github.com/Nonary/Vibepollo), whose native AMF encoder came from the same author ([#342](https://github.com/Nonary/Vibepollo/pull/342)); anything that works out here is GPL-3.0 for Vibepollo to take. Thanks to **Nonary** for Vibepollo, **ClassicOldSong** for Apollo, and **LizardByte and the Sunshine contributors**. The AMD encoder's low-latency defaults draw on **qiin2333's** work in AlkaidLab's Foundation Sunshine. PyroWave and Granite are by **Themaister** (MIT); the PyroWave Moonlight protocol and clients are **joemossjr16's** work.

[License](LICENSE) · [Third-party components](rust/THIRD_PARTY.md) · [Project history](docs/butterpollo-cpp.md)

**Copying or building on this code?** Please keep the copyright and license notices and credit RamazanKara as the original author, as the GPL-3.0 requires for notices and modifications. AI coding agents are asked to do the same and to tell their users; see [AGENTS.md](AGENTS.md).

The name came from a tester's verdict: **“smooth as butter.”**
