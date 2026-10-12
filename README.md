Still in Alpha. Expect Bugs to happen

<p align="center">
  <img src="docs/assets/banner.png" width="800" alt="RUBYLIGHT — Your games. Your screen. Your stream.">
</p>

<p align="center">Stream your PC games and desktop to Android, with ultra-low latency and the controls you love.</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/RamazanKara/rubylight-android/2836f32aced578cd9d0a0ad1bcae8373e8e32ebb/docs/assets/demo.gif" width="360" alt="Rubylight demo: open your game library, tap a game to start your stream, keep the performance overlay in view, and keep the stream floating in picture-in-picture.">
</p>

<p align="center"><a href="https://ramazankara.github.io/rubylight-android/#demo">Watch the full demo →</a></p>

<p align="center">
  <a id="download" href="https://github.com/RamazanKara/rubylight-android/releases/latest"><img src="docs/assets/download.svg" width="260" height="52" alt="Download Rubylight for Android"></a>
</p>

<p align="center">
  <img src="docs/assets/shots/pc-list.png" width="160" alt="Computers list showing online, offline and unpaired PCs">
  <img src="docs/assets/shots/library.png" width="160" alt="Game library with cover art and a running game">
  <img src="https://raw.githubusercontent.com/RamazanKara/rubylight-android/2836f32aced578cd9d0a0ad1bcae8373e8e32ebb/docs/assets/shots/stream.png" width="160" alt="Close-up of the stream with the compact one-line performance overlay">
  <img src="docs/assets/shots/pip.png" width="160" alt="The stream floating over the home screen in a picture-in-picture window">
</p>

## Features

- **React instantly** with ultra-low latency streaming and decoding.
- **See every detail** with AV1, HEVC and H.264 video, plus HDR10.
- **Keep motion fluid** with high refresh rates and VRR frame pacing.
- **Sharpen your stream** with client-side FSR and Snapdragon GSR upscaling.
- **Play your way** with USB and Bluetooth controllers, including DualSense.
- **Stay in control** with touch, trackpad, mouse, keyboard and [on-screen keys and desktop shortcuts](docs/controls.md).
- **Choose your PC display** with per-PC exclusive, extended and separated display layouts on compatible hosts. Separated displays share the same Windows session. See the [host feature matrix](docs/PARITY-MATRIX.md).
- **Choose a Snapdragon GPU driver** for PyroWave with a compatible driver ZIP and system-driver fallback. See [driver requirements](docs/video-and-latency.md).
- **Know your stream** with Artemis-style Statistics and detailed Debug overlays for frame rates, video throughput, network loss and measured latency stages.
- **Get playing sooner** with automatic PC discovery and Wake-on-LAN.

## Quick start

Requires **Android 5.0 or newer**. PyroWave requires Android 10+, a 64-bit device and a compatible Vulkan 1.3 GPU; see [video requirements](docs/video-and-latency.md#codecs).

1. **Set up your PC:** start the [Rubylight host](https://github.com/RamazanKara/Rubylight) and add your games.
2. **Install on Android:** [download the APK](https://github.com/RamazanKara/rubylight-android/releases/latest), open it and allow installation from your browser or file manager when prompted.
3. **Connect and pair:** select your PC on the same network and enter the PIN shown on your phone in the host's **Devices** page.
4. **Start your stream:** in the host's **Devices** page, enable your device and allow application listing, viewing, launching and the input types you use. Open your library on Android and tap a game. See [pairing and permissions](docs/pairing.md).

Use [Rubylight Windows 2.2.5 or newer](https://github.com/RamazanKara/Rubylight/releases/latest) for the automatic-bitrate improvements in Android 0.4.3.

## Docs

[Start here](docs/index.md) · [Quick start](docs/quick-start.md) · [Pairing](docs/pairing.md) · [Settings](docs/settings.md) · [Controls](docs/controls.md)<br>
[Video & latency](docs/video-and-latency.md) · [Troubleshooting](docs/TROUBLESHOOTING.md) · [Building](docs/building.md) · [Frontends](docs/FRONTENDS.md) · [FAQ](docs/faq.md)

[GPL-3.0](LICENSE.txt) · [Credits and third-party notices](docs/index.md#license-and-credits)
