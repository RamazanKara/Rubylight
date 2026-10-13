# Rubylight documentation

[Rubylight](../README.md) · [Download 2.2.13](https://github.com/RamazanKara/Rubylight/releases/tag/v2.2.13) · [Release notes](../rust/RELEASE_NOTES.md)

Rubylight runs on your Windows gaming PC; Moonlight runs on the device you play from. These guides take you from a first desktop stream to HDR, virtual displays and the Radeon frame pipeline.

**New here?** [Install Rubylight](getting-started.md#install-or-run-portable), [open the console](getting-started.md#open-the-console), then [pair Moonlight and launch Desktop](getting-started.md#pair-moonlight-and-start-desktop). Start at 1080p60 in SDR so you can check the picture, sound and controls before tuning the stream.

## Start streaming

| I want to… | Start here |
| --- | --- |
| **Get my first stream running** | [Getting started](getting-started.md): what you need, installer or portable setup, pairing and a first desktop stream. |
| Bring my settings or update an existing host | [Import a profile](getting-started.md#bring-an-existing-profile) · [Upgrade safely](getting-started.md#upgrading) |
| Choose a codec, enable HDR or try PyroWave | [Choose your stream format](getting-started.md#choose-your-stream-format) · [Capture and video settings](configuration.md#capture-and-video) |
| Give each device its own display or cap a game's frame rate | [Displays and RTSS](configuration.md#displays-and-rtss) · [Settings for one app or device](configuration.md#settings-for-one-app-or-device) |
| Set up a controller or use my client's microphone | [Controllers](configuration.md#controllers) · [Microphone](microphone.md) |
| Launch Steam games from Steam Deck with MoonDeck | [MoonDeck setup](configuration.md#moondeck) |
| **Fix a stream that will not start or feels wrong** | [Troubleshooting](troubleshooting.md): pairing, black screens, HDR colour, audio, controllers and smoothness. |

The full [configuration guide](configuration.md) starts with the defaults. If a problem needs a bug report, [collect the logs and stream details](troubleshooting.md#logs-and-a-useful-report) so someone else can reproduce it.

## Understand Rubylight

| Guide | What it covers |
| --- | --- |
| [Architecture](architecture.md) | What is written in Rust, where compute fits, frame ownership, native HDR and PyroWave. |
| [Performance](performance.md) | Comparisons under gaming load, the meaning of each measurement and links to recorded runs. |
| [Features](features.md) | Everything Rubylight does: video, capture, displays, input, library, console and clients. |
| [Release notes](../rust/RELEASE_NOTES.md) | Changes in every Windows release, from the first release candidate to the latest release. |

## Build and integrate

| Reference | What it covers |
| --- | --- |
| [Build guide](building.md) | Windows toolchain, pinned dependencies, packaging and CI. |
| [Developer guide](../rust/README.md) | Workspace layout, isolated profiles, validation tools and native integration work. |
| [HTTP API](api.md) | Authentication, CSRF, permissions and the current Rust endpoints. |
| [Control messages](control-messages.md) | Rubylight's own client-to-host messages (phase lock, display luminance, mid-stream resolution change) and how a client finds out the host takes them. |
| [Hardware metrics](hardware-metrics.md) | Client overlay telemetry: discovery, paired-client authentication, JSON readings, units, polling and errors. |
| [Third-party components](../rust/THIRD_PARTY.md) | Codec SDKs, drivers and licensing. |
| [Detailed performance record](../rust/PERFORMANCE.md) | Dated fixtures, raw results, rejected experiments and reproduction instructions. |

The [60-second launch film](media/demo.mp4) introduces the frame pipeline and its measured results. The [project history](butterpollo-cpp.md) and [archived C++ references](https://github.com/RamazanKara/Rubylight/blob/2.0.0-rc.23/docs/legacy/README.md) cover the earlier implementation.
