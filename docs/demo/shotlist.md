# Rubylight demo shotlist

Mission: bring your PC games to the screen in your hands. Use "stream" in all
demo copy. Keep the captions below verbatim; do not add performance claims.

The edit runs **67.4 seconds at 30 fps**: a 2.5-second intro, 66 seconds of UI,
a 4.5-second outro, and fourteen 0.4-second crossfades. Durations in the table
include the transition handles. Capture adds one second before and after each
shot; the renderer trims those handles. Do not rotate the device mid-shot.

Intro: **RUBYLIGHT** / **Your games. Your screen. Your stream.**

Outro: **RUBYLIGHT** / **Start your stream.** / **Get RUBYLIGHT** /
**https://github.com/RamazanKara/rubylight-android/releases**

This table is also the scripts' shot manifest. Keep its column order and IDs.
`~` is an intentional line break in headings and feature callouts. Screenshot
paths are relative to `docs/screenshots`; crop is `width:height:x:y` in source
pixels, or `full`. Crops only apply to the storyboard, never recorded clips.

| ID | Seconds | Exact caption | Heading | Feature callout | Storyboard screenshot | Crop | Driver |
| --- | ---: | --- | --- | --- | --- | --- | --- |
| 01-launch | 5 | Your PC games. In your hands. | Start~here. | First launch~Connect to your PC | 01-launch.png | full | auto |
| 02-discovery | 5 | Find your PC. | Find~your PC. | Local discovery~Add a PC by address | 02-manual-host.png | full | manual |
| 03-pairing | 5 | Pair. Make it yours. | Make~the connection. | PIN pairing~Your device, connected | 16-otp-pairing.png | full | manual |
| 04-library | 5 | Your library, ready to play. | Pick~your game. | Your PC library~Ready to play | 03-host-added.png | full | manual |
| 05-stream | 5 | Tap a game. Start your stream. | Press~play. | Start a stream~Play on your screen | 03-host-added.png | full | manual |
| 06-compact | 5 | Keep the essentials in view. | Stay~in the game. | Compact overlay~FPS / latency / loss | 07-latency-overlay.png | full | manual |
| 07-advanced | 5 | See every stage of your stream. | See~the detail. | Advanced overlay~Video / network / decode | 08-overlay-rotated.png | full | manual |
| 08-presets | 5 | Pick the setup that fits. | Set~your pace. | Stream presets~A setup for your screen | 04-settings.png | full | auto |
| 09-upscaling | 5 | Choose how your stream scales. | Make~it clear. | Upscaling options~Bilinear / FSR 1 / SGSR 1 | 05-video-settings.png | 1080:1010:0:0 | auto |
| 10-controller | 5 | Play with your controller. | Take~control. | Controller buttons~Map your inputs | 11-controller-choose.png | full | auto |
| 11-touch | 5 | Make touch controls your own. | Make~it yours. | Touch controls~Your layout, your way | 04-settings.png | full | auto |
| 12-pip | 5 | Keep your stream in view. | Keep~it in view. | Picture in picture~Your stream stays visible | 04-settings.png | full | manual |
| 13-performance | 6 | See FPS, latency and frame loss. | Know~your stream. | Live measurements~FPS / latency / frame loss | 07-latency-overlay.png | full | manual |

## Capture direction

Use an English-language installation of the current non-root app, a 1080x2400
portrait device, neutral host/app names, and clean notification/status areas.
Use gameplay you can publish. The script does not clear app data, pair a host,
change presets, or enable controls. Prepare stream preferences before recording.

1. **Launch:** open the first-run connection guide. The automatic driver reopens
   the same guide through Help if it has already been dismissed.
2. **Discovery:** show a discovered PC, then open Add PC. Keep addresses neutral.
3. **Pairing:** open the PIN prompt; complete pairing on the host during the take.
   Use a demo device and short-lived pairing credentials.
4. **Library:** enter the paired PC's library. Hold on a small, readable game grid.
5. **Stream:** begin on a game tile. Tap when the recording cue appears; show the
   first playable frames. Use a warmed-up host so launch fits the five-second edit.
6. **Compact:** play with Compact visible. Hold the metrics in a clear corner.
7. **Advanced:** open stream menu > Overlay > Advanced. Hold, then scroll once
   through Video, Network, Decode and Host. Keep the overlay inside the screen.
8. **Presets:** show Settings and its preset chips. The driver only opens this
   screen; it leaves the chosen preset intact.
9. **Upscaling:** Settings > Stream > Client-side upscaling. Hold on the choices.
   The driver opens the dialog without choosing a different mode.
10. **Controller:** Settings > Controls > Controller buttons. Connect a controller
    before the take to demonstrate a button mapping; the driver can open this
    screen without a controller or host.
11. **Touch:** Settings > Controls > On-screen controls. The driver scrolls to
    Show on-screen controls without changing it. For a live retake, show the
    in-stream layout editor and drag one control to its new position.
12. **PiP:** enable Picture in picture beforehand. Begin with a running stream;
    press Home after the cue and hold on the floating stream window.
13. **Performance:** hold a steady live scene with measured FPS, latency and frame
    loss visible. Record actual session values; do not type a benchmark into the
    caption or reuse synthetic overlay values as measurements.

## Storyboard sources

The checked-in screenshots are earlier UI captures. Launch, discovery, pairing,
settings and controller screens provide the draft's visual source material.
The host card stands in for library and stream start; settings stand in for
presets, touch and PiP; the video-settings crop stands in for upscaling. The
overlay captures contain no live measurements and stand in for both current
overlay modes and the performance shot. Capture current Rubylight screens
before releasing the demo.
The video-settings crop excludes the old host compatibility copy.

Replace these stills with the matching recorded clips for the release cut.
`out/demo/sources.json` records the actual input used for each shot.
