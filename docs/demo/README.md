# Nightjar debug demo

The non-root debug APK includes an offline marketing fixture mode. Package IDs stay
`com.butterpollo.client` / `com.butterpollo.client.root`. All demo activities,
fixtures, scripted metrics, PNG artwork and MP4 samples live in `app/src/debug`;
release variants do not compile or package them. Normal app launches still use
the real computer manager and streaming connection.

## Build

`ANDROID_HOME` (or `sdk.dir` in `local.properties`) and the native submodule must be
present. Build from the repository root:

```powershell
$env:JAVA_HOME = 'C:\Program Files\Microsoft\jdk-21.0.12.101-hotspot'  # any JDK 21; see docs/building.md
.\gradlew.bat :app:assembleNonRootDebug :app:testNonRootDebugUnitTest :app:lintNonRootDebug --no-daemon --max-workers=2
```

The unit tests need JDK 21: Robolectric's Android 16 tests stop with `UnsupportedOperationException` on JDK 17.

APK: `app/build/outputs/apk/nonRoot/debug/app-nonRoot-debug.apk`.
No extra flavor or dependency is required. If the build user's home is read-only,
point `GRADLE_USER_HOME` and `ANDROID_USER_HOME` at writable local directories.

## Launch on the caller's device

Install that APK on an English-language Android device/emulator. The caller runs
adb; these instructions and scripts do not start an emulator.

```powershell
adb install -r app/build/outputs/apk/nonRoot/debug/app-nonRoot-debug.apk
adb shell am start -n com.butterpollo.client.debug/com.limelight.demo.DemoLauncherActivity
adb shell am start -n com.butterpollo.client.debug/com.limelight.demo.DemoLauncherActivity --es state library
adb shell am start -n com.butterpollo.client.debug/com.limelight.demo.DemoLauncherActivity --es state pc-settings
```

| `state` extra | Screen |
| --- | --- |
| `hosts` (default) | Real PcView cards: Living-room PC online/paired, Studio PC online/unpaired, Laptop offline with Wake PC action |
| `pc-settings` | Real per-PC settings for Living-room PC: Video, Display on the PC, Connection and Host groups |
| `library` | Real AppView, ten original 3:4 covers, Racing Game marked running |
| `stream`, `compact` | Looping sample in the Game layout's StreamView with the compact performance overlay |
| `statistics` | Same sample with the Statistics overlay, including the host's CPU and GPU |
| `advanced`, `debug` | Same sample with the Debug overlay |
| `touch` | Same sample with the real on-screen controller and its layout editor |
| `pip` | Sample ready for PiP; press Home to enter it |
| `settings` | Real settings and presets, with isolated demo preferences |
| `gpu-driver` | Real Snapdragon GPU driver screen, shown as on an arm64 Adreno phone |
| `controller` | Real controller mapping screen, populated with a demo controller |

Living-room PC's host is a fixture of a paired Rubylight 2.2.21 with every permission and no
PyroWave. Its capabilities are cached in the demo preferences, so its PC settings (`pc-settings`, or
PC settings… in its menu) show the rows that host supports, with the host family as the subtitle,
and the stream menu lists its Quality and Host sections. Nothing is read from a host. The PC
settings screen alone sees the PC at the documentation address `192.0.2.10` (RFC 5737), so its
rows read as for an online PC; its remote setup, export and web UI actions stay in the fixture.

Tap Living-room PC to enter its library; tap any title to play the sample with the compact overlay.
Long-press the performance overlay to cycle Compact, Statistics and Debug. Tap the
stream, its background or Back for the real stream menu; Disconnect leaves. Add
`--ez hint true` to keep the first-stream hint up until it is tapped. Add
`--es position top_left|top_right|bottom_left|bottom_right` to start the overlay in that corner; the
stream menu's Overlay page moves it live. Studio pairing and
Laptop Wake PC are offline demonstration actions; no pairing request or magic
packet is sent. The fixture binder never discovers or polls hosts and creates
no launcher shortcuts. Preferences use `demo_` names, cached art stays under
`cache/demo` and the GPU driver screen lists `no_backup/demo`; the user's computer database,
normal preferences and imported drivers are untouched.
Force-stop the app and open its normal launcher to leave the demo.

