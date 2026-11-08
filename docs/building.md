# Building Rubylight

[Documentation](index.md)

## Get the source and toolchain

Clone the Android repository with its native dependencies:

```sh
git clone --recurse-submodules https://github.com/RamazanKara/rubylight-android.git
cd rubylight-android
```

To initialize submodules in an existing checkout:

```sh
git submodule update --init --recursive --jobs 2
```

The native core path is declared in [.gitmodules](../.gitmodules). A source ZIP without initialized submodules is insufficient for the native build.

Use **JDK 17**. Set `JAVA_HOME` to its directory and `ANDROID_HOME` to the Android SDK, or set `sdk.dir` in an untracked root `local.properties`. Use Windows toolchains from PowerShell and Linux toolchains from WSL. The SDK/NDK and accepted SDK licenses must be available before building.

| Tool or platform | Repository configuration |
| --- | --- |
| Gradle wrapper | 9.7.1 |
| Android Gradle Plugin | 9.4.0 |
| Java source/target | 17 |
| Compile / target / minimum SDK | 37 / 36 / 21 |
| NDK | 29.0.14206865 |

These values come from [app/build.gradle](../app/build.gradle), [build.gradle](../build.gradle) and [gradle-wrapper.properties](../gradle/wrapper/gradle-wrapper.properties).

## Build and tests

From the repository root in PowerShell:

```powershell
.\gradlew.bat --no-daemon --max-workers=2 :app:assembleNonRootDebug :app:lintNonRootDebug :app:testNonRootDebugUnitTest
```

From Linux/macOS:

```sh
bash gradlew --no-daemon --max-workers=2 :app:assembleNonRootDebug :app:lintNonRootDebug :app:testNonRootDebugUnitTest
```

`--no-daemon` avoids retaining a build daemon; `--max-workers=2` limits Gradle workers. Native make and JVM test forks are also capped at two. Add `-PabiFilters=arm64-v8a` for an arm64-only APK; the property also accepts comma-separated ABIs. Add `--offline` when dependencies are already cached; the wrapper distribution must also be present.

The JVM suite includes Robolectric tests. Test results are written to `app/build/reports/tests/testNonRootDebugUnitTest/index.html`; lint writes `app/build/reports/lint-results-nonRootDebug.html`. Assembly success alone does not mean tests or lint passed.

The [Android workflow](../.github/workflows/android.yml) also checks the native frame parser and negotiation on Linux:

```sh
g++ -std=c++17 -O2 app/src/test/native/pyrowave_frame_test.cpp -o /tmp/pyrowave-frame-test
/tmp/pyrowave-frame-test
```

Use the [device checklist](device-checklist.md) for actual pairing, codecs, controller feedback and sustained streams. Automated tests and synthetic UI captures do not verify those hardware behaviors.

## Flavors and outputs

| Flavor or task | Package or output |
| --- | --- |
| `nonRoot` | `com.butterpollo.client`; normal build, no root required |
| `root` | `com.butterpollo.client.root`; root mouse-capture path for Android API 21–25 |
| `:app:assembleNonRootDebug` | `app/build/outputs/apk/nonRoot/debug/app-nonRoot-debug.apk` |
| `:app:assembleNonRootRelease` | `app/build/outputs/apk/nonRoot/release/app-nonRoot-release.apk` |
| `:app:bundleNonRootRelease` | `app/build/outputs/bundle/nonRootRelease/app-nonRoot-release.aab` |

Root tasks use `Root` in place of `NonRoot`, with `root` in output paths. Release builds enable R8 shrinking. The Java namespace is `com.limelight`; the installation package remains `com.butterpollo.client`. English and German are packaged. The Vulkan renderer is included only for `arm64-v8a` and `x86_64`.

For a prebuilt development APK, open a successful [Android debug workflow run](https://github.com/RamazanKara/rubylight-android/actions/workflows/android.yml), download the `butterpollo-debug` artifact and extract `app-nonRoot-debug.apk`. This is the current artifact name in the workflow. Open the APK on Android and permit installation from the app opening it. Published APKs are on [Rubylight releases](https://github.com/RamazanKara/rubylight-android/releases).

## Release signing

The existing hook in [app/build.gradle](../app/build.gradle) reads:

| Environment variable | Root `keystore.properties` entry |
| --- | --- |
| `BP_KEYSTORE` | `storeFile` |
| `BP_KEYSTORE_PASSWORD` | `storePassword` |
| `BP_KEY_ALIAS` | `keyAlias` |
| `BP_KEY_PASSWORD` | `keyPassword` |

Supply all four values for the release key. Each nonempty environment variable overrides its matching property. Relative keystore paths resolve from the repository root; use forward slashes in Windows property paths. Git ignores `keystore.properties`, `*.jks` and `*.keystore`.

**With no signing values, release APKs and bundles use the debug key.** Gradle warns about this fallback. A partial configuration fails the build. Use the production signing key for distribution and retain it for updates. An APK signed with another key cannot update the installed app; uninstalling removes local data and pairing identity.

Build, lint and test the release configuration:

```powershell
.\gradlew.bat --no-daemon --max-workers=2 :app:assembleNonRootRelease :app:bundleNonRootRelease :app:lintNonRootRelease :app:testNonRootDebugUnitTest
```

After the checks pass, the artifact helpers build an arm64 APK and a bundle with default ABIs, then print SHA-256 hashes and sizes:

```powershell
.\scripts\build-release.ps1
```

```sh
bash scripts/build-release.sh
```

The helpers do not publish. Preserve `app/build/outputs/mapping/nonRootRelease/mapping.txt` and the matching native debug symbols. The Android workflow uses debug signing; its optional `prerelease_tag` dispatch publishes a debug prerelease, not a production-signed release.

## Documentation checks

Use Python 3.10+ and Node.js 18+. No packages are needed; these scripts use standard libraries and make no network requests.

```sh
python scripts/generate-settings.py
python scripts/test_generate_settings.py
node --test scripts/check-doc-links.test.mjs
node scripts/check-doc-links.mjs
```

Use `python3` if that is your Python 3 command. The generator reads preference XML, English strings/arrays, Java defaults and preset assignments. It rejects missing usage advice, XML/runtime default mismatches and preset expressions needing review. Commit the regenerated `docs/settings.md` with a preference change.

The CI documentation job regenerates the page and checks `git diff --exit-code -- docs/settings.md` for stale committed output. It also runs the link checker and its tests.

The link checker covers README and all Markdown under `docs/`: local files and images, exact filename case, heading fragments, explicit HTML anchors and reference links. External URLs are syntax-checked only; a passing local check does not establish remote availability.
