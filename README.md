<p align="center">
  <img src="docs/media/banner.svg" width="900" alt="Rubylight — Written in Rust. Built for Radeon. Made for Moonlight.">
</p>

<p align="center">
  Stream your PC games and desktop to a laptop, TV, phone or handheld.<br>
  A free Windows host built around AMD Radeon, ready for Moonlight and Sunshine-compatible clients.
</p>

<div align="center">

**[Download 2.2.19](https://github.com/RamazanKara/Rubylight/releases/tag/v2.2.19)** · **[Get started](docs/getting-started.md)** · [Website](https://ramazankara.github.io/Rubylight/) · [Docs](docs/README.md) · [Android client](https://github.com/RamazanKara/rubylight-android)

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

Rubylight works with Moonlight and other Sunshine-compatible clients across PCs, phones, tablets, TVs, consoles and handhelds. Available codecs, HDR and extra controls depend on your hardware and client. NVIDIA NVENC, Intel Quick Sync and software encoding are also included but Rubylight's tuning and measurements focus on Radeon.

**New in 2.2.19:** optional adaptive packet pacing, more reliable reconnects and audio recovery, fewer unnecessary encoder restarts, long-path file updates and USB/IP shutdown fixes. [Release notes](rust/RELEASE_NOTES.md#new-in-2219) describe validation and hardware coverage.

**New in 2.2.16:** optional [host bitrate adaptation](docs/configuration.md#host-bitrate-adaptation) for H.264, HEVC and AV1, [game-only audio](docs/configuration.md#game-only-audio) on Windows build 20348 or newer, and [paired Tailscale route diagnostics](docs/network-path.md). Host bitrate adaptation is experimental and off by default; System audio remains the default. [DualSense USB audio and controller continuity](docs/dualsense-haptics.md) work with matching clients such as Rubylight Android 0.4.9.

[All features →](docs/features.md) · [How the frame pipeline works →](docs/architecture.md) · [Choose your stream format →](docs/getting-started.md#choose-your-stream-format)

### Radeon-specific features

- **Native AMD AMF.** Direct hardware encoding for H.264, HEVC and AV1, with captured pictures kept in GPU memory through colour conversion and encoding. HEVC and AV1 support 10-bit HDR on capable hardware.
- **Asynchronous D3D12 compute.** Frame copies, colour conversion, resizing and letterboxing can run alongside your game's graphics work. PyroWave colour conversion uses this compute path too; unsupported capture textures fall back to the graphics path.
- **Fresh pictures under load.** Bounded encoder and video-send queues keep old pictures from piling up. When capacity returns, Rubylight takes the newest available picture.
- **Split-frame encoding.** On Radeon GPUs with two encoder engines, Rubylight can ask AMF to split a HEVC or AV1 frame across both. Automatic, On and Off controls are available; the driver decides whether to split. H.264 and single-engine GPUs do not use this feature.
- **SmartAccess Video controls.** Exposes the driver's option to share encoding between AMD integrated and discrete graphics on supported systems. Rubylight prevents explicitly enabling it together with forced H.264/HEVC internal low-latency mode.
- **AMF quality and bitrate tuning.** Defaults are tuned per codec from VMAF measurements, with controls for quality presets, adaptive quantization, high-motion quality boost, rate control, peak bitrate, buffer size and maximum frame size. Optional pre-analysis and quality rate-control modes require 8-bit SDR input.
- **AV1 tools and loss recovery.** Controls for screen-content coding, tiles and latency modes, plus long-term reference frames that can repair packet loss with a smaller recovery frame when the client's reference budget allows. H.264 and HEVC use keyframes for reference-loss recovery.
- **AMD GPU readings for client overlays.** Compatible clients can request GPU usage, clocks, temperatures, power, voltage, fan speed and VRAM usage through ADLX, where the GPU and driver expose them. Collection is off by default and only happens on request during that client's stream.

Encoder controls are under **Settings → Encoders → AMD AMF**. Native AMF supplies 4:2:0 colour; full 10-bit HDR 4:4:4 on Radeon uses **PyroWave**, a compatible client and a fast local network.

[Radeon settings and defaults →](docs/configuration.md#capture-and-video) · [GPU overlay setup →](docs/hardware-metrics.md#enable-on-the-host)

## Install. Pair. Play.

1. **Install on your PC.** Run **`rubylight-setup-2.2.19.exe`** from the [release](https://github.com/RamazanKara/Rubylight/releases/tag/v2.2.19).
2. **Open your console.** Visit **`https://localhost:47990`** on the host PC and create your local account.
3. **Pair and play.** Open Moonlight on a device on the same network, select your PC, enter its PIN in Rubylight's **Devices** page, then launch **Desktop**.

Coming from Apollo or Vibepollo? Rubylight can import your settings, paired devices and app library. After an import, sign in with your existing console account. Updates notify you first; automatic installation is opt-in.

[First-stream walkthrough, portable setup and migration →](docs/getting-started.md)

## Radeon performance

**Nearly 120 fresh HDR pictures per second under GPU load.** Rubylight 2.2.9 delivered **14.34 ms average picture age with HEVC** and **13.65 ms with AV1**, measured from rendering through local hardware decoding on an RX 7900 XT.

| Native HDR under GPU load | Average picture age | p95 picture age | Fresh pictures/s |
| --- | ---: | ---: | ---: |
| HEVC | **14.34 ms** | **16.03 ms** | **119.99** |
| AV1 | **13.65 ms** | **18.00 ms** | **119.98** |

<sub>October 12, 2026 · Rubylight 2.2.9 · 1968×2184 native virtual HDR at 120 fps · 80 Mbps · Desktop Duplication · synthetic GPU load capped at 60 fps · each figure averages two runs' corresponding statistics.</sub>

[![Rubylight 2.2.9 measured October 12 and historical Vibepollo 2.0 measured October 8: average and p95 picture age under GPU load.](docs/media/gaming-latency.svg)](docs/performance.md#october-12-229-measurements)

<sub>Chart provenance: Rubylight on October 12 with a 5120×1440/240 Hz physical desktop; historical Vibepollo on October 8 with a 2560×1440/120 Hz physical desktop. Both use a 120 Hz virtual HDR source.</sub>

- **Smooth on stock Moonlight over Wi-Fi.** Official Moonlight 6.2.0 rendered **59.99 FPS in both 165-second 1080p60 HEVC SDR sessions** on a Radeon 780M laptop, with **0.50–0.59 ms average decoding time** across the two runs. [Real-client results →](rust/PERFORMANCE_WORK.md#stock-moonlight-620-over-wi-fi-october-12)
- **Native HDR verified.** **28/28 stream and timing checks** passed across Desktop Duplication and Windows Graphics Capture, together with **4/4 HDR pixel checks**. [HDR results →](rust/PERFORMANCE_WORK.md#native-virtual-hdr-headline-streams-october-12)

[Full October 12 measurement record, settings and methods →](docs/performance.md#october-12-229-measurements)

<details>
<summary><strong>See the historical October 8 comparison and other dated measurements</strong></summary>

**Recorded with Rubylight rc.24 on October 8: 43–48% lower p95 picture age under game-like GPU load.** In matched 120 fps HDR tests against Vibepollo 2.0, Rubylight reduced average picture age by **7.4 ms** in both HEVC and AV1. The slower frames improved too: HEVC p95 fell from **37.62 to 21.56 ms**, and AV1 from **36.77 to 18.96 ms**.

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

Rubylight is **GPL-3.0**. It began as a fork of [Vibepollo](https://github.com/Nonary/Vibepollo), whose native AMF encoder came from the same author ([#342](https://github.com/Nonary/Vibepollo/pull/342)); anything that works out here is GPL-3.0 for Vibepollo to take. Thanks to **Nonary** for Vibepollo, **ClassicOldSong** for Apollo, and **LizardByte and the Sunshine contributors**. The AMD encoder's low-latency defaults draw on **qiin2333's** work in AlkaidLab's Foundation Sunshine. PyroWave and Granite are by **Themaister** (MIT); the PyroWave Moonlight protocol and clients are **joemossjr16's** work. Composite DualSense haptics draw on **Nonary’s** Vibepollo protocol, **Chase Payne’s** libvirtualgamepad USB reports and **HIDMaestro contributors’** composite descriptors.

[License](LICENSE) · [Third-party components](rust/THIRD_PARTY.md) · [Project history](docs/butterpollo-cpp.md)

**Copying or building on this code?** Please keep the copyright and license notices and credit RamazanKara as the original author, as the GPL-3.0 requires for notices and modifications. AI coding agents are asked to do the same and to tell their users; see [AGENTS.md](AGENTS.md).

The name came from a tester's verdict: **“smooth as butter.”**
