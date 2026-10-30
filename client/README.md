<p align="center">
  <img src="docs/assets/banner.png" width="800" alt="Nightjar: low-latency game streaming from your PC">
</p>

<p align="center">
  <a href="https://github.com/RamazanKara/nightjar/releases/latest"><img src="https://img.shields.io/github/v/release/RamazanKara/nightjar?label=release&color=e8a33d" alt="Latest release"></a>
  <a href="https://github.com/RamazanKara/nightjar/releases"><img src="https://img.shields.io/github/downloads/RamazanKara/nightjar/total?color=e8a33d" alt="Total downloads"></a>
  <a href="LICENSE.txt"><img src="https://img.shields.io/github/license/RamazanKara/nightjar?color=e8a33d" alt="License: GPL-3.0"></a>
</p>

# Nightjar

Nightjar is an open-source game-streaming client for Android, Android TV, Fire TV, Windows and Linux. It speaks the Moonlight protocol, so it works with Sunshine, Apollo, Vibepollo, Rubylight, NVIDIA GameStream and other compatible hosts. Nightjar reads what each PC's host reports and enables exactly the features that host supports. It never guesses from the host's name.

Phones, handhelds, tablets, TVs and the desktop client share one streaming engine, written in Rust, and one set of PCs, settings and controls. Nightjar has no accounts, ads or analytics, and sends nothing to its developers.

<p align="center">
  <a id="download" href="https://github.com/RamazanKara/nightjar/releases/latest"><img src="docs/assets/download.svg" width="260" height="52" alt="Download Nightjar"></a>
</p>

<p align="center">
  <img src="docs/assets/shots/tv-home.png" width="820" alt="Nightjar's Home on a TV with a Rubylight 3.0 PC: a game in the hero and a Recently played row of posters with store badges (demo library)">
</p>

<p align="center">
  <img src="docs/assets/shots/handheld-home.png" width="405" alt="Nightjar's Home on a handheld: Continue playing and the Library shelf (demo library)">
  <img src="docs/assets/shots/stream-stats.png" width="405" alt="A stream with the Statistics overlay: resolution, codec, frame rate, bitrate, round trip, decode time and the PC's CPU and GPU readings (demo data)">
</p>

## Features

- **Video:** H.264, HEVC and AV1 through the device's hardware decoder, with low-latency settings tuned per decoder family ([decoder flags](docs/decoder-flags.md)). HDR10 on any host with HDR, plus HDR10+, HLG and Dolby Vision where the PC and the screen support them. Also 10-bit SDR, full-chroma YUV 4:4:4, and PyroWave decoded on the GPU. Upscaling with FSR 1 or Snapdragon GSR, and optional rendering through Vulkan.
- **Timing:** follows the display's refresh rate, including Android's adaptive refresh and VRR on the desktop. Supported hosts get phase-lock reports, so frames arrive just before the display needs them. The desktop client can also beam race ([beam racing](docs/beam-racing.md)).
- **Network:** **Device controls bitrate** adapts to loss, round-trip variation and decode headroom. Bitrate and frame rate can change during a stream, and error correction can be left to the PC, follow measured loss or use a fixed share. **Recover interrupted streams** resumes the running game after a transient drop. Optional tolerance keeps a stream alive through up to 60 seconds of network silence.
- **Audio:** stereo, 5.1, 7.1 and 7.1.4, with low-latency AAudio for stereo. Passthrough sends PCM, Dolby Digital or Dolby Digital Plus to a receiver, soundbar or TV. The microphone gets echo cancellation on Android and the desktop, and noise suppression, gain and an in-stream mute button on Android.
- **Input:** up to 16 controllers with per-model mapping. Direct USB drivers for Xbox, DualSense (adaptive triggers and haptics) and Switch Pro controllers. Trackpad, direct-touch, multi-touch and local-cursor modes, with the PC's own cursor shapes. Native pen, a full PC keyboard, and a soft keyboard that opens when a text field on the PC takes focus. On-screen controls are Android-only.
- **Library:** Home merges the games of all your PCs, with a shelf per store on a TV and a page per game. A sleeping PC can be woken from Home. Nightjar also launches games from Android TV's home screen, Beacon, ES-DE, Daijisho and Pegasus.
- **Sharing:** clipboard sync, clipboard images, file transfer, the PC's shared folders, and USB devices forwarded to the PC.
- **Diagnostics:** an overlay in Compact, Statistics and Debug modes, and a stream health report that explains stutter. An input latency test, a decoder benchmark that runs without a PC, frame timings as CSV, a connection test, and redacted problem reports that leave the device only when you share them.