The network decoder requires a real host handshake, so demo playback decodes the sample
with MediaCodec into the same `activity_game` / `StreamView` surface container. It shows one
sample frame per sixtieth of a second of display refreshes, counted on Choreographer, so an
emulator whose refreshes drift against the media clock does not repeat and skip frames on
camera. The production `PerfOverlayListener`, `PerformanceOverlay` formatters, Material
ActionSheet and `VirtualController` render its UI. Supported AV1/HEVC samples
are preferred, with H.264 fallback. PiP is Android's actual floating video window
(Android 8+ with PiP support), not a composited image.

Some screens need hardware or a host the demo does not have. The debug build shows them as
they look where they apply, and only in demo mode:

- The demo preferences turn on adaptive refresh (ARR / VRR), the low-latency decoding switches
  and the USB DualSense driver with its audio and haptics.
- `DemoStreamSettings` shows the Snapdragon GPU driver row and the USB controllers group on any
  device, and the driver row opens `DemoGpuDriverSettings`. That screen reads as on an arm64
  Adreno phone, lists the demo's own empty driver folder, imports nothing and never loads a
  driver. The release classes carry the seams: `StreamSettings.supportsGpuDrivers()` and
  `supportsUsbHost()`, and a non-final `GpuDriverSettings` with `supportsDrivers()`. In
  release builds each returns the same check as before.
- The stream menu offers the Microphone toggle, as for a host that takes the phone's
  microphone. The toggle only changes its own state; the demo records nothing.

**All performance readings are scripted illustrations, not measurements.** The
sample is 1080p60 SDR; the displayed 119.9 FPS, approximately 6 ms decode, 0.1%
loss, AV1 10-bit HDR and FSR 1 profile are synthetic, and so are the host's CPU and
GPU readings (an AMD Radeon RX 7900 XT at about 72% load, 64 °C and 231 W) in the
Statistics and Debug overlays. The stream carries a small DEMO label. Keep that
distinction in published captions; use real-host footage for claims about measured
performance, HDR or upscaling quality.

## Capture stills and the guided tour

Use an unlocked device with platform-tools on PATH (or in `ANDROID_HOME`). Set
`ANDROID_SERIAL` when more than one device is connected. PiP must be enabled for
the app. No host or physical controller is needed.

```powershell
.\scripts\demo\capture-stills.ps1
.\scripts\demo\capture-video.ps1
# Optional retakes, using the existing shot IDs:
.\scripts\demo\capture-video.ps1 -Shot 05-stream,16-statistics
```

Run them with PowerShell 7 (`pwsh`). The scripts temporarily set 1080x2400, density 420, portrait rotation,
the dark system theme (`-Theme light` captures the day palette instead), a fixed status bar (SystemUI demo mode:
10:30, full battery and signal, no notification icons), no soft keyboard for the emulator's virtual hardware
keyboard and no "Viewing full screen" confirmation, then restore the prior values in `finally`. The published
stills, store screenshots and video use the dark theme: its night surfaces and amber accents are the Nightjar
identity, and they sit in the site's dark phone frames.
On an emulator, software AV1/HEVC 10-bit playback can render corrupt, so every state is launched with
`--es codec h264` to use the 8-bit sample (the codec extra is debug-only and sticks for the process).
They force-stop only this package
between states and at exit. They do not clear app data. Settings navigation and
the tour's taps use UIAutomator resource IDs, text and discovered
bounds; missing UI stops capture and preserves `out/demo/capture/last-ui.xml`.
PiP capture checks that Android actually entered pinned window mode. The presets still applies the Native
preset first (confirming its dialog), so a preset is selected; every launch writes the demo preferences again.

