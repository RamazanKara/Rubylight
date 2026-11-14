# Google Play readiness

State of the Nightjar source, last checked against 4.12.4 (`versionCode` 381, package `com.butterpollo.client`), against
what a first Play upload needs. Updated 2026-11-10. This is not a claim that the version is published or submitted to Play.

## Ready in the repository

| Item | Where |
| --- | --- |
| Minified (R8) release APK and AAB; a dedicated signing key is required, with no debug-key fallback | `app/build.gradle`, [building](../docs/building.md#release-signing), `scripts/build-release.*` |
| Version 4.12.4 in source, shown in Settings > About | `app/build.gradle` |
| Target SDK 36, 16 KB page-size alignment for native libraries | `app/build.gradle`, `app/src/main/jni` |
| Crash and abnormal-exit capture, local only, shared only by the user | Settings > App > Report a problem |
| 15 languages packaged (`localeFilters`); the store listing is English and German | `app/build.gradle`, [translations](../docs/TRANSLATIONS.md) |
| Store title, short and full description, changelogs (English through `versionCode` 381, German through 357) | `fastlane/metadata/android/{en-US,de}` |
| Icon 512 px, feature graphic 1024x500, TV banner 1280x720 | `fastlane/metadata/android/en-US/images` |
| Phone screenshots (1080x1920 and 1920x1080, from demo-mode captures) | `fastlane/metadata/android/en-US/images/phoneScreenshots`, made by `scripts/store/store-screenshots.py` |
| Android TV screenshots (1920x1080, 24-bit PNG: Home, a game's page, the stream, All games) | `fastlane/metadata/android/en-US/images/tvScreenshots`, made by `scripts/store/tv-screenshots.ps1` |
| Android TV support in the one bundle: `android.software.leanback` and the touchscreen not required, `LEANBACK_LAUNCHER`, `android:banner`, D-pad navigation throughout | `app/src/main/AndroidManifest.xml` |
| Privacy policy | [PRIVACY.md](../docs/PRIVACY.md), published at `https://ramazankara.github.io/nightjar/privacy.html` |
| Draft answers for Data safety and content rating | [PLAY-DATA-SAFETY.md](PLAY-DATA-SAFETY.md) |

Store screenshots: the demo mode plays a local sample video and its performance overlay shows
illustrative values. Regenerate the stills with `scripts/demo/capture-stills.ps1`, then run
`python scripts/store/store-screenshots.py`. The TV screenshots are plain captures of the same demo mode on an
Android TV at 1920x1080 (the `nj-tv` AVD, see [demo](../docs/demo/README.md)): install the debug APK there, set
`ANDROID_SERIAL` and run `pwsh scripts/store/tv-screenshots.ps1`.

## Owner steps in the Play Console

1. Create the Play developer account and the app entry (paid, no ads, no in-app purchases; trader status in the EU).
2. Create the upload key once and keep it safe (see [release signing](../docs/building.md#release-signing)),
   then build with `keystore.properties` or the `BP_KEYSTORE*` variables. Enroll in Play App Signing.
3. Upload `app-nonRoot-release.aab` with `mapping.txt` and native debug symbols from the same build.
4. Fill in Data safety and the content rating questionnaire from the draft. The draft's open point
   is the two public network checks (external address lookup and the connection test after a failed
   connection); both send only the device's public IP address and protocol traffic.
5. Enter the privacy policy URL above, choose countries, and start with an internal or closed test track.

### Android TV

The same bundle serves phones and TVs (it carries 64-bit libraries with 16 KB alignment, which TV apps need from
August 2026). Play lists it on Android TV only after an opt-in and a separate review against the
[TV app quality guidelines](https://developer.android.com/docs/quality-guidelines/tv-app-quality)
([Google's steps](https://developer.android.com/training/tv/publishing/distribute)).

1. Main store listing: upload `tvBanner.png` as the Android TV banner and the `tvScreenshots` (at least one is
   required). The descriptions name Android TV, as Google asks.
2. Advanced settings (under Test and release), Form factors tab: Add release type, Android TV. Keep it on the
   shared tracks rather than a dedicated TV track, since there is no TV-only build. Confirm the opt-in and the
   review policy.
3. The next release rolled out (the Play upload from a published GitHub release, see
   [building](../docs/building.md#google-play)) starts the TV review. Its status (Pending, Approved, Not approved)
   is under Android TV on the app's pricing and distribution page; a rejection email names what to fix, and only
   TV distribution waits for it.

## Not done yet

- German store screenshots (the German listing falls back to the English ones).
- Tablet screenshots (optional for a phone app; needed for the large-screen badge).
- German changelogs after `versionCode` 357.
- The exact production-signed Play build needs an in-place upgrade and smoke test; validation-key builds cannot establish signing continuity. Keep real-phone streaming checks separate from emulator fixtures.
