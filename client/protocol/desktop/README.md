# Nightjar for Windows and Linux

The desktop client: a Moonlight-protocol game-streaming client on the shared Rust engine
(`nightjar-core`). Like the Android app it works with Sunshine, Apollo, Vibepollo, Rubylight
and GeForce Experience; host-specific extras follow the host's capabilities.

Status: Experimental. It pairs, lists and launches apps, and streams with
hardware decoding, VRR pacing, audio, keyboard, mouse and controllers. See
[What works](#what-works) for the details and the gaps.

## Building

### Windows

The GNU toolchain (the default here, as for `nightjar-cli`) with SDL3, FFmpeg and Opus from
MSYS2:

```powershell
C:\msys64\usr\bin\pacman.exe -S --needed mingw-w64-ucrt-x86_64-sdl3 mingw-w64-ucrt-x86_64-ffmpeg mingw-w64-ucrt-x86_64-opus mingw-w64-ucrt-x86_64-pkgconf
$env:PATH = "C:\msys64\ucrt64\bin;$env:USERPROFILE\.cargo\bin;" + $env:PATH
$env:LIBCLANG_PATH = "C:\msys64\ucrt64\bin"      # only if another LLVM is installed (bindgen takes the first libclang)
cargo run --release -p nightjar-desktop      # in protocol/
```

`NIGHTJAR_FFMPEG_DIR` points the build at another FFmpeg prefix (`include/`, `lib/`); on Windows the prefix also holds
Opus, on Linux Opus comes from pkg-config. On Linux the executable then has the run path `$ORIGIN/lib` only, so
`cargo run` and `cargo test` binaries need the prefix's `lib` on `LD_LIBRARY_PATH`.

Why GNU and not MSVC: the binary needs only the system UCRT (no Visual C++ runtime to
redistribute), every native dependency comes from one package manager with import
libraries that match, and the build is the one `nightjar-cli` already uses. The executable
links four DLLs directly: `SDL3.dll`, `libopus-0.dll`, `avcodec-62.dll`, `avutil-60.dll`.
MSYS2's FFmpeg drags in every codec library it was built with (82 DLLs, 120 MB); a
package should ship a decode-only FFmpeg build instead (H.264, HEVC, AV1 with D3D11VA and
dav1d), which brings the whole set to a handful of files.

### Packaging for Windows

`package-windows.ps1` builds the release exe and zips it with the DLLs it imports (found from
the import tables, system libraries left out) into `dist/`, so it runs on a PC without MSYS2.
With `-FfmpegDir <prefix>` the exe is built against the decode-only FFmpeg that
`ffmpeg-decode-only.sh <ffmpeg 8 source> <prefix>` builds in an MSYS2 UCRT64 shell (libavcodec
and libavutil with the H.264, HEVC and AV1 decoders, their D3D11VA paths and dav1d).
The script also checks that no import is left outside the zip or Windows itself (the Visual C++ runtime counts as
missing), then starts the unpacked exe with only the Windows folders on `PATH`. `-Version` names the zip after a release
version; the release workflow builds it that way (`nightjar-desktop-<version>-windows-x86_64.zip`), and
`package-linux.sh -v` does the same for the Linux tar.gz.

`nightjar --check-pyrowave <file>` (no window; the packaging scripts run it on the unpacked package) writes whether the SDK
library beside the executable loads (`load:`) and whether a decoder builds on this PC's GPU (`probe:`) and exits 0, 2 (loaded,
no usable GPU) or 1 (did not load).

### Packaging for Linux

`package-linux.sh -f <FFmpeg prefix> [-s <PyroWave SDK>] [-v <version>]` builds the release binary against
the decode-only FFmpeg that `ffmpeg-decode-only.sh <ffmpeg 8 source> <prefix>` builds on Linux (libavcodec
and libavutil with the H.264, HEVC and AV1 decoders, their VAAPI paths and dav1d; needs `nasm`,
`libdav1d-dev` and `libva-dev`), and ships `libavcodec`, `libavutil` and `libdav1d` in `lib/` beside the
executable. The executable and the libraries have the run path `$ORIGIN/lib` and `$ORIGIN`, so the FFmpeg of the
machine (another version on a Steam Deck, or none) is never used; FFmpeg stays dynamic (LGPL) and its
licence, source offer and dav1d's licence are in `licenses-ffmpeg/` and `licenses-dav1d/`. libopus, libva and the desktop libraries stay the system's
(`LINUX-RUNTIME.txt` lists them, with the libraries `lib/` needs). The script unpacks the archive and fails if `ldd` shows a libav* or libdav1d outside it,
or a library that is not found.

