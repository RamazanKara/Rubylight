<p align="center">
  <img src="docs/assets/banner.png" width="800" alt="Nightjar: low-latency game streaming from your PC">
</p>

<h3 align="center">Your PC games, on every screen you own.</h3>

<p align="center">Nightjar is a game-streaming client for Android phones, handhelds, tablets and TVs, and for Windows and Linux PCs. One Rust streaming engine drives all of them. It plays with Sunshine, Apollo, Vibepollo, Rubylight, NVIDIA GameStream and every other Moonlight-compatible host, reads what your host can do and lights up every feature it offers. Paired with Rubylight 3.0, your PC becomes a console.</p>

<p align="center">
  <a id="download" href="https://github.com/RamazanKara/nightjar/releases/latest"><img src="docs/assets/download.svg" width="260" height="52" alt="Download Nightjar for Android"></a>
</p>

<p align="center">
  <a href="https://github.com/RamazanKara/nightjar/releases/latest/download/nightjar.apk"><b>Android APK</b></a> ·
  <a href="https://github.com/RamazanKara/nightjar/releases/latest"><b>Windows</b></a> ·
  <a href="https://github.com/RamazanKara/nightjar/releases/latest"><b>Linux</b></a> ·
  <a href="#android-tv-and-fire-tv"><b>Android TV and Fire TV</b></a>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/RamazanKara/nightjar/ef040817e7dc340ed5a874b4c159f49445bc2774/docs/assets/demo.gif" width="360" alt="Nightjar demo: tap a PC to open its game library, tap a game to start your stream, open the stream menu, and switch the overlay to statistics with the host's CPU and GPU readings.">
</p>

<p align="center"><a href="https://ramazankara.github.io/nightjar/#demo">Watch the 69-second tour: PCs and library, PC settings for your host, stream menu, host statistics, PyroWave, adaptive refresh, upscaling, the Snapdragon GPU driver, DualSense and picture-in-picture →</a></p>

<p align="center">
  <img src="docs/assets/shots/console-home.png" width="410" alt="Nightjar's Home on a Retroid Pocket 6: a Continue playing row with Space Sim and Open World, and a Library row of cover art with Racing Game marked Running">
  <img src="docs/assets/shots/stream-stats.png" width="410" alt="A Nightjar stream with the Statistics overlay: 1920 by 1080 AV1 HDR, 119.9 FPS displayed, 38.6 Mbps, 8 ms network round trip, host processing, decode time, and the PC's CPU and GPU load, temperature and power (demo sample data)">
</p>

## Fast where you feel it