Features beyond the base protocol depend on the host; see [Hosts](#hosts). The complete list, with a link to each guide, is in [features](docs/features.md).

<p align="center">
  <img src="docs/assets/shots/desktop-home.png" width="600" alt="Nightjar for Windows: Home with a library of games as cover art and the PC below (demo library)">
</p>

## Measured performance

All results are our own measurements against a Rubylight PC with a Radeon RX 7900 XT:

| Device and link | Stream | Result |
| --- | --- | --- |
| Retroid Pocket 6, Wi-Fi | HEVC 1080p60, 40 Mbps | About 7 ms median picture age on either engine (6.92 / 7.04 ms, within measurement error), from the PC's capture timestamp to the frame rendered on the device; a debug build, with ±0.5 ms error in joining the two clocks ([runs](https://github.com/RamazanKara/Rubylight/blob/main/rust/PERFORMANCE.md#october-23-rust-engine-against-the-c-engine-on-android-b8)) |
| Retroid Pocket 6, Wi-Fi | HEVC 1080p120, 28 Mbps | About 120 fps ([4.0.0 notes](https://github.com/RamazanKara/nightjar/releases/tag/v4.0.0)) |
| Retroid Pocket 6, Wi-Fi | PyroWave 1080p120, default bitrate | 119.6 to 120 fps, about 1.7 ms per frame for the decode steps ([4.0.0 notes](https://github.com/RamazanKara/nightjar/releases/tag/v4.0.0)) |

Decode times for more devices are in the [device results](docs/devices.md#devices); the built-in benchmark adds yours.

## Hosts

Every Moonlight-compatible host gets the full stream. That includes discovery, PIN pairing, Wake-on-LAN, the app library, H.264, HEVC and AV1, HDR10, surround sound, controllers, touch, mouse and keyboard, recovery and the diagnostics. Nightjar enables everything else per PC, from the capabilities the host reports:

| Host capability | Examples | What Nightjar adds |
| --- | --- | --- |
| Sunshine protocol extensions | Sunshine and hosts built on it | DualSense adaptive triggers and lightbar, controller motion and touchpads, native touch and pen |
| Device permissions and virtual displays | Apollo, Vibepollo, Rubylight | One-time PIN pairing, per-device permissions, a virtual display at the device's resolution, fractional refresh rates, clipboard and server commands |
| Rubylight extensions | Rubylight 3.0 | Home with games from every store, the Session and graphics menu with Radeon settings, QR pairing, sharing, joining co-op games, HDR10+, HLG and Dolby Vision, 7.1.4 and audio passthrough, bitrate and frame rate changes during a stream, host hardware readings |

Error correction choices and the PC's own automatic bitrate also work with Sunshine-based PCs that accept them, and PyroWave works with any host that offers it. A PC whose host lacks a feature hides it, or names the host setting or permission it needs. [Host feature support](docs/PARITY-MATRIX.md) lists every capability and the hosts known to provide it.

## Install

Download from the [latest release](https://github.com/RamazanKara/nightjar/releases/latest). Each release lists SHA-256 checksums of all its files in `SHA256SUMS-<version>.txt`.

- **Android:** install [`nightjar.apk`](https://github.com/RamazanKara/nightjar/releases/latest/download/nightjar.apk) and allow installing from the app that opened it. Release APKs are signed with the Nightjar release key, so later releases install over it. Nightjar checks GitHub for new releases and asks before installing one. To manage updates with [Obtainium](https://github.com/ImranR98/Obtainium), add `https://github.com/RamazanKara/nightjar`.
- **Android TV and Fire TV:** install AFTVnews' [Downloader](https://www.aftvnews.com/downloader/) from the TV's app store, enter the code **`3815143`** and install the APK it downloads. The code always points to the latest release.
- **Windows:** run `nightjar-desktop-<version>-windows-x86_64-setup.exe`. It installs for the current user, needs no administrator rights, and updates in place, keeping settings and paired PCs. The `.zip` contains the same files and runs without installation.
- **Linux:** install the `.deb` (`sudo apt install ./nightjar-desktop-<version>-linux-x86_64.deb`) or the `.rpm` (`sudo dnf install ./nightjar-desktop-<version>-linux-x86_64.rpm`). On other distributions, run the `.AppImage` or unpack the `.tar.gz`.

Nightjar installs beside Moonlight and other clients and has its own app ID, so you can try it without giving anything up. [Installing the desktop client](protocol/desktop/README.md#installing) covers removal and the portable files.

## Requirements

| Platform | Requirement |
| --- | --- |
| Android phones, tablets, handhelds, Android TV, Fire TV | Android 5.0 or newer. PyroWave needs 64-bit Android 10 or newer with Vulkan 1.3. Audio passthrough needs Android 10 or newer, and 7.1.4 needs Android 12L or newer. |
| Windows | Windows 10 or 11, x86-64. HDR10 needs an HDR display. |
| Linux | x86-64 with glibc 2.39 or newer: Ubuntu 24.04, Debian 13, Fedora 40 or later. Hardware decoding uses VA-API, V4L2 M2M or NVDEC. |
| PC | A Moonlight-compatible host. The host's own requirements apply. |

## Quick start

1. Install a streaming host on your PC, for example [Sunshine](https://github.com/LizardByte/Sunshine), [Apollo](https://github.com/ClassicOldSong/Apollo), [Vibepollo](https://github.com/Nonary/Vibepollo) or [Rubylight](https://github.com/RamazanKara/Rubylight).
2. Open Nightjar on the same network. It finds the PC; if not, add its address under **Add PC**.
3. Select the PC and enter the PIN Nightjar shows in the host's web UI. On a Rubylight PC, **Scan QR code** pairs from the code on its **Devices** page instead.
4. If the host sets per-device permissions, allow this device to list, view and launch apps, and the input it uses.
5. Pick a game.

[Quick start](docs/quick-start.md) and [pairing and permissions](docs/pairing.md) cover each step in detail.

## Documentation

The [documentation index](docs/index.md) lists every guide. The most used pages:

- [Quick start](docs/quick-start.md), [pairing and permissions](docs/pairing.md) and [remote play](docs/remote-setup.md)
- [Settings reference](docs/settings.md) for Android and the desktop client
- [Controls](docs/controls.md), [video and latency](docs/video-and-latency.md) and [clipboard, files and USB devices](docs/sharing.md)
- [Home and the session menu](docs/console.md) for Rubylight PCs
- [Troubleshooting](docs/TROUBLESHOOTING.md), [stream health](docs/stream-health.md) and the [FAQ](docs/faq.md)
- [Android and desktop compared](docs/CLIENT-PARITY.md) and the [desktop client](protocol/desktop/README.md)

## Building

The Android app builds with Gradle and JDK 21; its native code needs the Android NDK and Rust. The desktop client is a Cargo workspace under [`protocol/`](protocol). [Building](docs/building.md) covers the toolchains, signing, the release workflow and the documentation checks. The [desktop client's README](protocol/desktop/README.md#building) covers Windows and Linux builds and packaging.

## Contributing

Report bugs and request features in the [issue tracker](https://github.com/RamazanKara/nightjar/issues/new/choose). [Device results](docs/devices.md) explains how to submit your device's benchmark, and [translations](docs/TRANSLATIONS.md#help-translate) explains how to add or fix a language.

## Privacy

Streams, input, clipboard contents and files travel only between your device and your PC. Nightjar contacts GitHub to check for updates. It asks Moonlight's STUN server (`stun.moonlight-stream.org`) for your network's public address, to check whether your PC is reachable from outside, and on Android uses Moonlight's connection-test server (`android.conntest.moonlight-stream.org`) to diagnose connection failures. Problem reports stay on the device until you share them. Details are in the [privacy policy](docs/PRIVACY.md).

## License and credits

Nightjar is licensed under [GPL-3.0](LICENSE.txt). It is based on Moonlight Android by Cameron Gutman, Diego Waxemberg, Aaron Neyer, Andrew Hennessy and contributors. It also includes work from other GPL-3.0 Moonlight forks and bundled third-party components. [License and credits](docs/index.md#license-and-credits) names each of them and the notices to keep when redistributing.
