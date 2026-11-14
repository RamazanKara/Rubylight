<p align="center">
  <img src="docs/assets/banner.png" width="800" alt="Nightjar: low-latency game streaming from your PC">
</p>

<h3 align="center">Every PC. Every game. Every screen.</h3>

<p align="center">
  Stream your PC to a phone, handheld, TV or laptop with the picture, sound and controls of playing at your desk.<br>
  Free and open source. No accounts, no ads, no tracking.
</p>

<p align="center">
  <a href="https://github.com/RamazanKara/nightjar/releases/latest"><img src="https://img.shields.io/github/v/release/RamazanKara/nightjar?label=release&color=e8a33d" alt="Latest release"></a>
  <a href="https://github.com/RamazanKara/nightjar/releases"><img src="https://img.shields.io/github/downloads/RamazanKara/nightjar/total?color=e8a33d" alt="Total downloads"></a>
  <a href="LICENSE.txt"><img src="https://img.shields.io/github/license/RamazanKara/nightjar?color=e8a33d" alt="License: GPL-3.0"></a>
</p>

<p align="center">
  <a id="download" href="https://github.com/RamazanKara/nightjar/releases/latest"><img src="docs/assets/download.svg" width="260" height="52" alt="Download Nightjar"></a>
</p>

<p align="center">
  <b>Android</b> · <b>Android TV</b> · <b>Fire TV</b> · <b>Windows</b> · <b>Linux</b> · <b>macOS</b><br>
  <sub>Works with Sunshine, Apollo, Vibepollo, Rubylight, NVIDIA GameStream and every other Moonlight-compatible host.</sub>
</p>

<p align="center">
  <a href="https://ramazankara.github.io/nightjar/assets/trailer-16x9.mp4"><img src="docs/assets/nightjar-trailer.webp" width="880" alt="Nightjar trailer: Home on a TV, its glass tinted by each focused cover, a game's page, a handheld stream with the performance overlay, the stream menu, Radeon settings and the PC keyboard, picture-in-picture on a phone, the feature list and the supported platforms (offline demo library)"></a><br>
  <sub><a href="https://ramazankara.github.io/nightjar/assets/trailer-16x9.mp4"><b>▶ Watch the full trailer</b></a></sub>
</p>

## Why Nightjar

Nightjar speaks the Moonlight protocol, so it works with the host you already run, and every host gets the complete stream: hardware decoding, HDR10, surround sound, every controller, touch, mouse and keyboard, recovery and diagnostics. It turns on more from the capabilities each PC's host reports, never from the host's name.

### One library for every PC

Home gathers the games of every paired PC, with cover art, **Continue playing** and a page for each game that shows its store, the PC it runs on, whether it is running and the exact settings the stream will use. On a TV, each store gets its own shelf. Pick a game on a sleeping PC and Nightjar wakes the PC and starts it.

Store shelves, Steam collections and Playnite categories and the richer game pages come from a Rubylight PC; with any other Moonlight host, Home shows the PC's app list. Games from any host also launch from Android TV's home screen, Beacon, ES-DE, Daijisho, Pegasus and the desktop command line, which takes moonlight-qt's `stream` options too, so launchers such as MoonDeck keep working.

<table>
<tr>
<td width="50%" valign="top"><img src="docs/assets/shots/tv-game-page.png" alt="A game's page on a TV, lit by the cover's sunset colours: Racing Game running on Living-room PC, with Resume, Add to favourites and Quit game, the stream settings it will use and the switch to quit the game on leaving (demo library)"><br><sub><b>A page for every game.</b> Its store, the PC it runs on, whether it is running, and the exact settings the stream will use.</sub></td>
<td width="50%" valign="top"><img src="docs/assets/shots/tv-all-games.png" alt="All games on a TV under a starry night sky, filtered by store: All, Installed, Games, Steam, Epic Games, GOG and Xbox, with posters (demo library)"><br><sub><b>Every store in one place.</b> Steam, Epic Games, GOG, EA app, Ubisoft Connect, Battle.net and Xbox, filtered with one press.</sub></td>
</tr>
</table>

