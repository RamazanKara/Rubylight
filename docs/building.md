# Building Nightjar

[Documentation](index.md)

## Install a published or development build

Get published APKs from [Nightjar releases](https://github.com/RamazanKara/nightjar/releases), or build one yourself as described below. Open the APK on Android 5.0 or newer and allow installation from the app opening it; no root is needed. With USB debugging enabled, `adb install -r app-nonRoot-debug.apk` also installs it.

Updates require the same signing key. Debug APKs built on different machines use different keys; changing keys requires uninstalling the old app, which removes settings and pairing identity. The normal package ID remains `com.butterpollo.client`; debug builds use `com.butterpollo.client.debug` and a `(Debug)` label.

## Toolchain and source

Use JDK 21 and the Android SDK/NDK already installed on your machine. Android 16 Robolectric tests require JDK 21. The repository pins compile SDK 37, target SDK 36, minimum SDK 21, NDK `29.0.14206865`, Android Gradle Plugin `9.4.0` and Gradle `9.7.1`. Check [app/build.gradle](../app/build.gradle), [build.gradle](../build.gradle) and the [wrapper configuration](../gradle/wrapper/gradle-wrapper.properties) when updating the toolchain.

The SDK Manager package for compile SDK 37 is `platforms;android-37.0`, not `platforms;android-37`. Install it together with `build-tools;36.0.0`, `platform-tools` and `ndk;29.0.14206865` when preparing a clean build environment.

Clone the Android repository with its native submodules:

```sh
git clone --recurse-submodules https://github.com/RamazanKara/nightjar.git
cd nightjar
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

With the local build cache on (`org.gradle.caching=true` in `~/.gradle/gradle.properties`), another checkout or a clean build takes compiled classes and the Rust engine from the cache. A cold arm64 debug build and unit-test run in a second checkout dropped from 139 to 58 s.

The JVM suite includes Robolectric tests. Reports go to `app/build/reports/tests/testNonRootDebugUnitTest/index.html` and `app/build/reports/lint-results-nonRootDebug.html`. A failed lint/test task is a failed check even when assembly succeeds.

CI rebuilds the Rust protocol libraries from source and runs all native regression tests on Linux, including ASan/UBSan and the production Rust parser:

```sh
bash scripts/test-native.sh
```

This requires a C/C++ compiler, OpenSSL development headers and Rust. Gradle and the native runner apply the checked-in native fixes before compiling and fail if the patches do not apply. [The Android workflow](../.github/workflows/android.yml) also builds both release flavors, runs both flavors' unit tests and uploads the non-root debug APK as the `nightjar-debug` artifact. The optional [emulator smoke script](../scripts/emulator-smoke.py) requires an already configured Android SDK, Python and its expected AVD.

### UI smoke captures

After building debug, `python scripts/emulator-smoke.py` uses the `sunset` Android 35 AVD in a headless disposable session. Use `--avd NAME` (or set `NIGHTJAR_AVD`) for another Google APIs AVD and `--port 5556` if the default emulator port is occupied. `NIGHTJAR_EMULATOR_GPU` (default `swiftshader`) and `NIGHTJAR_BOOT_TIMEOUT` (seconds, default 180) override the emulator's GPU mode and boot timeout. It refuses to replace an emulator already using its selected port and stops only its own emulator in a `finally` block. It installs the APK, adds a loopback server-info fixture and walks the pairing guide, one-time PIN and host-details dialogs, manual-address validation, PC profile input/save/reopen/reset, settings, bitrate and global reset. In Settings it opens each section, toggles Low-latency decoding and Phone performance hints in Advanced, checks that diagnostics are directly available, and verifies all three performance overlay modes; it opens the bundled Help from the home screen in every viewport. It also checks search navigation, the PC menu and its close button, the PC settings screen and per-PC global inheritance. Every run first force-stops and relaunches the home screen five times and compares the top app bar and content with the window manager's status and navigation bar frames, then checks that D-pad up from the home content reaches Settings or Help. The fixture holds the PIN prompt open without completing pairing or starting a stream.

The capture matrix is 393 dp and 412 dp at default font size, plus 393 dp at font scales 1.3 and 2.0, including landscape at 2.0. The script applies landscape after launching the app so the portrait-only launcher cannot reset the test orientation, waits for physical rotation to settle, and checks both screenshot dimensions and UI-tree rotation. It scrolls the PC menu to its end and requires the final action and Close to retain visible touch targets of at least 48 dp outside system bars. Debug builds also open the demo home, require its running, unpaired and offline PCs in that order with their status lines and short menus, open a row's menu with the D-pad, and require the running game first in the cover grid with its chip and no per-tile action labels. They rotate the offline demo library to landscape before any search, requiring cards and a clear top app bar, then exercise library search, no results, clearing and query restoration through rotation. Screenshots go to `docs/screenshots/ui-v2/` (native dimensions, palette PNG when Pillow is available); UI dumps and logcat go to `app/build/emulator-smoke/`. Use `--output-dir D:\CodexArtifacts\nightjar-ui-smoke` to keep both under a separate task directory. The debug-only overlay preview uses labeled synthetic values, supports rotation and long-press switching, and is absent from release builds. Its captures are `overlay-statistics.png` and `overlay-debug.png`, with `-landscape` variants. It also renders the one-line Compact overlay (`overlay-compact*.png`) and, with `--ez controls true`, the on-screen controls it must keep clear of (`overlay-compact-controls*.png`), in portrait and both landscape rotations, checking that the line stays out of the camera cutout and, in landscape, off every control. Long-press switching belongs to this preview; during a real stream, select the mode from **stream menu → Display → Overlay and diagnostics**. Debug builds then open the demo stream with the first-stream hint (`stream-hint*.png`) and the on-screen gamepad (`stream-compact-controls*.png`), open its real stream menu with Back in each rotation (`stream-menu*.png`, `stream-menu-bottom*.png`), check the quick row, sections, Quit game and the sheet's distance from the cutout, open the overlay page (`stream-menu-overlay-page*.png`), step back with Back twice, and move the D-pad through the rows.

Focused runs reuse the full script's assertions. `--ui-only` installs fresh, adds the fixture PC and runs the UI matrix; it skips profile/preset persistence and import validation, so it does not replace the default full smoke run. Add repeatable `--ui-case` arguments to select `393dp-font-1`, `412dp-font-1`, `393dp-font-1.3`, `393dp-font-2` or `393dp-font-2-landscape`; omitting this option runs every case. For example, `python scripts/emulator-smoke.py --ui-only --ui-case 393dp-font-2-landscape` checks that viewport, then the debug library and overlays when using a debug APK. `--debug-only` runs only those library and overlay fixture checks and requires a debug APK; it cannot be combined with `--ui-only` or `--ui-case`. Every focused run still starts a disposable emulator and checks the app crash buffer; the default command runs every check.

Review both portrait overlays before the device pass. Emulator checks do not establish custom-driver compatibility or latency gains on Snapdragon hardware; validate those with a live stream on the target phone. Add live stream-menu captures for the same viewport/font matrix and check permission filtering, Quit game confirmation, mode persistence, Copy stats, the keyboard shortcut, German labels and PiP on a phone. Saved [settings](screenshots/04-settings.png), [bitrate](screenshots/06-bitrate.png), [PC profile](screenshots/09-host-profile.png), [pairing guide](screenshots/01-launch.png), [host details](screenshots/15-host-details.png) and [overlay](screenshots/07-latency-overlay.png) captures show UI fixtures, not measured hardware streaming performance.

## Flavors and outputs

| Flavor / task | Package / output |
| --- | --- |
| `nonRoot` | `com.butterpollo.client`; normal Android build, no root required. |
| `root` | `com.butterpollo.client.root`; older root mouse-capture path, with maximum SDK 25. |
| `:app:assembleNonRootDebug` | `app/build/outputs/apk/nonRoot/debug/app-nonRoot-debug.apk` |
| `:app:assembleNonRootRelease` | `app/build/outputs/apk/nonRoot/release/app-nonRoot-release.apk` |
| `:app:assembleRootRelease` | `app/build/outputs/apk/root/release/app-root-release.apk` |
| `:app:bundleNonRootRelease` | `app/build/outputs/bundle/nonRootRelease/app-nonRoot-release.aab` |
| Root equivalents | Replace `NonRoot` with `Root` in task names and `nonRoot` with `root` in output paths. |

Release builds enable R8 shrinking. The source namespace remains `com.limelight`; it is distinct from the Android package ID. The packaged locales are listed in `localeFilters` in `app/build.gradle` and in `app/src/main/res/xml/locales_config.xml`; see [translations](TRANSLATIONS.md).

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
keytool -genkeypair -keystore nightjar.keystore -alias nightjar -keyalg RSA -keysize 3072 -validity 10000
```

An unsigned APK produced outside the normal Gradle signing path cannot be installed directly. With Android SDK Build Tools on your PATH, copy it to `nightjar-unsigned.apk`, align it, sign with the same key and verify:

```sh
zipalign -P 16 -f 4 nightjar-unsigned.apk nightjar-aligned.apk
apksigner sign --ks nightjar.keystore --ks-key-alias nightjar --out nightjar.apk nightjar-aligned.apk
apksigner verify --verbose nightjar.apk
adb install -r nightjar.apk
```

See Android's [APK signing instructions](https://developer.android.com/tools/apksigner).

Build the release APK and bundle directly:

```powershell
.\gradlew.bat --no-daemon --max-workers=2 :app:assembleNonRootRelease :app:assembleRootRelease :app:bundleNonRootRelease :app:lintNonRootRelease :app:lintRootRelease :app:testNonRootDebugUnitTest :app:testRootDebugUnitTest
```

Or use the artifact helper, which runs release lint and both flavors' unit tests before building:

```powershell
.\scripts\build-release.ps1
```

The Linux helper is `bash scripts/build-release.sh`. Both helpers lint and build the universal non-root APK, legacy root APK, and non-root bundle with all four default ABIs (`arm64-v8a`, `armeabi-v7a`, `x86`, `x86_64`), then print SHA-256 hashes and byte sizes for all three artifacts. They stop on validation/build failure and do not publish. Single-ABI builds are for explicit device testing, not the default downloadable release. Preserve `app/build/outputs/mapping/nonRootRelease/mapping.txt` and native debug symbols for the matching release.

Releases are built and signed by [`release.yml`](../.github/workflows/release.yml) when a `v*` tag is pushed (see below); a local `gh release create` with the retained release key still works. Test prereleases carry a debug-signed arm64 APK, `nightjar-arm64-debug.apk`, with the separate debug package ID and are never marked Latest.

### Releasing from CI

Pushing a tag such as `v0.8.1` builds the non-root APK, the root APK and the non-root bundle with the release key, checks that all three are signed by the Nightjar certificate (SHA-256 `312ed322c7c29d1aec4fae4a3979ebcb557af44d821d7f4746c0968548eb630b`) and that the app IDs are `com.butterpollo.client` and `com.butterpollo.client.root` (the bundle's ID is not checked), and uploads `nightjar-<version>.apk`, `-root.apk`, `.aab`, `-mapping.txt`, `-native-debug-symbols.zip` (both of the non-root build, for reading crash traces), the desktop client as `nightjar-desktop-<version>-windows-x86_64.zip` and `nightjar-desktop-<version>-linux-x86_64.tar.gz` (built by the `desktop-windows` and `desktop-linux` jobs with `protocol/desktop/package-windows.ps1` and `package-linux.sh`; the release job waits for both, so a failed desktop build publishes nothing; both build the PyroWave SDK (`scripts/build-pyrowave-sdk.*`, pinned by `protocol/desktop/pyrowave-sdk.pin`; patch 0002 and the Windows build script come from the public Rubylight repo at the `RUBYLIGHT_SHA` in `release.yml`) and bundle it, and check that the unpacked package loads it) and `SHA256SUMS-<version>.txt` (it covers every file) to the release for the tag. CI replaces assets only on a draft release; on a published release it refuses to overwrite an asset that is already there and fails. The tag must equal `v` plus `versionName` in `app/build.gradle`. A tag with a `-` suffix publishes a prerelease that is not marked Latest. If the release does not exist yet, the workflow creates it with a short placeholder text; replace it with `gh release edit <tag> --notes-file <file>`. The job does not run tests, so tag a commit whose `android.yml` run on main is green.

It needs four repository secrets (Settings, Secrets and variables, Actions) and fails with a message naming any that is missing:

| Secret | Value |
| --- | --- |
| `NIGHTJAR_KEYSTORE_B64` | the release keystore file, base64-encoded |
| `NIGHTJAR_KEYSTORE_PASSWORD` | `storePassword` from `keystore.properties` |
| `NIGHTJAR_KEY_ALIAS` | `keyAlias` from `keystore.properties` |
| `NIGHTJAR_KEY_PASSWORD` | `keyPassword` from `keystore.properties` |

Set them with the GitHub CLI so no value lands in the shell history: pipe the keystore in from a file, and let `gh secret set` prompt for the rest.

```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes("C:\path\to\release-keystore")) | gh secret set NIGHTJAR_KEYSTORE_B64 -R RamazanKara/nightjar
gh secret set NIGHTJAR_KEYSTORE_PASSWORD -R RamazanKara/nightjar   # prompts
gh secret set NIGHTJAR_KEY_ALIAS -R RamazanKara/nightjar           # prompts
gh secret set NIGHTJAR_KEY_PASSWORD -R RamazanKara/nightjar        # prompts
```

The workflow passes them to Gradle as the `BP_*` variables above, so local builds and `keystore.properties` are unchanged, and it deletes the decoded keystore as soon as the build is verified.

**Dry run first.** Before the first real tag (and after changing the secrets or the workflow), start the workflow by hand from main: `gh workflow run release.yml -R RamazanKara/nightjar -f dry_run=true`, or Actions, Release, Run workflow. It checks the secrets, decodes the keystore, builds, verifies the signatures and app IDs and deletes the keystore, but publishes nothing. Only a tag push (or a manual run on a tag with `dry_run` off) publishes.

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
