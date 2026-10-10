# Rubylight documentation

[Rubylight](../README.md) · [Download 2.1.0](https://github.com/RamazanKara/Rubylight/releases/tag/2.1.0) · [Release notes](../rust/RELEASE_NOTES.md)

Rubylight is a Windows game-streaming host written in Rust, built around Radeon compute and Moonlight. Start with the setup guide, then choose the details that matter to your stream.

## Start streaming

| Guide | What it covers |
| --- | --- |
| **[Getting started](getting-started.md)** | Installer or portable setup, profile migration, pairing, your first stream and choosing a client. |
| **[Configuration](configuration.md)** | Capture, codecs, HDR, virtual displays, frame limiting, overrides and updates. |
| **[Troubleshooting](troubleshooting.md)** | Pairing, black screens, HDR colour, smoothness, RTSS and useful support reports. |
| [Microphone](microphone.md) | Talking through the PC from a streaming device, and the protocol for client developers. |

## Understand Rubylight

| Guide | What it covers |
| --- | --- |
| [Architecture](architecture.md) | What is written in Rust, where compute fits, frame ownership, native HDR and PyroWave. |
| [Performance](performance.md) | Readable comparisons, the meaning of each measurement and links to recorded runs. |
| [Features](features.md) | Everything Rubylight does: video, capture, displays, input, library, console and clients. |
| [Release notes](../rust/RELEASE_NOTES.md) | Changes in every Windows release, from the first release candidate to the latest release. |

## Build and integrate

| Reference | What it covers |
| --- | --- |
| [Build guide](building.md) | Windows toolchain, pinned dependencies, packaging and CI. |
| [Developer guide](../rust/README.md) | Workspace layout, isolated profiles, validation tools and native integration work. |
| [HTTP API](api.md) | Authentication, CSRF, permissions and the current Rust endpoints. |
| [Control messages](control-messages.md) | Rubylight's own control-stream messages, such as changing the resolution mid-stream, for client developers. |
| [Third-party components](../rust/THIRD_PARTY.md) | Codec SDKs, drivers and licensing. |
| [Detailed performance record](../rust/PERFORMANCE.md) | Dated fixtures, raw results, rejected experiments and reproduction instructions. |

The [60-second launch film](media/demo.mp4) introduces the frame pipeline and its measured results. The [project history](butterpollo-cpp.md) and [archived C++ references](https://github.com/RamazanKara/Rubylight/blob/2.0.0-rc.23/docs/legacy/README.md) cover the earlier implementation.
