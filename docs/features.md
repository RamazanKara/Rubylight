# Features

[Documentation](README.md) · [Getting started](getting-started.md) · [Configuration](configuration.md) · [Performance](performance.md)

The complete feature set of Rubylight 3.0, by area. Features that need client support, such as PyroWave, Dolby Vision or the clipboard, are enabled only for a client that requests them; every other Moonlight client streams as it would from Sunshine. Figures are our own measurements on an RX 7900 XT unless stated otherwise; [Performance](performance.md) explains them.

**On this page:** [Video](#video-and-encoding) · [Network](#network-resilience) · [Capture](#capture) · [Displays](#displays) · [Audio](#audio) · [Input](#input-and-controllers) · [Library](#game-library) · [Co-op](#co-op-and-background-streaming) · [Client control](#control-from-a-client) · [Sharing](#clipboard-files-and-usb) · [Browsers and TVs](#browsers-and-tvs) · [Console](#console-and-pairing) · [Installation](#installation-and-updates) · [Clients](#clients)

<a id="everything-it-does-in-30"></a>
<a id="everything-it-does"></a>

## Video and encoding

- **Native AMF encoding** of H.264, HEVC and AV1 on any Radeon with an AMF encoder, driven directly, with per-codec defaults tuned from VMAF measurements. The host probes each codec at startup with a real encode and offers only what the driver delivers.
- **Radeon compute.** Frame copies, colour conversion and scaling run on D3D12 compute queues alongside the game's graphics work, so neither waits on the other ([Architecture](architecture.md)).
- **HDR end to end.** 10-bit BT.2020 PQ through HEVC and AV1 with HDR metadata in the keyframes, and 10-bit SDR for clients that request it.
- **Dolby Vision, HDR10+ and HLG.** HDR streams carry per-scene brightness statistics measured on the GPU while the frame encodes, so the display tone-maps each scene rather than one fixed peak. Dolby Vision on HEVC (profiles 8.1 and 8.4) and AV1 (profile 10.1); HLG for displays that only show HLG, PyroWave included. Other clients receive HDR10 ([Configuration](configuration.md#capture-and-video), [measurements](../rust/PERFORMANCE.md#october-27-hdr10-and-hdr-vivid-dynamic-metadata)).
- **Windows HDR Calibration profiles** on virtual displays, when enabled.
- **Full chroma.** PyroWave, a wavelet codec, carries 4:4:4 and 10-bit HDR 4:4:4 over a fast local network. For Nightjar, the host also packs each 4:4:4 frame into an ordinary HEVC or AV1 picture from AMD's hardware encoder.
- **Stream presets.** Lowest latency, Balanced and Reference quality fill in the encoder settings you have not set yourself, for the host, one device or one app. Lowest latency made the first keyframe 29 % smaller on HEVC and 38 to 40 % smaller on H.264, with the same median end-to-end time ([Configuration](configuration.md#stream-presets)).
- **VRR-aware capture.** Under variable refresh the host waits for the game's next frame when it is about to arrive: for a 120 Hz game in a 60 fps stream, the picture reached the encoder 0.25 ms old instead of 4.68 ms ([measurements](performance.md#vrr-capture-age)). A 1000 Hz VRR mode serves clients that request it.
- **Encoder focus** (off by default). Static regions such as a HUD, text or an editor get a larger share of the bitrate than moving ones ([Configuration](configuration.md#encoder-focus)).
- **Slice delivery** (off by default). `video_slice_delivery = 4` hands H.264 and HEVC frames to the network slice by slice while the encoder finishes the rest. A 4K60 HEVC picture arrived 1.2 to 1.5 ms sooner over loopback and over Wi-Fi 6E to Nightjar on the desktop; an Android phone showed no measurable gain ([design](b2-slice-delivery.md), [measurements](../rust/PERFORMANCE.md#october-23-slice-delivery-b2-step-2f)).
- **Beam-raced PyroWave.** A client that requests it receives each PyroWave frame in display order, so Nightjar on the desktop decodes and presents the top of the picture while the bottom is still arriving. The middle row of the picture was on screen 1.4 ms sooner over loopback ([article](beam-racing-pyrowave.md)).
- **Neural rendering.** A switch on a game's page installs a third-party neural upscaling mod into games with DLSS, FSR or XeSS, and removes it again. It needs RDNA 3 or RDNA 4 graphics with 12 or more compute units and AMD Software 26.9.1 or newer ([Configuration](configuration.md#neural-rendering)).
- **Aspect ratio and scaling.** A source of a different shape is letterboxed. Streams below the desktop's resolution are downscaled with an area-averaging filter, which reduces shimmer on thin lines.
- **Per-frame timing record.** With `BUTTERPOLLO_TIMING_RECORD` set, each video stream writes capture, convert, encode, packetize and send times on one clock, ready to join with a client's frame log ([Timing record](timing-record.md)).

## Network resilience

- **Scene cuts without freezes.** A scene cut that leaves the encoder too large for its error correction is encoded again as a keyframe that fits. With 0.3 % random loss on a 250 Mbps HEVC stream, freezes after scene cuts went from 15 to 0 ([measurements](../rust/PERFORMANCE.md#october-28-oversize-scene-cuts-encoded-again-as-keyframes)). Recovery keyframes keep their full error correction within Moonlight's four FEC blocks.
- **Recovery without a keyframe.** A short loss on H.264, HEVC or AV1 is repaired with a frame that references an older picture the client still holds, for every client ([measurements](../rust/PERFORMANCE.md#october-28-ltr-recovery-on-h264-and-hevc-with-header-rewrites-on-by-default)). When the host's own network adapter briefly refuses packets, the host sends a keyframe immediately instead of waiting for the client to request one.
- **Link-rate adaptation.** When a client loses frames that error correction cannot repair, the host steps the bitrate down to what the link delivered, shows this on the stream card, probes back up and returns to your rate after ten clean seconds. With 75 Mbps requested over a 50 Mbit/s link, 1.17 % and 1.08 % of frames were lost in two runs, against 6.7 % before the 3.0 changes. A link that carries the stream is unaffected ([Configuration](configuration.md#lower-bitrate-when-the-link-cannot-carry-it), [measurements](performance.md#30-results)).
- **Wi-Fi pacing.** Clients that report Wi-Fi or mobile data are paced adaptively from the first frame. On 2.4 GHz Wi-Fi, unrecoverable frames per run fell from 61 to 15.7, with no change on a clean 5 GHz link ([measurements](../rust/PERFORMANCE.md#october-25-wi-fi-pacing-hint-on-a-real-laptop)). Adaptive pacing can also be enabled for every stream ([Configuration](configuration.md#capture-and-video)).
- **Client-chosen error correction.** A client that supports it chooses its own share of recovery packets, or lets the host follow its loss reports, up to `fec_max_percentage`, and can test the link before choosing its first bitrate ([Configuration](configuration.md#error-correction-chosen-by-the-device)).
- **Changes without reconnecting.** A Rubylight client can change the stream's resolution, bitrate and frame rate mid-stream, as a foldable phone does when it is opened ([Control messages](control-messages.md)).
- **Roaming.** A client whose address changes, such as a phone moving from Wi-Fi to mobile data, keeps its stream ([Configuration](configuration.md#roaming)). Control messages are resent as soon as the client is heard from after an outage.
- **Host bitrate adaptation** (off by default). For conventional Moonlight streams, the host adapts the bitrate to observed loss, accounting for FEC and audio, and yields to the client's own bitrate control ([Configuration](configuration.md#host-bitrate-adaptation)).

## Capture

- **Frames from Rubylight's display driver.** On its virtual displays, the driver hands the host each frame Windows composes with no capture API in between, and on Radeon the frame is converted in place. **Automatic** capture uses this path; a reconnect to a static desktop receives its picture immediately.
- **Windows Graphics Capture** on physical displays, and on a virtual display whose driver cannot push frames. **Desktop Duplication** takes over when WGC cannot open, and a lost Desktop Duplication session is reopened without ending the stream.
- **The lock screen and the sign-in prompt** stream too, so the PC can be unlocked from the client; capture returns to WGC when the desktop comes back. A locked virtual machine with no monitor attached streams as well ([#6](https://github.com/RamazanKara/Rubylight/issues/6)).
- **Services, virtual machines and containers.** The Windows service captures through a helper in the signed-in user's session, and the same host runs on headless machines, virtual machines and GPU-equipped Kubernetes pods.

## Displays

- **A virtual display for each device**, one shared by all devices, or none, with exclusive, extended, primary and isolated layouts, HDR, and the device's resolution and refresh rate.
- **Layout restore.** The saved monitor layout is restored after a stream or a crash, and a hotkey restores it on demand.
- **Client-side display choice.** A Rubylight client can request, per PC, the virtual display alone, a virtual display beside your monitors, or no virtual display, without changing the host's settings ([Configuration](configuration.md#display-choice-from-the-client)).
- **Rotation.** A device's own virtual display takes the stream's portrait or landscape orientation, and a monitor rotates to match and back after the stream; with **Match the device**, the display follows a device as it turns ([Configuration](configuration.md#displays-and-rtss)).
- **Monitors stay off while a device is away** (off by default). A monitor that wakes over HDMI while a device is disconnected stays off until the device returns ([#56](https://github.com/RamazanKara/Rubylight/issues/56)).
- **Concurrent clients**, each on its own virtual display.

## Audio

- **Surround** up to 7.1.4 over Opus, with endpoint matching by ID, name, description or adapter, and Steam Streaming Speakers.
- **Receiver formats.** A client that requests them receives uncompressed PCM up to 7.1.4, or Dolby Digital and Dolby Digital Plus, so a TV, soundbar or AV receiver decodes the sound itself ([Configuration](configuration.md#pcm-and-dolby-digital-audio)).
- **Night mode and headphone surround** (off by default) compress the dynamic range and render surround for headphones on a stereo device ([Configuration](configuration.md#surround-night-mode-and-headphone-surround)).
- **Game-only audio.** Captures a launched game and its child processes on supported Windows versions; when the game cannot be identified, the stream is silent and the host warns ([Configuration](configuration.md#game-only-audio)).
- **Microphone.** The client's microphone plays into Steam Streaming Microphone, encrypted in transit ([Microphone](microphone.md)).

## Input and controllers

- **Keyboard, mouse, touch, pen and controllers** through the signed VHF driver that the installer sets up.
- **A matching virtual pad.** PlayStation-type clients get a DualSense with adaptive triggers, Nintendo-type clients a Switch Pro controller, and others an Xbox Series pad. Controller identities persist across reconnects.
- **DualSense and Steam Deck over USB.** With usbip-win2 installed, a DualSense gets waveform haptics, speaker and headset audio and microphone input, and a Steam Deck appears as a genuine Steam Deck controller with trackpads, gyro and back grips for Steam Input ([setup and limits](dualsense-haptics.md)).
- **Haptics from game audio** (off by default). A DualSense renders the game's bass and impacts when the game sends no vibration, with strength, response and noise-gate settings that apply mid-stream ([how it works](dualsense-haptics.md#haptics-from-game-audio)).
- **Back grips.** Steam Deck L4, R4, L5 and R5 and Xbox Elite paddles press the buttons you assign.
- **On-screen keyboard follows text fields.** When a text field on the PC receives focus, Nightjar opens its keyboard, with digits for number fields and no suggestions for passwords. Keyboard layouts from the device, a client-drawn cursor, touch as a mouse or trackpad, and pen barrel roll are covered in [Configuration](configuration.md#keyboard-touch-pen-and-cursor).
- **Couch menu.** Hold L3+R3, press Ctrl+Alt+Backspace or hold the top-left corner, in any Moonlight client, for **Back to game**, **Disconnect**, **End session** and **Close game**, drawn into the stream ([Configuration](configuration.md#couch-menu)).
- **Steam Controller messages.** The host reads the Steam Controller attribute messages a client sends and converts Steam's haptic reports into rumble; a motor whose stop message never arrives stops after at most 10 seconds. Games do not receive these messages, because that needs a driver with the Steam Controller profile, which the host does not ship.

## Game library

- **Every installed game.** Steam, Playnite, Epic Games, Battle.net, EA app, Ubisoft Connect, GOG, Microsoft Store and Xbox PC games, and games in folders such as `D:\Games`, appear in one **Games** list with their covers, one switch per launcher and one **Sync now**.
- **Steam.** Syncs on demand and every 30 seconds, with covers from Steam's cache or store. A Steam game's stream ends when the game's processes exit.
- **Playnite.** Library sync, launches through Playnite with the stream's environment, a **Playnite (Fullscreen)** entry, and the game brought to the foreground.
- **Apps.** Commands with preparation and undo steps, detached commands, URLs and documents, an inferred working folder, and start before sign-in.
- **Per-game and per-device settings.** Stream, display, input and encoder settings, a Radeon profile, RTSS frame limits and Lossless Scaling profiles, applied at launch and removed at the end ([Configuration](configuration.md#settings-for-one-app-or-device)).
- **Close apps on disconnect.** An optional default closes the app when its last device leaves; each app can follow it or override it ([Configuration](configuration.md#close-apps-on-disconnect)).
- **Confirmed game switch.** Starting another game closes the running one only after the player confirms.
- **Launcher status.** When a Steam, Epic Games, EA app or Ubisoft Connect window waits for an update or a sign-in, the session reports it and a client can show it. The host only reads window titles; it never clicks or types into a launcher.

## Co-op and background streaming

- **Split-screen co-op.** A game that Nucleus Co-op supports gets a "(Co-op)" entry. Each device that starts it joins as the next player, up to four, each in its own instance with its own picture, sound and virtual controller. **Home** shows who plays in which slot, and Nightjar lists the sessions and joins a free slot ([Co-op and background streaming](nucleus-coop.md)).
- **Background streaming.** A device plays its own instance of a game while someone else keeps using the PC: the PC's displays and focus stay with the local user, only the game's sound is streamed, and only the device's controllers reach it. A single-player game needs no handler from the Nucleus hub, and a Steam game starts through its Steam app ID; other launchers and games with anti-cheat are refused ([Configuration](configuration.md#background-streaming)).

<a id="control-from-the-couch-30"></a>

## Control from a client

- **Native API.** A paired client on the streaming HTTPS port can read the host's capabilities, the game library with covers and the session state. Stock Moonlight and Artemis are unaffected ([Native API](native-api.md)).
- **Nightjar as a remote console.** Nightjar opens on a Home with favourites, what is running, the library of every paired PC, and a page for each game with Play or Resume. A session menu closes the game, ends the session or changes Radeon settings without leaving the stream.
- **Session actions.** **Disconnect** leaves the game running, **End session** stops the stream and keeps the game, **Close game** also quits it ([Configuration](configuration.md#session-actions)).
- **Host control.** A per-device permission to close games and change Radeon settings, off until you enable it ([Configuration](configuration.md#host-control-for-a-device)).
- **Radeon settings with restore.** A device with host control changes Radeon settings such as Anti-Lag and Radeon Chill during a stream; every change is journaled and reverted when the session ends ([Configuration](configuration.md#radeon-settings-with-restore)).
- **A Radeon profile per game.** **Competitive** (Anti-Lag on), **Smoothness** (the game capped at the stream's frame rate), **Quality** (anisotropic filtering and the driver's anti-aliasing enhancement) or a custom list, written when the game starts and restored when the session ends, including after a crash ([Configuration](configuration.md#radeon-settings-per-game)).
- **Sleep, wake and sleep after idle.** A device you authorise puts the PC to sleep while nothing is streaming, using the same requests as Vibepollo, on a PC with S1, S2 or S3 sleep; on a PC with only Modern Standby, the console says why the action is unavailable. The console shows whether devices can wake the PC and each network adapter's wake settings, and can put the PC to sleep after a period of inactivity ([Configuration](configuration.md#sleep-pc-and-sleep-after-idle)).
- **Live hardware readings.** On request, a streaming client can show the PC's CPU load and the Radeon's usage, temperatures, power, clocks and VRAM in its overlay ([Hardware metrics](hardware-metrics.md)).

## Clipboard, files and USB

Each of these needs a permission you grant the device; pairing never grants them ([Privacy](privacy.md#your-own-devices)).

- **Clipboard.** Text copied on either side is sent to the other as you copy it, and images can be copied in both directions ([Configuration](configuration.md#clipboard-files-and-usb)).
- **Shared folders.** Folders you share with a device can be browsed and downloaded from, and with a write grant uploaded to ([Configuration](configuration.md#shared-folders)).
- **File transfer** (off by default). Devices send files to the PC and fetch them back, into the signed-in user's `Downloads\Rubylight` or a folder you choose, paced while a stream runs ([Configuration](configuration.md#file-transfer)).
- **USB forwarding** (off by default). A USB device plugged into the client is attached to the PC during a stream, over USB/IP inside the stream's TLS connection ([USB forwarding](usb-forwarding.md)).

## Browsers and TVs

- **WebRTC server** (off by default). Pair a Samsung Tizen TV or a browser with a PIN and stream H.264, HEVC or AV1 with HDR, surround sound, gamepads, keyboard and mouse. The median glass-to-glass time for AV1 1080p60 was 17 ms with the browser on the host over loopback ([WebRTC](webrtc.md)).
- **Stream in the console.** With the WebRTC server on, the console's **Stream in the browser** page plays a game in the browser tab, authenticated by the console session instead of pairing: H.264 1080p60, stereo, keyboard and mouse ([WebRTC](webrtc.md#stream-in-the-console)).

## Console and pairing

- **Pairing** with a PIN, a one-time PIN or a QR code that Nightjar scans, with per-device permissions and an enable switch. Pairing from the console offers **Play games** (the default), **Watch only** or **Everything**; the first device gets everything except host control. WebRTC devices, and pairings made through the API without a choice, start view-only ([Configuration](configuration.md#host-control-for-a-device)).
- **Home** leads through pairing and playing, then shows the current stream, running co-op games and your games. **Games**, **Devices** and **Settings** cover the library, pairing and stream options; **Settings** shows the ten settings most people change, and **All settings** has every other one, with search.
- **The stream card** shows frame rate, bitrate and encoder timing together, the frame rate at each stage of the pipeline, and what the host changed and why.
- **Help** reads the host log and explains the failures it recognises, such as AMD encoder hangs, lost capture, ports in use, network loss and an outdated display driver, with the corrective action ([Troubleshooting](troubleshooting.md)).
- **Client build warnings.** The device list shows the build each device last reported and warns when that build has a known problem with this host.
- **Webhooks.** Signed events to your own URLs when devices pair, apps start and stop, and streams begin and end ([Webhooks](webhooks.md)).
- **Tray icon** with notifications for pairing requests, new versions and apps starting, pausing and stopping.
- **Logs and support.** Rotating logs, a live log view, crash dumps and a one-click support bundle.
- **Connection route.** An authenticated client can check whether its Tailscale route to the host is direct, relayed or unknown, without seeing other peers ([Remote connection route](network-path.md)).

## Installation and updates

- **One installer** sets up the service, drivers, firewall rule and shortcuts, and upgrades in place. It bundles optional usbip-win2 for Steam Deck controllers, DualSense USB audio and haptics, and USB forwarding, and imports an existing Vibepollo or Apollo profile with its settings, paired devices, apps and covers.
- **Updates.** New versions are offered, not forced; automatic installation is opt-in. The host downloads the installer from the official release, verifies its size and SHA-256 digest, waits for an idle minute and upgrades through the service. A failed copy or start restores the previous files.
- **Crash recovery.** The service restarts the host after a crash.
- **Separate instances** with their own profile folders and base ports ([Multiple instances](configuration.md#multiple-instances)).

## Clients

Rubylight implements the Moonlight protocol, so Moonlight and other Sunshine-compatible clients work with it on Windows, macOS, Linux, Android, iOS, Apple TV, Android TV, consoles and handhelds. H.264, HEVC and AV1 stream in SDR and HDR wherever the device decodes them. PyroWave needs a PyroWave-capable client such as [Nightjar](https://github.com/RamazanKara/nightjar), which also uses the native API and runs on Android, Android TV, Windows and Linux ([releases](https://github.com/RamazanKara/nightjar/releases)). The [hardware matrix](hardware-matrix.md#4-clients) lists the clients that have been tested.

To choose a stream format, see [Getting started](getting-started.md#choose-your-stream-format).
