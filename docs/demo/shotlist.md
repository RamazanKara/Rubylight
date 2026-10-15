# Nightjar demo shotlist

Offline fixture tour: **47 seconds at 30 fps**, including a 3-second intro, 39
seconds of UI and a 5-second outro. Shots are joined with hard cuts: a crossfade
would double the captions and UI text for a few frames. Only the very start
(fade in) and the very end (fade out) are faded. Capture adds one second before
and after each shot; the renderer trims those handles. The caller runs the
capture scripts on a 1080x2400 portrait device.

Brand: the Nightjar amber moon (#F2A93B) on night (#14161F), with the README
banner's moon-and-nightjar tile on the intro and outro. Safe areas for the 9:16 cut: captions end
by y=1440 (75% of 1920), the wordmark and headline start below y=290, and no
text enters the right 120 px.

Intro: **NIGHTJAR** / **Your games. Your screen. Your stream.**

Outro: **NIGHTJAR** / **Start your stream.** / **Get NIGHTJAR** /
**Download on GitHub** / **github.com/RamazanKara/nightjar-android**

This table is the scripts' manifest. Keep the column order and IDs; IDs are
stable, so gaps are expected (the presets and controller-mapping screens were
dropped: they are plain settings lists that read poorly in a short video; the
advanced overlay, touch controls and performance panel shots were dropped after
review, see below). The last column is the demo state; `upscaling` navigates
through the real settings UI. `~` denotes a caption line break. Screenshot paths
are relative to `docs/screenshots`. Every still comes from the populated debug demo. The Crop
column is `full` (use the whole screen) or `crop=w:h:x:y` for the storyboard
fallback only.

| ID | Seconds | Exact caption | Heading | Feature callout | Storyboard screenshot | Crop | Driver |
| --- | ---: | --- | --- | --- | --- | --- | --- |
| 01-launch | 4 | Your PC games. In your hands. | Start~here. | Find your PC.~Start your stream. | demo/pc-list.png | full | hosts |
| 02-discovery | 4 | Your PCs, together. | Find~your PC. | Every PC at a glance~Online and offline | demo/pc-list.png | full | hosts |
| 03-pairing | 4 | Paired and ready to play. | Make~the connection. | Living-room PC~Paired and ready | demo/pc-list.png | full | hosts |
| 04-library | 5 | Your library, ready to play. | Pick~your game. | Your whole library~One tap to play | demo/library.png | full | library |
| 05-stream | 5 | Tap a game. Start your stream. | Press~play. | Tap to play~Your stream starts here | demo/landscape-stream.png | full | stream |
| 06-compact | 6 | Keep the essentials in view. | Stay~in the game. | Compact overlay~Essentials at a glance | demo/landscape-compact.png | full | compact |
| 09-upscaling | 5 | Choose how your stream scales. | Make~it clear. | Upscaling options~Choose what looks best | demo/upscaling-options.png | full | upscaling |
| 12-pip | 6 | Keep your stream in view. | Keep~it in view. | Picture in picture~Your stream floats on top | demo/pip.png | full | pip |

Dropped after the judge round (their clips and stills stay on disk, but the
render and capture scripts read only the table above, so they are not used):

- `07-advanced` and `13-performance` showed the advanced overlay. Its scripted
  numbers read as contradictory (120 FPS and AV1 10-bit HDR next to "Sample
  playback: 1080p60 SDR"), so no shot may show that panel.
- `11-touch` showed the real on-screen controller, which covers the sample's HUD
  and the DEMO label.

Capture direction:

1. Hold the populated PC list for shots 01-03; the pair badge is a fixture state,
   not a recorded pairing exchange. Laptop's Wake PC action sends no packets.
2. For shot 04, the video script opens the real AppView grid with Racing Game
   running. The demo build tidies the cards (no empty "Resume / quit" slot on the
   idle ones, the Running chip in the cover's empty top corner).
3. Stream shots (05, 06) are recorded in landscape: the 16:9 sample fills the
   screen. Shot 05 starts in the library and taps Racing Game while recording.
4. The compact overlay takes the top-left corner of the screen, as in a real stream;
   the sample's HUD panels start below it. Retain the DEMO label and illustrative callouts.
5. Upscaling shows the actual settings screen and choice dialog.
6. PiP begins in the stream, then presses Home and verifies Android's pinned
   window. The script enables the debug app's plain dark `DemoHomeActivity` as the
   home screen for this shot, so the floating window is not over third-party
   launcher icons, and restores the device's launcher afterwards.

The source clip is 1080p60 SDR. The compact overlay's 119.9 FPS / 14 ms / 0.1% loss
readings are scripted illustrations (the DEMO label says so), not measurements:
do not describe them as measured latency, throughput, HDR output or upscaling
performance. Use a real host session for benchmark claims.
`out/demo/sources.json` from the renderer records each actual capture used.
