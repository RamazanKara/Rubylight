# Nightjar demo shotlist

Offline fixture tour: **69 seconds at 60 fps**, including a 3-second intro, 61
seconds of UI and a 5-second outro. It leads with what sets Nightjar apart: host-aware
PC settings, the stream menu, host telemetry, PyroWave, adaptive refresh, upscaling,
the Snapdragon GPU driver, the DualSense driver and picture-in-picture. Shots are joined with hard cuts: a
crossfade would double the captions and UI text for a few frames. Each shot's heading,
callout and caption fade in over 0.3 s; the intro and outro meet the first and last shot
through the night background, and only the very start (fade in) and the very end (fade
out) go to black. Every shot shows one action with a visible result, or two at most, then
holds; nothing zooms or pans. Capture adds one second before and after each shot; the
renderer trims those handles. The caller runs the capture scripts on a 1080x2400 portrait
device. The video has no audio track: add licensed music when posting it.

Brand: the Nightjar amber moon (#F2A93B) on night (#14161F), with the README
banner's moon-and-nightjar tile on the intro and outro. Safe areas for the 9:16 cut: captions end
by y=1440 (75% of 1920), the wordmark and headline start below y=290, and no
text enters the right 120 px.

Intro: **NIGHTJAR** / **Your games. Your screen. Your stream.**

Outro: **NIGHTJAR** / **Start your stream.** / **Get NIGHTJAR** /
**Download on GitHub** / **github.com/RamazanKara/nightjar**

This table is the scripts' manifest. Keep the column order and IDs; IDs are
stable, so gaps are expected (see the dropped shots below). Rows play in table order, so a new
ID can sit between older ones. The last column names the take's plan in `capture-video.ps1`:
`hosts` taps from the PC list into the library, `pc-settings` scrolls Living-room PC's
settings and opens the display-mode picker, `stream` taps a game in the landscape library,
`menu` opens the stream menu, `statistics` long-presses the overlay, `codec-refresh`,
`upscaling`, `gpu-driver` and `usb` go through the real settings, and `pip` presses Home.
`~` denotes a line break. Screenshot paths are relative to `docs/screenshots`. Every still
comes from the populated debug demo. The Crop column is `full` (use the whole screen) or
`crop=w:h:x:y` for the storyboard fallback only.

| ID | Seconds | Exact caption | Heading | Feature callout | Storyboard screenshot | Crop | Driver |
| --- | ---: | --- | --- | --- | --- | --- | --- |
| 01-launch | 6 | Your PC games. In your hands. | Start~here. | Your PCs at a glance~One tap to your library | demo/pc-list.png | full | hosts |
| 14-pc-settings | 8 | Made for your host. | Set up~each PC. | Settings your host supports~Display modes and connection tools | demo/pc-settings.png | full | pc-settings |
| 05-stream | 6 | Stay in the game. | Press~play. | Compact overlay~Essentials at a glance | demo/landscape-compact.png | full | stream |
| 15-stream-menu | 7 | Tune it while you play. | Take~control. | Automatic bitrate and quality~Microphone and quick toggles | demo/landscape-stream-menu.png | full | menu |
| 16-statistics | 6 | See the whole pipeline. | Read~the stream. | Network, decode and host stats~Scripted values in this demo | demo/landscape-statistics.png | full | statistics |
| 17-pyrowave-arr | 7 | Smoother, sharper, sooner. | Built~for speed. | PyroWave codec~Android adaptive refresh | demo/video-codecs.png | full | codec-refresh |
| 09-upscaling | 5 | Crisp on any screen. | Make~it clear. | AMD FSR 1~Snapdragon GSR | demo/upscaling-options.png | full | upscaling |
| 18-gpu-driver | 5 | Bring your own driver. | Snapdragon~ready. | Custom Adreno Vulkan drivers~Safe fallback | demo/gpu-driver.png | full | gpu-driver |
| 19-dualsense | 5 | Your DualSense, fully featured. | Plug~and play. | DualSense driver over USB~Audio and haptics | demo/usb-controllers.png | full | usb |
| 12-pip | 6 | Keep your stream in view. | Keep~it in view. | Picture in picture~Your stream floats on top | demo/pip.png | full | pip |

Dropped shots (any clips and stills of them left on disk are not used: the
render and capture scripts read only the table above):

- `07-advanced` and `13-performance` showed the Debug overlay. Its scripted
  numbers read as contradictory (120 FPS and AV1 10-bit HDR next to "Sample
  playback: 1080p60 SDR"), so no shot may show that panel. `16-statistics` shows
  the Statistics overlay instead, which has no sample line.
- `11-touch` showed the real on-screen controller, which covers the sample's HUD
  and the DEMO label.
- `02-discovery` and `03-pairing` held the same PC list as `01-launch`.
- `04-library` became the end of `01-launch`, which taps from the PC list into the
  library in one take, and `06-compact` became `05-stream`, which taps a game and
  plays the stream with the compact overlay.
- The presets and controller-mapping screens are plain settings lists that read
  poorly in a short video.

Capture direction:

1. `01-launch` holds the populated PC list, then taps Living-room PC and its library
   opens with Racing Game running. The pair badge is a fixture state, not a recorded
   pairing exchange; Laptop's Wake PC action sends no packets.
2. `14-pc-settings` opens Living-room PC's real PC settings: its name as the title, the
   host family (Rubylight 2.2.21) as the subtitle and the Video group. One fling brings
   up Display on the PC and Connection (the stroke is rehearsed before recording and
   lands within a few dozen pixels every take), then a tap on PC display mode opens the
   picker: the PC's own setting, exclusive, extend (two ways), extended and separated (two
   ways) and physical. The demo PC reports the capabilities of a paired Rubylight host
   with every permission (a debug fixture; nothing is read from a host), so the page
   shows the rows that host supports. This screen alone sees the PC at the documentation
   address 192.0.2.10, so its rows read as for an online PC.
3. Stream shots (05, 15, 16) are recorded in landscape: the 16:9 sample fills the
   screen. `05-stream` starts in the landscape library and taps Racing Game; the stream
   opens with the compact overlay in the top-left corner, as in a real stream.
4. `15-stream-menu` presses Back in the compact stream, which opens the real stream menu
   with its quick toggles (Keyboard, Overlay, Microphone; the demo offers the microphone
   as for a host that takes it, and records nothing). Two slow strokes then scroll it to
   Quality (Set bitrate, Automatic stream quality, Enable Automatic bitrate) with Host
   below. The menu lists these because the demo stream's host is the same Rubylight
   fixture; the demo only shows them, and choosing one says it needs a real host.
5. `16-statistics` long-presses the compact overlay, which switches it to Statistics:
   the stream line, frame rates, payload and loss, network, host processing and decode,
   then the host's CPU and GPU (usage, temperature and power). Every value is scripted,
   the stream carries the DEMO label and the callout says so.
6. `17-pyrowave-arr` opens Video codec in the real Video settings, whose list includes
   PyroWave (host support required), then goes back to Display & image, where Adaptive
   refresh (ARR / VRR) is on. The take does not choose PyroWave, so the emulator's
   readiness check (it has no Adreno GPU) never shows.
7. `09-upscaling` taps Client-side upscaling on the same page: Off, Smooth, Sharp (FSR)
   and Sharp, Snapdragon (SGSR, Snapdragon Game Super Resolution).
8. `18-gpu-driver` taps Snapdragon GPU driver in Advanced. The emulator is x86_64, so
   the release app hides this row and screen there; the debug demo shows them as on an
   arm64 Adreno phone (see the [demo README](README.md)), with the system driver
   selected, the import button and the explanation of the fallback.
9. `19-dualsense` flings Controls to USB controllers, where the USB DualSense driver
   and its audio and haptics are on. The emulator has no USB host support; the debug
   demo shows the group anyway.
10. `12-pip` begins in the stream, then presses Home and verifies Android's pinned
    window. The script enables the debug app's plain dark `DemoHomeActivity` as the home
    screen for this shot, so the floating window is not over third-party launcher icons,
    and restores the device's launcher afterwards. It and `16-statistics` were recorded
    from the PC (`-HostCapture`, see the [demo README](README.md)).

The source clip is 1080p60 SDR. The compact overlay's 119.9 FPS / 8 ms / 0.1% loss
readings and the Statistics overlay's stream and host readings are scripted
illustrations (the DEMO label says so), not measurements: do not describe them as
measured latency, throughput, HDR output, host load or upscaling performance. Use a
real host session for benchmark claims.
`out/demo/sources.json` from the renderer records each actual capture used.

## TV stills

The TV screens are stills, not shots of the video. They come from the same debug demo
on a TV, or on a handheld with `--es ui tv`, which turns on the TV layouts (the
Interface layout setting). `L` below is
`adb shell am start -n com.butterpollo.client.debug/com.limelight.demo.DemoLauncherActivity`.

| Still | Command | Then |
| --- | --- | --- |
| Home, library rows and store chips | `L --es state home --es host native --es ui tv` | Down to a store row |
| All games under a store | `L --es state allgames --es host native --es filter store:steam --es ui tv` | none |
| A game's page, running | `L --es state detail --es host native --ei app 2 --es ui tv` | none |
| A game's page, not installed | `L --es state detail --es host native --ei app 13 --es ui tv` | none: Cinder Road says "Not installed on Living-room PC" and Play is off |
| PCs as tiles | `L --es state hosts --es host native --es ui tv` | none |
| Settings rail | `L --es state settings --es ui tv` | Right into the pane, Left back to the rail, Back to the rail then out |

Frame times for the same screens: `python scripts/tv-perf.py --state library --extra "--es ui tv"`,
then `--state home --host native` and `--state settings`, each with the same `--extra`.