Stills go to `docs/screenshots/demo/`: `pc-list.png`, `library.png`, `stream.png`,
`stream-compact.png`, `stream-advanced.png`, `settings-presets.png`,
`upscaling-options.png`, `controller.png`, `touch-controller.png`, `pip.png`, `pc-settings.png`,
`video-codecs.png`, `gpu-driver.png`, `usb-controllers.png`.
The script then rotates to landscape and also saves `landscape-stream.png`, `landscape-compact.png`,
`landscape-advanced.png`, `landscape-touch.png`, `landscape-stream-menu.png` and `landscape-statistics.png`
(2400x1080), where the sample fills the screen, the real on-screen controller is laid out for it and the stream
menu is scrolled to Quality.
`-Only` takes still names, for example `-Only pc-settings,landscape-stream-menu`. PNG dimensions are checked
after pulling. These files are created by the caller; no empty-app placeholders are substituted.

`capture-video.ps1` records the ten [shots](shotlist.md) at 60 fps with `adb shell screenrecord` at 0.6 of the
screen size (648x1440 portrait, 1440x648 for the landscape stream shots 05, 15 and 16). Use an emulator with the
host GPU (`-gpu host`): there the recorder keeps up with every 60 Hz refresh at that size. On a software-rendered
emulator (swiftshader) or at full size it manages about 30, and the tour stutters. On a busy host, raising the
emulator process's priority helps.

Each take follows a plan on a clock that starts with the recording: the shot is the window from 1 s to 1 s plus
its length, with a second of handle either side. Before recording, the script opens the screen, finds what to
tap and opens each dialog or screen once (the first opening loads classes and can miss a frame on camera); where
a list scrolls, it rehearses the same fling and checks where it stops. During the take it plays one action with a
visible result, or two at most: a tap into the library, a fling and the display-mode picker, a tap on a game,
Back for the stream menu, a long press on the overlay, a tap on a setting, Home for PiP. Each shot is taken up
to three times and the take with the fewest missed refreshes is kept. The recorder's start-up delay is measured
from the take itself (its second frame is the first action's first change), so the action lands on the planned
time.

screenrecord writes a frame only when the screen changes, stamped with the time it was composed. The script puts
every frame on the 60 fps grid at its time; a frame composed a little early, whose slot is already taken, moves
to the next one, so every recorded frame is shown once and a still screen repeats its last frame. Clips go to
`out/demo/clips/<shot-id>.mp4`, beside the recording as it came (`<shot-id>.raw.mp4`). Shot 12 presses Home
while recording and leaves the PiP window in Android's default bottom-right corner (the demo home's rings are
centred there); for it the script enables the debug app's plain dark `DemoHomeActivity` as the home screen (so
the window floats over a clean background, not third-party launcher icons) and restores the device's launcher
afterwards. Recording is silent.
`-HostCapture` records the emulator's window on the PC instead of using screenrecord in the device. Start the AVD
with a window (drop `-no-window`, keep `-gpu host -no-audio`). The script runs `emulator-window.ps1`, which pins
the window on top at a fixed place on the primary monitor and sizes it so its display area is `-WindowHeight`
pixels tall (1320 by default, 594x1320, which fits a 1440-pixel monitor); it then grabs exactly that area with
FFmpeg's Desktop Duplication source and encodes it on the GPU:

```powershell
ffmpeg -f lavfi -i ddagrab=output_idx=0:framerate=60:offset_x=X:offset_y=Y:video_size=WxH `
    -vf hwdownload,format=bgra,format=nv12 -c:v h264_amf -quality quality -rc cqp -qp_i 16 -qp_p 18 take.mp4