### Linux

```sh
sudo apt install cmake ninja-build pkg-config libavcodec-dev libavutil-dev libopus-dev libva-dev nasm libdav1d-dev \
  libx11-dev libxext-dev libxrandr-dev libxcursor-dev libxi-dev libxss-dev libxfixes-dev libxtst-dev \
  libxkbcommon-dev libwayland-dev wayland-protocols libegl-dev libgl-dev libgles-dev libdrm-dev \
  libgbm-dev libpulse-dev libasound2-dev libpipewire-0.3-dev libudev-dev libdbus-1-dev libdecor-0-dev
cargo run --release -p nightjar-desktop      # in protocol/
```

Distributions without an SDL3 package (Ubuntu 24.04) get SDL 3.4 built from source and
linked statically by `sdl3-sys`; the X11, Wayland, PulseAudio/PipeWire/ALSA and udev
headers above decide which SDL backends are compiled in.

The crate is a member of the `protocol/` workspace but not a default member, so
`cargo test` there and `build-android.sh` stay the engine's; test it with
`cargo test -p nightjar-desktop` and `cargo clippy -p nightjar-desktop --all-targets`.

PyroWave GPU decoding (`pyrowave_decoder = Gpu`, [docs/pyrowave-decode.md](../../docs/pyrowave-decode.md))
loads `libpyrowave-shared.so` from beside the executable and needs a Vulkan 1.3 device. Build the
library with `scripts/build-pyrowave-sdk.sh` (git, cmake 3.27 or newer, ninja, a C++ compiler and `libvulkan-dev`;
it needs the Rubylight host's checkout for its patches, see the script's header) and set `NIGHTJAR_PYROWAVE_SDK` to that folder
to run from `cargo`, or use `package-linux.sh -f <FFmpeg prefix> -s <folder>`, which checks the library against
`pyrowave-sdk.pin` and packs a tar.gz with it, its `build-info.txt`, its licences and the list of
system libraries the executable needs. `Gpu` is the default; without the library (a source build)
PyroWave is not offered and the settings say why, until the decoder is switched to CPU. The client
never switches to the CPU decoder by itself.

## Design

### One window: SDL3, with egui drawn through SDL's renderer

The launcher and the stream share one SDL3 window and one `SDL_Renderer`. egui is used
without eframe: SDL events become egui input (`src/gui.rs`) and egui's triangles are drawn
with `SDL_RenderGeometryRaw`, so the performance overlay is painted over the video in the
same frame and the same swap chain, and going from the app grid to the stream does not
create a new window.

Why this combination:

- SDL3 already is the right layer for a streaming client: gamepads (rumble, trigger
  rumble, LEDs, gyro, touchpads, battery), low-latency audio, relative mouse capture,
  keyboard grabbing (system keys), Wayland and X11, HiDPI, display modes. moonlight-qt
  uses SDL for the same reasons.
- SDL's renderer gives D3D11 on Windows (flip model, tearing presents, NV12/P010 textures,
  and access to its `ID3D11Device` so the decoder can share it) and OpenGL/Vulkan on Linux.
- egui is immediate mode and has keyboard focus navigation with arrow keys, so a
  controller drives the launcher (D-pad/stick move focus, A selects, B goes back, Start
  opens settings) without a second navigation system. It scales with the display's
  content scale.
- eframe (winit + wgpu) would mean a second windowing stack beside SDL and an extra copy
  or interop step for decoded D3D11 surfaces. Slint and Qt were heavier for the same.

### Decoding and presentation

- Video: FFmpeg's libavcodec through Rust bindings (`src/media.rs`), low-delay mode.
  On Windows the decoder uses D3D11VA on the renderer's own D3D11 device, and each decoded
  surface is copied on the GPU into the renderer's NV12/P010 texture under FFmpeg's device
  lock (no CPU copy). The device is made multithread-protected, since the decoder thread and
  the renderer share the immediate context. On Linux the decoder uses VAAPI and maps each
  picture for reading on the decoder thread (the driver copies the surface into an image); the
  presenter uploads from that mapping, with no second copy into a new frame. Either falls back to
  software decoding. Packets go to FFmpeg in pooled, reused buffers: a fresh allocation per frame
  costs page faults once frames reach about 1 MiB on Windows (high bitrates, key frames).
