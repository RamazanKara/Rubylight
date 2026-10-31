# Nightjar for Windows and Linux

Nightjar for Windows and Linux is the desktop edition of Nightjar: a Moonlight-protocol game-streaming client built on the same Rust engine (`nightjar-core`) as the Android app. It works with Sunshine, Apollo, Vibepollo, Rubylight and other Moonlight-compatible hosts; features beyond the standard protocol follow the capabilities each host reports.

The first half of this page covers installing and using the client; the second half covers building, packaging and the design. [Android and desktop](../../docs/CLIENT-PARITY.md) compares the two clients feature by feature.

## Installing

Every release carries these files. The installer and the packages contain the same build as the archives.

| File | Install | Uninstall |
| --- | --- | --- |
| `nightjar-desktop-<version>-windows-x86_64-setup.exe` | Run it. It installs for the current user, without an administrator prompt, into `%LOCALAPPDATA%\Programs\Nightjar`, with a Start menu shortcut, an optional desktop shortcut and an entry in Apps & features. A newer setup updates in place and closes a running Nightjar first. | **Apps & features → Nightjar → Uninstall** removes the program folder, the shortcuts and the entry. |
| `nightjar-desktop-<version>-windows-x86_64.zip` | Unzip it anywhere and run `nightjar.exe`. | Delete the folder. |
| `nightjar-desktop-<version>-linux-x86_64.deb` | `sudo apt install ./nightjar-desktop-<version>-linux-x86_64.deb` (Debian 12, Ubuntu 22.04 and later) | `sudo apt remove nightjar` |
| `nightjar-desktop-<version>-linux-x86_64.rpm` | `sudo dnf install ./nightjar-desktop-<version>-linux-x86_64.rpm` (Fedora 36 and later) | `sudo dnf remove nightjar` |
| `nightjar-desktop-<version>-x86_64.AppImage` | Make it executable (`chmod +x`) and run it; nothing is installed. Without FUSE, run it with `--appimage-extract-and-run`. | Delete the file. |
| `nightjar-desktop-<version>-linux-x86_64.tar.gz` | Unpack it and run `./nightjar`. `share/` holds the desktop entry and icons, laid out as under `/usr/share`. | Delete the folder. |

The `.deb` and `.rpm` install the program in `/opt/nightjar`, `nightjar` in `/usr/bin`, and the desktop entry `io.github.ramazankara.nightjar.desktop`, the hicolor icons and the AppStream metadata in `/usr/share`. They require glibc 2.35 or newer, libopus, libva, libudev, and the ALSA, libX11 and libXext libraries without which SDL3 cannot start, and recommend the libraries SDL3 and PyroWave load at run time (Wayland, PulseAudio, PipeWire, Vulkan, a VA-API driver). The AppImage also carries libopus, plus libva, libva-drm and libdrm as fallbacks: its `AppRun` uses them only where the system has none, so an installed libva still finds the system's VA-API drivers (without one, decoding falls back to software, with a notice). ALSA and X11 come from the system.

Settings, paired PCs, controller layouts, app lists, cover art, logs, screenshots and exported statistics are stored in `%APPDATA%\Nightjar\config` on Windows or `~/.config/nightjar` on Linux, outside every install folder. Updates never touch them, and uninstalling keeps them, except that the Windows uninstaller offers **Remove them too** (off unless ticked; `unins000.exe /VERYSILENT /REMOVESETTINGS` does the same silently). That option removes everything except the `screenshots` and `statistics` folders. On Linux, delete `~/.config/nightjar` by hand.

`nightjar --version` prints the version and exits without opening a window; on Windows it prints into the Command Prompt or PowerShell window it was started from.

## Using Nightjar

1. Start Nightjar on the same network as the PC. PCs found on the network appear automatically; otherwise choose **Add PC** and enter the address (`host`, `host:port` or an IPv6 address), or paste the pairing link from a Rubylight PC's QR code to add and pair it in one step.
2. Select the PC, choose **Pair** and enter the PIN that Nightjar shows in the host's web UI.
3. Choose a game. **Ctrl+Alt+Shift+Tab**, or **Start+Back** on a controller, opens the stream controls; **Ctrl+Alt+Shift+Q**, or **Start+Select+LB+RB**, leaves the stream.