```

The emulator draws a landscape app on its side inside the portrait window, so landscape takes are turned back
(`transpose=2`). The grab has a frame every 1/60 s, so takes are scored by the repeats FFmpeg's `mpdecimate` finds
while something moves. With the window open, the in-device recorder kept up less well than the host grab (about
45 against 55 distinct frames per second on the busy capture host); on a quiet host both reach 60. The current
tour's `12-pip` and `16-statistics` come from the host grab, the other shots from screenrecord. Close the
emulator window when done.
The older `capture.ps1` entry point forwards to this offline tour; its `-NoHost`
option is now redundant.

The existing edit remains available after capture:

```powershell
.\scripts\demo\render.ps1 -Storyboard
.\scripts\demo\render.ps1
```

It uses the shotlist's demo stills or the matching recorded clips and writes
landscape/portrait exports, posters, review frames, contact sheets, a smoothness report and `sources.json` under
`out/demo/`. Review the outputs before using them in README/site/video assets.
Stream shots preserve the real 16:9 surface inside the portrait viewport; landscape clips get a landscape phone frame.
The renderer needs an FFmpeg that supports `-/filter_complex` (FFmpeg 8 and later).

The picture uses the Nightjar palette (amber moon #F2A93B on night #14161F) and the moon-and-nightjar tile
(`scripts/demo/assets/nightjar-tile.png`, the README banner's mark). It runs at 60 fps. The clips are only scaled
down into a still phone frame: there is no zoom or pan, so the screen never moves by itself. Shots meet with hard
cuts (a crossfade would double the captions and UI text for a few frames); each shot's heading, callout and
caption fade in over 0.3 s, and the title cards meet the first and last shot through the night background. The
9:16 cut stays inside the usual platform safe areas: wordmark and headline below y=290, captions ending by
y=1440, no text in the right 120 px; the renderer checks every line's width.
It writes `demo-16x9.mp4` (1920x1080) and `demo-9x16.mp4` (1080x1920), H.264 High at 60 fps with the
metadata at the start of the file and no audio track (add licensed music when posting the video), and validates
both. The posters are a frame of the stream shot (game moving, compact overlay in view) as PNG and as the site's
1280x720 and 720x1280 WebP. Review frames (both title cards and the end of every shot) go to `out/demo/frames/`,
contact sheets (three frames of every shot) to `out/demo/contact-*.png`, and `segments.json` records each
shot's start time.
`smoothness-*.txt` lists, for every second of every shot, how many distinct frames it shows, by FFmpeg's
`mpdecimate=hi=64*4:lo=64*2:frac=0.2`. Motion (a scroll, a transition, the game) should stay close to 60; a
still screen shows 1.

## README assets

```powershell
.\scripts\demo\readme-assets.ps1 -Docs C:\path\to\readme-checkout   # shots/*.png and demo.gif under docs/assets
.\scripts\demo\readme-preview.ps1 -Docs C:\path\to\readme-checkout  # 1440 px and 393 px README previews
```

`readme-assets.ps1` builds the four phone shots (PCs, library, the landscape stream's top-left slice with the
compact overlay, picture-in-picture) from the stills, and `demo.gif` from the rendered 9:16 video cropped to the
phone column: the tour's strongest motion (the tap into the library, the tap into a stream, the stream menu and the
switch to Statistics) at 30 fps and 360 px wide, about 2 MB. It uses `out/demo` from a fresh render and
otherwise the published `site/assets/demo-9x16.mp4`, with shot start times worked out from the shotlist.
`readme-preview.ps1` renders the README with GitHub's own markdown renderer
(read-only) in headless Edge so legibility can be checked at phone width before pushing.

## Regenerate bundled assets

```powershell
$env:JAVA_HOME = 'C:\jdk17'
.\scripts\demo\generate-assets.ps1
```

The generator finds FFmpeg on PATH or under
`%LOCALAPPDATA%\Microsoft\WinGet\Packages\Gyan.FFmpeg_*\*\bin`; it downloads
nothing. `GenerateAssets.java` uses JDK 17 Java2D and system Segoe UI fonts to
make ten slate gradient covers (one accent colour and genre per title, no app name) and an original ten-second seamless flight
animation: parallax mountains, a neon ground grid, towers, particles, a moving
ship and HUD. Raw frames are piped to FFmpeg at 1920x1080/60. H.264 is mandatory;
HEVC and AV1 10-bit SDR copies are encoded when their software encoders are
available. All footage and art are synthetic and contain no third-party games.
The PNG preview and ffprobe verification reports go to `out/demo/assets/`.
Only the generated `app/src/debug/assets/demo/` files ship in the debug APK.
