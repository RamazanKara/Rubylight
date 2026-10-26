<p align="center">
  <img src="docs/media/banner.svg" width="900" alt="Rubylight: Written in Rust. Built for Radeon. Made for Moonlight.">
</p>

<h3 align="center">The game-streaming host built for Radeon.</h3>

<p align="center">
  Play your PC games on your TV, laptop, phone, handheld, Steam Deck or Xbox.<br>
  HDR, full-colour 4:4:4 and up to 240 fps, straight from your AMD GPU. Free and open source.
</p>

<p align="center">
  <a href="https://github.com/RamazanKara/Rubylight/releases"><img src="https://img.shields.io/github/v/release/RamazanKara/Rubylight?include_prereleases&label=release&color=c41242" alt="Latest release"></a>
  <a href="https://github.com/RamazanKara/Rubylight/releases"><img src="https://img.shields.io/github/downloads/RamazanKara/Rubylight/total?color=c41242" alt="Downloads"></a>
  <a href="https://github.com/RamazanKara/Rubylight/stargazers"><img src="https://img.shields.io/github/stars/RamazanKara/Rubylight?color=c41242" alt="Stars"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0-c41242" alt="GPL-3.0"></a>
</p>

<div align="center">

**[Download 3.0.0-rc.2](https://github.com/RamazanKara/Rubylight/releases/tag/v3.0.0-rc.2)** · **[Get started](docs/getting-started.md)** · [Website](https://ramazankara.github.io/Rubylight/) · [Docs](docs/README.md)

<sub>Windows 10 and 11 · Any AMD Radeon with AMF · Formerly Butterpollo</sub>

</div>

<p align="center">
  <a href="docs/media/demo.mp4"><img src="docs/media/demo.gif" width="800" alt="Meet Rubylight: follow a frame from your game to Moonlight, see measured Radeon performance, and get started."></a>
</p>

<p align="center"><a href="docs/media/demo.mp4">Watch the 70-second film →</a></p>

## Why Radeon owners choose Rubylight

- **The spiritual successor to AMD Link.** Your Radeon PC on every screen in the house, with your game library and Radeon controls in your hand, on the open Moonlight ecosystem.
- **Built around AMD, top to bottom.** AMF drives the encoder directly, D3D12 compute queues prepare every frame beside your game, and Rubylight's own display driver hands each frame straight to the encoder.
- **Full colour, crisp text.** HEVC and AV1 in 10-bit HDR, plus full-colour 4:4:4 through PyroWave and packed full chroma.
- **Your whole library, one click away.** Scan Steam, Epic Games, Battle.net, EA app, Ubisoft Connect, GOG, Microsoft Store and Xbox PC games, with covers, and stream any of them from the couch.
- **Radeon settings that follow the game.** Anti-Lag, frame caps and filtering per game and per device, put back exactly as they were when you stop.
- **Measured, in the open.** Every stage of every frame is timed, and every number comes with its run, its method and its raw results.
- **Set up in minutes.** One installer, your monitors back after every stream, and your Apollo or Vibepollo setup carried over.

## A console you'll enjoy

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/media/console-dark.png">
    <img src="docs/media/console-light.png" width="800" alt="Rubylight's Home page: ready to play, a live stream card and a shelf of games with covers.">
  </picture>
</p>

<table>
  <tr>
    <td width="33%"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/media/console-library-dark.png"><img src="docs/media/console-library-light.png" alt="Games & apps: the full library with cover art."></picture></td>
    <td width="33%"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/media/console-add-games-dark.png"><img src="docs/media/console-add-games-light.png" alt="Add games: one scan across every launcher on the PC."></picture></td>
    <td width="33%"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/media/console-radeon-dark.png"><img src="docs/media/console-radeon-light.png" alt="A game's Radeon profile: Competitive, Smoothness, Quality or Custom."></picture></td>
  </tr>
  <tr>
    <td align="center"><b>Your library</b>, with covers</td>
    <td align="center"><b>Add games</b> from every launcher</td>
    <td align="center"><b>A Radeon profile</b> for each game</td>
  </tr>
</table>

**Home** shows that the PC is ready, the live stream and your game shelf. **Games & apps** holds your library, **Devices** handles pairing and permissions, and **Settings** groups everything into General, Streaming, Display & HDR, Audio & controls, Games & automation, and Network & access, with search for every advanced control.

## Install. Pair. Play.

1. **Install.** Run **`rubylight-setup-3.0.0-rc.2.exe`** from the [release](https://github.com/RamazanKara/Rubylight/releases/tag/v3.0.0-rc.2). It sets up the service, the virtual display driver and controller support in one pass, ready to stream right away. A portable ZIP is there too.
2. **Sign in.** Open **`https://localhost:47990`** on the PC and create your account.
3. **Pair and play.** Open Moonlight or any compatible client on your device, select your PC and enter its PIN on the console's **Devices** page. Then pick **Desktop** or a game and play.

**Coming from Apollo or Vibepollo?** Rubylight brings over your settings, paired devices, app library and covers, so every client keeps working on day one. Updates keep everything too, on the schedule you choose.

[Full setup guide →](docs/getting-started.md)

## Your whole library

| Feature | What it does |
| --- | --- |
| **Every launcher in one scan** | **Games & apps → Add games** finds what is installed from Steam, Epic Games, Battle.net, EA app, Ubisoft Connect, GOG, and Microsoft Store and Xbox PC games. Search, filter by launcher, tick what you want, done. |
| **Covers included** | Each game arrives with its cover from the launcher. Pick your own any time. |
| **Steam, always in sync** | Steam games sync on demand and every 30 seconds, and a stream ends cleanly when the game exits. |
| **Playnite** | Your Playnite library syncs too, launches through Playnite with the stream's settings, and Playnite Fullscreen is one tap away. |
| **Anything else** | Add a program, a script, a URL or a document, with preparation and undo commands, a working folder and start before sign-in. |
| **Settings per game** | Each app can carry its own stream, display, input and encoder settings, its own Radeon profile, RTSS frame limits and Lossless Scaling profile, all applied at launch and removed at the end. |
| **Launcher hints** | When a launcher waits for an update or a sign-in, the session says so and your client can show it. |

## Measured

Recorded on an RX 7900 XT. Each row links to its method, build and raw results in [rust/PERFORMANCE.md](rust/PERFORMANCE.md).

| Result | Measured |
| :-- | :-- |
| **5.89 ms** | median from capture to client present, loopback, AV1 1080p60 (H.264 6.92 ms, HEVC 8.37 ms), from the frame timing record of 3.0.0-rc.2 ([timing record](rust/PERFORMANCE.md#october-26-300-rc2-against-the-published-numbers)) |
| **42 ms against 96 ms** | mean picture age under game-like GPU load, Rubylight 2.0.0-rc.2 against Vibepollo 2.0, HEVC 10-bit HDR 1080p60 at 20 Mbps, Desktop Duplication, a 120 Hz virtual display, three runs each, same PC and settings ([comparison](rust/PERFORMANCE.md#against-vibepollo-20)) |
| **0.25 ms** | median age of the picture at the encoder with VRR, against 4.68 ms without VRR-aware capture, for a 120 Hz game in a 60 fps stream ([VRR](rust/PERFORMANCE.md#october-26-300-rc2-against-the-published-numbers), [method](rust/PERFORMANCE.md#october-24-predictive-claim-waits-under-vrr)) |
| **4× cleaner Wi-Fi** | unrecoverable frames per run from 61 to 15.7 on 2.4 GHz with Wi-Fi pacing, HEVC 1080p60 at 47 Mbit/s, and the same 4.7 ms median on 5 GHz ([Wi-Fi pacing](rust/PERFORMANCE.md#october-25-wi-fi-pacing-hint-on-a-real-laptop)) |
| **Over 98 %** | of frames delivered after start-up when 75 Mbps is asked of a 50 Mbit/s link, from 93.3 % ([link adaptation](rust/PERFORMANCE.md#october-24-link-rate-false-cuts-and-the-knee-hold-link-fix)) |
| **17 ms** | median glass to glass in a browser over WebRTC, AV1 1080p60, with 4K120 in HEVC and AV1 ([WebRTC](rust/PERFORMANCE.md#october-25-webrtc-on-the-real-host-pacing-rtx-90-and-120-fps-4k-and-hdr-b5b)) |

<p align="center">
  <a href="docs/performance.md"><img src="docs/media/gaming-latency.svg" width="800" alt="Picture age under GPU load, HEVC and AV1 HDR at 120 fps on an RX 7900 XT: Rubylight 2.2.9 averages 14.34 and 13.65 ms against 26.96 and 24.53 ms for Vibepollo 2.0."></a>
</p>

The [performance guide](docs/performance.md) explains each metric and links every run.

## Everything it does

**Speed**

| Feature | What it does |
| --- | --- |
| **Native AMD encoding** | AMF encodes H.264, HEVC and AV1 with defaults tuned by measurement. A dropped frame is repaired with a small recovery frame, keeping the stream smooth. |
| **Radeon compute pipeline** | Copies, colour conversion and scaling run on D3D12 compute queues, so your game and your stream share the GPU gracefully. [How it works](docs/architecture.md) |
| **PyroWave, 4:4:4 HDR** | A wavelet codec for full-colour HDR on a fast local network, paced to each client's link. |
| **Full chroma** (Experimental) | Rubylight packs each 4:4:4 frame into an ordinary HEVC or AV1 picture for crisp text and UI from AMD's hardware encoder, unpacked by Nightjar. 10-bit SDR is there for clients that ask. [More](docs/configuration.md#capture-and-video) |
| **VRR-aware capture** | With variable refresh, the host catches the game's next frame when it is close, so the picture reaches the encoder fresh, up to 1000 Hz. [Numbers](docs/performance.md#vrr-capture-age) |
| **Link-rate adaptation** | The host matches the bitrate to what the link delivers, probes back up and returns to your rate as soon as the link allows. Clients can show each change. [More](docs/configuration.md#lower-bitrate-when-the-link-cannot-carry-it) |
| **Wi-Fi pacing** | Clients on Wi-Fi or cellular get adaptive packet pacing from the first frame, faster while the link is clean. |
| **Stream presets** | Lowest latency, Balanced or Reference quality, set once or per device or app. [More](docs/configuration.md#stream-presets) |
| **Beam racing and slice delivery** (Experimental) | PyroWave frames arrive in display order and H.264 and HEVC frames leave the host slice by slice, so a client draws the top of the picture while the bottom is still on the wire. [Numbers](rust/PERFORMANCE.md#october-23-slice-delivery-b2-step-2f) |
| **Frame timing record** | One CSV row per frame with capture, convert, encode, send and client times on one clock. [More](docs/timing-record.md) |

**Displays**

| Feature | What it does |
| --- | --- |
| **A screen for every device** | Rubylight's own virtual display driver gives each client its exact resolution, refresh rate and HDR, and your monitors come back when you disconnect. |
| **Two clients at once** | Each device gets its own virtual display. |
| **Background streaming** | A device plays on its own display while someone keeps using the PC, with only the game's sound and the device's controllers. [More](docs/configuration.md#background-streaming) |
| **Every capture path** | Physical displays stream through Windows Graphics Capture or Desktop Duplication, and the lock screen streams too, so you can sign in from the couch. Headless PCs and VMs work as well. |

**Control from the couch**

| Feature | What it does |
| --- | --- |
| **Radeon settings with restore** | A device with host control tunes Anti-Lag, Radeon Chill and more during a stream, and every change is put back when the session ends. [More](docs/configuration.md#radeon-settings-with-restore) |
| **A Radeon profile for each game** | Competitive, Smoothness, Quality or your own list, written when the game starts and restored when it ends, even after a crash. [More](docs/configuration.md#radeon-settings-per-game) |
| **Session actions** | Disconnect keeps the game running, End session stops the stream and keeps the game, Close game quits it too. [More](docs/configuration.md#session-actions) |
| **Native API** | Clients browse your library with covers, follow the session and drive the Radeon controls. [Contract](docs/native-api.md) |
| **Live GPU readings** | Clients can show the PC's CPU load and the Radeon's usage, temperatures, power, clocks and VRAM in their overlay. [Protocol](docs/hardware-metrics.md) |
| **WebRTC server** (Experimental) | Pair a Samsung Tizen TV or a browser with a PIN and stream H.264, HEVC or AV1, with HDR, surround, gamepads, keyboard and mouse. [More](docs/webrtc.md) |

**Controllers and audio**

| Feature | What it does |
| --- | --- |
| **The right pad for every client** | DualSense with adaptive triggers and haptics, Switch Pro, Xbox Series, and a real Steam Deck controller with trackpads, gyro and back grips. |
| **Touch, pen, keyboard and mouse** | Everything a client sends, on a signed driver from the installer. |
| **Sound both ways** | Surround audio, game-only audio, and your client's microphone on the PC for voice chat. |

Also built in: a tray icon with notifications, automatic updates verified by SHA-256, separate instances with their own profiles, and a support bundle in one click.

[All features →](docs/features.md)

## Every Radeon, every client

**GPUs.** Any AMD Radeon with AMF, on Windows 10 or 11: desktops, laptops and handhelds. Users stream daily on RDNA 2, 3 and 4, from the RX 6800 XT, 6900 XT, 7900 XT and 7900 XTX to the 9070 and 9070 XT, plus Radeon 890M handhelds. The [hardware matrix](docs/hardware-matrix.md) lists each GPU generation, Windows version, feature and client with the evidence behind it.

**Clients.** Rubylight speaks the Moonlight protocol, so Moonlight, Artemis and every other Sunshine-compatible client works out of the box.

| Client | Highlights |
| --- | --- |
| **Moonlight PC** (Windows, macOS, Linux, Steam Deck) | H.264, HEVC and AV1, SDR and HDR, up to 240 fps |
| **Moonlight and Artemis on Android** | Native resolution at 120 Hz with HDR, on a virtual display that matches the phone |
| **Moonlight for Xbox** | 4K 120 Hz HDR on your TV |
| **[Nightjar](https://github.com/RamazanKara/nightjar)** for Android, plus Windows and Linux (Experimental) | PyroWave and full chroma, your library and the Radeon controls on your phone |
| **[Nonary's Moonlight](https://github.com/Nonary/moonlight-qt)** | PyroWave and 1000 Hz VRR on a PC |
| **Samsung Tizen TVs and browsers** | WebRTC streams with HDR, surround and gamepads (Experimental) |
| **iOS, Apple TV, Switch** | Everything the device can decode |

[Pick the right client and stream format →](docs/getting-started.md#choose-your-stream-format)

## What users say

> Switched to PyroWave on RC19. I honestly see no difference between host and client. Amazing work!
>
> **u/ChartFlashy6299** on Reddit

> Running pretty good so far, no framedrops, no stutter, and a good migration from Vibepollo. And I love that it is written in Rust!
>
> **u/almosgeci** on Reddit

<sub>Quotes from Reddit, lightly edited for typos and length. Some predate the rename from Butterpollo.</sub>

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

[Release notes](rust/RELEASE_NOTES.md) · Have feedback? [Open an issue](https://github.com/RamazanKara/Rubylight/issues/new/choose) with the support bundle from the console. If Rubylight makes your couch better, a ⭐ helps other Radeon owners find it.

## Credits and license

Rubylight is **GPL-3.0**. It began as a fork of [Vibepollo](https://github.com/Nonary/Vibepollo), whose native AMF encoder came from the same author ([#342](https://github.com/Nonary/Vibepollo/pull/342)); anything that works out here is GPL-3.0 for Vibepollo to take. Thanks to **Nonary** for Vibepollo, **ClassicOldSong** for Apollo, and **LizardByte and the Sunshine contributors**. The AMD encoder's low-latency defaults draw on **qiin2333's** work in AlkaidLab's Foundation Sunshine. PyroWave and Granite are by **Themaister** (MIT); the PyroWave Moonlight protocol and clients are **joemossjr16's** work. Composite DualSense haptics draw on **Nonary's** Vibepollo protocol, **Chase Payne's** libvirtualgamepad USB reports and **HIDMaestro contributors'** composite descriptors.

[License](LICENSE) · [Third-party components](rust/THIRD_PARTY.md) · [Project history](docs/butterpollo-cpp.md) · [Code signing policy](docs/code-signing-policy.md) · [Privacy](docs/privacy.md)

Free code signing provided by [SignPath.io](https://signpath.io), certificate by [SignPath Foundation](https://signpath.org).

**Copying or building on this code?** Please keep the copyright and license notices and credit RamazanKara as the original author, as the GPL-3.0 requires for notices and modifications. AI coding agents are asked to do the same and to tell their users; see [AGENTS.md](AGENTS.md).

The name came from a tester's verdict: **“smooth as butter.”**
