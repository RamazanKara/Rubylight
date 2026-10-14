# Nightjar debug demo

The non-root debug APK includes an offline marketing fixture mode. Package IDs stay
`com.butterpollo.client` / `com.butterpollo.client.root`. All demo activities,
fixtures, scripted metrics, PNG artwork and MP4 samples live in `app/src/debug`;
release variants do not compile or package them. Normal app launches still use
the real computer manager and streaming connection.

## Build

`local.properties` and the native submodule must be present (copy the missing
files from `C:\src\bp-T5` if needed). Build from the repository root:

```powershell
$env:JAVA_HOME = 'C:\jdk17'
.\gradlew.bat :app:assembleNonRootDebug :app:testNonRootDebugUnitTest :app:lintNonRootDebug --no-daemon --max-workers=2
```

APK: `app/build/outputs/apk/nonRoot/debug/app-nonRoot-debug.apk`.
No extra flavor or dependency is required. If the build user's home is read-only,
point `GRADLE_USER_HOME` and `ANDROID_USER_HOME` at writable local directories.

## Launch on the caller's device

Install that APK on an English-language Android device/emulator. The caller runs
adb; these instructions and scripts do not start an emulator.

```powershell
adb install -r app/build/outputs/apk/nonRoot/debug/app-nonRoot-debug.apk
adb shell am start -n com.butterpollo.client/com.limelight.demo.DemoLauncherActivity
adb shell am start -n com.butterpollo.client/com.limelight.demo.DemoLauncherActivity --es state library
```

| `state` extra | Screen |
| --- | --- |
| `hosts` (default) | Real PcView cards: Living-room PC online/paired, Studio PC online/unpaired, Laptop offline with Wake PC action |
| `library` | Real AppView, ten original 3:4 covers, Racing Game marked running |
| `stream`, `compact` | Looping sample in the Game layout's StreamView with compact performance overlay |
| `advanced` | Same sample with advanced overlay |
| `touch` | Same sample with the real on-screen controller and its layout editor |
| `pip` | Sample ready for PiP; press Home to enter it |
| `settings` | Real settings and presets, with isolated demo preferences |
| `controller` | Real controller mapping screen, populated with a demo controller |

Tap Living-room PC to enter its library; tap any title to play the sample.
Long-press the performance overlay to switch compact/advanced. Tap the stream
or its background for overlay and touch-control actions. Studio pairing and
Laptop Wake PC are offline demonstration actions; no pairing request or magic
packet is sent. The fixture binder never discovers or polls hosts and creates
no launcher shortcuts. Preferences use `demo_` names and cached art stays under
`cache/demo`; the user's computer database and normal preferences are untouched.
Force-stop the app and open its normal launcher to leave the demo.

The network decoder requires a real host handshake, so demo playback uses
MediaPlayer in the same `activity_game` / `StreamView` surface container. The
production `PerfOverlayListener`, `PerformanceOverlay` formatters, Material
ActionSheet and `VirtualController` render its UI. Supported AV1/HEVC samples
are preferred, with H.264 fallback. PiP is Android's actual floating video window
(Android 8+ with PiP support), not a composited image.

**All performance readings are scripted illustrations, not measurements.** The
sample is 1080p60 SDR; the displayed 119.9 FPS, approximately 6 ms decode, 0.1%
loss, AV1 10-bit HDR and FSR 1 profile are synthetic. The stream carries a small
DEMO label. Keep that distinction in published captions; use real-host footage
for claims about measured performance, HDR or upscaling quality.

## Capture stills and the guided tour

Use an unlocked device with platform-tools on PATH (or in `ANDROID_HOME`). Set
`ANDROID_SERIAL` when more than one device is connected. PiP must be enabled for
the app. No host or physical controller is needed.

```powershell
.\scripts\demo\capture-stills.ps1
.\scripts\demo\capture-video.ps1
# Optional retakes, using the existing shot IDs:
.\scripts\demo\capture-video.ps1 -Shot 04-library,05-stream,12-pip
```

Run them with PowerShell 7 (`pwsh`). The scripts temporarily set 1080x2400, density 420, portrait rotation,
a fixed status bar (SystemUI demo mode: 10:30, full battery and signal, no notification icons) and no soft
keyboard for the emulator's virtual hardware keyboard, then restore the prior values in `finally`.
On an emulator, software AV1/HEVC 10-bit playback can render corrupt, so every state is launched with
`--es codec h264` to use the 8-bit sample (the codec extra is debug-only and sticks for the process).
They force-stop only this package
between states and at exit. They do not clear app data. Settings navigation and
the library/stream tour taps use UIAutomator resource IDs, text and discovered
bounds; missing UI stops capture and preserves `out/demo/capture/last-ui.xml`.
PiP capture checks that Android actually entered pinned window mode.

