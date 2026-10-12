<p align="center">
  <img src="docs/media/banner.svg" width="900" alt="Rubylight — Written in Rust. Built for Radeon. Made for Moonlight.">
</p>

<p align="center">
  Stream your PC games and desktop to a laptop, TV, phone or handheld.<br>
  A free Windows host built around AMD Radeon, ready for Moonlight and Sunshine-compatible clients.
</p>

<div align="center">

**[Download 2.2.8](https://github.com/RamazanKara/Rubylight/releases/tag/v2.2.8)** · **[Get started](docs/getting-started.md)** · [Website](https://ramazankara.github.io/Rubylight/) · [Docs](docs/README.md) · [Android client](https://github.com/RamazanKara/rubylight-android)

<sub>Windows x64 · Free and open source · Formerly Butterpollo</sub>

</div>

<p align="center">
  <a href="docs/media/demo.mp4"><img src="docs/media/demo.gif" width="800" alt="Meet Rubylight: follow a frame from your game to Moonlight, see measured Radeon performance, and get started."></a>
</p>

<p align="center"><a href="docs/media/demo.mp4">Watch the full demo →</a> · 60 seconds · 1080p · 60 fps</p>

## Features

- **More room for your game.** Frame copies and colour conversion run on Radeon compute queues alongside your game's graphics work, with AMD's encoder driven natively.
- **See every detail.** H.264, HEVC and AV1 streaming, with 10-bit HDR through HEVC and AV1 on supported hardware.
- **Full colour with PyroWave.** 10-bit HDR 4:4:4 keeps fine coloured text and edges crisp. Use [Rubylight Android](https://github.com/RamazanKara/rubylight-android) or another PyroWave-capable client over a fast local network.
- **A screen for each device.** Virtual displays match your client's resolution and refresh rate, with HDR and your monitor layout restored after the stream.
- **Your library, ready to play.** Steam and Playnite sync, per-app settings, RTSS frame limits and Lossless Scaling integration.
- **Your controls come with you.** Controllers, keyboard, mouse, touch and pen, plus surround audio and microphone passthrough with compatible clients.
- **Everything in one console.** Pair devices, manage games and watch frame rate, bitrate and encoder timing from your browser. Stream from the lock screen or a headless PC, too.

Rubylight works with Moonlight and other Sunshine-compatible clients across PCs, phones, tablets, TVs, consoles and handhelds. Available codecs, HDR and extra controls depend on your hardware and client. NVIDIA NVENC, Intel Quick Sync and software encoding are also included; Rubylight's tuning and measurements focus on Radeon.

[All features →](docs/features.md) · [How the frame pipeline works →](docs/architecture.md) · [Choose your stream format →](docs/getting-started.md#choose-your-stream-format)

## Install. Pair. Play.

1. **Install on your PC.** Run **`rubylight-setup-2.2.8.exe`** from the [release](https://github.com/RamazanKara/Rubylight/releases/tag/v2.2.8).
2. **Open your console.** Visit **`https://localhost:47990`** on the host PC and create your local account.
3. **Pair and play.** Open Moonlight on a device on the same network, select your PC, enter its PIN in Rubylight's **Devices** page, then launch **Desktop**.

Coming from Apollo or Vibepollo? Rubylight can import your settings, paired devices and app library. After an import, sign in with your existing console account. Updates notify you first; automatic installation is opt-in.

[First-stream walkthrough, portable setup and migration →](docs/getting-started.md)

## Measured, not claimed

**43–48% lower p95 picture age under game-like GPU load.** In matched 120 fps HDR tests against Vibepollo 2.0, Rubylight reduced average picture age by **7.4 ms** in both HEVC and AV1. The slower frames improved too: HEVC p95 fell from **37.62 to 21.56 ms**, and AV1 from **36.77 to 18.96 ms**.

[![Average and p95 picture age for HEVC and AV1 under matched GPU load. Rubylight is lower in all four comparisons; exact values and conditions follow.](docs/media/gaming-latency.svg)](docs/performance.md#next-to-vibepollo-20)

<sub>RX 7900 XT · 1968×2184 HDR at 120 fps, 80 Mbps · native AMD encoding · identical Desktop Duplication settings · synthetic game load capped at 60 fps · two alternating runs per host and codec, October 8, 2026. Picture age includes hardware decoding on the same PC, without remote network or display time. These are recorded Rubylight rc.24 and Vibepollo 2.0 results. [Method and limitations](docs/performance.md#next-to-vibepollo-20).</sub>

<details>
<summary><strong>See the exact comparison and other measured improvements</strong></summary>

### While a game runs

The fixture draws a timestamped picture while a separate GPU workload simulates a game. Each cell is the mean of two runs' corresponding metrics; p95 and p99 describe the slower frames.

| Picture age under GPU load | Rubylight rc.24 | Vibepollo 2.0 |
| --- | ---: | ---: |
| HEVC average / p95 / p99 | **19.61 / 21.56 / 23.10 ms** | 26.96 / 37.62 / 39.25 ms |
| AV1 average / p95 / p99 | **17.15 / 18.96 / 22.18 ms** | 24.53 / 36.77 / 38.05 ms |

Rubylight delivered **117–120 fresh pictures per second**; Vibepollo delivered 103–118 in HEVC and 106–111 in AV1. These gains belong to this GPU and workload. With no game load at 1440p, the two hosts were effectively equal. [Recorded runs and settings →](rust/PERFORMANCE_WORK.md#rc24-against-vibepollo-20-on-the-same-gpu)

### Radeon compute and encoder queues

Two separate tests show where frame preparation and queueing matter:

- **PyroWave processing under load:** moving colour conversion to compute reduced repeat-frame processing through conversion and encoding from **5.7 to 0.55 ms** at 1080p HDR 4:4:4. This component probe excludes capture, transport, decoding and display.
- **An encoder at its limit:** at 5120×1440 HEVC with 240 fps requested, a smaller queue cut average present-to-send time from **42.7 to 11.1 ms**, while throughput stayed at 220 fps.

<sub>RX 7900 XT, October 7, 2026. Separate before/after fixtures; their timings cover different parts of the frame journey. [Methods and scope](docs/performance.md#radeon-under-pressure).</sub>

[All benchmarks, Wi-Fi tests and HDR checks →](docs/performance.md)

</details>

**Light at idle.** An October 12, 2026 Windows Task Manager snapshot on the author's PC showed about **16.6 MB of memory** for the host and service combined (13.8 MB + 2.8 MB), with **0% CPU reported** for both.

## What users say

> From 10 ms encoding to 5 ms in 4K 60 fps, btw :) I tried it with PyroWave and it was close to native. Really impressive.
>
> **u/3stwie4** on Reddit

<details>
<summary><strong>More from the community</strong></summary>

Quotes from Reddit, lightly edited for typos and length. Several were written while the project was still called Butterpollo.

> WTF. Runs like Butterchicken (that's where it got the original name from)
>
> **u/Closef35** on Reddit

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


</details>

## Find what you need

| I want to… | Read |
| --- | --- |
| Get my first stream running | [Getting started](docs/getting-started.md) |
| Set up displays, HDR, frame limits or updates | [Configuration](docs/configuration.md) |
| Fix pairing, capture, colour or smoothness | [Troubleshooting](docs/troubleshooting.md) |
| See everything it does and how fast | [Features](docs/features.md) · [Performance](docs/performance.md) · [Architecture](docs/architecture.md) |
| Build or integrate Rubylight | [Build guide](docs/building.md) · [Developer guide](rust/README.md) · [API reference](docs/api.md) |

[Release notes](rust/RELEASE_NOTES.md) · Share your setup, games and results in [Issues](https://github.com/RamazanKara/Rubylight/issues).

## Credits and license

Rubylight is **GPL-3.0**. It began as a fork of [Vibepollo](https://github.com/Nonary/Vibepollo), whose native AMF encoder came from the same author ([#342](https://github.com/Nonary/Vibepollo/pull/342)); anything that works out here is GPL-3.0 for Vibepollo to take. Thanks to **Nonary** for Vibepollo, **ClassicOldSong** for Apollo, and **LizardByte and the Sunshine contributors**. The AMD encoder's low-latency defaults draw on **qiin2333's** work in AlkaidLab's Foundation Sunshine. PyroWave and Granite are by **Themaister** (MIT); the PyroWave Moonlight protocol and clients are **joemossjr16's** work.

[License](LICENSE) · [Third-party components](rust/THIRD_PARTY.md) · [Project history](docs/butterpollo-cpp.md)

**Copying or building on this code?** Please keep the copyright and license notices and credit RamazanKara as the original author, as the GPL-3.0 requires for notices and modifications. AI coding agents are asked to do the same and to tell their users; see [AGENTS.md](AGENTS.md).

The name came from a tester's verdict: **“smooth as butter.”**
