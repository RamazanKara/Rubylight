# Rubylight documentation

Stream your PC games and desktop to the Android screen you want to use. Rubylight brings your library, sound and controls together so you can start playing from your phone, tablet or TV.

| I want to… | Guide |
| --- | --- |
| Start my first stream | [Quick start](quick-start.md) |
| Connect a PC and grant access | [Pairing and permissions](pairing.md) |
| Understand a setting or choose a preset | [Settings reference](settings.md) |
| Use a controller, touch, mouse or keyboard | [Controls](controls.md) |
| Tune picture quality and response time | [Video and latency](video-and-latency.md) |
| Fix a problem or export a report | [Troubleshooting](troubleshooting.md) |
| Build and sign an APK | [Building](building.md) |
| Launch games from ES-DE, Daijisho or Pegasus | [Frontends](frontends.md) |
| Find a short answer | [FAQ](faq.md) |

Development references: [Rubylight host parity matrix](PARITY-MATRIX.md), [device checklist](device-checklist.md) and [documentation checks](building.md#documentation-checks).

[Download Rubylight](../README.md#download) · [Privacy](PRIVACY.md)

## Licenses and Credits

Rubylight is licensed under [GPL-3.0](../LICENSE.txt). Credits include Moonlight Android by Cameron Gutman, Diego Waxemberg, Aaron Neyer, Andrew Hennessy and contributors; Artemis Android by ClassicOldSong and contributors; and the Android PyroWave renderer adapted from joemossjr16's work.

Keep the source copyright notices and these complete bundled notices with redistributions:

- [PyroWave, Granite and volk: MIT licenses and renderer credits](../app/src/main/assets/pyrowave_notices.txt).
- [FSR 1.0 EASU/RCAS shaders: AMD copyright and MIT license](../app/src/main/assets/NOTICE-FSR1.txt).
- [SGSR 1 shader: Qualcomm copyright and BSD-3-Clause license](../app/src/main/assets/NOTICE-SGSR1.txt).
- [DualSense report layouts adapted from SDL: Sam Lantinga's zlib notice](../app/src/main/java/com/limelight/binding/input/driver/DualSenseReport.java).

Java and native dependencies retain their own licenses. Fetch the [native submodules](../.gitmodules) when preparing a source distribution; the app license does not replace dependency notices.
