<p align="center">
  <img src="docs/media/banner.svg" width="900" alt="Rubylight — Written in Rust. Built for Radeon. Made for Moonlight.">
</p>

<p align="center">
  Stream your PC games and desktop to a laptop, TV, phone or handheld.<br>
  A free Windows host built for AMD Radeon and Moonlight, written in Rust.
</p>

<div align="center">

**[Download 2.4.6](https://github.com/RamazanKara/Rubylight/releases/tag/v2.4.6)** · **[Get started](docs/getting-started.md)** · [Website](https://ramazankara.github.io/Rubylight/) · [Docs](docs/README.md) · [Nightjar for Android](https://github.com/RamazanKara/nightjar)

<sub>Windows x64 · Free and open source · Formerly Butterpollo</sub>

</div>

<p align="center">
  <a href="docs/media/demo.mp4"><img src="docs/media/demo.gif" width="800" alt="Meet Rubylight: follow a frame from your game to Moonlight, see measured Radeon performance, and get started."></a>
</p>

<p align="center"><a href="docs/media/demo.mp4">Watch the full demo →</a> · 60 seconds · 1080p · 60 fps</p>

- **13.5 ms** from a rendered picture to a laptop over Wi-Fi, at 1968×2184 AV1 HDR and 120 fps, with all 120 pictures a second arriving.
- **About 14 ms** average picture age beside a game, in HEVC and AV1 HDR at 120 fps on an RX 7900 XT.
- **7.4 ms sooner than Vibepollo 2.0** on average beside a game, on the same GPU with identical settings.
- **Up to 240 fps**, and full-colour 10-bit HDR 4:4:4 with PyroWave on a fast local network.

[How these were measured →](#measured-on-real-hardware)

## The mission

AMD rarely gets any love in this space. The protocol started as NVIDIA's GameStream, and even after NVIDIA dropped GameStream in 2023, the hosts that replaced it kept NVIDIA first. Upstream Sunshine has had a native NVENC encoder since 2023, while AMD still goes through FFmpeg's generic AMF wrapper. AMD hasn't helped itself either. It shut down its own streaming app, AMD Link, in 2024, saying there are plenty of other ways to stream, and its drivers still have quirks like an RDNA4 freeze that hosts have to work around.

So Radeon owners have spent years hearing their cards are just worse at streaming. That is where Rubylight comes into play, and it is the sole reason Rubylight exists. Most of the time the card was never the problem. Nobody had sat down with it.

In practice that means a native AMF encoder instead of a generic wrapper, frame copies and colour conversion on Radeon compute queues, and someone reading AMD's driver release notes so you don't have to. When a driver freezes the stream, Rubylight works around it. When AMD fixes it, Rubylight removes the workaround and pretends nothing happened.

Rubylight is not here to win a big userbase. There is no growth plan and no campaign to convert the NVIDIA crowd. As long as AMD users are happy, Rubylight is happy.

## Install. Pair. Play.

1. **Install on your PC.** Run **`rubylight-setup-2.4.6.exe`** from the [release](https://github.com/RamazanKara/Rubylight/releases/tag/v2.4.6). It also installs Rubylight's own virtual display driver, which gives every device a screen of its own: no test mode and no restart. Until releases are code-signed, setup signs the driver on your PC with a key it deletes at once; Smart App Control, S mode and App Control policies refuse such a driver, and then streams use your monitor.
2. **Open your console.** Visit **`https://localhost:47990`** on the host PC and create your local account. Setup trusts the console's own certificate, so the browser opens it without a security warning.
3. **Pair and play.** Open Moonlight on a device on the same network, select your PC, enter its PIN in Rubylight's **Devices** page, then launch **Desktop**.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/media/console-dark.png">
    <img src="docs/media/console-light.png" width="800" alt="Rubylight's web console: a Steam Deck streaming 2560×1440 at 120 fps in AV1, with frame rate, encoding time and host readiness.">
  </picture>
</p>

Coming from Apollo or Vibepollo? Rubylight imports your settings, paired devices and app library. Upgrades keep them too. Updates notify you first; automatic installation is opt-in.

[First-stream walkthrough, portable setup and migration →](docs/getting-started.md)

## Why Rubylight

| What you get | How it helps |
| --- | --- |
| **Radeon compute** | Frame copies and colour conversion run on D3D12 compute queues beside your game's graphics work, so the stream takes less from the game. |
| **Native AMD encoding** | AMF drives the encoder directly for H.264, HEVC and AV1, with defaults tuned per codec. AV1 can repair packet loss with a small recovery frame instead of a keyframe. |
| **HDR, and full colour with PyroWave** | HEVC and AV1 carry 10-bit HDR. PyroWave adds 10-bit HDR 4:4:4, which keeps coloured text and fine edges crisp, over a fast local network. It paces each frame to the link your device reports, so a 1 GbE device behind a 2.5 GbE PC doesn't drop packets, and clients that calibrate their network, such as Nonary's Moonlight, get the rate and pace they measured. |
| **A screen for each device** | Rubylight's own virtual display driver matches each client's resolution and refresh rate, and hands every frame straight to the host, with no capture API in between. Your monitor layout and HDR come back after the stream. |
| **One calm console** | Pair devices, manage games and per-app settings, and watch frame rate, bitrate and encoder timing from your browser. |

Also built in: Steam and Playnite library sync, RTSS frame limits, Lossless Scaling, VRR, controllers (including DualSense haptics and a real Steam Deck controller), keyboard, mouse, touch and pen, surround audio, microphone passthrough, and streaming from the lock screen or a headless PC.

Rubylight is built for Radeon. NVIDIA NVENC, Intel Quick Sync and software encoding are included, but the tuning and the measurements are Radeon-only. On NVIDIA, Vibepollo is the better fit.

[All features →](docs/features.md) · [Radeon settings and defaults →](docs/configuration.md#capture-and-video) · [How the frame pipeline works →](docs/architecture.md)

## Measured on real hardware

**Nearly 120 fresh HDR pictures a second beside a game.** Rubylight 2.2.9 averaged **14.34 ms** picture age with HEVC and **13.65 ms** with AV1, from rendering to local hardware decoding on an RX 7900 XT.

| Native HDR beside a game-like GPU load | Average picture age | p95 picture age | Fresh pictures/s |
| --- | ---: | ---: | ---: |
| HEVC | **14.34 ms** | **16.03 ms** | **119.99** |
| AV1 | **13.65 ms** | **18.00 ms** | **119.98** |

<sub>October 12, 2026 · 1968×2184 virtual HDR display at 120 fps · 80 Mbps · synthetic GPU load capped at 60 fps · each figure averages two runs.</sub>

[![Picture age beside a game: Rubylight 2.2.9 against Vibepollo 2.0.](docs/media/gaming-latency.svg)](docs/performance.md#october-12-229-measurements)

- **High refresh, on request.** A device that launches at 60 fps and asks for 120, as Artemis's Warp modes do, gets **120.0 fps** on its virtual display, and 120 → 240 gets **240.0 fps**, at the bitrate the device set.
- **Over Wi-Fi to a real laptop.** A rendered picture reached a Radeon 780M laptop on 5 GHz Wi-Fi in **13.5 ms** on average at 1968×2184 AV1 HDR, 120 fps. [Method →](rust/PERFORMANCE_WORK.md#october-8-real-client-picture-age-over-wi-fi-rc24-laptop)
- **Next to Vibepollo 2.0.** On the same GPU with identical settings, pictures arrived **7.4 ms sooner** on average beside a game, and p95 picture age fell by 43–48%. With no game running the two were close. [Runs and settings →](rust/PERFORMANCE_WORK.md#rc24-against-vibepollo-20-on-the-same-gpu)
- **Stock Moonlight.** Moonlight 6.2.0 rendered **59.99 fps** in two 165-second 1080p60 HEVC sessions over Wi-Fi. [Results →](rust/PERFORMANCE_WORK.md#stock-moonlight-620-over-wi-fi-october-12)
- **Light at idle.** About **16.6 MB** of memory for host and service together, with 0% CPU.

[All benchmarks, Wi-Fi tests and HDR checks →](docs/performance.md)

**Next: Direct3D 12 from capture to encoder.** Colour conversion and AMD encoding already run on D3D12, but capture still receives each frame through Direct3D 11 and hands it across to D3D12, with a synchronisation step at every hand-off. The next step moves capture onto D3D12 as well, so a frame travels from the display to the encoder in one API, which will take that hand-off out of every frame's latency.

## Works with

| Client | What you get |
| --- | --- |
| **Moonlight PC** | H.264, HEVC and AV1, in SDR and HDR, over Wi-Fi or wired |
| **Moonlight for Xbox** | HDR at 3840×2160, 120 Hz |
| **Moonlight and Artemis on Android** | Up to the phone's native resolution at 120 Hz, HDR, on its own virtual display |
| **[Nightjar](https://github.com/RamazanKara/nightjar)** | PyroWave HDR 4:4:4 on Vulkan phones and tablets, plus DualSense audio |
| **[Nonary's Moonlight](https://github.com/Nonary/moonlight-qt)** | PyroWave and 1000 Hz VRR on a PC |

Rubylight streams from RX 7900 XT, RX 9070 XT, RX 6900 XT and Radeon 890M systems, among others. Codecs, HDR and extra controls depend on your GPU and client. [Pick the client and stream format for your setup →](docs/getting-started.md#choose-your-stream-format)

## What users say

> From 10 ms encoding to 5 ms in 4K 60 fps, btw :) I tried it with PyroWave and it was close to native. Really impressive.
>
> **u/3stwie4** on Reddit

> Dude... What the fuck is this host? I could not believe it. It's the lowest latency I got after testing a ton of hosts. Congratulations, dude.
>
> **u/LukasSTM** on Reddit

<details>
<summary><strong>More from the community</strong></summary>

Quotes from Reddit, lightly edited for typos and length. Several were written while the project was still called Butterpollo.

> WTF. Runs like Butterchicken (that's where it got the original name from)
>
> **u/Closef35** on Reddit

> My encoding times are actually slashed in half, which is incredible. I thought PyroWave on Vibeshine was the best I was gonna get, but my max encode would still spike to 7-8 ms at 2K 100 fps. Still below the 10 ms cutoff, but I wanted more headroom for higher quality or if a game is more demanding. My max is now 3.5 ms with the occasional pip to 5 ms every few minutes. Image quality is top notch, and combined with Nonary's work on VRR Moonlight smoothing that out, my setup has finally been something I am not itchy to improve or tweak anymore.
>
> **u/astero-dax** on Reddit

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

[Release notes](rust/RELEASE_NOTES.md) · Share your Radeon setup, games and results in [Issues](https://github.com/RamazanKara/Rubylight/issues). The [troubleshooting guide](docs/troubleshooting.md) explains which logs make a report useful.

## Credits and license

Rubylight is **GPL-3.0**. It began as a fork of [Vibepollo](https://github.com/Nonary/Vibepollo), whose native AMF encoder came from the same author ([#342](https://github.com/Nonary/Vibepollo/pull/342)); anything that works out here is GPL-3.0 for Vibepollo to take. Thanks to **Nonary** for Vibepollo, **ClassicOldSong** for Apollo, and **LizardByte and the Sunshine contributors**. The AMD encoder's low-latency defaults draw on **qiin2333's** work in AlkaidLab's Foundation Sunshine. PyroWave and Granite are by **Themaister** (MIT); the PyroWave Moonlight protocol and clients are **joemossjr16's** work. Composite DualSense haptics draw on **Nonary’s** Vibepollo protocol, **Chase Payne’s** libvirtualgamepad USB reports and **HIDMaestro contributors’** composite descriptors.

[License](LICENSE) · [Third-party components](rust/THIRD_PARTY.md) · [Project history](docs/butterpollo-cpp.md) · [Code signing policy](docs/code-signing-policy.md) · [Privacy](docs/privacy.md)

Free code signing provided by [SignPath.io](https://signpath.io), certificate by [SignPath Foundation](https://signpath.org).

**Copying or building on this code?** Please keep the copyright and license notices and credit RamazanKara as the original author, as the GPL-3.0 requires for notices and modifications. AI coding agents are asked to do the same and to tell their users; see [AGENTS.md](AGENTS.md).

The name came from a tester's verdict: **“smooth as butter.”**
