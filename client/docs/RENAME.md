# Rename history

[Documentation](index.md)

| When | Name | Why |
| --- | --- | --- |
| Until 2026-10-09 | Butterpollo | The client shared its name with the owner's Windows host. |
| 2026-10-09 | Rubylight Android | The host was renamed Rubylight and the client followed it. |
| 2026-10-14 | **Nightjar** | The client became a standalone Moonlight-protocol client for every host: Apollo, NVIDIA GameStream, Rubylight, Sunshine and Vibepollo. Rubylight is now one supported host among several. |

Nightjar still uses Rubylight's extensions (PyroWave, PC display modes, host metrics, mid-stream
reconfiguration and others) when a host advertises them. See the [host feature matrix](PARITY-MATRIX.md).

## What changed for Nightjar

- **One brand source.** `brandName=Nightjar` in `gradle.properties` generates the `app_name`
  string, the launcher labels (Nightjar, Nightjar (Root) and their Debug variants) and
  `BuildConfig.BRAND_NAME`.
- **Icon and graphics.** A nightjar crossing an amber moon (`brand_night` #14161F,
  `brand_moon` #F2A93B in `values/brand_colors.xml`): adaptive and themed launcher icon, TV
  banner and channel logo, store icon and graphics, README banner, website images.
- **Text.** English and German strings, docs, website and store listing call the app Nightjar
  and send users to "your host's settings or web UI" instead of one host's pages.
- **Files the app writes for you.** `nightjar-problem.txt`, `nightjar-keyboard-shortcuts.json`,
  `nightjar-touch-layout.json`, `nightjar-latency.csv`. Imports read the file content, so older
  `rubylight-*.json` exports still load.
- **Code names.** `NightjarApplication`, the `Nightjar` theme styles, Wi-Fi lock tags, the Vulkan
  application name and the root evdev socket name.
- **Repository and site.** `RamazanKara/nightjar` and
  `https://ramazankara.github.io/nightjar/`.
- **CI and scripts.** Artifacts `nightjar-debug` and `nightjar-device-test`, the prerelease APK
  `nightjar-arm64-debug.apk`, and the smoke-test variables `NIGHTJAR_AVD`,
  `NIGHTJAR_EMULATOR_GPU` and `NIGHTJAR_BOOT_TIMEOUT` (the old `RUBYLIGHT_*`/`BUTTERPOLLO_*`
  names are no longer read).

## Kept on purpose

| Identifier | Why it keeps its old name |
| --- | --- |
| Application IDs `com.butterpollo.client`, `.root` and the `.debug` suffix | Installed copies update in place and keep their pairings, settings, shortcuts and signing continuity. |
| Java namespace and packages `com.limelight` | Moonlight heritage. Renaming would touch every file and gain users nothing. |
| `BP_KEYSTORE`, `BP_KEYSTORE_PASSWORD`, `BP_KEY_ALIAS`, `BP_KEY_PASSWORD` | The release environment and `keystore.properties` setup depend on them. |
| SharedPreferences names, `computers4.db`, `client.crt`, `client.key`, `uniqueid` | Saved PCs, settings and the pairing identity. |
| Provider authorities `${applicationId}.reports` and `poster.${applicationId}` | Derived from the application ID. |
| ES-DE system `butterpollo` and emulator `BUTTERPOLLO` | Users' ROM and `downloaded_media` folders are named after them. Only the displayed name changed to Nightjar; export again and restart ES-DE to see it. |
| `butterpollo-latency.csv` in the app's private files | Keeps the last stream's timings across the update. The export is offered as `nightjar-latency.csv`. |
| Host wire identifiers: `RustHostVersion`, `RustHostEncoderBitrate*`, `x-rl-control`, `rlControllers`, control messages 0x5530–0x5535 | Defined by the Rubylight host. |
| Protocol crate `rubylight-protocol` / `rubylight-protocol-ffi`, `librubylight_protocol.a`, `rubylight_protocol.h` | The host pins this crate; it describes the host's wire formats. |
| Resource key `host_details_software_butterpollo` | Only the key; its value is the host label "Rubylight %1$s". |
| Intent extras `rubylight.outputDisplay`, `rubylight.outputResumeOnly`, `rubylight.microphoneMuted` | Runtime-only and internal. Left alone during the rename to avoid clashing with parallel work in `Game.java`; safe to rename together with `GameRecoveryTest` and `GameDisplayLifecycleTest`. |
| Debug task affinity `com.butterpollo.client.demohome` | Follows the application ID. |
| Test fixtures `app/src/test/resources/protocol/butterpollo-*.xml` | Recorded Rubylight host responses from commits made while the host was called Butterpollo. |
| `internal/BUTTERPOLLO_PARITY.md`, store changelogs | Historical records, kept as written. |

Protocol identities that were never product names stay as well: the client certificate CN
`NVIDIA GameStream Client`, the key alias `Limelight-RSA`, the mDNS service `_nvstream._tcp`, the
pairing `devicename` and `uniqueid`, and RTSP `X-GS-ClientVersion`.

## Old URLs

GitHub redirects the old repository names (`butterpollo-android`, `rubylight-android`,
`nightjar-android`) to `nightjar`. Never create a new repository under an old name, or the redirect stops.

GitHub Pages does not redirect: the site moved from
`https://ramazankara.github.io/rubylight-android/` (and briefly `/nightjar-android/`) to
`https://ramazankara.github.io/nightjar/`, so the Play Console privacy policy URL must be
updated after the site is republished (see [store readiness](../internal/STORE-READINESS.md)).
