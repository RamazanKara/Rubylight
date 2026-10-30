# Nightjar documentation

These pages describe Nightjar 4.0 for Android, Android TV, Fire TV, Windows and Linux. Nightjar works with any Moonlight-compatible host, such as Sunshine, Apollo, Vibepollo, Rubylight and NVIDIA GameStream, and enables each host extension from the capabilities the host reports.

Conventions used throughout:

- The **PC** is the computer you stream from; the **host** is the streaming software running on it; **this device** is the one running Nightjar.
- Settings are named exactly as they appear in the app, with their path: **Settings → Streaming → Recover interrupted streams**.
- Where a feature depends on the host, the page names the capability it needs and the hosts known to provide it.

New to Nightjar? Read the [quick start](quick-start.md), then return here when you want to tune something. [Download Nightjar](../README.md#download).

## User guide

| Page | Contents |
| --- | --- |
| [Quick start](quick-start.md) | Installing a host, adding a PC and starting the first stream |
| [Pairing and permissions](pairing.md) | PIN, one-time PIN and QR pairing; the per-device permissions a host can set |
| [Play away from home](remote-setup.md) | Streaming over Tailscale or a forwarded connection, and testing the path |
| [Home and the session menu](console.md) | Home, game pages, Join co-op, the Session and graphics menu, TV and accessibility |
| [Controls](controls.md) | Controllers, touch, mouse, keyboard, pen, on-screen controls and the microphone |
| [Gamepad navigation](gamepad-navigation.md) | Using the menus and the PC keyboard with a controller |
| [DualSense USB audio and haptics](dualsense-haptics.md) | Waveform haptics and controller audio over USB |
| [Video and latency](video-and-latency.md) | Codecs, HDR, PyroWave, refresh rate, scaling, bitrate, error correction and audio |
| [Recovery and automatic quality](recovery-and-quality.md) | Recovering interrupted streams, riding out network drops, automatic resolution and frame rate |
| [Clipboard, files, folders and USB devices](sharing.md) | Sharing with a PC whose host supports it |
| [Launcher integrations](FRONTENDS.md) | Starting games from Beacon, ES-DE, Daijisho, Pegasus and Android TV's home screen |
| [Nightjar for Windows and Linux](../protocol/desktop/README.md) | Installing, running and building the desktop client |
| [Features](features.md) | Every feature, with a link to its guide |

## Reference

| Page | Contents |
| --- | --- |
| [Settings reference](settings.md) | Every setting on Android and the desktop client, with its default and advice |
| [Host feature support](PARITY-MATRIX.md) | Each host capability and the hosts known to provide it |
| [Android and desktop compared](CLIENT-PARITY.md) | Feature availability in the two clients |
| [Decoder flags](decoder-flags.md) | The low-latency decoder settings applied per chip family |
| [Device results](devices.md) | Decoder benchmark results from real devices, and how to add yours |
| [Privacy](PRIVACY.md) | What Nightjar stores, what it sends and to whom |

## Troubleshooting

| Page | Contents |
| --- | --- |
| [Troubleshooting](TROUBLESHOOTING.md) | Symptoms, causes and fixes, and how to share a problem report |
| [Stream health](stream-health.md) | Reading **Why did my stream stutter?** and its reversible bitrate trial |
| [FAQ](faq.md) | Short answers to common questions |

## Contributing

| Page | Contents |
| --- | --- |
| [Building](building.md) | Toolchains, tests, signing, the release workflow and the [documentation checks](building.md#documentation-checks) |
| [Translations](TRANSLATIONS.md) | Supported languages and how to contribute one |
| [Device results](devices.md#send-results) | Submitting a benchmark from your device |

## Engineering notes

Design notes, measurements and audits for contributors. They describe internals and are not needed to use Nightjar.

| Note | Subject |
| --- | --- |
| [Beam racing](beam-racing.md), [early decode](early-decode.md) | Desktop presentation timing |
| [PyroWave GPU decoding](pyrowave-decode.md), [PyroWave decode stages](pyrowave-decode-stages.md) | The desktop PyroWave decoder and its measured stages |
| [Refresh rate switching](expected-refresh-switch.md), [HDR on Android displays](expected-hdr10.md) | How the Android app requests refresh rates and HDR, and reports refusals |
| [Client roaming](protocol/client-roaming.md), [host cursor shapes](protocol/cursor-shape.md) | Protocol extensions proposed for Moonlight-compatible hosts |
| [Android audit, October 2026](android-audit-2026-10.md) | Audit findings and their resolution |
| [Rename history](RENAME.md) | The renames to Nightjar, and the identifiers that keep their old names |
| [Demo mode](demo/README.md) | The offline demo fixtures used for screenshots and the promotional video |

## License and credits

Nightjar is licensed under [GPL-3.0](../LICENSE.txt). It includes Moonlight Android code by [Cameron Gutman](https://github.com/cgutman), [Diego Waxemberg](https://github.com/dwaxemberg), Aaron Neyer, [Andrew Hennessy](https://github.com/yetanothername) and contributors. Moonlight began as a project at [MHacks](https://mhacks.org/) by students at [Case Western Reserve University](https://case.edu/).

Apollo extension behavior draws on [Artemis Android](https://github.com/ClassicOldSong/moonlight-android) by ClassicOldSong and contributors (GPL-3.0). The decoder low-latency keys for NVIDIA Tegra, MediaTek and Snapdragon follow Artemis's decoder setup and the [moonlight-noir fork](https://github.com/derflacco/moonlight-android) by derflacco (GPL-3.0). The Exynos and Tensor keys follow [Moonlight X](https://github.com/MoreOrLessSoftware/moonlight-android/tree/mlsoft-x) by MoreOrLessSoftware (GPL-3.0); see [decoder flags](decoder-flags.md). The soft keyboard for the host's text fields (when it opens, staying closed once closed, and moving the stream up over it), the declared-only Snapdragon keys, the shorter MediaTek queues on the first configure try, the Amlogic HEVC low-latency exception, decoder input buffer growth and the controller gyroscope liveness check follow [Moonlight V+](https://github.com/qiin2333/moonlight-vplus) by qiin2333 and contributors (GPL-3.0).

The [DualSense waveform extension](dualsense-haptics.md) uses the GPL-3.0-or-later protocol definition from [Nonary/moonlight-common-c](https://github.com/Nonary/moonlight-common-c) and interoperates with Nonary's Vibepollo hosts. DualSense HID report handling is adapted from SDL and retains its original notice in the source.

The optional Vulkan renderer for MediaCodec streams (`app/src/main/jni/vulkan`) is adapted from [MoreOrLessSoftware/moonlight-android](https://github.com/MoreOrLessSoftware/moonlight-android) at commit [3e6665e](https://github.com/MoreOrLessSoftware/moonlight-android/tree/3e6665e7d192ac5e7f639f8f8172cda04701924e) (GPL-3.0).

The desktop client ports the keyboard table, controller mapping, special key combinations, FFmpeg decoder setup, D3D11VA surface handling and default bitrate curve from [moonlight-qt](https://github.com/moonlight-stream/moonlight-qt) by Cameron Gutman and contributors (GPL-3.0). Its VRR pacing, tearing presentation and D3D11VA rendering were checked against Nonary's (Chase Payne) moonlight-qt fork (GPL-3.0). Both clients build on [moonlight-common-c](https://github.com/moonlight-stream/moonlight-common-c) (GPL-3.0). The [desktop credits](../protocol/desktop/README.md#credits) list the details and the Figtree font license (SIL OFL 1.1).

Retain the bundled third-party notices when redistributing: [PyroWave, Granite, volk, libadrenotools and renderer credits](../app/src/main/assets/pyrowave_notices.txt), [AMD FSR 1.0 shaders (MIT)](../app/src/main/assets/NOTICE-FSR1.txt), and [Qualcomm SGSR 1 shaders (BSD-3-Clause)](../app/src/main/assets/NOTICE-SGSR1.txt). Native dependencies retain their license notices in the source submodules and vendored source directories. libadrenotools supplies the per-app Vulkan driver loader; it does not replace the Android system driver.
