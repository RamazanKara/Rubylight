# Nightjar documentation

[Nightjar](../README.md) · [Download](../README.md#download) · [Settings reference](settings.md) · [Troubleshooting](TROUBLESHOOTING.md)

Nightjar streams games from your PC to Android phones, tablets and handhelds, Android TV and Fire TV, and Windows, Linux and macOS computers. It works with any Moonlight-compatible host, such as Sunshine, Apollo, Vibepollo, Rubylight and NVIDIA GameStream, and turns on each host extension from the capabilities the host reports.

Find your task below. Conventions used throughout:

- The **PC** is the computer you stream from; the **host** is the streaming software running on it; **this device** is the one running Nightjar.
- Settings are named exactly as they appear in the app, with their path: **Settings → Streaming → Recover interrupted streams**.
- Where a feature depends on the host, the page names the capability it needs and the hosts known to provide it.
- **Windows and Linux** in a page's text means the desktop app, which also runs on macOS; [Nightjar for macOS, Windows and Linux](../protocol/desktop/README.md#macos) says what differs on a Mac.

<a id="user-guide"></a>

## Get started

New to Nightjar? Read the [quick start](quick-start.md): install a host, add the PC, pair and start the first stream.

| I want to | Read |
| --- | --- |
| Install Nightjar | [Download](../README.md#download) · [Nightjar for macOS, Windows and Linux](../protocol/desktop/README.md) · [Steam Deck and Linux handhelds](steam-deck.md) |
| Stream for the first time | [Quick start](quick-start.md) |
| Pair with a PIN, a one-time PIN or a QR code | [Pairing and permissions](pairing.md) |
| Know what Nightjar can do with my host | [Features](features.md) · [Host feature support](PARITY-MATRIX.md) |

## Everyday use

| I want to | Read |
| --- | --- |
| Find my way around Home, a game's page and the PC's options | [Home and the session menu](console.md) |
| Use the Control Center during a stream, or leave a game | [The Control Center](controls.md#the-control-center) · [Leaving a game](controls.md#leaving-a-game) |
| Wake or put the PC to sleep | [Wake a sleeping PC](pairing.md#wake-a-sleeping-pc) · [The PC's power](console.md#the-pcs-power-and-host-updates) |
| Pause the game on the PC, or save a clip of the last moments | [Quick Suspend](console.md#quick-suspend) · [Instant Replay](console.md#instant-replay) |
| Keep the PC's desktop hidden while a game starts | [The launch curtain](console.md#the-launch-curtain) |
| Start games from Beacon, ES-DE, Daijisho, Pegasus or the TV's home screen | [Launcher integrations](FRONTENDS.md) |
| Share the clipboard, files, folders or USB devices with the PC | [Clipboard, files, folders and USB devices](sharing.md) |
| Change how a setting, a profile or one game behaves | [Settings reference](settings.md) |
| Use Nightjar on a TV, or with a screen reader | [On a TV](console.md#on-a-tv) · [Accessibility on Android](console.md#accessibility-on-android) · [Accessibility on Windows and Linux](console.md#accessibility-on-windows-and-linux) |

## Play anywhere

| I want to | Read |
| --- | --- |
| Play away from home | [Play away from home](remote-setup.md) |
| Reach a PC without a VPN | [Without a VPN](remote-setup.md#without-a-vpn) |
| Wake a sleeping PC from away | [Wake the PC from away](remote-setup.md#wake-the-pc-from-away) · [Run a wake relay](../protocol/desktop/README.md#away-wake-relay) |
| Keep my stream when the network changes | [Recovering a connection](recovery-and-quality.md#recovering-a-connection) |

## Play together

| I want to | Read |
| --- | --- |
| Join the game someone else plays on the PC, or let a friend join mine | [Play together](console.md#play-together) |
| Take a seat in a split-screen co-op game | [Join co-op](console.md#join-co-op) |
| Move my game to the device in my hand | [Home](console.md#home) |

## Picture and smoothness

| I want to | Read |
| --- | --- |
| Choose a codec, HDR, a refresh rate or an upscaler | [Video and latency](video-and-latency.md) |
| Keep the picture clear of a camera cutout or curved edges | [Video size](video-and-latency.md#video-size) · [Picture position](video-and-latency.md#picture-position) |
| Keep a VRR screen on the game's rhythm | [Real VRR](real-vrr.md) |
| See camera turns on the next refresh with a Rubylight PC | [Frame Warp](video-and-latency.md#frame-warp) |
| Let Nightjar pick resolution, frame rate and bitrate | [Recovery and automatic quality](recovery-and-quality.md) |
| Find out why the stream stuttered | [Stream health](stream-health.md) |

## Controllers

| I want to | Read |
| --- | --- |
| Set up a controller, gyro, touch, mouse, keyboard or pen | [Controls](controls.md) · [The controller editor](controls.md#the-controller-editor) |
| Use the menus with a controller or a remote | [Gamepad navigation](gamepad-navigation.md) · [Menu navigation](controls.md#menu-navigation) |
| Use on-screen controls on a phone or tablet | [On-screen controls](controls.md#on-screen-controls) |
| Get DualSense haptics and controller audio over USB | [USB DualSense audio and haptics](dualsense-haptics.md) |
| Use the microphone | [Microphone](controls.md#microphone) |
| Play from a Steam Deck in Game Mode | [Steam Deck and Linux handhelds](steam-deck.md) |

## Troubleshooting

| Page | Contents |
| --- | --- |
| [Troubleshooting](TROUBLESHOOTING.md) | Symptoms, causes and fixes, and how to share a problem report |
| [Stream health](stream-health.md) | Reading the Stream Doctor (**Why did my stream stutter?**): the cause, one-tap fixes with undo and upscaler advice |
| [FAQ](faq.md) | Short answers to common questions |

## Reference

| Page | Contents |
| --- | --- |
| [Settings reference](settings.md) | Every setting on Android and the desktop clients, with its default and advice |
| [Features](features.md) | Every feature, with a link to its guide |
| [Host feature support](PARITY-MATRIX.md) | Each host capability and the hosts known to provide it |
| [Android and desktop compared](CLIENT-PARITY.md) | Feature availability in the two clients |
| [Decoder flags](decoder-flags.md) | The low-latency decoder settings applied per chip family |
| [Device results](devices.md) | Decoder benchmark results from real devices, and how to add yours |
| [Privacy](PRIVACY.md) | What Nightjar stores, what it sends and to whom |

<a id="contributing"></a>

## For developers

| Page | Contents |
| --- | --- |
| [Building](building.md) | Toolchains, tests, signing, the release workflow and the [documentation checks](building.md#documentation-checks) |
| [Code map](code-map.md) | Where each part lives, the files one kind of change touches, and the narrowest test for it |
| [Translations](TRANSLATIONS.md) | Supported languages and how to contribute one |
| [Device results](devices.md#send-results) | Submitting a benchmark from your device |

## Engineering notes

Design notes, measurements and audits for contributors. They describe internals and are not needed to use Nightjar.

| Note | Subject |
| --- | --- |
| [Beam racing](beam-racing.md), [early decode](early-decode.md), [mouse pace](mouse-pace.md) | Desktop presentation timing, and input sent at the mouse's pace |
| [PyroWave GPU decoding](pyrowave-decode.md), [PyroWave decode stages](pyrowave-decode-stages.md) | The desktop PyroWave decoder and its measured stages |
| [Measurements](measurements.md) | The figures behind Real VRR, frame pacing and PyroWave decoding, with their conditions |
| [MetalFX on macOS](macos-metalfx.md) | The macOS spatial upscaler and its verification |
| [Refresh rate switching](expected-refresh-switch.md), [HDR on Android displays](expected-hdr10.md) | How the Android app requests refresh rates and HDR, and reports refusals |
| [Client roaming](protocol/client-roaming.md), [host cursor shapes](protocol/cursor-shape.md), [Frame Warp](protocol/frame-warp.md) | Protocol extensions proposed for Moonlight-compatible hosts |
| [Android audit, October 2026](android-audit-2026-10.md) | Audit findings and their resolution |
| [Rename history](RENAME.md) | The renames to Nightjar, and the identifiers that keep their old names |
| [Demo mode](demo/README.md) | The offline demo fixtures used for screenshots and the promotional video |

## License and credits

Nightjar is licensed under [GPL-3.0](../LICENSE.txt). It includes Moonlight Android code by [Cameron Gutman](https://github.com/cgutman), [Diego Waxemberg](https://github.com/dwaxemberg), Aaron Neyer, [Andrew Hennessy](https://github.com/yetanothername) and contributors. Moonlight began as a project at [MHacks](https://mhacks.org/) by students at [Case Western Reserve University](https://case.edu/).

Apollo extension behavior draws on [Artemis Android](https://github.com/ClassicOldSong/moonlight-android) by ClassicOldSong and contributors (GPL-3.0). The decoder low-latency keys for NVIDIA Tegra, MediaTek and Snapdragon follow Artemis's decoder setup and the [moonlight-noir fork](https://github.com/derflacco/moonlight-android) by derflacco (GPL-3.0). The Exynos and Tensor keys follow [Moonlight X](https://github.com/MoreOrLessSoftware/moonlight-android/tree/mlsoft-x) by MoreOrLessSoftware (GPL-3.0); see [decoder flags](decoder-flags.md). The soft keyboard for the host's text fields (when it opens, staying closed once closed, and moving the stream up over it), the declared-only Snapdragon keys, the shorter MediaTek queues on the first configure try, the Amlogic HEVC low-latency exception, decoder input buffer growth and the controller gyroscope liveness check follow [Moonlight V+](https://github.com/qiin2333/moonlight-vplus) by qiin2333 and contributors (GPL-3.0).

The per-controller gyroscope mapping (axes, inversion, sensitivity and the Flydigi Apex 4 and Vader 4 Pro preset) and the in-stream motion readings are ported from [jvanderv42's Nightjar fork](https://github.com/jvanderv42/nightjar) at commit d0e14cf25 (GPL-3.0); the desktop client's gyroscope axes and sensitivity follow the same design.

The desktop Settings screen predicts the display mode Windows' Fullscreen takes by SDL 3's own rule (`SDL_GetClosestFullscreenDisplayMode`, SDL 3.4.20, [libsdl-org/SDL](https://github.com/libsdl-org/SDL), zlib license, Copyright (C) 1997-2026 Sam Lantinga), in `protocol/desktop/src/app/session.rs`.

The [DualSense waveform extension](dualsense-haptics.md) uses the GPL-3.0-or-later protocol definition from [Nonary/moonlight-common-c](https://github.com/Nonary/moonlight-common-c) and interoperates with Nonary's Vibepollo hosts. DualSense HID report handling is adapted from SDL and retains its original notice in the source.

The optional Vulkan renderer for MediaCodec streams (`app/src/main/jni/vulkan`) is adapted from [MoreOrLessSoftware/moonlight-android](https://github.com/MoreOrLessSoftware/moonlight-android) at commit [3e6665e](https://github.com/MoreOrLessSoftware/moonlight-android/tree/3e6665e7d192ac5e7f639f8f8172cda04701924e) (GPL-3.0).

The desktop client ports the keyboard table, controller mapping, special key combinations, FFmpeg decoder setup, D3D11VA surface handling and default bitrate curve from [moonlight-qt](https://github.com/moonlight-stream/moonlight-qt) by Cameron Gutman and contributors (GPL-3.0). Its VRR pacing, tearing presentation and D3D11VA rendering were checked against Nonary's (Chase Payne) moonlight-qt fork (GPL-3.0). Nightjar's controller button hints (`protocol/policy/src/input_glyphs.rs`, `protocol/desktop-ui/src/glyphs.rs`) and the desktop client's Windows D3D11 draw pass, AI enhancement, touchpad input, Linux USB forwarding, crash dumps and update check are ported from [Moonlight V+ PC](https://github.com/qiin2333/moonlight-qt) by qiin2333 and contributors (GPL-3.0). Both clients build on [moonlight-common-c](https://github.com/moonlight-stream/moonlight-common-c) (GPL-3.0). The [desktop credits](../protocol/desktop/README.md#credits) list the details and the Figtree font license (SIL OFL 1.1).

Artwork is provided by [SteamGridDB](https://www.steamgriddb.com) and game facts (genres, themes, time to beat) by [IGDB.com](https://www.igdb.com), both with the player's own keys and neither bundled; their images and data stay under their services' terms. IGDB's API is free for non-commercial use under the Twitch Developer Services Agreement. Nightjar's client for both is its own, written from their public API documentation.

Nightjar for macOS on Apple silicon bundles KosmicKrisp, the Vulkan-on-Metal driver from Mesa 26.2.7 by LunarG and the Mesa contributors (MIT), with SPIRV-Tools and SPIRV-Headers vulkan-sdk-1.4.357.0 (Apache-2.0 and the Khronos MIT-style license) and the Khronos Vulkan loader vulkan-sdk-1.4.357.0 (Apache-2.0); their license texts are in `Nightjar.app/Contents/Resources/Licenses`. The PyroWave library it bundles is Granite-based and carries a macOS build patch to Granite (MIT).

Retain the bundled third-party notices when redistributing: [PyroWave, Granite, volk, libadrenotools and renderer credits](../app/src/main/assets/pyrowave_notices.txt), [AMD FSR 1.0 shaders (MIT)](../app/src/main/assets/NOTICE-FSR1.txt), and [Qualcomm SGSR 1 shaders (BSD-3-Clause)](../app/src/main/assets/NOTICE-SGSR1.txt). Native dependencies retain their license notices in the source submodules and vendored source directories. libadrenotools supplies the per-app Vulkan driver loader; it does not replace the Android system driver.