### At home on every screen

Every screen works with a controller or a TV remote as well as touch, mouse and keyboard. On a handheld, Home, the library and the stream menu follow the controller, with the button for each action on screen. Android adds picture-in-picture, rotation and foldables; the desktop app runs natively on Windows (x86-64 and ARM64), Linux, the Steam Deck in Game Mode and macOS, with the same engine and settings.

Phones, TVs and computers share one settings layout with search and presets. Settings can differ per PC and per game, with profiles that switch by network: mobile data, home Wi-Fi or other Wi-Fi. Pick System, Dark or OLED black. Nightjar comes in 15 languages, with high contrast, reduced motion and screen reader support.

<table>
<tr>
<td width="50%" valign="top"><img src="docs/assets/shots/handheld-home.png" alt="Home on a handheld: Racing Game running, with Resume, Favourite and Quit game, a Continue playing list and the Recently played row, under a glass top bar (demo library)"><br><sub><b>Made for handhelds.</b> Every action has its button on screen.</sub></td>
<td width="50%" valign="top"><img src="docs/assets/shots/desktop-home.png" alt="Nightjar for Windows: Home tinted by Racing Game's cover, the game in progress with Resume, Favourite and Quit game, a Continue list and a row of cover art per store, under a top bar with Home, PCs, Settings and Help (demo library)"><br><sub><b>On the desktop too.</b> Windows, Linux and macOS, with a row per store on Home. <a href="protocol/desktop/README.md#using-nightjar">More of the desktop app</a></sub></td>
</tr>
</table>

### Smooth, and it stays in the game

Hardware decoding is tuned for [each decoder family](docs/decoder-flags.md), and frames are paced to your display's refresh rate, including Android's adaptive refresh rate and VRR on the desktop. Hosts that support it receive phase-lock reports, so frames arrive just before the display needs them, and the desktop client can [beam race](docs/beam-racing.md).

- **Stays in the game.** The bitrate drops when the network is busy, following loss, round-trip variation and decode headroom, and error correction can follow measured loss, with hosts that accept it. An interrupted stream recovers into the running game, and a stream holds through up to a minute of network silence. With a Rubylight PC, the stream follows your device to a new network.
- **Real VRR,** on by default with Variable refresh rate: on a G-Sync, FreeSync or HDMI VRR screen, frames land on the game's own rhythm instead of the network's. [How it works](docs/real-vrr.md)
- **Frame Warp,** with a Rubylight PC: a mouse or right-stick camera turn shows on your screen's next refresh, and the game's HUD stays in place. [How it works](docs/protocol/frame-warp.md)

### The picture and sound your setup can play

H.264, HEVC and AV1 on the hardware decoder, and the PyroWave codec decoded on the GPU. HDR10 with any HDR-capable host; HDR10+, HLG and Dolby Vision where the PC, decoder and display support them, and on Linux HDR straight to the desktop. Full-chroma 4:4:4 keeps small text sharp, and Nightjar checks before a stream starts whether the PC and the device can do it. Upscaling uses FSR 1, Snapdragon GSR or, on Windows, the graphics card's video upscaler.

Sound comes in stereo, 5.1, 7.1 and 7.1.4, or as PCM, Dolby Digital or Dolby Digital Plus passed through to a receiver, soundbar or TV. The microphone has echo cancellation on every platform, plus noise suppression, gain and one-tap mute on Android.

### Every controller, touch and pen

Up to 16 controllers, with mapping per model. Sunshine-based PCs add DualSense adaptive triggers, motion and touchpads, and on Android Nightjar's own USB drivers handle the DualSense's lightbar and haptics, Xbox and Switch Pro controllers. Touch works as a trackpad, directly, as multi-touch or with a local cursor, with pinch zoom, and pens send pressure and tilt; a laptop's touchpad gives the PC its own touchpad gestures on PCs that take them.