- **Near-zero decoding.** A Snapdragon 8 Elite turns each 1080p frame into a picture in about half a millisecond: 0.39 ms for AV1 and 0.55 ms for HEVC on a Galaxy S25 Ultra, measured with Nightjar's built-in decoder benchmark ([device results](docs/devices.md#devices)).
- **About 7 ms from PC to screen** on a Retroid Pocket 6 over Wi-Fi ([runs](https://github.com/RamazanKara/Rubylight/blob/main/rust/PERFORMANCE.md#october-23-rust-engine-against-the-c-engine-on-android-b8)).
- **1080p at 120 fps in 10-bit HDR** with full colour detail, without a dropped frame.

## What sets Nightjar apart

### One engine, every screen

Nightjar's streaming engine is written in Rust and runs on Android, Android TV, Windows and Linux, and Home, your PCs, your settings and the session menu come along to every one of them. The desktop client brings hardware decoding, VRR pacing, controllers, keyboard and mouse, and its own FFmpeg, so the Linux build runs across distributions, from Ubuntu to Arch. [Desktop client](protocol/desktop/README.md).

<p align="center">
  <img src="docs/assets/shots/desktop-home.png" width="600" alt="Nightjar's desktop client on Windows: the Home screen with a library of ten games as cover art, and the Living-room PC below">
</p>

### Your PC becomes a console

With a Rubylight 3.0 host, Nightjar opens on Home: favourites, Continue playing, your library across every paired PC with covers, and your PCs. On a TV, Home is built from shelves with a large hero for the focused game, your games are grouped by store (Steam, Epic, GOG and more) with a badge on every poster, and **All games** filters the whole library by store. A sleeping PC's games stay on Home: pick one and Nightjar wakes the PC, waits for it and starts the game. A game's page shows where it runs and which stream settings it will use, then Play or Resume.

While you play, the stream menu's **Session and graphics** page has Resume, Disconnect, End session and Close game, the connection facts, and a **Radeon panel** for the host's AMD driver features such as Anti-Lag and Radeon Chill. The host puts every setting back when the session ends. [Home and the session menu](docs/console.md).

<p align="center">
  <img src="docs/assets/shots/console-pcs.png" width="410" alt="Nightjar's PCs screen: Living-room PC playing Racing Game, Studio PC not paired, Laptop offline, and an Add PC button">
  <img src="docs/assets/shots/console-game.png" width="410" alt="A game's page in Nightjar: cover art for Puzzle Night, the host it runs on, and Play and Add to favourites buttons">
</p>

### Latency engineered at every stage

- **Decoders tuned per chip.** Low-latency keys for Snapdragon, Tegra, MediaTek, Exynos, Google Pixel and Amlogic decoders, with the applied keys listed in the stream log ([decoder flags](docs/decoder-flags.md)).
- **Frames timed to your display.** Pacing follows Android's adaptive refresh and the game's content rate, and on Rubylight Nightjar tells the host how long frames wait for your screen so they arrive just in time.
- **The network, told the truth.** Nightjar tells a Rubylight PC your wired link speed, or that you are on Wi-Fi, so the PC paces the stream for your link from the very first frame.
- **Beam racing on the desktop.** A PyroWave picture is decoded in slices while the rest of it is still arriving, and the top is drawn before the bottom has left the network.
- **Low-latency audio** through AAudio for stereo, plus 5.1 and 7.1 surround.

### PyroWave on the GPU

Nightjar decodes PyroWave, the low-latency wavelet codec, on the GPU: through Vulkan on Android, and on Windows and Linux in the desktop client. It carries full-colour 4:4:4 and HDR, early decode starts on blocks while a frame is still arriving, and on Snapdragon phones you can load a newer Adreno Vulkan driver for it. PyroWave runs with Rubylight and with Vibepollo.

### A picture worth the bandwidth

- AV1, HEVC and H.264 with **HDR10**, **10-bit SDR** that smooths banding on any screen, and **full chroma** (4:4:4), HDR included, for sharp coloured text.
- **Upscaling on the device** with FSR 1 or Snapdragon GSR: send fewer pixels, see a sharp picture.
- **Automatic bitrate** follows loss, jitter and your decoder's headroom, up to 2000 Mbps, and **automatic quality** changes resolution and frame rate mid-stream for smooth motion, sharp detail or battery life. Both watch your device's temperature and back off before it throttles.
- **Render through Vulkan** so frame-generation overlays can hook the video.

### You see everything

A one-line overlay in any corner shows frame rate, round trip, loss and a health dot. Statistics and Debug views break the delay into stages, and with Rubylight or Vibepollo they add the PC's CPU and GPU usage, temperatures, power, clocks and VRAM. **Why did my stream stutter?** names the likely cause, whether network, decoder, host, display or heat, and can run a reversible bitrate test. A built-in test measures input-to-screen latency, a decoder benchmark runs without any host, and frame timings export as CSV.

### Every way to play

- **A full PC keyboard over the stream** with Esc, F1 to F12, arrows and sticky Ctrl, Alt, Win and Shift, driven by touch, controller or TV remote.
- **DualSense over USB** with rumble, Edge paddles, adaptive triggers, the lightbar, waveform haptics and its speaker, headset and microphone. Built-in Xbox and Switch Pro USB drivers, per-controller mappings and controllers that keep their identity across reconnects.
- **Per-game touch controls**: keys, buttons, sticks, triggers and a D-pad, shared as files and imported from Artemis ([controls](docs/controls.md)).
- Trackpad with acceleration and glide, a local cursor, pen, mouse and keyboard, and three- and four-finger shortcuts.
- **Voice chat** from your phone's microphone to the PC, muted from the stream menu.

<p align="center">
  <img src="docs/assets/shots/stream-keyboard.png" width="600" alt="Nightjar's on-screen PC keyboard over a stream: Esc, F1 to F12, a full letter block, modifier keys and arrows, in the English (US) layout">
</p>

### Your PC, your way

Give the stream its own virtual display at your device's exact resolution, and choose whether your monitors stay on, switch off or sit beside it. Watch a PC without taking its input with Remote Monitor, or drive it without video with Input-only. Copy text both ways, run the host's commands, pair with a one-time PIN and open your host's web UI from the app.

### Anywhere, and all yours

- **Play away from home** with a built-in guide that sets up a private Tailscale connection and tests your PC from wherever you are.
- **Network profiles** that pick stream settings by network, a mobile data cap, and streams that ride out network drops of up to 60 seconds.
- **PIN lock** for the app, every stream or the PCs you pick, typed with touch, a gamepad or a TV remote.
- **Your library in your launcher**: Beacon, ES-DE, Daijisho and Pegasus, kept up to date with one tap ([launchers](docs/FRONTENDS.md)).
- Picture-in-picture, an external display with the phone as touchpad, System, Dark and OLED black themes, interface size, high contrast and reduced motion.
- Built-in help that works offline, 15 languages, and no ads, analytics or accounts.

Nightjar reads what each host reports and shows exactly the features it offers. The [host feature matrix](docs/PARITY-MATRIX.md) lists them per host.

## Get started

1. Run a streaming host on your PC, such as [Rubylight](https://github.com/RamazanKara/Rubylight), [Apollo](https://github.com/ClassicOldSong/Apollo), [Sunshine](https://github.com/LizardByte/Sunshine) or [Vibepollo](https://github.com/Nonary/Vibepollo), and add your games.
2. Install Nightjar:
   - **Android** (5.0 and newer): [download the APK](https://github.com/RamazanKara/nightjar/releases/latest/download/nightjar.apk), open it and allow the install. PyroWave runs on 64-bit Android 10 and newer with a Vulkan 1.3 GPU ([video requirements](docs/video-and-latency.md#codecs)).
   - **Windows**: download `nightjar-desktop-<version>-windows-x86_64.zip` from the [latest release](https://github.com/RamazanKara/nightjar/releases/latest), unzip it and run `nightjar.exe`.
   - **Linux**: download `nightjar-desktop-<version>-linux-x86_64.tar.gz` from the [latest release](https://github.com/RamazanKara/nightjar/releases/latest), unpack it and run `./nightjar`.
3. Pick your PC on the network and enter the PIN Nightjar shows in your host's web UI.
4. If your host has per-device permissions, allow your device to list, view and launch apps and the input types you use. Then pick a game. With a Rubylight 3.0 host, Nightjar opens on [Home](docs/console.md); turn on Host control for your device to close games and change Radeon settings. [Pairing and permissions](docs/pairing.md).

### Android TV and Fire TV

Install AFTVnews' [Downloader](https://www.aftvnews.com/downloader/) from your TV's app store, enter the code **`3815143`** in its URL field and install the APK it downloads, or type the link `https://github.com/RamazanKara/nightjar/releases/latest/download/nightjar.apk`. Both always fetch the latest release. Allow Downloader to install apps when your TV asks.

Coming from Rubylight Android? Nightjar is the same app under its new name. It installs over Rubylight Android 0.5.0 and keeps your PCs, pairings and settings.

## Docs

[Start here](docs/index.md) · [Quick start](docs/quick-start.md) · [Pairing](docs/pairing.md) · [Settings](docs/settings.md) · [Controls](docs/controls.md)<br>
[Home and session menu](docs/console.md) · [Desktop client](protocol/desktop/README.md) · [Beam racing](docs/beam-racing.md) · [Video & latency](docs/video-and-latency.md) · [Troubleshooting](docs/TROUBLESHOOTING.md) · [Building](docs/building.md) · [Frontends](docs/FRONTENDS.md) · [Translations](docs/TRANSLATIONS.md) · [FAQ](docs/faq.md)

[GPL-3.0](LICENSE.txt) · [Credits and third-party notices](docs/index.md#license-and-credits)