- Pacing (`src/stream/present.rs`), chosen at stream start:
  - VRR with "Show frames at the PC's frame times" (the default when the display path can
    present with tearing): `rubylight_protocol::present_clock::PresentClock` schedules each
    picture at the host's frame time (RTP timestamp) plus a steady delay from the recent
    lateness, so the display refreshes at the game's cadence; presents are immediate with
    tearing allowed (`DXGI_PRESENT_ALLOW_TEARING` on SDL's flip-model swap chain), and the
    UI thread sleeps to the scheduled time with a 1 ms timer.
  - VRR without it: presented as soon as decoded.
  - No VRR: V-Sync (newest picture each refresh), or immediate.
  The launch asks the host for VRR handling when pacing by host times.
- Audio: Opus multistream (libopus) into an SDL3 audio stream; the queue is capped at
  40 ms plus one packet (late packets are dropped rather than letting latency grow), and
  when it runs dry 10 ms of silence goes ahead of the next packet.
- Statistics: clock sync (0x5537) every 250 ms gives the host-to-client clock offset, so
  the overlay shows the PC-frame-to-display latency; host timing (0x5536) gives the host's
  present-to-send and encode times when the host offers them.
  The overlay's Host processing is the video frame header's per-frame time averaged over the
  frames received in the last two seconds, as Artemis and Moonlight show it; the host's
  present-to-send is a separate Host to network line. The statistics CSV column `host_to_network_ms`
  (named `host_ms` before) is that present-to-send, not host processing.

  When the host announces link notices (0x5538) the client asks for them, and the overlay adds a
  "Notice" line, "Host lowered the bitrate to X Mbps: network", while the host holds the
  bitrate down for the network; it clears when the host is back at the requested rate.

### Files

| File | Role |
| --- | --- |
| `src/main.rs` | SDL setup, the main loop, launcher/stream switching, the overlay |
| `src/cli.rs` | `--host`/`--app`/`--quit-on-exit` starts from game frontends |
| `src/gui.rs` | egui input from SDL events, painting with `SDL_RenderGeometryRaw` |
| `src/theme.rs` | palette, Figtree type, the brand mark rasterised from its vector path |
| `src/ui.rs` | the launcher: PCs, pairing, library, settings, dialogs |
| `src/console.rs` | what Home and the game page show, from what the PCs reported (no egui; tested) |
| `src/hosts.rs` | host list, mDNS discovery, serverinfo polling, pairing, app list, box art, the native library and session |
| `src/config.rs` | settings, identity, paired hosts and saved libraries on disk |
| `src/stream/mod.rs` | launch, session start, input, statistics |
| `src/stream/pipeline.rs` | the session's decoder, audio renderer and listener |
| `src/stream/present.rs` | video texture, pacing |
| `src/logfile.rs`, `src/logpipe.rs` | the persistent log file, and the diversion of stderr into it |
| `src/timing_csv.rs` | the client timing CSV (below) |
| `src/gamepad.rs` | controllers |
| `src/keymap.rs` | SDL scancodes to Windows virtual keys |
| `src/media.rs`, `src/media_shim.c` | FFmpeg and Opus |

Config lives in `%APPDATA%\Nightjar\config` or `~/.config/nightjar` (`NIGHTJAR_CONFIG_DIR`
overrides it). F12 saves a screenshot of the window to the config directory's
`screenshots/` (or `NIGHTJAR_SCREENSHOT_DIR`).

Logs: everything the client writes to stderr is kept, with UTC timestamps, in `logs/nightjar.log` in the config
directory (release builds on Windows have no console); the previous run is `nightjar.prev.log`. Settings, App,
"Open logs folder" opens it. `src/logpipe.rs` diverts stderr into a pipe, so the stream threads pay no more than for
`eprintln!`. A separate thread writes the file; the reader holds back up to 4 MB for it, and only a disk that stalls past
that drops log lines (the log says how many) instead of stalling the stream. `src/logfile.rs` holds rotation (also at 4 MB) and stamping.
The file is redacted line by line with the Android app's problem-report rules (`nightjar-core`'s `report::redact`):
the saved PCs' names and addresses, other IP and MAC addresses, URLs, UUIDs, keys and PINs become marks such as `[PC]`
or `[address]`, so the log can be attached to an issue. The console keeps the raw output.

