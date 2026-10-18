<p align="center">
  <img src="docs/assets/banner.png" width="800" alt="Nightjar: low-latency game streaming from your PC">
</p>

<p align="center">Nightjar streams your PC games and desktop to Android with as little delay as your network allows. It works with Sunshine, Apollo, Vibepollo, Rubylight, NVIDIA GameStream and other Moonlight-compatible hosts, and it adjusts itself to whichever one you run.</p>

<p align="center"><b>Early release:</b> expect rough edges, and please <a href="https://github.com/RamazanKara/nightjar/issues">report what breaks</a>.</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/RamazanKara/nightjar/ef040817e7dc340ed5a874b4c159f49445bc2774/docs/assets/demo.gif" width="360" alt="Nightjar demo: tap a PC to open its game library, tap a game to start your stream, open the stream menu, and switch the overlay to statistics with the host's CPU and GPU readings.">
</p>

<p align="center"><a href="https://ramazankara.github.io/nightjar/#demo">Watch the 69-second tour: PCs and library, PC settings for your host, stream menu, host statistics, PyroWave, adaptive refresh, upscaling, the Snapdragon GPU driver, DualSense and picture-in-picture →</a></p>

<p align="center">
  <a id="download" href="https://github.com/RamazanKara/nightjar/releases/latest"><img src="docs/assets/download.svg" width="260" height="52" alt="Download Nightjar for Android"></a>
</p>

<p align="center">
  <img src="docs/assets/shots/pc-list.png" width="160" alt="Nightjar's PCs list: Living-room PC playing Racing Game, Studio PC not paired and Laptop offline">
  <img src="docs/assets/shots/library.png" width="160" alt="Living-room PC's game library in Nightjar: ten games with cover art, Racing Game marked Running">
  <img src="https://raw.githubusercontent.com/RamazanKara/nightjar/ef040817e7dc340ed5a874b4c159f49445bc2774/docs/assets/shots/stream.png" width="160" alt="Close-up of a Nightjar stream's top-left corner, where the one-line overlay shows a green health dot, 119.9 FPS and 8 ms RTT">
  <img src="docs/assets/shots/pip.png" width="160" alt="A Nightjar stream floating over the home screen in a picture-in-picture window">
</p>

## Why Nightjar

**It learns your host.** Every host supports a different set of extras. Nightjar reads what yours reports and only shows the options that work there, with a short reason when something needs a host setting or an update. Offline PCs keep what they last reported, and a "not supported" note appears once per PC instead of on every stream.

**Quality that adjusts itself.** Automatic bitrate follows packet loss, network jitter and how much headroom your phone's decoder has left, up to 2000 Mbps where the host allows it. Automatic quality goes further and changes resolution and frame rate mid-stream, with three goals to pick from: keep motion smooth, keep detail sharp, or save battery. Both watch your phone's temperature and back off before it starts throttling. (Bitrate and quality control: Rubylight. Manual bitrate changes during a stream: Rubylight and Vibepollo.)

**It works hard on latency.** Decoders get per-chip low-latency tuning, frames are paced to your display, including Android's adaptive refresh on newer phones, and stereo audio uses low-latency AAudio. On Rubylight, Nightjar tells the host how long frames wait for your screen so it can time them to arrive just in time.

**PyroWave, upscaling and your own GPU driver.** On hosts with PyroWave (currently Rubylight, and Vibepollo with PyroWave enabled), Nightjar decodes the wavelet codec on the GPU through Vulkan, including full-colour 4:4:4. Any SDR stream can be sent at a lower resolution and upscaled on the phone with FSR 1 or Snapdragon GSR. On Snapdragon phones you can load a newer Adreno Vulkan driver for PyroWave, and Nightjar falls back to the system driver if it fails.

**You can see everything.** A one-line overlay shows frame rate, network round trip, loss and a health dot. Statistics and Debug views break the delay into stages, and with host telemetry turned on (Rubylight) they add the PC's CPU usage and AMD GPU usage, temperatures, power, clocks and VRAM. "Why did my stream stutter?" points at the likely cause, whether that's the network, the decoder, the host, the display or heat, and can run a reversible bitrate test. A built-in test measures input-to-screen latency, a decoder benchmark runs without any host, and frame timings export as CSV.

**Talk and play.** Send your phone's microphone to the PC for voice chat and mute it from the stream menu (Rubylight). Over USB, a DualSense gets rumble and Edge paddles on any host, and adaptive triggers and the lightbar on hosts that present a DualSense to the game; on hosts that support it, also waveform haptics (Rubylight, Vibepollo) and the controller's speaker, headset and microphone (Rubylight). Controllers keep their identity across reconnects on Rubylight, and Xbox USB controllers work through a built-in driver.

