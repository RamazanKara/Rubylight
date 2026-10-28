# Rubylight documentation

[Rubylight](../README.md) · [Download 3.0.0-rc.3](https://github.com/RamazanKara/Rubylight/releases/tag/v3.0.0-rc.3) · [Release notes](../rust/RELEASE_NOTES.md)

Rubylight runs on your Windows gaming PC; Moonlight runs on the device you play from. These guides take you from a first desktop stream to HDR, virtual displays and the Radeon frame pipeline.

**Rubylight 3.0 needs an AMD Radeon GPU** with a working AMF encoder, on Windows 10 or 11. NVIDIA and Intel graphics stay on [Rubylight 2.6.x](https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0). The [3.0 roadmap](rubylight-3.0-plan.md) shows how the release was planned and what comes next.

**New here?** [Install Rubylight](getting-started.md#install-or-run-portable), [open the console](getting-started.md#open-the-console), then [pair Moonlight and launch Desktop](getting-started.md#pair-moonlight-and-start-desktop). Start at 1080p60 in SDR so you can check the picture, sound and controls before tuning the stream.

## Start streaming

| I want to… | Start here |
| --- | --- |
| **Get my first stream running** | [Getting started](getting-started.md): what you need, installer or portable setup, pairing and a first desktop stream. |
| Bring my settings or update an existing host | [Import a profile](getting-started.md#bring-an-existing-profile) · [Upgrade safely](getting-started.md#upgrading) |
| Choose a codec, enable HDR or try PyroWave | [Choose your stream format](getting-started.md#choose-your-stream-format) · [Capture and video settings](configuration.md#capture-and-video) |
| Let the host adjust bitrate after video packet loss | [Host bitrate adaptation](configuration.md#host-bitrate-adaptation): optional, experimental, and separate from client bitrate control. |
| Stream the game's sound without other applications | [Game-only audio](configuration.md#game-only-audio): supported Windows versions, verified launches and silence on capture failure. |
| Let a device play while someone keeps using the PC | [Background streaming](configuration.md#background-streaming): how [Nucleus Co-op](nucleus-coop.md) runs it and its limits; or [a Hyper-V VM](configuration.md#background-streaming-from-a-hyper-v-vm) with full input. |
| Play a game together on one PC from several devices | [Nucleus Co-op](nucleus-coop.md): the install, handlers, “Can join co-op games” and joining from Nightjar. |
| Give each device its own display or cap a game's frame rate | [Displays and RTSS](configuration.md#displays-and-rtss) · [Settings for one app or device](configuration.md#settings-for-one-app-or-device) |
| Let a device close games or change Radeon settings, and see what each session action does | [Host control](configuration.md#host-control-for-a-device) · [Session actions](configuration.md#session-actions) · [Radeon settings with restore](configuration.md#radeon-settings-with-restore) · [A Radeon profile for each game](configuration.md#radeon-settings-per-game) |
| Keep the stream from stalling on a slow link, or pick a latency or quality preset | [Link-rate adaptation](configuration.md#lower-bitrate-when-the-link-cannot-carry-it) · [Stream presets](configuration.md#stream-presets) |
| Send frames earlier, or record where each frame's time goes | [Slice delivery](b2-slice-delivery.md) · [Timing record](timing-record.md) |
| Set up a controller or use my client's microphone | [Controllers](configuration.md#controllers) · [DualSense USB audio and haptics](dualsense-haptics.md) · [Microphone](microphone.md) |
| Use a USB device plugged into my client on the PC | [USB forwarding (Experimental)](usb-forwarding.md): setup, which devices forward, detaching. |
| Stream to a Samsung TV or a browser | [WebRTC server (Experimental)](webrtc.md): setup, pairing, what streams, and the protocol extensions for client authors. |
| Launch Steam games from Steam Deck with MoonDeck | [MoonDeck setup](configuration.md#moondeck) |
| Tell another service when apps start or devices pair | [Webhooks](webhooks.md): signed events, verifying them and the replay window. |
| Understand an away-from-home Tailscale connection | [Remote connection route](network-path.md): paired route diagnostics and what an unknown result means. |
| **Fix a stream that will not start or feels wrong** | [Troubleshooting](troubleshooting.md): pairing, black screens, HDR colour, audio, controllers and smoothness. |

The full [configuration guide](configuration.md) starts with the defaults. If a problem needs a bug report, [collect the logs and stream details](troubleshooting.md#logs-and-a-useful-report) so someone else can reproduce it.

## Understand Rubylight

| Guide | What it covers |
| --- | --- |
| [Architecture](architecture.md) | What is written in Rust, where compute fits, frame ownership, native HDR and PyroWave. |
| [Performance](performance.md) | The 3.0 results, comparisons under gaming load, the meaning of each measurement and links to recorded runs. |
| [3.0 roadmap](rubylight-3.0-plan.md) | What 3.0 contains, what is done and what is next. |
| [Features](features.md) | Everything Rubylight does: video, capture, displays, input, library, console and clients. |
| [Release notes](../rust/RELEASE_NOTES.md) | Changes in every Windows release, from the first release candidate to the latest release. |
| [Privacy](privacy.md) | Every connection Rubylight makes, what it sends and how to turn it off. |
| [Code signing policy](code-signing-policy.md) | Who signs releases, what is signed, and how to check a download. |

## Build and integrate

| Reference | What it covers |
| --- | --- |
| [Build guide](building.md) | Windows toolchain, pinned dependencies, packaging and CI. |
| [Developer guide](../rust/README.md) | Workspace layout, isolated profiles, validation tools and native integration work. |
| [HTTP API](api.md) | Authentication, CSRF, permissions and the current Rust endpoints, plus a [short reference for the paired-client native API](api.md#native-api-v1-paired-clients). |
| [Native API v1](native-api.md) | The paired-client API behind Nightjar: capabilities, library, session actions and Radeon settings. The design and contract. |
| [Control messages](control-messages.md) | Rubylight's own client-to-host messages (phase lock, display luminance, mid-stream resolution change) and how a client finds out the host takes them. |
| [Hardware metrics](hardware-metrics.md) | Client overlay telemetry: discovery, paired-client authentication, JSON readings, units, polling and errors. |
| [Hardware matrix](hardware-matrix.md) | What we have tested and what is still Experimental, per GPU generation, Windows version, feature and client, plus the Radeon settings probes per GPU and driver. |
| [Third-party components](../rust/THIRD_PARTY.md) | Codec SDKs, drivers and licensing. |
| [Detailed performance record](../rust/PERFORMANCE.md) | Dated fixtures, raw results, rejected experiments and reproduction instructions. |

The [70-second product film](media/demo.mp4) shows the console, pairing and the measured results. The [project history](butterpollo-cpp.md) (the C++ fork that came before the Rust host) and [archived C++ references](https://github.com/RamazanKara/Rubylight/blob/2.0.0-rc.23/docs/legacy/README.md) cover the earlier implementation.
