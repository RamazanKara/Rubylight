# Nightjar

Host-neutral Moonlight-protocol client for Android and desktop (public repo RamazanKara/nightjar, mirrored from this folder).

- Start from the [code map](docs/code-map.md) instead of searching: crates, both apps, the Android app's two engines, every file a setting, a control message or a host-dependent feature touches, and the narrowest test.
- Gate host-specific extras by the `HostCapabilities` model, never by brand checks. User-facing text says Nightjar and stays host-neutral.
- Android and desktop stay at parity: a feature or setting lands on both in one change, with shared logic in the `client/protocol` Rust crate. On-screen touch controls are Android-only.
- Desktop (`protocol/desktop`) builds for Windows, Linux and macOS. `cfg(unix)` includes macOS: gate Linux-only code (VAAPI, Vulkan video, DMA-BUF, D-Bus, sysfs) with `target_os = "linux"`. No Mac is available here; the macOS CI job is the Mac gate. A changed shader `.spv` needs its Metal translation regenerated (`protocol/desktop/src/metal/generate.py`).
- A behaviour whose default is in question becomes a setting in its fitting section (Debug is for diagnostic switches only); measurements decide the default. Never delete a setting.
- A setting's help says, in this order, what the player gets, what it costs (delay, picture, network, battery, heat, or what stops working) and when to change it ("Leave it on unless ..."), in two short sentences. Facts from the code or our measurements only. Well-known terms (VRR, HDR, bitrate, HEVC) stay as they are; a lesser-known one gets a few words of explanation. Both apps and all translations change together.
- The overlay's "Host processing" is the video header's per-frame value, as stock Moonlight shows it; other overlay timings cover only Nightjar's own interval.
- Never change applicationId `com.butterpollo.client` or the signing key: in-place updates depend on them.
- Never rename existing preference keys (`SettingsKeysTest` enforces this).
- Native moonlight-common-c fixes live in `moonlight-core/patches/*.patch`; Gradle and `test-native.sh` reapply the set when its stamp changes.

## Release signing

Keystore and `keystore.properties` live in `C:\Users\ramaz\git\signing\nightjar\`. For a release build, copy `keystore.properties` to the Gradle root (`client/`, gitignored), build, then delete it. Verify with `apksigner verify --print-certs`: SHA-256 must be `312ed322c7c29d1aec4fae4a3979ebcb557af44d821d7f4746c0968548eb630b`. Never print or commit the properties file.