- **A real PC keyboard.** Esc, F1 to F12, arrows and sticky modifiers, by touch, controller or TV remote, and a soft keyboard that opens when a text field on the PC takes focus.
- **Your own hotkeys.** Every stream hotkey can be changed or turned off, your keyboard shortcuts become stream-menu tiles, and controller chords open the menu.
- **On-screen and input-only.** On Android, an on-screen pad can be its own player. An **Input-only** stream sends controls and feedback without the video, to any PC.

<p align="center"><img src="docs/assets/shots/stream-keyboard.png" width="720" alt="A handheld stream with Nightjar's PC keyboard open: Esc, F1 to F12, arrows, navigation keys and modifiers, with G focused for a controller (demo data)"></p>

### Tune it while you play

The stream menu changes the bitrate and frame rate during a stream, with hosts that accept it. With a Rubylight PC it does much more: the PC's Radeon or NVIDIA graphics settings, **Continue here** to move a running game to the device in your hands, **Stream here too**, a second screen for dual-screen devices, a free slot in a co-op game, **Microphone only**, and the PC's power and update controls.

With Apollo, Vibepollo and Rubylight, clipboard text travels to the PC. With a Rubylight PC, the clipboard syncs both ways, with images, and files, the PC's shared folders and USB devices plugged into your device reach the PC.

<p align="center"><img src="docs/assets/shots/stream-menu-radeon.png" width="720" alt="The Session and graphics panel in glass beside the stream: the Wi-Fi link and the PC's Radeon settings, Anti-Lag, Chill, its maximum FPS and Frame Rate Target Control (demo data)"></p>

### Know why a stream stuttered

A performance overlay in three levels of detail shows the network and decoding, and the PC's CPU and GPU readings from a host that sends them. A [stream health](docs/stream-health.md) report explains stutter in plain words, and an input latency test, a decoder benchmark that runs without a PC, frame timings as CSV, a network speed test and redacted problem reports help find the rest.

<p align="center"><img src="docs/assets/shots/stream-stats.png" width="720" alt="A handheld stream with the Statistics overlay: resolution, codec, frame rate, bitrate, round trip, decode time and the PC's CPU and GPU readings (demo data)"></p>

### Play away from home

**Play away from home** in a PC's settings walks you through connecting this device and the PC over a private VPN such as Tailscale, then tests the PC from where you are. Nightjar also tries the PC's IPv6 address and a forwarded port, and with a Rubylight PC that offers it, a direct path both sides punch through their routers, without port forwarding. [Play away from home](docs/remote-setup.md)

### Private by design

No accounts, advertising or analytics, and nothing sent to its developers. Video, audio, input, clipboard and files travel only between your device and your PC. Licensed under the GPL-3.0 and developed in the open.

<sub>Screenshots and the trailer use Nightjar's offline demo with a sample library and an original test animation; the performance numbers shown in them are illustrations. Measured results are [below](#measured-performance).</sub>