| Topic | Reference |
| --- | --- |
| Every setting, with defaults | [Settings: Nightjar for Windows and Linux](../../docs/settings.md#nightjar-for-windows-and-linux) |
| Keyboard shortcuts and controllers | [Controls: Nightjar for Windows and Linux](../../docs/controls.md#nightjar-for-windows-and-linux) |
| Problems and their fixes | [Troubleshooting: Nightjar for Windows and Linux](../../docs/TROUBLESHOOTING.md#nightjar-for-windows-and-linux) |
| Clipboard, files, shared folders and USB devices | [Sharing](../../docs/sharing.md) |
| Home, game pages and the session menu | [Home and the stream menu](../../docs/console.md) |

**Files.** The log is `logs/nightjar.log` in the settings folder, with the previous run in `nightjar.prev.log`; **Settings → App → Logs → Open logs folder** opens it. Every line is redacted with the Android app's problem-report rules: saved PC names and addresses, other IP and MAC addresses, URLs, UUIDs, keys and PINs become marks such as `[PC]` or `[address]`, so the log can be attached to an issue as it is. On Windows, a crash also writes a minidump (threads, stacks and modules, no memory contents) to the logs folder. **F12** saves a screenshot to `screenshots/` (or `NIGHTJAR_SCREENSHOT_DIR`), and **Export statistics (last hour)** in the stream menu writes to `statistics/`.

**Starting from a game frontend** (ES-DE, Pegasus, Playnite and similar):

```text
nightjar --host <name|uuid> [--app <name|uuid>] [--quit-on-exit]
```

The PC must be saved and paired. Names match the PC's own name or the name given in Nightjar, ignoring case. Without `--app`, the PC's app list opens. The app the PC is running is resumed; a different running app is an error unless the host asks its user itself. `--quit-on-exit` closes Nightjar when the stream ends. Exit codes: 0 for a normal end or a cancelled start, 1 for an unknown, ambiguous or unreachable PC or app or a stream that cannot start (the reason goes to stderr and the log, never to a message box that could hide behind the frontend), and 64 for an invalid command line. `--help` and an invalid command line print the usage; a Windows release build, which has no console, also shows it in a dialog, except for command lines with `--host`.

## What works

Everything below works on Windows and Linux unless a platform is named.

**PCs and pairing**

- Discovery over mDNS (`_nvstream._tcp`) and manual addresses, including IPv6. Online state, pairing state and the running app are polled every 4 s, and addresses are merged by the host's unique ID.
- PIN pairing, with the PIN in large type; one-time PIN pairing (PIN and passphrase) on hosts that report device permissions; QR pairing links. The host certificate is pinned.
- Wake-on-LAN sends a magic packet for every MAC address the PC reports (from serverinfo, and its other adapters from a native `GET /wake`) to ports 7, 9, 47009 and the Moonlight ports, at every address the PC was known by, 255.255.255.255, each local adapter's subnet broadcast, the PC's own subnet broadcasts and `ff02::1`. **Waking <PC>…** resends every 8 s until the PC answers, for up to 90 s, or until cancelled.
- The PC's web UI opens from its details, and **Remote access → Test this PC** checks that the paired PC answers over HTTPS from the current network.

**Library**

- Library with cached cover art; launch, resume, quit, and quit-and-start another app. Remote Monitor and Input-only entries start in their role where the host publishes them.
- Home and the game page on hosts with the native API (Rubylight 3.0): games to continue, the library of all PCs merged by title, the PCs and what each knows, recently played games, and a page per game with Play or Resume, the stream settings that will apply, game settings and Close game. The last library of each PC is kept, so Home works while a PC is away. **Open on Home** (on by default) chooses between Home and the PC list.
- **Co-op sessions** join or start a Nucleus Co-op session on hosts that offer it.
- The session and graphics page in the stream menu (Rubylight 3.0): Resume, Disconnect, End session and Close game with confirmation, the connection facts, and the Radeon panel.
- **Quick resume**, per-game quit on leave, and **Input-only on any PC**.

**Picture**

- H.264, HEVC and AV1 with hardware decoding: D3D11VA on Windows; VA-API, then V4L2 M2M, then NVDEC on Linux. A fallback to software decoding is announced. PyroWave decodes on the GPU or the CPU, with a bandwidth test and a frame-pace calibration per network.
- HDR: HDR10 and HLG, plus Dolby Vision profiles 8.1 and 10.1 on Windows from hosts that send them. On Linux the client requests an HDR surface from the compositor and draws HLG for hardware-decoded pictures. 10-bit SDR, YUV 4:4:4 and full chroma where the host and decoder support them.
- Upscaling with bilinear, FSR 1, SGSR 1 or AI enhancement (the GPU vendor's video super resolution).
- Variable refresh rate with frames shown at the PC's frame times, frame pacing modes and V-Sync without VRR, fractional refresh rates, virtual displays and PC display modes.

**Sound**

- Opus in stereo, 5.1 and 7.1; 7.1.4 on Windows. Uncompressed PCM passthrough from hosts that offer it. Microphone with echo cancellation.

**Input**

- Keyboard with system-key capture, captured and desktop-pointer mouse modes with a locally drawn cursor, high-resolution scrolling, native touch and pen, touchpad gestures.
- Controllers through SDL: rumble, trigger rumble, LEDs, motion, touchpad and battery; DualSense adaptive triggers, and DualSense USB audio and waveform haptics. Controller mouse mode, button mapping, stick dead zones, and saved keyboard shortcuts that import and export in the Android app's format.

**Sharing**

- Clipboard sync, file transfer, shared folders and USB forwarding on hosts that offer them (see [sharing](../../docs/sharing.md)).

**Network and quality**

- Recovery of interrupted streams (up to five attempts within one minute), network interruption grace, client roaming, stream traffic priority.
- Device controls bitrate, automatic quality (resolution and frame rate), loss-based bitrate, a link test before the first bitrate, and error correction choices.

**Diagnostics and interface**

- Performance overlay in Compact, Statistics and Debug modes, with the host's CPU and GPU readings where offered; the stream health report (**Why did my stream stutter?**) with the reversible bitrate trial; statistics export.
- English and German; UI zoom, high contrast, reduced motion, and screen readers through UI Automation on Windows and AT-SPI on Linux. PIN lock. On Windows, an update check once per launch.
- Beam racing and early decode for PyroWave on wired links, off by default (see [beam racing](../../docs/beam-racing.md) and [early decode](../../docs/early-decode.md)).

### Differences from the Android app

Not available on the desktop:

- Settings search and presets.
- On-screen controls and the touch modes (trackpad, direct, multi-touch); the desktop sends native touch.
- PyroWave automatic bitrate.
- A problem-report export; the logs folder serves that purpose.
- The latency benchmark, the decoder benchmark, and the network path and delay test of the connection test.
- HDR10+, noise suppression and microphone gain, and Dolby passthrough.
- Languages other than English and German.

Platform limits:

- On Linux, 7.1.4 is unavailable (SDL opens at most eight channels) and Dolby Vision is not shown. Phase-lock frame timing needs X11 with the OpenGL renderer.
- On Linux, 8-bit 4:2:0 VA-API pictures reach the OpenGL renderer without copies; 10-bit pictures and those drawn by the GPU renderer (HDR, upscaling, 4:4:4) are copied through memory.
- A hardware decoder that returns 4:4:4 as packed surfaces (AYUV, Y410) streams in 4:2:0; the software decoder draws 4:4:4.

## Building

### Windows

The GNU toolchain (the default here, as for `nightjar-cli`), with SDL3, FFmpeg and Opus from MSYS2:

```powershell
C:\msys64\usr\bin\pacman.exe -S --needed mingw-w64-ucrt-x86_64-sdl3 mingw-w64-ucrt-x86_64-ffmpeg mingw-w64-ucrt-x86_64-opus mingw-w64-ucrt-x86_64-pkgconf
$env:PATH = "C:\msys64\ucrt64\bin;$env:USERPROFILE\.cargo\bin;" + $env:PATH
$env:LIBCLANG_PATH = "C:\msys64\ucrt64\bin"      # only if another LLVM is installed (bindgen takes the first libclang)
cargo run --release -p nightjar-desktop      # in protocol/
```

GNU rather than MSVC: the binary needs only the system UCRT (no Visual C++ runtime to redistribute), every native dependency comes from one package manager with matching import libraries, and the build matches `nightjar-cli`'s. The executable links four DLLs directly: `SDL3.dll`, `libopus-0.dll`, `avcodec-62.dll` and `avutil-60.dll`. MSYS2's FFmpeg pulls in every codec library it was built with (82 DLLs, 120 MB), so packages ship a decode-only FFmpeg build instead (see [Packaging for Windows](#packaging-for-windows)).

`NIGHTJAR_FFMPEG_DIR` points the build at another FFmpeg prefix (`include/`, `lib/`). On Windows the prefix also provides Opus; on Linux Opus comes from pkg-config. On Linux the executable then has the run path `$ORIGIN/lib` only, so `cargo run` and `cargo test` binaries need the prefix's `lib` on `LD_LIBRARY_PATH`.

### Linux

```sh
sudo apt install cmake ninja-build pkg-config libavcodec-dev libavutil-dev libopus-dev libva-dev nasm libdav1d-dev \
  libx11-dev libxext-dev libxrandr-dev libxcursor-dev libxi-dev libxss-dev libxfixes-dev libxtst-dev \
  libxkbcommon-dev libwayland-dev wayland-protocols libegl-dev libgl-dev libgles-dev libdrm-dev \
  libgbm-dev libpulse-dev libasound2-dev libpipewire-0.3-dev libudev-dev libdbus-1-dev libdecor-0-dev
cargo run --release -p nightjar-desktop      # in protocol/
```

On distributions without an SDL3 package (Ubuntu 22.04 and 24.04), `sdl3-sys` builds SDL 3.4 from source and links it statically; the X11, Wayland, PulseAudio, PipeWire, ALSA and udev headers above decide which SDL backends are compiled in.

### PyroWave

PyroWave GPU decoding (`pyrowave_decoder = Gpu`, the default; see [PyroWave decoding](../../docs/pyrowave-decode.md)) loads the PyroWave SDK library from beside the executable and needs a Vulkan 1.3 device on Linux. Build the library with `scripts/build-pyrowave-sdk.sh` (git, CMake 3.27 or newer, Ninja, a C++ compiler and `libvulkan-dev`; it needs the Rubylight host's checkout for its patches, see the script's header), then either set `NIGHTJAR_PYROWAVE_SDK` to that folder to run from `cargo`, or pass it to `package-linux.sh -s`, which checks it against `pyrowave-sdk.pin`. Without the library, PyroWave is not offered and the settings say why, until the decoder is switched to CPU; the client never switches to the CPU decoder by itself.

`nightjar --check-pyrowave <file>` (no window; the packaging scripts run it on the unpacked package) writes whether the SDK library loads (`load:`) and whether a decoder can be created on this PC's GPU (`probe:`), and exits with 0, 2 (loaded, no usable GPU) or 1 (did not load).

### Tests

The crate is a member of the `protocol/` workspace but not a default member, so `cargo test` there and `build-android.sh` cover only the engine. Test the desktop client with `cargo test -p nightjar-desktop` and `cargo clippy -p nightjar-desktop --all-targets`.

### Packaging for Windows

`package-windows.ps1` builds the release executable and zips it into `dist/` with the DLLs it imports (found from the import tables, system libraries excluded), so it runs on a PC without MSYS2. With `-FfmpegDir <prefix>`, the executable is built against the decode-only FFmpeg that `ffmpeg-decode-only.sh <ffmpeg 8 source> <prefix>` builds in an MSYS2 UCRT64 shell: libavcodec and libavutil with the H.264, HEVC and AV1 decoders, their D3D11VA paths and dav1d. The script fails if an import is satisfied neither by the zip nor by Windows itself (the Visual C++ runtime counts as missing), then starts the unpacked executable with only the Windows folders on `PATH`. `-Version` names the zip after a release version, as the release workflow does.

### Packaging for Linux

`package-linux.sh -f <FFmpeg prefix> [-s <PyroWave SDK>] [-v <version>]` builds the release binary against the decode-only FFmpeg that `ffmpeg-decode-only.sh <ffmpeg 8 source> <prefix>` builds on Linux: libavcodec and libavutil with the H.264, HEVC and AV1 decoders, their VA-API and NVDEC paths, the V4L2 M2M decoders and dav1d (needs `nasm`, `libdav1d-dev`, `libva-dev` and `libffmpeg-nvenc-dev`; where dav1d is older than 1.0 or the NVIDIA headers older than 11.1.5.3, as on Ubuntu 22.04, `ffmpeg-deps-linux.sh <prefix>` builds both at pinned versions, and `PKG_CONFIG_PATH=<prefix>/lib/pkgconfig` points the FFmpeg build at them). It ships `libavcodec`, `libavutil` and `libdav1d` in `lib/` beside the executable, with the run paths `$ORIGIN/lib` and `$ORIGIN`, so the system's FFmpeg (another version on a Steam Deck, or none) is never used. FFmpeg stays dynamically linked (LGPL); its license, source offer and dav1d's license are in `licenses-ffmpeg/` and `licenses-dav1d/`. libopus, libva and the desktop libraries come from the system; `LINUX-RUNTIME.txt` lists them, together with the libraries `lib/` needs. The script unpacks the archive and fails if `ldd` shows a libav* or libdav1d library from outside it, or a library that is not found.

### The Windows installer and the Linux packages

Both are built from the finished archives, so they carry exactly the same files (`packaging/`):

- `packaging/windows/build-installer.ps1 -Zip <the zip> [-Smoke]` compiles `packaging/windows/nightjar.iss` with Inno Setup 6 into the setup executable. Inno Setup provides a per-user install (`PrivilegesRequired=lowest`), the uninstaller, the Apps & features entry and in-place upgrades by a fixed AppId out of the box; NSIS would need all of that written by hand, and a Rust self-installer would mean maintaining an installer, an uninstaller and their registry entries. `-Smoke` installs silently, compares every file with the zip, checks the shortcuts and the uninstall entry, runs `nightjar.exe --version`, upgrades in place with a second run (a planted stale DLL must be removed, the settings must stay), uninstalls and fails on anything left behind. It then checks that a plain uninstall keeps the settings and that `/REMOVESETTINGS` removes them but keeps screenshots and statistics. On GitHub Actions it runs against the runner's `%APPDATA%\Nightjar`; anywhere else, a test build of the installer (`/DTestDataDir`) uses a temporary folder and the real one is never touched. It refuses to run where Nightjar or its shortcuts are already installed. The packages are not code-signed; `-SignCommand` is the hook for a signing command (see the script's header).
- `packaging/linux/build-packages.sh -n <nfpm> -a <appimagetool> -r <AppImage runtime> <the tar.gz>` builds the `.deb` and `.rpm` with nfpm (one description for both, no rpmbuild needed on the Ubuntu build machine) and the AppImage with appimagetool and a pinned type 2 runtime. Dependencies are read from the binaries: the `.deb` names the packages that own the linked libraries on the build machine and the newest `GLIBC_` symbol version; the `.rpm` requires the same libraries by soname. The script also writes `PKGBUILD` (Arch Linux) and `io.github.ramazankara.nightjar.yml` (Flatpak) from `packaging/linux/aur` and `packaging/linux/flatpak` with the archive's version and SHA-256; these recipes are kept in the repository and not published.
- `packaging/make-icons.py` generates the icons (hicolor PNGs, `nightjar.ico`, the installer's side panel) from the store icon. `build.rs` embeds `nightjar.ico` in `nightjar.exe`; the Windows build needs `windres`, which MSYS2's UCRT64 binutils provide.

The release workflow pins nfpm, appimagetool, the AppImage runtime and Inno Setup by version and SHA-256. It builds on Ubuntu 22.04, the oldest Ubuntu LTS still in standard support, so the packages need only its glibc 2.35. It installs the `.deb` on that runner and in a clean Ubuntu 24.04, the `.rpm` in a clean Fedora container, and the AppImage in another Fedora container without libva and libdrm and in a clean Ubuntu 22.04 without libva (the AppImage catalog's test system), and runs the installer's `-Smoke` on its Windows runner. Because `--version` returns before SDL starts, it also checks that the `.deb` and `.rpm` declare the libraries SDL cannot start without and, with `packaging/linux/check-start.sh`, that each package and the AppImage open the launcher window under Xvfb in a clean container.

## Design

### One window: SDL3, with egui drawn through SDL's renderer

The launcher and the stream share one SDL3 window and one `SDL_Renderer`. egui is used without eframe: SDL events become egui input (`src/gui.rs`), and egui's triangles are drawn with `SDL_RenderGeometryRaw`, so the performance overlay is painted over the video in the same frame and swap chain, and moving from the library to the stream creates no new window.

- SDL3 is the natural layer for a streaming client: gamepads (rumble, trigger rumble, LEDs, gyro, touchpads, battery), low-latency audio, relative mouse capture, keyboard grabbing for system keys, Wayland and X11, HiDPI and display modes. moonlight-qt uses SDL for the same reasons.
- SDL's renderer provides D3D11 on Windows (flip model, tearing presents, NV12 and P010 textures, and access to its `ID3D11Device`, which the decoder shares) and OpenGL or Vulkan on Linux.
- egui is immediate-mode and has keyboard focus navigation, so a controller drives the launcher (D-pad or stick moves focus, A selects, B goes back, Start opens settings) without a second navigation system. It scales with the display's content scale. Its AccessKit tree reaches UI Automation and AT-SPI through `src/a11y.rs`, built only while a screen reader asks for it.
- eframe (winit and wgpu) would add a second windowing stack beside SDL and an extra copy or interop step for decoded D3D11 surfaces. Slint and Qt would be heavier for the same result.

### Decoding and presentation

- **Video decoding.** FFmpeg's libavcodec through Rust bindings (`src/media.rs`), in low-delay mode. Packets reach FFmpeg in pooled, reused buffers: a fresh allocation per frame costs page faults once frames reach about 1 MiB on Windows (high bitrates, key frames).
- **Windows.** D3D11VA decodes on the renderer's own D3D11 device, made multithread-protected because the decoder thread and the renderer share the immediate context. A draw pass (`src/stream/direct.rs`) renders hardware-decoded pictures straight into SDL's back buffer. With decoder-texture binding (Intel GPUs by default; `NIGHTJAR_D3D11_BIND=1` or `0` overrides) it reads the picture from the decoder's texture array; otherwise it reads a GPU copy made under FFmpeg's device lock. Its tone map takes the content peak per frame from the Dolby Vision RPU, else HDR10+ metadata, else the picture itself, and HLG goes through BT.2100's inverse OETF and OOTF for the display's peak. `NIGHTJAR_D3D11_PASS=0` leaves drawing to SDL. 7.1.4 audio plays through WASAPI (`src/stream/wasapi.rs`), since SDL opens at most eight channels.
- **Linux.** The decoder tries VA-API, then the V4L2 memory-to-memory decoders (ARM handhelds and boards, H.264 and HEVC), then NVDEC, the order moonlight-qt and Moonlight V+ use; each failure is logged before the next is tried. With the OpenGL renderer on EGL (`SDL_HINT_VIDEO_FORCE_EGL` makes X11 use it too), an 8-bit 4:2:0 VA-API picture never crosses the CPU (`src/dmabuf.rs`): the decoder thread exports the surface as DMA-BUFs, one layer per plane, and the presenter binds an EGL image of each to the matching plane of SDL's NV12 texture, holding the picture until the next replaces it. Otherwise (the GPU renderer, 10-bit, packed formats, or a failed import, logged once) the decoder maps each picture for reading and the presenter uploads from that mapping without a second copy; NVDEC pictures are copied out of CUDA memory. `NIGHTJAR_DMABUF=0` turns the import off for A/B measurements.
- **Pacing** (`src/stream/present.rs`), chosen at stream start:
  - VRR with **Show frames at the PC's frame times**: `rubylight_protocol::present_clock::PresentClock` schedules each picture at the host's frame time (RTP timestamp) plus a steady delay derived from recent lateness, so the display refreshes at the game's cadence. Presents are immediate with tearing allowed (`DXGI_PRESENT_ALLOW_TEARING` on SDL's flip-model swap chain), and the UI thread sleeps to the scheduled time with a 1 ms timer. The launch asks the host for VRR handling when pacing by host times.
  - VRR without it: each picture is presented as soon as it is decoded.
  - Without VRR: the selected frame pacing mode, V-Sync (the newest picture each refresh) or immediate.
- **VRR display check** (Windows). A VRR stream that presents with tearing in fullscreen runs the beam race's display check (the scan line around each present, `src/stream/race_probe.rs`). A display that keeps its own refresh switches the stream to the pacing it would have without VRR, and the overlay's Pacing line says "V-Sync: this display does not refresh on present". To be classed as fixed-refresh, the stream must run below about 70% of the display's rate, the calibration must show little blanking, and 60 presents must start no scan, or the restart check must fail or starve. A faster stream leaves a VRR display no idle time either, so a fixed-refresh display still tears there. Only presents to a focused fullscreen window count; entering fullscreen, regaining focus, or the stream slowing below 60% of the display's rate after a check at 70% or more measures again. `NIGHTJAR_VRR_CHECK=0` turns the check off. `NIGHTJAR_VRR_SYNC=present1|scan` selects alternative VRR present strategies for measurement (`present1` uses sync interval 1; `scan` waits out a scan in progress); neither became the default ([measurements](https://github.com/RamazanKara/Rubylight/blob/main/rust/PERFORMANCE.md#october-29-nightjar-desktop-on-a-vrr-panel-and-a-fixed-one)). Exclusive fullscreen with VRR requests the display's highest refresh rate at the stream's resolution, not the stream's rate.
- **Audio.** Opus multistream (libopus) into an SDL3 audio stream. The queue is capped at 40 ms plus one packet, dropping late packets rather than letting latency grow; when it runs dry, 10 ms of silence goes ahead of the next packet.

### Statistics

- Clock sync (0x5537) every 250 ms gives the host-to-client clock offset, so the overlay shows the latency from the PC's frame to the display. Host timing (0x5536) gives the host's present-to-send and encode times where the host offers them.
- The overlay's **Host processing** is the video frame header's per-frame time averaged over the frames received in the last two seconds, as Artemis and Moonlight show it. The host's present-to-send is a separate **Host to network** line; the statistics CSV column `host_to_network_ms` (formerly `host_ms`) is that present-to-send time, not host processing.
- When the host announces link notices (0x5538), the client subscribes, and the overlay adds a **Notice** line, "Host lowered the bitrate to X Mbps: network", while the host holds the bitrate down for the network.
- When the host announces pipeline rates (0x553D), the client requests a report every second, and the overlay adds a **Host pipeline** line: the frames per second the host's display produced, captured, encoded, sent and repeated, with "-" for a stage the host cannot count.

### Logging

`src/logpipe.rs` diverts stderr into a pipe, so the stream threads pay no more than for `eprintln!`. A separate thread writes the file with UTC timestamps; the reader buffers up to 4 MB for it, and only a disk that stalls beyond that drops log lines (the log records how many) instead of stalling the stream. `src/logfile.rs` handles rotation (also at 4 MB), stamping and redaction (`nightjar-core`'s `report::redact`). Release builds on Windows have no console; elsewhere the console keeps the raw output.

### Diagnostic environment variables

| Variable | Effect |
| --- | --- |
| `NIGHTJAR_CONFIG_DIR`, `NIGHTJAR_SCREENSHOT_DIR` | Override the settings folder and the screenshot folder. |
| `NIGHTJAR_TIMING_CSV=<file or folder>` | Writes one row per displayed picture in the format of the Android app's `nightjar-latency.csv`: times in nanoseconds on the session clock, with the host's stamps from the extended 0x5536 report and the 0x5537 clock offset in the last nine columns. Rubylight's `rust/release/timing_report.py` joins it with the host's record (`BUTTERPOLLO_TIMING_RECORD`). A writer thread does the file work. |
| `NIGHTJAR_PACKET_TRACE=<prefix>` | Records each video and audio datagram's receive time, the receive thread's wait for it, its length, RTP sequence and timestamp (0 for encrypted video) in memory, and writes `<prefix>-video.csv`, `<prefix>-audio.csv` and `<prefix>-canary.csv` (a 1 ms sleeper's late wake-ups) when the stream ends. It distinguishes a network that delivers packets in bursts from a receive thread that woke late (`nightjar-core`'s `session/trace.rs`). |
| `NIGHTJAR_AUTOSTREAM=address[:port]` | Measurement only: starts one stream without the launcher and ends it after `NIGHTJAR_AUTOSTREAM_SECONDS` (default 60), with `NIGHTJAR_AUTOSTREAM_APP` (default "Desktop") and, for an unpaired PC, `NIGHTJAR_AUTOSTREAM_PIN` (default 1234, entered on the PC). Exits with 0 after a full run and 2 otherwise. |
| `NIGHTJAR_BEAM_RACE`, `NIGHTJAR_EARLY_DECODE` | Turn on [beam racing](../../docs/beam-racing.md) and [early decode](../../docs/early-decode.md) for PyroWave, as the `beam_race` and `early_decode` settings keys do. |
| `NIGHTJAR_D3D11_PASS`, `NIGHTJAR_D3D11_BIND`, `NIGHTJAR_DMABUF`, `NIGHTJAR_VRR_CHECK`, `NIGHTJAR_VRR_SYNC` | A/B switches for the presentation paths described above. |

All are off unless set.

### Source layout

| File | Role |
| --- | --- |
| `src/main.rs` | SDL setup, the main loop, switching between launcher and stream, the overlay |
| `src/cli.rs` | `--host`, `--app` and `--quit-on-exit` for game frontends |
| `src/gui.rs`, `src/a11y.rs` | egui input from SDL events and painting with `SDL_RenderGeometryRaw`; screen reader access |
| `src/theme.rs` | Palette, Figtree type, the brand mark rasterized from its vector path |
| `src/ui.rs` | The launcher: screens, navigation and focus, the header |
| `src/ui/library.rs`, `src/ui/pages.rs`, `src/ui/settings.rs`, `src/ui/dialogs.rs` | PCs, Home, libraries and a game's page; a PC's pages, profiles, shortcuts and controllers; settings; dialogs, pairing and the connecting card |
| `src/ui/widgets.rs`, `src/ui/cards.rs`, `src/ui/files.rs` | Buttons, switches, rows and scroll areas; PC cards and covers; the shared folder browser |
| `src/console.rs` | What Home and the game page show, derived from what the PCs reported (no egui; unit-tested) |
| `src/hosts.rs`, `src/wol.rs` | Host list, mDNS discovery, serverinfo polling, pairing, app lists, cover art, the native library and session; Wake-on-LAN |
| `src/config.rs` | Settings, identity, paired hosts and saved libraries on disk |
| `src/pin_lock.rs` | The PIN lock, with the Android app's modes, grace time, backoff and hash format |
| `src/update_check.rs` | The Windows update check |
| `src/crash_dump.rs` | Windows minidumps for unhandled crashes |
| `src/stream/mod.rs` | The running stream: its state, menu actions, clipboard and the end of the stream |
| `src/stream/render.rs`, `src/stream/notices.rs` | The picture loop (pump, pacing, present, beam race); notices and control events |
| `src/stream/negotiate.rs`, `src/stream/connect.rs` | What a launch asks for (mode, bitrate, codecs, color, HDR); the launch and session start |
| `src/stream/input.rs`, `src/stream/stats.rs` | Keyboard, mouse, touch and controller events; the overlay's statistics |
| `src/stream/pipeline.rs` | The session's decoder, audio renderer and listener |
| `src/stream/present.rs`, `src/stream/direct.rs`, `src/dmabuf.rs` | Video textures and pacing; the Windows draw pass; the Linux DMA-BUF import |
| `src/logfile.rs`, `src/logpipe.rs` | The persistent log file, and the diversion of stderr into it |
| `src/timing_csv.rs` | The client timing CSV |
| `src/gamepad.rs` | Controllers |
| `src/keymap.rs` | SDL scancodes to Windows virtual keys |
| `src/usb.rs`, `src/usb_linux.rs`, `src/usb_winusb.rs` | USB forwarding |
| `src/media.rs`, `src/media_shim.c` | FFmpeg and Opus |

## Credits

Copyright (C) 2026 RamazanKara. GPL-3.0-only.

Nightjar's desktop client ports logic and approaches from:

- [moonlight-qt](https://github.com/moonlight-stream/moonlight-qt) (GPL-3.0, Cameron Gutman
  and contributors): the keyboard table (`keyboard.cpp`), controller mapping
  (`gamepad.cpp`), the special key combos, the FFmpeg decoder setup and the D3D11VA surface
  copy under the device lock (`ffmpeg.cpp`, `d3d11va.cpp`), and the default bitrate curve.
- [Moonlight V+ PC](https://github.com/qiin2333/moonlight-qt) (GPL-3.0, qiin2333 and
  contributors): the Windows D3D11 draw pass and its tone mapping (`d3d11va.cpp`), AI
  enhancement with the GPU vendors' video super resolution and AMF's HQ Scaler
  (`amfsession.cpp`), the Windows touchpad input (`wintouchpad.cpp`), Linux USB forwarding
  through usbip-host, controller glyphs and quit combos, stream menu controls, crash dumps
  (`main.cpp`), the update check (`autoupdatechecker.cpp`), and the VA-API DMA-BUF import as
  Moonlight V+ ships it.
- Nonary's (Chase Payne) moonlight-qt fork (GPL-3.0): the reference for VRR pacing,
  tearing presentation and D3D11VA rendering that this client's pacing and decoder path
  were checked against.
- [moonlight-common-c](https://github.com/moonlight-stream/moonlight-common-c) (GPL-3.0),
  through `nightjar-core`.

[Figtree](https://github.com/erikdkennedy/figtree) is under the SIL Open Font License 1.1
(`assets/Figtree-OFL.txt`).