**Your PC, your way.** Pick per PC whether the stream gets its own virtual display at your phone's resolution (Apollo, Rubylight, Vibepollo), and on Rubylight whether your monitors stay on, switch off or sit beside it. Watch a PC without taking over its input with Remote Monitor, or control it without video with Input-only (Rubylight, Vibepollo; Input-only also on Apollo, and on any other host as a small hidden stream if you turn it on). Copy text in both directions, run the host's commands and pair with a one-time PIN on Apollo, Rubylight and Vibepollo, and open your host's web UI from the app.

**Play away from home.** A built-in guide sets up a private Tailscale connection and tests the paired PC from wherever you are. On Rubylight it also shows whether the connection runs direct or through a relay.

**Your library in your launcher.** Export games to Beacon, ES-DE, Daijisho or Pegasus from **PC settings → Add games to a launcher**. Pick the launcher and folder, then follow the built-in setup instructions or copy its launch command. Nightjar remembers successful export folders for each PC and launcher, so **Update games** refreshes entries in place. ES-DE can also receive its system configuration and cached covers. [Launcher setup](docs/FRONTENDS.md).

## Everything else

- AV1, HEVC and H.264 with HDR10, and 4:4:4 where the host offers it
- Touch, trackpad, mouse, pen and keyboard, with editable keyboard shortcuts and per-game touch controls (keys, gamepad buttons, sticks, triggers, D-pad) you can export, share and import from Artemis ([controls](docs/controls.md))
- Touch modes: natural trackpad with adjustable acceleration, glide and active area, a Local cursor drawn on the phone, or Touch off for controller play; configurable three- and four-finger tap shortcuts
- Video scaling (Fit, Fill, Stretch, 1:1), picture position, custom stream resolutions, and portrait or rotate-with-device streams
- Named stream profiles picked by network, per-game stream settings and a mobile data bitrate cap
- Per-controller button and stick/trigger mappings, a controller test page, an opt-in physical Back shortcut for handhelds such as AYN Thor, rumble off, keyboard Back or Right Alt as the Windows key, and an opt-in Switch Pro USB driver
- Quick resume, quit the game when you leave, and opt-in tolerance for network drops of up to 60 seconds
- Wi-Fi details and stutter-rhythm detection in the stream doctor, and shareable device results ([devices](docs/devices.md))
- Desktop tools: a compact keyboard and trackpad panel with key banks and shortcuts, pinch zoom, and video on an external display with the phone as a touchpad
- Settings per PC, Native and Battery saver presets, Wake-on-LAN and automatic discovery
- Phones, foldables, tablets, Android TV and Fire TV with D-pad navigation, plus picture-in-picture; TV remote Back and Menu open stream controls, and D-pad/Select send arrow keys/Enter to the PC ([controls](docs/controls.md#tv-and-fire-tv-remotes))
- Optional OLED black appearance for app screens, menus and dialogs, even with a light system theme
- Built-in help that works offline, and problem reports that stay on your phone until you share them
- In 15 languages; native speakers can [help translate](docs/TRANSLATIONS.md#help-translate)
- No ads, no analytics, no accounts

See the [host feature matrix](docs/PARITY-MATRIX.md) for exactly what each host provides.

## Quick start

Requires **Android 5.0 or newer**. PyroWave runs on hosts that support it (currently Rubylight, and Vibepollo with PyroWave enabled) and requires Android 10+, a 64-bit device and a compatible Vulkan 1.3 GPU; see [video requirements](docs/video-and-latency.md#codecs).

1. Start a streaming host on your PC, such as [Apollo](https://github.com/ClassicOldSong/Apollo), [Rubylight](https://github.com/RamazanKara/Rubylight), [Sunshine](https://github.com/LizardByte/Sunshine) or [Vibepollo](https://github.com/Nonary/Vibepollo), and add your games.
2. [Download the APK](https://github.com/RamazanKara/nightjar/releases/latest), open it and allow installation from your browser or file manager when prompted.
3. Select your PC on the same network and enter the PIN shown on your phone in your host's settings or web UI.
4. If your host has per-device permissions, enable your device and allow application listing, viewing, launching and the input types you use. Open your library on Android and tap a game. See [pairing and permissions](docs/pairing.md).

Coming from Rubylight Android? Nightjar is the same app under a new name. It installs over Rubylight Android 0.5.0 and keeps your PCs, pairings and settings.

## Docs

[Start here](docs/index.md) · [Quick start](docs/quick-start.md) · [Pairing](docs/pairing.md) · [Settings](docs/settings.md) · [Controls](docs/controls.md)<br>
[Video & latency](docs/video-and-latency.md) · [Troubleshooting](docs/TROUBLESHOOTING.md) · [Building](docs/building.md) · [Frontends](docs/FRONTENDS.md) · [Translations](docs/TRANSLATIONS.md) · [FAQ](docs/faq.md)

[GPL-3.0](LICENSE.txt) · [Credits and third-party notices](docs/index.md#license-and-credits)