The [feature overview](docs/features.md) lists everything, with a link to each guide, and [each release](https://github.com/RamazanKara/nightjar/releases) says what changed.

## What players are saying

<table width="100%">
  <tr>
    <td width="50%" align="center" valign="top"><p><em>“The inclusion of support for high polling mice and keyboard, the smoothness of the VRR, the reduced latency overall; I'm absolutely floored by how good it's turned out!”</em></p><p><sub><strong>— /u/flyingk</strong></sub></p></td>
    <td width="50%" align="center" valign="top"><p><em>“Nightjar is currently my favourite Android client. I really love its modern, clean UI. It feels great to use!”</em></p><p><sub><strong>— daniel7277</strong></sub></p></td>
  </tr>
  <tr>
    <td width="50%" align="center" valign="top"><p><em>“The integrated library, session controls, and host-aware options make it feel like a complete console experience.”</em></p><p><sub><strong>— tylerlane</strong></sub></p></td>
    <td width="50%" align="center" valign="top"><p><em>“thank you for creating such a beautiful, clean Android client.”</em></p><p><sub><strong>— rishisinghawadhiya</strong></sub></p></td>
  </tr>
  <tr>
    <td width="50%" align="center" valign="top"><p><em>“I really like your modern on-screen controller design.”</em></p><p><sub><strong>— magicwand9099</strong></sub></p></td>
    <td width="50%" align="center" valign="top"><p><em>“the vrr works wonders”</em></p><p><sub><strong>— u/Potential_Sorbet1809</strong></sub></p></td>
  </tr>
</table>

<details>
<summary><strong>More feedback from players</strong></summary>

- *“Played for about 30 minutes, worked great, no issues.”* — **elkinjosetm**
- *“I believe Nightjar is currently the superior client for Android.”* — **jun182**
- *“It looks absolutely beautiful”* — **daniel7277**
- *“feels absolutely amazing! It's so satisfying.”* — **daniel7277**
- *“I love nightjar”* — **ASE236-lang**
- *“the telemetry/export tools make this kind of investigation much easier.”* — **VirtualGIT20**
- *“its working perfectly right now.”* — **leonardo7999-byte**
- *“I have been using nightjar and reporting issues to the developer! It has been great!”* — **u/ssjucrono**
- *“much more plug and play and easier to understand.”* — **u/wolf0x**

</details>

## Install

Download from the [latest release](https://github.com/RamazanKara/nightjar/releases/latest). Every release publishes the SHA-256 of each file in `SHA256SUMS-<version>.txt`.

| Platform | Package | How to install |
| --- | --- | --- |
| Android phones, tablets and handhelds | [`nightjar.apk`](https://github.com/RamazanKara/nightjar/releases/latest/download/nightjar.apk) | Open the APK and allow installation from the app that opened it. To receive updates through [Obtainium](https://github.com/ImranR98/Obtainium), add `https://github.com/RamazanKara/nightjar`. |
| Android TV and Fire TV | The same APK | Install AFTVnews' [Downloader](https://www.aftvnews.com/downloader/) from the TV's app store, enter the code **`3815143`** and install the APK it downloads. The code always points to the latest release. |
| macOS 13 or later | `nightjar-desktop-<version>-macos-arm64.zip` (Apple Silicon), `…-macos-x86_64.zip` (Intel) | Unpack it, move `Nightjar.app` to Applications and [allow it to open](protocol/desktop/README.md#install) on first start. |
| Windows 10 and 11 | `nightjar-desktop-<version>-windows-x86_64-setup.exe`, or `…-windows-arm64-setup.exe` on ARM64 PCs such as Snapdragon X laptops | Run the installer. It installs for the current user without administrator rights and updates in place, keeping settings and paired PCs. The `.zip` contains the same files and runs without installation. |
| Debian, Ubuntu | `nightjar-desktop-<version>-linux-x86_64.deb` | `sudo apt install ./nightjar-desktop-<version>-linux-x86_64.deb` |
| Fedora | `nightjar-desktop-<version>-linux-x86_64.rpm` | `sudo dnf install ./nightjar-desktop-<version>-linux-x86_64.rpm` |
| Other Linux distributions | `.AppImage` or `.tar.gz` | Make the AppImage executable and run it, or unpack the archive and run `./nightjar`. |
| Steam Deck and other Linux handhelds | `nightjar-desktop-<version>-x86_64.flatpak` | Install it with `flatpak install --user`; [Steam Deck](docs/steam-deck.md) covers Game Mode and HDR. |
| Linux on ARM handhelds and boards | The same packages and the Flatpak, with `aarch64` in the name | As above. |

Release APKs are signed with the Nightjar release key (certificate SHA-256 `312ed322c7c29d1aec4fae4a3979ebcb557af44d821d7f4746c0968548eb630b`), so every later release installs over the previous one. Outside Google Play, the Android app checks GitHub for new releases and asks before installing one. The Windows installer and the AppImage check once per launch and update with one click, keeping settings and paired PCs; a portable Windows copy links to the release page.

Nightjar has its own application ID and pairs separately, so it installs beside other Moonlight clients without affecting them. [Installing the desktop client](protocol/desktop/README.md#installing) covers removal, signing and the location of settings.

> [!NOTE]
> ### Testers wanted: help bring Nightjar to Google Play
>
> Nightjar is getting ready for Google Play, and Google asks for one thing first: at least 12 testers who keep the app for 14 days. If you stream to an Android phone, tablet, handheld or TV, you can be one of them, see each new build before anyone else and help put Nightjar in the store.
>
> - **What you need:** an Android device with the Play Store and a PC running a Moonlight-compatible host.
> - **What you do:** keep Nightjar installed for 14 days, play the way you always do, and tell us what breaks in [issues](https://github.com/RamazanKara/nightjar/issues).
> - **What it costs:** nothing. Testers get a free code for Nightjar on Google Play. If Google Play charges you anyway, you get the full amount back.
> - **How to join:** email [ramazan.kara.fo@gmail.com](mailto:ramazan.kara.fo@gmail.com?subject=Nightjar%20tester) with the subject "Nightjar tester" and the Google account you use on Google Play. You get the invite link and your code by email.
>
> The download here on GitHub stays free and gets the same updates.

## Getting started

1. Install a Moonlight-compatible host on the PC, such as [Sunshine](https://github.com/LizardByte/Sunshine), [Apollo](https://github.com/ClassicOldSong/Apollo), [Vibepollo](https://github.com/Nonary/Vibepollo) or [Rubylight](https://github.com/RamazanKara/Rubylight).
2. Open Nightjar on the same network. It lists the PCs it finds; otherwise choose **Add PC** and enter the PC's address.
3. Select the PC and enter the PIN that Nightjar shows in the host's web UI. With a Rubylight PC you can instead choose **Scan QR code** and scan the code on its **Devices** page.
4. If the host grants permissions per device, allow this device to list, view and launch apps, and the input types you use.
5. Pick a game and play.

The [quick start](docs/quick-start.md) and [pairing guide](docs/pairing.md) cover each step, including one-time PINs and Wake-on-LAN. To play outside your home network, see [Play away from home](docs/remote-setup.md).

## Host compatibility

Discovery, PIN pairing, Wake-on-LAN, the app library, H.264, HEVC and AV1, HDR10, surround sound, all input types, stream recovery and diagnostics work with every Moonlight-compatible host. Further features depend on what the host reports:

| Host capability | Provided by | What Nightjar adds |
| --- | --- | --- |
| Sunshine protocol extensions | Sunshine and hosts built on it | DualSense adaptive triggers and lightbar, controller motion and touchpads, native touch and pen |
| Device permissions and virtual displays | Apollo, Vibepollo, Rubylight | One-time PIN pairing, per-device permissions, a virtual display at the device's resolution, fractional refresh rates, clipboard transfer and server commands |
| Rubylight native API and stream extensions | [Rubylight](https://ramazankara.github.io/Rubylight/) 3.0 and later | Home with games from every store, the Session and graphics menu with Radeon settings, QR pairing, clipboard sync, files, shared folders and USB forwarding, co-op sessions, HDR10+, HLG and Dolby Vision, 7.1.4 and audio passthrough, bitrate and frame-rate changes during a stream, host hardware readings |

The **Error correction** setting and host-side automatic bitrate also work with Sunshine-based PCs that support them, and PyroWave works with any host that offers it. When a PC's host lacks a feature, Nightjar hides it or names the host setting or permission it requires. [Host feature support](docs/PARITY-MATRIX.md) lists every capability and the hosts known to provide it.

## Requirements

| Platform | Requirement |
| --- | --- |
| Android, Android TV, Fire TV | Android 5.0 or newer. PyroWave needs 64-bit Android 10 or newer with Vulkan 1.1. Audio passthrough needs Android 10 or newer; 7.1.4 needs Android 12L or newer and a 12-channel output. |
| macOS | macOS 13 or later, on Apple Silicon or Intel. |
| Windows | Windows 10 or 11, x86-64 or ARM64 (a native build for each). HDR needs an HDR display with HDR turned on in Windows. PyroWave GPU decoding needs Vulkan 1.3. |
| Linux | x86-64 with glibc 2.35 or newer: Ubuntu 22.04, Debian 12, Fedora 36 or later; aarch64 packages for ARM handhelds and boards. Hardware decoding through VA-API, Vulkan video, V4L2 M2M or NVDEC. |
| PC | A Moonlight-compatible host. The host's own requirements apply. |

## Measured performance

Our own measurements, against a Rubylight PC with a Radeon RX 7900 XT:

| Device and link | Stream | Result |
| --- | --- | --- |
| Retroid Pocket 6, Wi-Fi | HEVC 1080p60, 40 Mbps | 7 ms median picture age, from the PC's capture timestamp to the frame rendered on the device, with either streaming engine (6.92 and 7.04 ms; clock join error about ±0.5 ms; [method and runs](https://github.com/RamazanKara/Rubylight/blob/main/rust/PERFORMANCE.md#october-23-rust-engine-against-the-c-engine-on-android-b8)) |
| Retroid Pocket 6, Wi-Fi | HEVC 1080p120, 28 Mbps | About 120 fps ([4.0.0 release notes](https://github.com/RamazanKara/nightjar/releases/tag/v4.0.0)) |
| Retroid Pocket 6, Wi-Fi | PyroWave 1080p120, default bitrate | 119.6 to 120 fps ([4.0.0 release notes](https://github.com/RamazanKara/nightjar/releases/tag/v4.0.0)) |
| Retroid Pocket 6, Wi-Fi | PyroWave 1080p60, 400 Mbps | 3.0 ms median from a frame's last packet to its decoded picture, p95 3.4 ms (was 6.0 ms; [4.2.0 release notes](https://github.com/RamazanKara/nightjar/releases/tag/v4.2.0)) |
| Retroid Pocket 6, Wi-Fi | HEVC 1080p60, 150 Mbps | No frames lost in four 60 s runs with Automatic error correction, against 14 over six runs before its floor ([method and runs](https://github.com/RamazanKara/Rubylight/blob/main/rust/PERFORMANCE.md#november-1-client-error-correction-on-real-wi-fi-retroid)) |
| Desktop PC, 5 % packet loss | HEVC 1080p60, 150 Mbps | 3.39 ms median from a frame's arrival to the screen (was 3.66 ms), 17 % less CPU ([4.2.0 release notes](https://github.com/RamazanKara/nightjar/releases/tag/v4.2.0)) |

Decoder benchmark results for other devices are in [device results](docs/devices.md#devices); the built-in benchmark measures yours.

## Documentation

The [documentation index](docs/index.md) lists every page. The most used:

| Topic | Pages |
| --- | --- |
| Setup | [Quick start](docs/quick-start.md), [pairing and permissions](docs/pairing.md), [play away from home](docs/remote-setup.md) |
| Daily use | [Home and the session menu](docs/console.md), [controls](docs/controls.md), [clipboard, files and USB devices](docs/sharing.md) |
| Tuning | [Video and latency](docs/video-and-latency.md), [recovery and automatic quality](docs/recovery-and-quality.md), [settings reference](docs/settings.md) |
| Problems | [Troubleshooting](docs/TROUBLESHOOTING.md), [stream health](docs/stream-health.md), [FAQ](docs/faq.md) |
| Desktop | [Nightjar for macOS, Windows and Linux](protocol/desktop/README.md), [Android and desktop compared](docs/CLIENT-PARITY.md) |

## Building from source

The [macOS guide](protocol/desktop/README.md#macos) covers installing the release download, running the Rust client and packaging Nightjar.app yourself.

The Android app builds with Gradle and JDK 21; its native code needs the Android NDK and Rust. The streaming engine and the desktop client are a Cargo workspace under [`protocol/`](protocol), shared by both apps.

- [Building Nightjar](docs/building.md): toolchains, tests, signing, the release workflow and the documentation checks.
- [Building the desktop client](protocol/desktop/README.md#building): Windows and Linux builds and packaging.

## Contributing

Report bugs and request features in the [issue tracker](https://github.com/RamazanKara/nightjar/issues/new/choose). [Device results](docs/devices.md) explains how to submit your device's benchmark, and [translations](docs/TRANSLATIONS.md#help-translate) explains how to correct or add a language. Nightjar ships in 15 languages on Android, Windows, Linux and macOS.

## Privacy

Video, audio, input, clipboard contents and files travel only between your device and your PC. Nightjar contacts GitHub to check for updates (the Google Play build updates through Play instead) and Moonlight's STUN server (`stun.moonlight-stream.org`) to learn the network's public address, so it can tell whether the PC is reachable from outside. On Android it also uses Moonlight's connection-test server (`android.conntest.moonlight-stream.org`) to diagnose connection failures. Problem reports stay on the device until you share them. The [privacy policy](docs/PRIVACY.md) has the details.

## License and credits

Nightjar is licensed under the [GNU General Public License v3.0](LICENSE.txt). Everyone who helped, with their GitHub names, is in [CONTRIBUTORS.md](CONTRIBUTORS.md).

Nightjar is a fork of [Moonlight Android](https://github.com/moonlight-stream/moonlight-android) by Cameron Gutman, Diego Waxemberg, Aaron Neyer, Andrew Hennessy and contributors, and was previously published as Butterpollo and as Rubylight Android ([rename history](docs/RENAME.md)). It incorporates work from these GPL-3.0 projects:

- [moonlight-common-c](https://github.com/moonlight-stream/moonlight-common-c), the classic streaming engine, and [moonlight-qt](https://github.com/moonlight-stream/moonlight-qt) by Cameron Gutman and contributors, for the desktop client's keyboard and controller tables, decoder setup, default bitrate curve and drawing straight to the display through DRM/KMS on Linux, and Moonlight's FFmpeg patch for HEVC 4:4:4 hardware decoding on Windows ([moonlight-qt-deps](https://github.com/moonlight-stream/moonlight-qt-deps))
- [Artemis Android](https://github.com/ClassicOldSong/moonlight-android) by ClassicOldSong, for Apollo extension behavior, decoder setup and the touchpad gestures (tap to click, two-finger scroll, tap-and-drag), which Umberto Marotta ported
- [Moonlight V+](https://github.com/qiin2333/moonlight-vplus) by qiin2333, for soft-keyboard, decoder and controller handling, and [Moonlight V+ PC](https://github.com/qiin2333/moonlight-qt), for the desktop client's D3D11 draw pass, AI enhancement, touchpad input, Linux USB forwarding, crash dumps and update check
- [Moonlight X](https://github.com/MoreOrLessSoftware/moonlight-android/tree/mlsoft-x) by MoreOrLessSoftware, for Exynos and Tensor decoder settings and the Vulkan renderer
- [jvanderv42's Nightjar fork](https://github.com/jvanderv42/nightjar), for the per-controller gyroscope mapping (axes, inversion, sensitivity and the Flydigi preset) and the in-stream motion readings
- [moonlight-noir](https://github.com/derflacco/moonlight-android) by derflacco, for decoder low-latency settings
- Nonary's moonlight-qt fork, the reference for the desktop client's VRR pacing and D3D11VA rendering, and the source of the Linux high-performance GPU power hold (`client/protocol/desktop/src/stream/gpu_hold.rs`), the Windows VRR sync switch `blank` (`stream/present/pacer.rs`) and the macOS refresh range (`src/mac_display.rs`), all from commit 19758e5e6
- [Nonary/moonlight-common-c](https://github.com/Nonary/moonlight-common-c), for the DualSense waveform protocol

The bundled third-party components (PyroWave, Granite, volk, libadrenotools, AMD FSR 1, Qualcomm SGSR 1, SDL, FFmpeg, the Figtree font and others) keep their own licenses. [License and credits](docs/index.md#license-and-credits) names each contribution and the notices to retain when redistributing.
