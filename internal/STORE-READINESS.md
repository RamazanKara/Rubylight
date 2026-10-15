# Google Play readiness

State of the Nightjar source, last checked against 0.4.6 (`versionCode` 323, package `com.butterpollo.client`), against
what a first Play upload needs. Updated 2026-10-13. This is not a claim that the version is published or submitted to Play.

## Ready in the repository

| Item | Where |
| --- | --- |
| Minified (R8) release APK and AAB; a dedicated signing key is required, with no debug-key fallback | `app/build.gradle`, [building](../docs/building.md#release-signing), `scripts/build-release.*` |
| Version 0.4.6 prepared in source, shown in Settings > About | `app/build.gradle` |
| Target SDK 36, 16 KB page-size alignment for native libraries | `app/build.gradle`, `app/src/main/jni` |
| Crash and abnormal-exit capture, local only, shared only by the user | Settings > Support > Report a problem |
| English and German only (`localeFilters`) | `app/build.gradle` |
| Store title, short and full description, 0.4.6 changelog (EN and DE) | `fastlane/metadata/android/{en-US,de}` |
| Icon 512 px, feature graphic 1024x500, TV banner | `fastlane/metadata/android/en-US/images` |
| Phone screenshots (1080x1920 and 1920x1080, from demo-mode captures) | `fastlane/metadata/android/en-US/images/phoneScreenshots`, made by `scripts/store/store-screenshots.py` |
| Privacy policy | [PRIVACY.md](../docs/PRIVACY.md), published at `https://ramazankara.github.io/nightjar/privacy.html` once the repository is renamed to `nightjar` and Pages is republished (previously `https://ramazankara.github.io/rubylight-android/privacy.html`, which GitHub does not redirect) |
| Draft answers for Data safety and content rating | [PLAY-DATA-SAFETY.md](PLAY-DATA-SAFETY.md) |

Store screenshots: the demo mode plays a local sample video and its performance overlay shows
illustrative values. Regenerate the stills with `scripts/demo/capture-stills.ps1`, then run
`python scripts/store/store-screenshots.py`.

## Owner steps in the Play Console

1. Create the Play developer account and the app entry (free, no ads, no in-app purchases).
2. Create the upload key once and keep it safe (see [release signing](../docs/building.md#release-signing)),
   then build with `keystore.properties` or the `BP_KEYSTORE*` variables. Enroll in Play App Signing.
3. Upload `app-nonRoot-release.aab` with `mapping.txt` and native debug symbols from the same build.
4. Fill in Data safety and the content rating questionnaire from the draft. The draft's open point
   is the two public network checks (external address lookup and the connection test after a failed
   connection); both send only the device's public IP address and protocol traffic.
5. Enter the privacy policy URL above, choose countries, and start with an internal or closed test track.
   If the Play entry was already created with the old `rubylight-android` URL, update it to the
   `nightjar` one after the site is republished.

## Not done yet

- German store screenshots (the German listing falls back to the English ones).
- Tablet screenshots (optional for a phone app; needed for the large-screen badge).
- The exact production-signed 0.4.6 APK needs an in-place upgrade and smoke test; validation-key builds cannot establish signing continuity. Keep real-phone streaming checks separate from emulator fixtures.
