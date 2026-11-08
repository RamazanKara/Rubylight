# RUBYLIGHT demo production

The scripts produce a 67.4-second demo in landscape and portrait, using the
captions and source mapping in [shotlist.md](shotlist.md). Run them from any
directory in PowerShell 7. All generated files stay in
the worktree's gitignored `out/demo/` directory. No publishing step is included.

## Render the storyboard

Use the existing Windows FFmpeg installation, including `ffprobe.exe`. The
renderer searches PATH, the WinGet link, then
`%LOCALAPPDATA%\Microsoft\WinGet\Packages\Gyan.FFmpeg_*\*\bin`. If the installation
is not accessible, put a readable installed `bin` directory on PATH. The scripts
do not download or install tools. They use Windows' Segoe UI regular and bold
fonts from `%WINDIR%\Fonts`, and Windows System.Drawing for the gradient and
phone bezel.

```powershell
.\scripts\demo\render.ps1 -Storyboard
```

This builds the entire edit from `docs/screenshots`, with alternating gentle
zoom-in and zoom-out motion. The motion has padded edges to preserve UI content.
The older screenshots provide a storyboard: several current features need live
footage, as listed in the shotlist. The draft preserves the source screenshots;
it does not manufacture a library, a live stream, PiP, or performance readings.

```powershell
.\scripts\demo\render.ps1
```

The normal render uses `out/demo/clips/<shot-id>.mp4` wherever present and falls
back to the listed still for each missing clip. A present but invalid, corrupt,
wrong-sized, or short clip stops the render rather than being silently replaced.
`sources.json` identifies every actual source. Check it before selecting files
for release. Running again replaces the outputs and intermediates.

## Record footage on the caller's device

Use the current non-root RUBYLIGHT package (`com.butterpollo.client`), an English
UI, USB debugging, an unlocked portrait device, and Android platform-tools on
PATH or under `ANDROID_HOME` / `%LOCALAPPDATA%\Android\Sdk`. Connect one authorized
device, or set Android's standard `ANDROID_SERIAL` environment variable. No
emulator is started by either script.

```powershell
# Deterministic screens that need no PC host.
.\scripts\demo\capture.ps1 -NoHost

# All thirteen shots: automatic navigation plus cues for host footage.
.\scripts\demo\capture.ps1

# Retake specific shots without redoing the session.
.\scripts\demo\capture.ps1 -Shot 05-stream,06-compact,07-advanced,13-performance
```

`capture.ps1` uses `adb input` and exact resource IDs/text from `uiautomator dump`
to reopen the connection guide, settings presets, upscaling choices, controller
buttons and on-screen control settings. It polls for expected UI before moving
on and saves the last XML tree on failure. It does not clear app data or change
preferences. The current screen is recorded for each automatic take.

For manual takes, prepare the screen described in the shotlist, press Enter, and
perform the action at the `ACTION` cue. Pairing, the library, stream launch, live
overlays, PiP and measured performance require a prepared PC host. Have the host
ready and a game warmed up before capture. Connect a controller before recording
its mapping screen. Use a neutral device background for the PiP take.

Each take runs:

```text
adb shell screenrecord --size 1080x2400 --bit-rate 20000000 --time-limit <seconds+2> /sdcard/rubylight-demo/<shot-id>.mp4
adb pull /sdcard/rubylight-demo/<shot-id>.mp4 out/demo/clips/<shot-id>.capture.mp4
```

The pulled file becomes `<shot-id>.mp4` after successful recording and transfer.
The first second is trimmed; the last second is a recording handle. UI XML and
recorder logs remain beside the clips. Successful takes remove their remote
MP4. The small working UI dump remains in `/sdcard/rubylight-demo/ui.xml`.
Use a device whose recorder supports 1080x2400; a rejected recording size stops
the take. Keep portrait orientation fixed through each take so the stream stays
inside the recorded viewport. A landscape stream can be letterboxed within that
portrait recording. `screenrecord` supplies the picture; the render supplies the
audio. See the [Android screenrecord reference](https://developer.android.com/tools/adb#screenrecord).

## Edit and audio

The launcher vector colours are `#C41242` ruby and `#FFFFFF` white. The intro is a
RUBYLIGHT wordmark with the mission tagline. The outro carries the releases URL.
Both layouts use a centred phone bezel over a diagonal gradient; landscape adds
feature callouts. Captions are burned in with Segoe UI, kept outside the phone,
and wrapped explicitly in portrait. Text files avoid shell and filter escaping
problems. Fonts are copied only to the ignored work directory.

The edit uses 0.4-second crossfades and 30 fps throughout. Pairwise assembly keeps
only two video decoders active at once. Source footage is fitted without cropping;
the shotlist's storyboard crops exclude older compatibility and about copy.

The music is synthesized entirely with FFmpeg `aevalsrc`: a slow stereo sine
chord, low-pass filtering and echo. There are no sampled recordings or downloaded
music assets. A separate inaudible cue ducks the bed under the caption sequence
using `sidechaincompress`, with attack/release smoothing. Two-pass `loudnorm`
targets -20 LUFS integrated, -2 dBTP and 7 LU loudness range. Final AAC audio is
measured again; its report is in `work/<aspect>-audio-check.log`. Source clip audio
is not mixed into the soundtrack. Filter details are in the
[FFmpeg reference](https://ffmpeg.org/ffmpeg-filters.html).

## Outputs and review

| File | Content |
| --- | --- |
| `out/demo/demo-16x9.mp4` | 1920x1080, H.264, yuv420p, 30 fps, AAC stereo, faststart |
| `out/demo/demo-9x16.mp4` | 1080x1920, H.264, yuv420p, 30 fps, AAC stereo, faststart |
| `out/demo/poster-16x9.png` / `poster-9x16.png` | Full-size branded posters at 1 second |
| `out/demo/frames/demo-<aspect>-01.png` through `08.png` | Eight evenly spaced frames, from 0.8 seconds to 0.8 seconds before the end |
| `out/demo/demo-<aspect>.ffprobe.json` | Actual codec, dimensions, pixel format, frame rate and duration |
| `out/demo/sources.json` | Clip/still provenance for all thirteen shots |
| `out/demo/work/` | Generated artwork, text, music, segments, filter graphs and FFmpeg logs |

The renderer checks export dimensions, codec, pixel format and duration, and
extracts all sixteen review frames automatically. Inspect them at full size:
captions must be readable, the phone and callouts must be separated, text must
stay inside the canvas, and ruby/white branding must be consistent. Portrait
captions end above the bottom 192-pixel safe area; landscape copy stays at least
54 pixels from the lower edge. Play both exports to check crossfades and music.

For the release cut, replace the mapped storyboard sources with recorded clips,
render again, inspect the source manifest and review frames, then upload the two
MP4s and chosen posters as assets on
[RUBYLIGHT releases](https://github.com/RamazanKara/rubylight-android/releases).
The caller owns the release upload. Neither script commits, pushes, invokes
`gh`, creates a release, or changes the app.
