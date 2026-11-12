# Rubylight documentation

[Rubylight](../README.md) · [Download 4.0.2](https://github.com/RamazanKara/Rubylight/releases/tag/v4.0.2) · [Release notes](../rust/RELEASE_NOTES.md)

Rubylight is a Moonlight-compatible game-streaming host for Windows 10 and 11 PCs with an AMD Radeon GPU that has a working AMF encoder or an NVIDIA GPU with NVENC ([NVENC](nvenc.md)). PCs with only Intel graphics use [Rubylight 2.6.x](https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0).

## Start here

**First stream:** [install the host](getting-started.md#install-or-run-portable), [open the console](getting-started.md#open-the-console), [pair a device](getting-started.md#pair-a-device-and-start-desktop), then start **Desktop** at 1080p60 in SDR. Already running another host? Read [Coming from another host](getting-started.md#coming-from-another-host) first.

| Guide | Contents |
| --- | --- |
| [Getting started](getting-started.md) | Requirements, the installer and the portable package, moving from another host, the console, pairing, choosing a stream format, importing a profile and upgrading. |
| [Troubleshooting](troubleshooting.md) | Pairing, black pictures, HDR, audio, controllers, smoothness, updates, and what to include in a report. |

## Guides

| Guide | Contents |
| --- | --- |
| [Configuration](configuration.md) | Every feature's settings by topic, then every configuration key with its default. |
| [Features](features.md) | The complete feature set, by area. |
| [Frame Warp](frame-warp.md) | Camera turns on the client's next refresh: what the host learns per game, the HUD it keeps in place, and the measurements. |
| [Co-op and background streaming](nucleus-coop.md) | Several players on one PC, each with their own picture, sound and controller, and play while someone else uses the PC. |
| [WebRTC server](webrtc.md) | Streaming to browsers and Samsung TVs, paired with a PIN. |
| [Guests in the browser](browser-guests.md) | Guest links for friends, with access controls. |
| [USB forwarding](usb-forwarding.md) | Using a device's USB devices on the PC. |
| [Microphone](microphone.md) | The client's microphone on the PC. |
| [DualSense USB audio and haptics](dualsense-haptics.md) | DualSense and Steam Deck audio and haptics over USB. |
| [Remote connection route](network-path.md) | Checking the route a remote client takes, such as Tailscale. |

## By task

| Task | Read |
| --- | --- |
| Move from another host, try Rubylight beside it, or update the host | [Coming from another host](getting-started.md#coming-from-another-host) · [Import a profile](getting-started.md#bring-an-existing-profile) · [Install alongside](getting-started.md#keep-another-host-and-install-alongside) · [Upgrading](getting-started.md#upgrading) |
| Choose a codec, turn on HDR or use PyroWave | [Choose your stream format](getting-started.md#choose-your-stream-format) · [Capture and video](configuration.md#capture-and-video) |
| Stream Dolby Vision, HDR10+ or HLG to a TV | [Capture and video](configuration.md#capture-and-video) |
| Send PCM or Dolby Digital to a receiver, or use 7.1.4, night mode or headphone surround | [PCM and Dolby Digital audio](configuration.md#pcm-and-dolby-digital-audio) · [Surround, night mode and headphone surround](configuration.md#surround-night-mode-and-headphone-surround) |
| Stream only the game's sound | [Game-only audio](configuration.md#game-only-audio) |
| Give each device its own display, or cap a game's frame rate | [Displays and RTSS](configuration.md#displays-and-rtss) · [Settings for one app or device](configuration.md#settings-for-one-app-or-device) |
| Keep each device's own in-game settings, or add Neural Rendering to a game | [Managed game files](configuration.md#managed-game-files) · [Neural Rendering](configuration.md#neural-rendering) |
| Move a running game to another device, share it, or stream a second screen | [Displays](features.md#displays) · [Control from a client](features.md#control-from-a-client) |
| Keep the stream steady on a slow or lossy link, or pick a latency or quality preset | [Link-rate adaptation](configuration.md#lower-bitrate-when-the-link-cannot-carry-it) · [Host bitrate adaptation](configuration.md#host-bitrate-adaptation) · [Stream presets](configuration.md#stream-presets) |
| Let a device play while someone else uses the PC, or play together on one PC | [Background streaming](configuration.md#background-streaming) · [Co-op and background streaming](nucleus-coop.md) |
| Let a device close games or change Radeon settings | [Host control](configuration.md#host-control-for-a-device) · [Session actions](configuration.md#session-actions) · [Radeon settings with restore](configuration.md#radeon-settings-with-restore) · [Radeon settings per game](configuration.md#radeon-settings-per-game) |
| Set up a controller or use the client's microphone | [Controllers](configuration.md#controllers) · [DualSense USB audio and haptics](dualsense-haptics.md) · [Microphone](microphone.md) |
| Open a menu inside the stream | [Couch menu](configuration.md#couch-menu) |
| Share the clipboard, files, folders or USB devices with a device | [Clipboard, files and USB](configuration.md#clipboard-files-and-usb) · [USB forwarding](usb-forwarding.md) |
| Stream to a browser or a Samsung TV, play from a TV or console browser, or invite friends | [WebRTC server](webrtc.md) · [Play in a TV browser](webrtc.md#play-in-a-tv-browser) · [Guests in the browser](browser-guests.md) |
| Launch Steam games from a Steam Deck with MoonDeck | [MoonDeck](configuration.md#moondeck) |
| Check a remote Tailscale route | [Remote connection route](network-path.md) |
| Report a problem | [Logs and a useful report](troubleshooting.md#logs-and-a-useful-report) |

## Performance and hardware

| Document | Contents |
| --- | --- |
| [Performance](performance.md) | Measured results with their conditions, what each metric means, and how to measure your own setup. |
| [Measurement record](../rust/PERFORMANCE.md) | Dated fixtures, raw results, reverted experiments and reproduction steps. |
| [Hardware matrix](hardware-matrix.md) | What has been tested on each GPU generation, Windows version, feature and client, with the evidence. |
| [NVENC](nvenc.md) | The NVIDIA encoder and how it compares with AMF, feature by feature. |

## Articles

- [Racing the beam with PyroWave](beam-racing-pyrowave.md): how Rubylight and Nightjar draw a frame before all of it has arrived, with our measurements.

## Reference for integrators and client authors

| Reference | Contents |
| --- | --- |
| [HTTP API](api.md) | The console API: authentication, CSRF, permissions and endpoints. |
| [Native API v1](native-api.md) | The paired-client API used by Nightjar: capabilities, library, session actions, Radeon settings, files and USB. |
| [Control messages](control-messages.md) | Rubylight's client-to-host control messages and how a client discovers them. |
| [Hardware metrics](hardware-metrics.md) | CPU and GPU readings for client overlays. |
| [Webhooks](webhooks.md) | Signed event notifications to your own services. |
| [Timing record](timing-record.md) | The per-frame CSV record of capture, encode, send and client times. |

## Building and internals

| Document | Contents |
| --- | --- |
| [Architecture](architecture.md) | The crates, the frame path from capture to packet, compute queues, HDR and PyroWave. |
| [Build guide](building.md) | Toolchain, pinned dependencies, packaging, the web console and the website. |
| [Developer guide](../rust/README.md) | Workspace layout, isolated profiles, running the host from source and validation tools. |
| [Release signing](code-signing.md) | The maintainer's SignPath setup and what each release signs. |
| [Third-party components](../rust/THIRD_PARTY.md) | Codec SDKs, drivers and their licences. |

## Engineering notes

Working documents from the project's development: plans, designs, investigations and audits. Each records the state on the date it was written and is not maintained as user documentation; where a note and a guide above disagree, the guide is current.

- Plan: [Rubylight 3.0 plan](rubylight-3.0-plan.md) and its status table.
- Designs: [couch menu](couch-menu-design.md), [encoder focus](encoder-focus-design.md), [Wake-on-LAN, sleep and couch audio](wake-and-audio-design.md), [WebRTC server](webrtc-design.md), [slice delivery](b2-slice-delivery.md).
- Audits: [streaming stability, October 18](stability-audit-2026-10-18.md), [PyroWave and D3D12, October 18](pyrowave-d3d12-audit-2026-10-18.md), [bug hunt, October 28](bug-hunt-2026-10-28.md).
- Work log: [Windows performance work, October 2](../rust/PERFORMANCE_WORK.md).

## Project

| Document | Contents |
| --- | --- |
| [Privacy](privacy.md) | Every connection the host makes, what it sends, and how to turn it off. |
| [Code signing policy](code-signing-policy.md) | Who signs releases and what is signed. |
| [Release notes](../rust/RELEASE_NOTES.md) | Every release, with what was tested. |
| [Project history](butterpollo-cpp.md) | The C++ fork that preceded the Rust host, and the [archived C++ references](https://github.com/RamazanKara/Rubylight/blob/2.0.0-rc.23/docs/legacy/README.md). |
| [Contributors](../CONTRIBUTORS.md) | Everyone who helped, with their GitHub names. |