Stills go to `docs/screenshots/demo/`: `pc-list.png`, `library.png`, `stream.png`,
`stream-compact.png`, `stream-advanced.png`, `settings-presets.png`,
`upscaling-options.png`, `controller.png`, `touch-controller.png`, `pip.png`.
The script then rotates to landscape and also saves `landscape-stream.png`, `landscape-compact.png`,
`landscape-advanced.png` and `landscape-touch.png` (2400x1080), where the sample fills the screen and the real
on-screen controller is laid out for it. PNG dimensions are checked after pulling. These files are created by
the caller; no empty-app placeholders are substituted.

`capture-video.ps1` records all eight [shots](shotlist.md) with
`adb shell screenrecord` at half resolution (540x1200 portrait, 1200x540 landscape for the stream shots 05 and 06;
the emulator's software encoder cannot keep up at full size). screenrecord writes variable-rate video and a single
frame for a static screen, so FFmpeg normalizes each take to 30 fps and holds the last frame. Clips go to
`out/demo/clips/<shot-id>.mp4`, with one second of handles at each end. Shot 04
shows the library directly (taps on a PC card do not navigate under `adb input` on the emulator), and shot 05 taps
Racing Game while recording. Shot 12 presses Home while recording and leaves the PiP window in Android's default bottom-right corner (the demo home's rings are centred there); for it the script enables the debug app's plain dark `DemoHomeActivity` as the home screen (so the window floats over a clean background, not third-party launcher icons) and restores the device's launcher afterwards. The advanced overlay, touch controls and performance panel shots (07, 11, 13) were dropped from the tour after review, see the [shotlist](shotlist.md); their clips are not rendered. Logs stay beside the clips. Recording is silent.
The older `capture.ps1` entry point forwards to this offline tour; its `-NoHost`
option is now redundant.

The existing edit remains available after capture:

```powershell
.\scripts\demo\render.ps1 -Storyboard
.\scripts\demo\render.ps1
```

It uses the shotlist's new demo stills or the matching recorded clips and writes
landscape/portrait exports, posters, review frames and `sources.json` under
`out/demo/`. Review the outputs before using them in README/site/video assets.
Stream shots preserve the real 16:9 surface inside the portrait viewport; landscape clips get a landscape phone frame.
The renderer needs an FFmpeg that supports `-/filter_complex` (FFmpeg 8 and later).

The picture uses the Nightjar palette (amber moon #F2A93B on night #14161F) and the moon-and-nightjar tile
(`scripts/demo/assets/nightjar-tile.png`, the README banner's mark), joins shots with hard cuts
(a crossfade would double the captions and UI text for a few frames) and keeps the 9:16 cut inside the usual
platform safe areas: wordmark and headline below y=290, captions ending by y=1440, no text in the right 120 px.
It writes `demo-16x9.mp4` (1920x1080) and `demo-9x16.mp4` (1080x1920), posters, eight review frames per video in
`out/demo/frames-v2/` and `segments.json` (each shot's start time).

## README assets

```powershell
.\scripts\demo\readme-assets.ps1 -Docs C:\path\to\readme-checkout   # shots/*.png and demo.gif under docs/assets
.\scripts\demo\readme-preview.ps1 -Docs C:\path\to\readme-checkout  # 1440 px and 393 px README previews
```

`readme-assets.ps1` builds the four phone shots (Computers, library, the landscape stream's centre slice with the
compact overlay, picture-in-picture) from the stills, and `demo.gif` from the rendered 9:16 video cropped to the phone
column. `readme-preview.ps1` renders the README with GitHub's own markdown renderer (read-only) in headless Edge so
legibility can be checked at phone width before pushing.

## Regenerate bundled assets

```powershell
$env:JAVA_HOME = 'C:\jdk17'
.\scripts\demo\generate-assets.ps1
```

The generator finds FFmpeg on PATH or under
`%LOCALAPPDATA%\Microsoft\WinGet\Packages\Gyan.FFmpeg_*\*\bin`; it downloads
nothing. `GenerateAssets.java` uses JDK 17 Java2D and system Segoe UI fonts to
make ten ruby/slate gradient covers and an original ten-second seamless flight
animation: parallax mountains, a neon ground grid, towers, particles, a moving
ship and HUD. Raw frames are piped to FFmpeg at 1920x1080/60. H.264 is mandatory;
HEVC and AV1 10-bit SDR copies are encoded when their software encoders are
available. All footage and art are synthetic and contain no third-party games.
The PNG preview and ffprobe verification reports go to `out/demo/assets/`.
Only the generated `app/src/debug/assets/demo/` files ship in the debug APK.