`NIGHTJAR_TIMING_CSV=<file or folder>` writes one row per picture shown, in the format of the
Android app's `nightjar-latency.csv` (a folder gets that file name): times in nanoseconds on the
session clock, the host's stamps from the extended 0x5536 report and the 0x5537 clock offset in
the last nine columns. Rubylight's `rust/release/timing_report.py` joins it with the host's
record (`BUTTERPOLLO_TIMING_RECORD`). It is off unless the variable is set, and a writer thread
does the file work.

## What works

Done, on Windows and Linux unless noted:

- PCs found by mDNS (`_nvstream._tcp`) and added by address (`host`, `host:port`, IPv6);
  online state, paired state and the running app polled every 4 s; addresses learned and
  merged by the host's unique ID. Wake-on-LAN sends a magic packet for every MAC the PC
  lists (serverinfo's, and its other adapters from a native `GET /wake`) on ports 7, 9,
  47009 and the Moonlight ports to every address the PC was known by, 255.255.255.255,
  each adapter's subnet broadcast, the PC's own subnet broadcasts and `ff02::1` (best
  effort). "Waking <PC>…" resends every 8 s until the PC answers, up to 90 s, or Cancel.
- PIN pairing with the PIN shown in large type, and one-time PIN pairing (PIN and
  passphrase) on hosts that report device permissions. The host certificate is pinned and
  stored.
- Library with box art (cached on disk) and placeholders; launch, resume the running app,
  quit it (and "quit and start" another). Remote Monitor and Input-only tiles launch in
  their role when the host publishes them: Remote Monitor sends no input and no microphone,
  clipboard or commands; Input-only starts no decoders.
- Home and the game page (Rubylight 3.0 hosts, through the native API; same rules as
  Nightjar Android's console): Home shows the games to continue (running or suspended on
  a PC that is online), the library of all PCs merged by game and sorted by title, and the
  PCs with what each knows ("Games: 12", "Offline. Saved games: 12"). A game opens its page:
  Play or Resume, the PC, the stream settings that will apply (resolution, frame rate,
  bitrate, profile), Game settings, and Close game (with the host's Host control
  permission; otherwise Quit game). Starting a game while another runs asks first. The last
  library of each PC is kept on disk, so Home works while a PC is away. Back returns to the
  screen and the item it came from, and a controller drives all of it. "Open on Home" in
  Settings (on by default) chooses between starting on Home and on the PCs. Read failures
  show on Home and never block the UI.
  Home also lists the games played most recently on this PC (a local list, updated when a
  stream starts).
- Session and graphics page (menu in a stream, Rubylight 3.0 hosts; modelled on Nightjar
  Android's Session page): Resume, Disconnect, End session and Close game (with Host
  control; otherwise Quit game), asked about before the destructive ones, plus the
  connection facts. Its Radeon panel shows the five states the host reports at `/radeon`,
  edits the settings only when the host says it can write now, shows write errors inline,
  and says they go back to Off when the session ends. The page reads the host only while it
  is open (the session every 2 s, the Radeon state at most once a second), on its own
  thread. A controller and the keyboard drive it; Esc or B closes it.
- Settings in seven sections, in the Android app's order (Video, Display & image, Controls,
  Audio, Streaming, App, Advanced). Every stream setting can be overridden per PC and per
  game. Profiles are picked by hand or by network (mobile, home Wi-Fi, other Wi-Fi), with a
  bitrate cap on mobile data. The launcher is in English and German.
- Video: H.264, HEVC and AV1 with hardware decoding (D3D11VA zero-copy on Windows, VAAPI on
  Linux). A fallback to software decoding is announced, never silent. PyroWave decodes on
  the GPU or the CPU, with a bandwidth test and a frame pace calibration per network. The
  stream follows the display's refresh, fractional rates included; virtual display, PC
  display mode, full range and upscaling (bilinear, FSR 1 or SGSR 1) are settings.
- HDR10 on Windows: Main10, BT.2020 PQ, shown in HDR on an HDR display with the host's
  luminance metadata.
- Stream: VRR host-time pacing, V-Sync, Opus audio, microphone, keyboard with system-key
  capture, relative and absolute mouse, high-resolution scrolling, native touch and pen,
  controllers, error messages for the host's termination reasons and for no video.
