# Nightjar documentation

Stream your PC games and desktop to the Android screen you want to use. Nightjar brings your library, sound and controls together so you can start playing from your phone, tablet or TV. It works with Apollo, NVIDIA GameStream, Rubylight, Sunshine, Vibepollo and other Moonlight-compatible hosts.

| I want to… | Guide |
| --- | --- |
| Start my first stream | [Quick start](quick-start.md) |
| Connect a PC and grant access | [Pairing and permissions](pairing.md) |
| Understand a setting or choose a preset | [Settings reference](settings.md) |
| Use Home, a game's page and the in-stream Session and graphics menu (Rubylight 3.0) | [Home and the session menu](console.md) |
| Try the Windows and Linux client or beam-raced PyroWave | [Desktop client](../protocol/desktop/README.md) · [Beam racing](beam-racing.md) · [Early decode](early-decode.md) · [PyroWave decode](pyrowave-decode.md) |
| Use a controller, touch, mouse or keyboard | [Controls](controls.md), [USB and Bluetooth pads in the menus](gamepad-navigation.md) |
| Tune picture quality and response time | [Video and latency](video-and-latency.md) |
| Recover interruptions or adapt resolution and FPS | [Recovery and automatic quality](recovery-and-quality.md) |
| Fix a problem or export a report | [Troubleshooting](TROUBLESHOOTING.md) |
| Understand a stutter and try a reversible change | [Stream health](stream-health.md) |
| Measure my device and share the results | [Device results](devices.md) |
| Connect from outside my home | [Remote play setup](remote-setup.md) |
| Build and sign an APK | [Building](building.md) |
| Launch games from Beacon, ES-DE, Daijisho or Pegasus | [Frontends](FRONTENDS.md) |
| Find a short answer | [FAQ](faq.md) |
| Translate Nightjar or fix a translation | [Translations](TRANSLATIONS.md#help-translate) |

Development references: [host feature matrix](PARITY-MATRIX.md), [October 2026 audit and remaining gaps](android-audit-2026-10.md), [rename history](RENAME.md), the Experimental [refresh rate switching](expected-refresh-switch.md) and [HDR10](expected-hdr10.md) notes, and [documentation checks](building.md#documentation-checks).

[Download Nightjar](../README.md#download) · [Privacy](PRIVACY.md)

## License and credits

Nightjar is licensed under [GPL-3.0](../LICENSE.txt). It includes Moonlight Android code by [Cameron Gutman](https://github.com/cgutman), [Diego Waxemberg](https://github.com/dwaxemberg), Aaron Neyer, [Andrew Hennessy](https://github.com/yetanothername) and contributors. Moonlight began as a project at [MHacks](https://mhacks.org/) by students at [Case Western Reserve University](https://case.edu/).

Apollo extension behavior draws on [Artemis Android](https://github.com/ClassicOldSong/moonlight-android) by ClassicOldSong and contributors (GPL-3.0). The decoder low-latency keys for NVIDIA Tegra, MediaTek and Snapdragon follow Artemis's decoder setup and the [moonlight-noir fork](https://github.com/derflacco/moonlight-android) by derflacco (GPL-3.0); see [decoder flags](decoder-flags.md).

The [DualSense waveform extension](dualsense-haptics.md) uses the GPL-3.0-or-later protocol definition from [Nonary/moonlight-common-c](https://github.com/Nonary/moonlight-common-c) and interoperates with Nonary's Vibepollo hosts. DualSense HID report handling is adapted from SDL and retains its original notice in the source.

The optional Vulkan renderer for MediaCodec streams (`app/src/main/jni/vulkan`) is adapted from [MoreOrLessSoftware/moonlight-android](https://github.com/MoreOrLessSoftware/moonlight-android) at commit [3e6665e](https://github.com/MoreOrLessSoftware/moonlight-android/tree/3e6665e7d192ac5e7f639f8f8172cda04701924e) (GPL-3.0).

Retain the bundled third-party notices when redistributing: [PyroWave, Granite, volk, libadrenotools and renderer credits](../app/src/main/assets/pyrowave_notices.txt), [AMD FSR 1.0 shaders (MIT)](../app/src/main/assets/NOTICE-FSR1.txt), and [Qualcomm SGSR 1 shaders (BSD-3-Clause)](../app/src/main/assets/NOTICE-SGSR1.txt). Native dependencies retain their license notices in the source submodules and vendored source directories. libadrenotools supplies the per-app Vulkan driver loader; it does not replace the Android system driver.
