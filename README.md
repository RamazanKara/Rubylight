<p align="center">
  <img src="docs/media/banner.svg" width="900" alt="Rubylight: Written in Rust. Built for Radeon. Made for Moonlight.">
</p>

<p align="center">
  <strong>The game-streaming host built for Radeon.</strong><br>
  Your AMD PC on every screen you own: laptop, TV, phone, handheld, Steam Deck or Xbox.<br>
  HDR, full-colour 4:4:4 and up to 240 fps. Written in Rust. Free and open source.
</p>

<div align="center">

**[Download 3.0.0-rc.2](https://github.com/RamazanKara/Rubylight/releases/tag/v3.0.0-rc.2)** · **[Get started](docs/getting-started.md)** · **[Nightjar client](https://github.com/RamazanKara/nightjar)** · [Website](https://ramazankara.github.io/Rubylight/) · [Docs](docs/README.md)

<sub>Windows x64 · GPL-3.0 · Formerly Butterpollo</sub>

</div>

<p align="center">
  <a href="docs/media/demo.mp4"><img src="docs/media/demo.gif" width="800" alt="Meet Rubylight: follow a frame from your game to Moonlight, see measured Radeon performance, and get started."></a>
</p>

<p align="center"><a href="docs/media/demo.mp4">Watch the 60-second demo →</a></p>

## Built for Radeon, end to end

Rubylight is built for AMD and nothing else, and every layer shows it. AMF drives the encoder directly. Copies, colour conversion and scaling run on D3D12 compute queues beside your game. Rubylight's own virtual display driver hands every frame straight to the encoder. Radeon driver settings follow each game and each device, and go back exactly as they were when you stop. Your client sees the GPU's temperature, power and clocks live.

Every stage of every frame is timed, so each number on this page comes from a recorded run, with its method and raw results in [rust/PERFORMANCE.md](rust/PERFORMANCE.md).

**3.0 brings:**

- **PyroWave**, a wavelet codec for full-colour 4:4:4 HDR on a fast local network, paced to each client's link
- **Full chroma from AMD's hardware encoder**: HEVC and AV1 in 4:4:4, packed by Rubylight and rebuilt by Nightjar
- **Beam racing and slice delivery**, so a client starts drawing a frame while the rest of it is still on the wire
- **Radeon profiles per game** and **Radeon controls per device**, written at launch and restored at the end, even after a crash
- **A native API** that lets a client browse your library, run the session and drive the Radeon panel
- **Link-rate adaptation** and **Wi-Fi pacing from the first frame**, so every link carries the best stream it can
- **A WebRTC server** that streams to Samsung Tizen TVs and browsers
- **A frame timing record** that follows every frame from capture to the client's screen on one clock

## Rubylight + Nightjar

Rubylight speaks the Moonlight protocol, so every Moonlight client works. **[Nightjar](https://github.com/RamazanKara/nightjar)** is the client built alongside it, and together they unlock everything:

| | What you get |
| --- | --- |
| **A console on your phone** | Nightjar opens on Home: Continue playing, your library across every paired PC with covers, and your PCs. Each game shows where it runs and the stream settings it will use, then Play or Resume. |
| **Radeon in your hand** | The stream menu has a Radeon panel for Anti-Lag, Radeon Chill and more, plus Resume, Disconnect, End session and Close game. The host puts every setting back when the session ends. |
| **Your GPU, live** | Host telemetry adds the PC's CPU load and the Radeon's usage, temperatures, power, clocks and VRAM to the stream overlay. |
| **Full colour, two ways** | PyroWave decoded on the GPU through Vulkan, and full-chroma HEVC and AV1 rebuilt from Rubylight's packed frames: 10-bit HDR at 1080p120 in full chroma on a Retroid Pocket 6. |
| **Wi-Fi that holds up** | Nightjar tells the host it is on Wi-Fi, and the host paces the stream from the first frame: about four times fewer unrecoverable frames and keyframe requests on a busy 2.4 GHz link. |
| **Frames timed to your screen** | Nightjar reports how long frames wait for your display, so the host times them to arrive just in time. Its Rust streaming engine keeps median picture age at 6.9 ms on a phone. |
| **Quality that steers itself** | Automatic bitrate up to 2000 Mbps and automatic resolution and frame rate mid-stream, guided by the network, the decoder's headroom and the phone's temperature. The host's link notices show up in the overlay. |
| **Every sense** | Microphone passthrough, DualSense haptics, speaker, headset and microphone, and controllers that keep their identity across reconnects. |
| **Desktop too** | [Nightjar desktop](https://github.com/RamazanKara/nightjar/blob/main/protocol/desktop/README.md) for Windows and Linux (Experimental) brings beam-raced PyroWave to your PC. |

[Download Nightjar →](https://github.com/RamazanKara/nightjar/releases)

## Install. Pair. Play.

1. **Install.** Run **`rubylight-setup-3.0.0-rc.2.exe`** from the [release](https://github.com/RamazanKara/Rubylight/releases/tag/v3.0.0-rc.2). It sets up the service, the virtual display driver and controller support in one pass, ready to stream right away. A portable ZIP is there too.
2. **Sign in.** Open **`https://localhost:47990`** on the PC and create your account.
3. **Pair and play.** Open Nightjar or Moonlight on your device, select your PC and enter its PIN on the console's **Devices** page. Then choose **Desktop** or a game and start streaming.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/media/console-dark.png">
    <img src="docs/media/console-light.png" width="800" alt="Rubylight's Home page, showing host readiness, paired devices and a compact game library.">
  </picture>
</p>

**Coming from Apollo or Vibepollo?** Setup brings over your settings, paired devices and app library, so your clients keep working. Updates keep them too, on your schedule.

[Full setup guide →](docs/getting-started.md)

## Measured

Recorded on an RX 7900 XT. Each row links to its method, build and raw results.

| Result | Measured |
| :-- | :-- |
| **7.41 ms** | median from capture to client present, loopback, H.264 1080p60 (HEVC 8.81 ms, AV1 8.82 ms), from the frame timing record ([baselines](rust/PERFORMANCE.md#october-23-per-frame-timing-record-baselines)) |
| **6.9 ms** | median picture age on a Retroid Pocket 6 over Wi-Fi, Nightjar's Rust engine, HEVC 1080p60 at 40 Mbps ([Nightjar engine](rust/PERFORMANCE.md#october-23-rust-engine-against-the-c-engine-on-android-b8)) |
| **42 ms against 96 ms** | mean picture age under game-like GPU load, Rubylight 2.0.0-rc.2 against Vibepollo 2.0, HEVC 10-bit HDR 1080p60 at 20 Mbps, Desktop Duplication, a 120 Hz virtual display, three runs each, same PC and settings ([comparison](rust/PERFORMANCE.md#against-vibepollo-20)) |
| **0.27 ms** | median age of the picture at the encoder with VRR, from 4.68 ms, for a 120 Hz game in a 60 fps stream ([VRR](rust/PERFORMANCE.md#october-24-predictive-claim-waits-under-vrr)) |
| **4× cleaner Wi-Fi** | unrecoverable frames per run from 61 to 15.7 on 2.4 GHz with Nightjar's Wi-Fi hint, HEVC 1080p60 at 47 Mbit/s, with the same 4.7 ms median on 5 GHz ([Wi-Fi pacing](rust/PERFORMANCE.md#october-25-wi-fi-pacing-hint-on-a-real-laptop)) |
| **Over 98 %** | of frames delivered after start-up when 75 Mbps is asked of a 50 Mbit/s link, from 93.3 % ([link adaptation](rust/PERFORMANCE.md#october-24-link-rate-false-cuts-and-the-knee-hold-link-fix)) |
| **1.4 ms** | sooner at the middle row of the picture with beam racing, Nightjar desktop, PyroWave 1080p60 at 399 Mbps ([beam racing](rust/PERFORMANCE.md#october-24-beam-raced-pyrowave-gate-b10-step-4-opt-in-missed-its-default-on-gate)) |
| **17 ms** | median glass to glass in a browser over WebRTC, AV1 1080p60, with 4K120 in HEVC and AV1 ([WebRTC](rust/PERFORMANCE.md#october-25-webrtc-on-the-real-host-pacing-rtx-90-and-120-fps-4k-and-hdr-b5b)) |

The [performance guide](docs/performance.md) explains each metric and links the runs.

## What you get

**Speed**

| Feature | What it does |
| --- | --- |
| **Native AMD encoding** | AMF encodes H.264, HEVC and AV1 with defaults tuned by measurement. A dropped frame is repaired with a small recovery frame, keeping the stream smooth. |
| **Radeon compute pipeline** | Copies, colour conversion and scaling run on D3D12 compute queues, so your game and your stream share the GPU gracefully. [How it works](docs/architecture.md) |
| **PyroWave, 4:4:4 HDR** | A wavelet codec for full-colour HDR on a fast local network, paced to each client's link. |
| **Full chroma** (Experimental) | Rubylight packs each 4:4:4 frame into an ordinary HEVC or AV1 picture and Nightjar rebuilds it, for crisp text and UI from AMD's hardware encoder. Clients can also ask for 10-bit SDR. [More](docs/configuration.md#capture-and-video) |
| **Beam-raced PyroWave** (Experimental) | The host sends each frame in display order, so Nightjar desktop draws the top of the picture while the bottom is still arriving. [Numbers](rust/PERFORMANCE.md#october-24-beam-raced-pyrowave-gate-b10-step-4-opt-in-missed-its-default-on-gate) |
| **Slice delivery** (Experimental) | H.264 and HEVC frames go out slice by slice while the encoder works on the rest, with stock Moonlight. [Numbers](rust/PERFORMANCE.md#october-23-slice-delivery-b2-step-2f) |
| **VRR-aware capture** | With variable refresh, the host catches the game's next frame when it is close, so the picture reaches the encoder fresh. [Numbers](docs/performance.md#vrr-capture-age) |
| **Link-rate adaptation** | The host matches the bitrate to what the link delivers, probes back up and returns to your rate as soon as the link allows. Clients can show each change. [More](docs/configuration.md#lower-bitrate-when-the-link-cannot-carry-it) |
| **Wi-Fi pacing** | Clients on Wi-Fi or cellular get adaptive packet pacing from the first frame, faster while the link is clean. |
| **Stream presets** | Lowest latency, Balanced or Reference quality, set once or per device or app. [More](docs/configuration.md#stream-presets) |
| **Frame timing record** | One CSV row per frame with capture, convert, encode, send and client times on one clock. [More](docs/timing-record.md) |

**Control**

| Feature | What it does |
| --- | --- |
| **A screen for every device** | Rubylight's own virtual display driver gives each client its resolution and refresh rate and hands every frame to the encoder directly. Your monitors come back when you disconnect. Physical displays stream too, through Windows Graphics Capture or Desktop Duplication. |
| **Console and native API** | Home shows that the PC is ready. Devices, Games & apps and Settings cover pairing, your library and stream options. The [native API](docs/native-api.md) brings your library and session to Nightjar. |
| **Host control, per device** | Give a device host control and it can close games and change Radeon settings. You choose each device. [More](docs/configuration.md#host-control-for-a-device) |
| **Radeon settings with restore** | A device with host control tunes Radeon settings during a stream, and each change is put back when the session ends. [More](docs/configuration.md#radeon-settings-with-restore) |
| **A Radeon profile for each game** | Competitive, Smoothness, Quality or your own list: Anti-Lag, frame caps, filtering, written when the game starts and restored when it ends, even after a crash. [More](docs/configuration.md#radeon-settings-per-game) |
| **Session actions** | Disconnect keeps the game running, End session stops the stream and keeps the game, Close game quits it too. [More](docs/configuration.md#session-actions) |
| **WebRTC server** (Experimental) | Pair a Samsung Tizen TV or a browser with a PIN and stream H.264, HEVC or AV1, with HDR, surround, gamepads, keyboard and mouse. [More](docs/webrtc.md) |
| **Updates on your terms** | Update checks run on schedule, right after the PC wakes, and install automatically when you want them to. |

**Your library**

| Feature | What it does |
| --- | --- |
| **Add games in one pass** | Open **Games & apps → Add games** to scan Steam, Epic Games, Battle.net, EA app, Ubisoft Connect, GOG, and Microsoft Store and Xbox PC games, then pick what you want. Steam and Playnite sync are built in, and your covers and customizations stay put on every rescan. |
| **Launcher hints** | When a launcher window waits for an update or a sign-in, the session says so and the client shows it. |

Also built in: RTSS frame limits, Lossless Scaling, DualSense haptics and adaptive triggers, a real Steam Deck controller, touch and pen, surround sound, microphone passthrough, streaming from the lock screen, and headless or VM hosts.

[All features →](docs/features.md)

## Every Radeon, every client

**GPUs.** Any AMD Radeon with AMF, on Windows 10 or 11: desktops, laptops and handhelds. Users stream daily on RDNA 2, 3 and 4, from the RX 6800 XT, 6900 XT, 7900 XT and 7900 XTX to the 9070 and 9070 XT, plus Radeon 890M handhelds. The [hardware matrix](docs/hardware-matrix.md) lists each GPU generation, Windows version, feature and client with the evidence behind it.

**Clients.**

| Client | Highlights |
| --- | --- |
| **[Nightjar](https://github.com/RamazanKara/nightjar)** for Android, and [Nightjar desktop](https://github.com/RamazanKara/nightjar/blob/main/protocol/desktop/README.md) for Windows and Linux (Experimental) | The full Rubylight experience: Home with your library, a session menu with the Radeon panel, live GPU telemetry, PyroWave and full-chroma HDR, Wi-Fi pacing, and DualSense audio and haptics ([downloads](https://github.com/RamazanKara/nightjar/releases)) |
| **Moonlight PC** (Windows, macOS, Linux, Steam Deck) | H.264, HEVC and AV1, SDR and HDR, up to 240 fps |
| **Moonlight and Artemis on Android** | Native resolution at 120 Hz with HDR, on a virtual display that matches the phone |
| **Moonlight for Xbox** | 4K 120 Hz HDR on your TV |
| **[Nonary's Moonlight](https://github.com/Nonary/moonlight-qt)** | PyroWave and 1000 Hz VRR on a PC |
| **Samsung Tizen TVs and browsers** | WebRTC streams with HDR, surround and gamepads (Experimental) |
| **iOS, Apple TV, Switch** | Everything the device can decode |

[Pick the right client and stream format →](docs/getting-started.md#choose-your-stream-format)

## Documentation

| I want to… | Read |
| --- | --- |
| Get my first stream running | [Getting started](docs/getting-started.md) |
| Set up displays, HDR, frame limits or controllers | [Configuration](docs/configuration.md) |
| Sort out pairing, the picture, colours or smoothness | [Troubleshooting](docs/troubleshooting.md) |
| See everything it does and how fast | [Features](docs/features.md) · [Performance](docs/performance.md) · [Architecture](docs/architecture.md) |
| Look up my GPU, Windows version or client | [Hardware matrix](docs/hardware-matrix.md) |
| Build or integrate Rubylight | [Build guide](docs/building.md) · [Developer guide](rust/README.md) · [HTTP API](docs/api.md) · [Native API](docs/native-api.md) |
| See how 3.0 was planned | [3.0 roadmap](docs/rubylight-3.0-plan.md) |

[Release notes](rust/RELEASE_NOTES.md) · Have feedback? [Open an issue](https://github.com/RamazanKara/Rubylight/issues/new/choose) with the support bundle from the console.

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