- Host extras, each only when the host reports it: clipboard text in both directions and
  the host's commands (with the device permission), host cursor shapes, automatic bitrate,
  automatic resolution and frame rate, phase lock reports (Windows) and display caps (on
  Windows with the display's luminance).
- Reconnect after an interruption (off by default): up to five tries within one minute.
  Network interruption grace, instant mouse and pen input, and on Linux "Prioritize stream
  traffic".
- Performance overlay with host processing; "Export statistics (last hour)" in the stream
  menu writes to the config directory's `statistics/`.
- Starting from a game frontend (ES-DE, Pegasus, Playnite and the like):
  `nightjar --host <name|uuid> [--app <name|uuid>] [--quit-on-exit]` (`src/cli.rs`). The PC
  must be saved and paired; names match the PC's own name or the one given here, ignoring
  case. Without `--app` the PC's app list opens. The app the PC runs is resumed; another
  running app is an error unless the PC asks its user itself. `--quit-on-exit` closes
  Nightjar when the stream ends. An unknown, ambiguous or unreachable PC or app, or a stream
  that cannot start, ends with exit code 1 and the reason on stderr and in the log (no
  message box, which could hide behind the frontend); cancelling the start with 0; a wrong
  command line with 64.
- Shortcuts (from moonlight-qt): Ctrl+Alt+Shift+Q leaves, +Z releases/captures input,
  +S overlay, +M mouse mode, +X fullscreen, +D minimize; Start+Select+LB+RB on a controller
  leaves. Saved key chords are sent from the stream menu and import and export in the
  Android app's shortcut format.

Experimental:

- HDR on Linux: the SDL GPU renderer asks the compositor for an HDR swap chain. Not checked
  on an HDR display yet.
- Phase lock on Linux: X11 with the OpenGL renderer only. Wayland sends no phase reports
  yet.
- Beam race and early decode for PyroWave on a wired link (both off by default; see
  [docs/beam-racing.md](../../docs/beam-racing.md) and
  [docs/early-decode.md](../../docs/early-decode.md)).

Partial:

- Linux decoding uploads VAAPI pictures from a CPU mapping (no dmabuf import yet).
- Controllers: rumble, trigger rumble, LEDs, motion, touchpad, battery, DualSense adaptive
  triggers and controller audio and haptics are wired to SDL's gamepad API but were not
  tried with physical controllers yet.
- The performance overlay is on or off; Android's Compact, Statistics and Debug modes are
  not there.
- 4:4:4: only the experimental packed full chroma mode and PyroWave's chroma modes.

Missing compared with the Android app:

- PyroWave automatic bitrate, the stream health report and its bitrate trial, host CPU and
  GPU telemetry, the network path and remote access check, and the delay test in the
  connection test.
- Host family and version in the PC's details, and a button for the host's web UI.
- Quick resume, quit on leave per game, and Input-only on any PC.
- Problem reports and crash records (the logs folder is there).
- Frame pacing modes (Balanced, Smoothest, drop late) and 10-bit SDR.
- Touch trackpad mode and on-screen controls, a command line for frontends, a screen reader,
  settings search, presets, a PIN lock, more languages, and an updater on Windows.
- A dedicated D3D11 renderer with a waitable swap chain (moonlight-qt's d3d11va/VRR pacer
  design) for frame-latency control beyond what SDL's renderer offers.

## Credits

Copyright (C) 2026 RamazanKara. GPL-3.0-only.

Nightjar's desktop client ports logic and approaches from:

- [moonlight-qt](https://github.com/moonlight-stream/moonlight-qt) (GPL-3.0, Cameron Gutman
  and contributors): the keyboard table (`keyboard.cpp`), controller mapping
  (`gamepad.cpp`), the special key combos, the FFmpeg decoder setup and the D3D11VA surface
  copy under the device lock (`ffmpeg.cpp`, `d3d11va.cpp`), and the default bitrate curve.
- Nonary's (Chase Payne) moonlight-qt fork (GPL-3.0): the reference for VRR pacing,
  tearing presentation and D3D11VA rendering that this client's pacing and decoder path
  were checked against.
- [moonlight-common-c](https://github.com/moonlight-stream/moonlight-common-c) (GPL-3.0),
  through `nightjar-core`.

[Figtree](https://github.com/erikdkennedy/figtree) is under the SIL Open Font License 1.1
(`assets/Figtree-OFL.txt`).
