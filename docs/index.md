# Rubylight documentation

Stream your PC games and desktop to the Android screen you want to use. Rubylight brings your library, sound and controls together so you can start playing from your phone, tablet or TV.

| I want to… | Guide |
| --- | --- |
| Start my first stream | [Quick start](quick-start.md) |
| Connect a PC and grant access | [Pairing and permissions](pairing.md) |
| Understand a setting or choose a preset | [Settings reference](settings.md) |
| Use a controller, touch, mouse or keyboard | [Controls](controls.md) |
| Tune picture quality and response time | [Video and latency](video-and-latency.md) |
| Fix a problem or export a report | [Troubleshooting](TROUBLESHOOTING.md) |
| Build and sign an APK | [Building](building.md) |
| Launch games from ES-DE, Daijisho or Pegasus | [Frontends](FRONTENDS.md) |
| Find a short answer | [FAQ](faq.md) |

Development references: [host feature matrix](PARITY-MATRIX.md), [October 2026 audit and remaining gaps](android-audit-2026-10.md) and [documentation checks](building.md#documentation-checks).

[Download Rubylight](../README.md#download) · [Privacy](PRIVACY.md)

## License and credits

Rubylight is licensed under [GPL-3.0](../LICENSE.txt). It includes Moonlight Android code by [Cameron Gutman](https://github.com/cgutman), [Diego Waxemberg](https://github.com/dwaxemberg), Aaron Neyer, [Andrew Hennessy](https://github.com/yetanothername) and contributors. Moonlight began as a project at [MHacks](https://mhacks.org/) by students at [Case Western Reserve University](https://case.edu/).

Apollo extension behavior draws on [Artemis Android](https://github.com/ClassicOldSong/moonlight-android) by ClassicOldSong and contributors (GPL-3.0). Rubylight also streams from Sunshine and Apollo hosts.

Retain the bundled third-party notices when redistributing: [PyroWave, Granite, volk, libadrenotools and renderer credits](../app/src/main/assets/pyrowave_notices.txt), [AMD FSR 1.0 shaders (MIT)](../app/src/main/assets/NOTICE-FSR1.txt), and [Qualcomm SGSR 1 shaders (BSD-3-Clause)](../app/src/main/assets/NOTICE-SGSR1.txt). Native dependencies retain their license notices in the source submodules and vendored source directories. libadrenotools supplies the per-app Vulkan driver loader; it does not replace the Android system driver.
