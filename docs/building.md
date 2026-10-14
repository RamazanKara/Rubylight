# Building Rubylight

[Documentation](index.md)

## Install a published or development build

Get published APKs from [Rubylight releases](https://github.com/RamazanKara/rubylight-android/releases), or build one yourself as described below. Open the APK on Android 5.0 or newer and allow installation from the app opening it; no root is needed. With USB debugging enabled, `adb install -r app-nonRoot-debug.apk` also installs it.

Updates require the same signing key. Debug APKs built on different machines use different keys; changing keys requires uninstalling the old app, which removes settings and pairing identity. The normal package ID remains `com.butterpollo.client`; debug builds use `com.butterpollo.client.debug` and a `(Debug)` label.

## Toolchain and source

Use JDK 21 and the Android SDK/NDK already installed on your machine. Android 16 Robolectric tests require JDK 21. The repository pins compile SDK 37, target SDK 36, minimum SDK 21, NDK `29.0.14206865`, Android Gradle Plugin `9.4.0` and Gradle `9.7.1`. Check [app/build.gradle](../app/build.gradle), [build.gradle](../build.gradle) and the [wrapper configuration](../gradle/wrapper/gradle-wrapper.properties) when updating the toolchain.

The SDK Manager package for compile SDK 37 is `platforms;android-37.0`, not `platforms;android-37`. Install it together with `build-tools;36.0.0`, `platform-tools` and `ndk;29.0.14206865` when preparing a clean build environment.

Clone the Android repository with its native submodules:

```sh
git clone --recurse-submodules https://github.com/RamazanKara/rubylight-android.git
cd rubylight-android
```

For an existing checkout:

```sh
git submodule update --init --recursive --jobs 2
```

[.gitmodules](../.gitmodules) identifies the native core submodule; recursive initialization includes its dependencies. Use a Windows JDK and Windows SDK for PowerShell, or Linux toolchains when building inside WSL. Set `JAVA_HOME` to the installed JDK 21 directory and `ANDROID_HOME` to the installed SDK, or use `sdk.dir` in untracked `local.properties`. SDK licenses and the pinned SDK/NDK must already be available.

## Build and tests

From the repository root in PowerShell:

```powershell
.\gradlew.bat --no-daemon --max-workers=2 :app:assembleNonRootDebug :app:lintNonRootDebug :app:testNonRootDebugUnitTest
```

On Linux/macOS:

```sh
bash gradlew --no-daemon --max-workers=2 :app:assembleNonRootDebug :app:lintNonRootDebug :app:testNonRootDebugUnitTest
```

`--no-daemon` avoids retaining a build daemon and `--max-workers=2` limits Gradle workers. Native make and unit-test forks are also capped at two in the app build. Add `--offline` only when the wrapper distribution and dependencies are cached; it does not supply missing artifacts. For a smaller arm64-only APK, add `-PabiFilters=arm64-v8a`; comma-separated ABI values are also accepted.

The JVM suite includes Robolectric tests. Reports go to `app/build/reports/tests/testNonRootDebugUnitTest/index.html` and `app/build/reports/lint-results-nonRootDebug.html`. A failed lint/test task is a failed check even when assembly succeeds.

CI rebuilds the Rust protocol libraries from source and runs all native regression tests on Linux, including ASan/UBSan and the production Rust parser:

```sh
bash scripts/test-native.sh
```

This requires a C/C++ compiler, OpenSSL development headers and Rust. Gradle and the native runner apply the checked-in native fixes before compiling and fail if the patches do not apply. [The Android workflow](../.github/workflows/android.yml) also builds both release flavors and runs both flavors' unit tests. The optional [emulator smoke script](../scripts/emulator-smoke.py) requires an already configured Android SDK, Python and its expected AVD.

### UI smoke captures

After building debug, `python scripts/emulator-smoke.py` uses the `sunset` Android 35 AVD in a headless disposable session. Use `--avd NAME` for another Google APIs AVD and `--port 5556` if the default emulator port is occupied. It refuses to replace an emulator already using its selected port and stops only its own emulator in a `finally` block. It installs the APK, adds a loopback server-info fixture and walks the pairing guide, one-time PIN and host-details dialogs, manual-address validation, PC profile input/save/reopen/reset, settings, bitrate and global reset. It also checks search navigation, visible card menus, long-sheet scrolling and per-PC global inheritance. The fixture holds the PIN prompt open without completing pairing or starting a stream.

The capture matrix is 393 dp and 412 dp at default font size, plus 393 dp at font scales 1.3 and 2.0, including landscape at 2.0. Debug builds also exercise library search, no results, clearing and query restoration through rotation using the offline demo. Screenshots go to `docs/screenshots/ui-v2/` (native dimensions, palette PNG when Pillow is available); UI dumps and logcat go to `app/build/emulator-smoke/`. Use `--output-dir D:\CodexArtifacts\rubylight-ui-smoke` to keep both under a separate task directory. The debug-only overlay preview uses labeled synthetic values, supports rotation and long-press switching, and is absent from release builds. Its captures are `overlay-statistics.png` and `overlay-debug.png`, with `-landscape` variants. Long-press switching belongs to this preview; during a real stream, select the mode from **stream menu → Overlay**.

For a focused repeat of layout and navigation checks after the functional flows have passed, add `--ui-only`. It still starts a fresh disposable install, adds the fixture PC, runs the full capture matrix and checks app crashes. It skips profile/preset persistence and import validation, so it does not replace the default full smoke run.

Review both portrait overlays before the device pass. Emulator checks do not establish custom-driver compatibility or latency gains on Snapdragon hardware; validate those with a live stream on the target phone. Add live stream-menu captures for the same viewport/font matrix and check permission filtering, Quit app confirmation, mode persistence, Copy stats, the keyboard shortcut, German labels and PiP on a phone. Saved [settings](screenshots/04-settings.png), [bitrate](screenshots/06-bitrate.png), [PC profile](screenshots/09-host-profile.png), [pairing guide](screenshots/01-launch.png), [host details](screenshots/15-host-details.png) and [overlay](screenshots/07-latency-overlay.png) captures show UI fixtures, not measured hardware streaming performance.

## Flavors and outputs

| Flavor / task | Package / output |
| --- | --- |
| `nonRoot` | `com.butterpollo.client`; normal Android build, no root required. |
| `root` | `com.butterpollo.client.root`; older root mouse-capture path, with maximum SDK 25. |
| `:app:assembleNonRootDebug` | `app/build/outputs/apk/nonRoot/debug/app-nonRoot-debug.apk` |
| `:app:assembleNonRootRelease` | `app/build/outputs/apk/nonRoot/release/app-nonRoot-release.apk` |
| `:app:bundleNonRootRelease` | `app/build/outputs/bundle/nonRootRelease/app-nonRoot-release.aab` |
| Root equivalents | Replace `NonRoot` with `Root` in task names and `nonRoot` with `root` in output paths. |

Release builds enable R8 shrinking. The source namespace remains `com.limelight`; it is distinct from the Android package ID. English and German are the packaged locales.

## Release signing

The signing hook in [app/build.gradle](../app/build.gradle) reads these four values:

| Environment variable | Root `keystore.properties` entry |
| --- | --- |
| `BP_KEYSTORE` | `storeFile` |
| `BP_KEYSTORE_PASSWORD` | `storePassword` |
| `BP_KEY_ALIAS` | `keyAlias` |
| `BP_KEY_PASSWORD` | `keyPassword` |

Each nonempty environment variable overrides its matching property. Supply all four values using the existing release key. Relative keystore paths resolve from the repository root; use forward slashes in Windows property paths. The properties file and `*.jks`, `*.keystore`, `*.p12` and `*.pfx` files are ignored by Git. Never commit signing keys or passwords, regardless of filename.

Release builds require a dedicated signing key. Missing or partial signing configuration and the standard Android debug key fail the build, including when using the artifact helpers. CI uses a disposable validation key for minified build tests; it never publishes those release APKs. Updates must use the installed app's signing identity.

Create a signing key once and retain it for future updates. JDK 21 supplies `keytool`; it prompts for the keystore password:

```sh
keytool -genkeypair -keystore rubylight.keystore -alias rubylight -keyalg RSA -keysize 3072 -validity 10000
```

An unsigned APK produced outside the normal Gradle signing path cannot be installed directly. With Android SDK Build Tools on your PATH, copy it to `rubylight-unsigned.apk`, align it, sign with the same key and verify:

```sh
zipalign -P 16 -f 4 rubylight-unsigned.apk rubylight-aligned.apk
apksigner sign --ks rubylight.keystore --ks-key-alias rubylight --out rubylight.apk rubylight-aligned.apk
apksigner verify --verbose rubylight.apk
adb install -r rubylight.apk
```

See Android's [APK signing instructions](https://developer.android.com/tools/apksigner).

Build the release APK and bundle directly:

```powershell
.\gradlew.bat --no-daemon --max-workers=2 :app:assembleNonRootRelease :app:bundleNonRootRelease :app:lintNonRootRelease :app:testNonRootDebugUnitTest
```

Or use the artifact helper, which runs release lint and both flavors' unit tests before building:

```powershell
.\scripts\build-release.ps1
```

The Linux helper is `bash scripts/build-release.sh`. Both helpers build a universal APK and bundle with all four default ABIs (`arm64-v8a`, `armeabi-v7a`, `x86`, `x86_64`), then print SHA-256 hashes and byte sizes. They stop on validation/build failure and do not publish. Single-ABI builds are for explicit device testing, not the default downloadable release. Preserve `app/build/outputs/mapping/nonRootRelease/mapping.txt` and native debug symbols for the matching release.

Releases are built and published from a local machine with `gh release create` using the retained release key. Test prereleases carry a debug-signed arm64 APK with the separate debug package ID and are never marked Latest.

## Documentation checks

Use Python 3.10+ and Node.js 18+; these scripts use only standard libraries and do not install packages or contact the network.

```sh
python scripts/generate-settings.py
node --test scripts/check-doc-links.test.mjs scripts/build-release.test.mjs
node scripts/check-doc-links.mjs
```

Use `python3` instead of `python` where that is your Python 3 command. The generator reads the real XML and Java presets; it fails if a preference lacks usage guidance or a preset expression needs review. The CI documentation job regenerates the page and uses `git diff --exit-code -- docs/settings.md` to reject stale committed output.

The [Documentation workflow](../.github/workflows/docs.yml) also checks documentation-only pushes and pull requests, which intentionally do not rebuild Android. Release-helper tests use a disposable fake Gradle launcher to verify task ordering, universal packaging and failure handling; the PowerShell helper receives a static parity check, not execution on Windows.

The link checker checks README and every Markdown page under `docs/`: relative file/image paths, filename case, Markdown heading fragments, explicit HTML anchors and reference links. External URLs are syntax-checked and counted, not fetched; an offline pass does not certify remote availability.
