# Release tooling

Pending changes go in fragments, one file per topic: `rust/release/notes.d/<topic>.md` (see [its README](notes.d/README.md)), so parallel branches never edit `rust/RELEASE_NOTES.md` at once. The version-bump tool moves `## Unreleased` and then the fragments, in file name order, into the new release and deletes the fragments, also when `--notes` supplies additional release text. `python rust/release/fragments.py` collects the fragments into `## Unreleased` between releases. Nightjar's fragments in `client/notes.d` become its "What's new" draft and store changelogs with `tools/nightjar_notes.py`.

A release is a pushed tag:

```sh
python rust/release/bump.py 2.2.2   # versions, download links, a "## New in 2.2.2" section
# fill in "## New in 2.2.2" in rust/RELEASE_NOTES.md, commit and push to main
git tag v2.2.2 && git push origin v2.2.2
```

Without git, the same release is one click: **Actions → Rubylight Windows → Run workflow** on `main` with **Publish** ticked. It releases the version in `Cargo.toml` at main's head and creates its `v<version>` tag. An existing tag with or without `v` prevents publishing that version again. Package filenames keep the bare version.

The tag's run (or the published run) of [`rust-windows.yml`](../../.github/workflows/rust-windows.yml) does the rest:

| Step | Where |
| --- | --- |
| Check that the tag matches `Cargo.toml` and that its release notes section is filled in | `tag` job, `notes.py` |
| Reuse the installer main's run already built and tested for the tagged commit. Only when there is none (the run was cancelled by a later push, or still running): formatting, tests, clippy, the release build, the web console and the package with its installer, from scratch | `tag` job, `windows` job (`rust/build.ps1 -Package`) |
| Publish `Rubylight <version>` (a prerelease for `-rc.N`) with three assets: the installer, the portable ZIP and `SHA256SUMS`. The host check adds `VALIDATION.json` and the laptop's smoke test `REAL-CLIENT.json`, five in all. The text is `body.md` with the `## New in rc.N` section; links relative to `rust/` point at the tagged source. A release that already exists is left alone | `publish` job, `notes.py` |

Tag a commit whose main run is green and the release is out in about a minute. A failed run can be re-run from the Actions page. To release again after a fix, bump to the next version.

## Host check

The only part GitHub's runners cannot do needs the Radeon release workstation, with the installed host idle:

```powershell
pwsh rust/release/check.ps1                # the newest release
pwsh rust/release/check.ps1 v2.2.2 -NoInstall -NoUpload
```

It downloads the release and checks it against `SHA256SUMS`, streams H.264, HEVC, HEVC VRR, AV1, HEVC with keyframe requests (16, one every 0.5 s, while the picture moves at the stream rate; the host's per-claim trace must show no unchanged picture encoded again for a request; a static repeat, the host keeping a still source alive, is counted, not judged), HEVC at 2560x1440 and 120 FPS with 80 Mbps, decoded with D3D11VA, plain and launched at 60 FPS as Artemis's Warp frame pacing does (both recorded as skipped when the display refreshes below 120 Hz) and PyroWave (SDR and HDR 4:4:4 at 1080p60) for 12 seconds each through the packaged host on an isolated profile with the independent moonlight-common-c client, runs the pairing and launch protocol checks (`e2e.py`, `protocol.py`), runs the display self-test as SYSTEM and installs over the running host (`elevated.ps1`), then records the results with `validation.py`, uploads them as `VALIDATION.json` and links them from the release text. The stream checks start a second host on ports 48518-48544 and do not touch displays, HDR or audio. The receivers and probes are kept in `C:\src\butterpollo-release\fixtures` and only rebuild what changed.

Other work on the machine starves the fixtures: rc.4's check failed twice at 25-100 % CPU from other sessions' builds and passed on a quiet CPU. So `check.ps1` takes the shared locks in `C:\src\butterpollo-release` itself (`check_streams.ps1`): `BUILD-LOCK` for the fixture builds, and for each stream `GPU-LOCK` and then `BUILD-LOCK` without waiting (busy: the GPU lock goes back for 20 s and both are tried again). Each lock is released after its step, with a 16 s pause so waiting sessions get their turn, and refreshed during a stream and between the fixture builds so it never looks stale. Only a lock this run wrote is removed; one another session removed or took over is printed as `LOCK LOST` and listed in the case's `result.json` as `lock_warnings`. `GPU-LOCK` is never broken: the check warns every minute while it waits and fails after 30 minutes. Run it without a lock around it: a lock held by one of its parent processes is reused, `-GpuLockHeld` says the caller holds `GPU-LOCK` under another name, and `tools/hw/run.py` gives its batch's `GPU-LOCK` back for the `release-check` case (`manages_locks: true`) and waits to take it again after. Without `build-lock.ps1` there (another machine) it takes no locks. Each stream and its retry start only on a quiet CPU (`machine_load.py`: under 30 % over 5 s and no `cargo`, `rustc`, `java`, `gradle` or `ninja` using CPU; an idle Gradle daemon does not count), with 15 minutes of waiting for the whole check; after that it warns and samples the load once per stream. Every `result.json` records the wait as `quiet_wait` and the load beside the stream as `machine_load`, and a failed stream's warning names it, so a failure under load can be told apart. The sink's volume and mute are read (never set) into `audio_sink`; a muted sink or one below 10 % adds a loud line to `warnings`, as rc.4's Steam Streaming Speakers sat at 2 %. An encoder hang ends the check with exit code 3, like `e2e.py`. `soak.ps1` waits the same way before it starts and stops if the wait fails.

A stream that shows an AMD video engine hang (`gpu_hang.py`: an encoder call of 2 s or more, the stall watch's encoding stall unresolved after 5 s, or a new display driver watchdog report in `C:\Windows\LiveKernelReports`) stops at once, records `gpu_hang` in its `result.json` and exits with code 3; `check.ps1` then stops without the usual retry, and `soak.py` skips its remaining cases. Stopping does not prevent the GPU reset Windows has already begun; it keeps a retry or the next case off a GPU that is resetting. Reboot if Device Manager shows the Radeon with Code 31.

Demo stream, for hardware A/Bs on real game content without launching a game: `e2e.py ... --mode 1920x1080x60 --content D:\testmedia\xiph-twitch\GTAV.y4m` (a raw Y4M clip, such as Xiph's Twitch set) plays the clip in a borderless ffplay window behind the user's windows, and the test host captures only that window (`output_name = window:<hwnd>/<pid>@<display>`, WGC; hosts after 3.0.0-rc.5). The window never takes focus, has no taskbar button and lets clicks through, but it shows where no window covers its corner of the monitor (0,0, `--content-at`). It waits paused at the clip's start and plays once the host's capture opens, so runs stream the clip from about the same point (a few pictures apart). Raw Y4M needs no decoder, so the player stays off the video engine; it cost 22-41 % of one core (SDL's Direct3D 11 renderer on the vblank) and about 1.3 % of the 3D engine on the RX 7900 XT host (no video engine time), and `result.json`'s `content` records its CPU and the share of real time it kept (under 99 % fails the case). Motion coverage, unique FPS and picture age need the motion strip and are listed in `not_applicable`; the tone check stays. The player's presents add delivery jitter of their own (30 s HEVC arms alternating with the strip, 50-96 % CPU from other lanes: 18-22 long intervals of 1615 and p99 25.0-25.4 ms against the strip's 0 and 18.1-20.9 ms), so content runs are for A/B comparisons: the long-interval and p99 gap limits are `not_applicable` too and the gap numbers stay in the result, while a gap over 200 ms still fails as a freeze. For a device arm (Retroid, TV, laptop) `tools\hw\demo-stream.ps1 -Clip <y4m> -Seconds N -Package <dir>` starts the player and an isolated host on a paired test profile and plays the clip when the device's stream starts.

Machine settings live outside the repository in `C:\src\butterpollo-release\settings.ps1`, which is dot-sourced: the build environment for the test receivers and probes (MSYS2 UCRT64 on `PATH`, `BUTTERPOLLO_PYROWAVE_ROOT`, ...) and `BUTTERPOLLO_TEST_PYTHON` (a Python with `requests` and `cryptography`). Work files go to `C:\src\butterpollo-release\<version>`. Not `%LOCALAPPDATA%`: a packaged (MSIX) app such as the Claude desktop app sees a private copy of that folder, and the SYSTEM self-test could not find a package unpacked there.

If `check.ps1` stops while building the test receivers and probes, delete the stale `fixtures\moonlight-client-build` folder (the CMake build dir from `rust/tests/build-moonlight-client.ps1`) and run it again.

### No prompts

- **UAC.** Run `rust/release/elevation.ps1` once from an elevated PowerShell. It copies `elevated.ps1` and `elevated-task.ps1` to `C:\ProgramData\ButterpolloRelease` (writable only by administrators and SYSTEM) and registers the on-demand task `ButterpolloReleaseElevated`, which runs them with highest privileges for you. `check.ps1` starts that task instead of asking, as long as the installed scripts match the checkout; after they change it asks once more and says to run `elevation.ps1` again. The trade-off: any program running as you can start the task, which installs the setup and runs the self-test from `C:\src\butterpollo-release\<version>` with administrator rights. `elevation.ps1 -Remove` uninstalls it. The WebRTC end-to-end harness (`rust/tests/webrtc_e2e.cjs`) uses the same task: it also installs `rust/tests/webrtc_client/elevated_tasks.ps1` as `webrtc-e2e-elevated.ps1` and the built `system_launch` example as `system_launch.exe` (`-SystemLaunch <path>`; the harness's default build is taken when it is there). The script creates and runs the one-shot SYSTEM task in which that installed launcher starts only `C:\tests\webrtc-host\butterpollo.exe`, and stops what that host leaves. The harness adds and changes no firewall rule: its host binds 127.0.0.1, and a client on another device needs the user's own rule for `C:\tests\webrtc-host`. The staged host and the DLLs beside it in `C:\tests\webrtc-host` run as SYSTEM and stay writable by you, so anything running as you can replace them before a run: that is the trade-off of installing it. The harness refuses to start, and says to run `elevation.ps1` again, while the installed scripts differ from the checkout.
- **Firewall.** The test hosts listen on 127.0.0.1 only, so Windows Firewall has nothing to ask about.

## Real-client smoke test

After the install, from the measurement laptop (Moonlight-qt paired as client "lap", whose Extended layout streams a virtual display, with `motion_probe` drawing on it from the host):

```powershell
pwsh rust/release/smoke.ps1 -Version v2.2.2
```

It streams the Desktop app for 30 seconds per codec at 1968x2184, HDR, 120 fps, 80 Mbps with AV1 and HEVC, closes Moonlight and reads its "Global video stats". A run passes with at least 115 fps received and a host processing average under 5 ms. The result goes to the release as `REAL-CLIENT.json` when the GitHub CLI is signed in on the laptop. If the laptop is offline the release has no `REAL-CLIENT.json`: the smoke test was skipped.

## Slow-link check

`bottleneck.py` streams a packaged host like `e2e.py` (isolated profile and ports, the independent receiver, a moving strip at twice the stream rate), but the receiver reaches the host through `udp_shaper` on `127.0.0.2`. The shaper relays RTSP, control and audio unchanged and drains the video port toward the client at a fixed rate through a drop-tail queue, as a 100 Mbps switch port in front of a TV box does. It reports the frames the host sent, the frames the receiver got whole, keyframe requests and the shaper's drops; it does not judge.

```powershell
cargo build --release -p butterpollo-windows --example udp_shaper   # copy udp_shaper.exe beside the receiver
& $python rust\release\bottleneck.py --package <pkg> --work <dir> --client <fixtures>\moonlight-client.exe `
    --codec av1 --mode 2560x1440x60 --bitrate 40000 --rate-kbps 100000 --queue-kib 64 --rfi
```

`--rate-kbps 0` relays without shaping, the control. `--loss-percent P` drops that share of video datagrams at random, and `--loss-seconds S` stops the loss S seconds after the first video datagram, a path that heals mid-stream. `udp_shaper --selftest` checks the shaper on loopback first: what it forwards must stay within 2% of the queue model fed with the sender's own send times, and back-to-back departures within 2% of the link's spacing. Its link thread runs at game priority, so other load on the machine does not slow the link.

## Timing record

`timing_report.py` joins a host timing record (`BUTTERPOLLO_TIMING_RECORD`) with a receiver's `frames.csv` or a Nightjar latency CSV and prints per-stage median, p95 and p99 with the join mode (`clock_sync`, `shared_clock` or `min_owd`) and its error bound, `--json PATH` for a file and `--check-bound` to compare a nightjar-cli clock sync with the shared QPC. `e2e.py --record` writes the record and `frames.csv` into the case and the stage table into `result.json` as `timing`. The formats are in [docs/timing-record.md](../../docs/timing-record.md); the tests are `test_timing_report.py` with fixtures in `fixtures/timing/`.

## Encoder focus A/B

`focus_ab.py` measures encoder focus (`encoder_focus`, `encoder_focus_levels`, `encoder_focus_verify`) against off, per section 9 of [docs/encoder-focus-design.md](../../docs/encoder-focus-design.md). `quality` (M2, or `--plan m4` on the laptop) encodes 180-frame fixtures from `chroma_fixtures.py` (the text scenes and `mixed`, a still HUD and editor over moving pictures, each with a `regions.json`) and the two game clips through `amf_quality.exe` in ABBA quartets with bitrate matching, and scores VMAF, PSNR-Y and SSIM on the full frame, the still, moving and HUD regions and the settled frames; `--centre` adds the centre arms at HEVC 10 Mbps. `levels` (M1) ranks the tuning candidates. `timing` (M3, or `--plan m4`) runs `e2e.py --record` ABBA streams on an isolated host, plus slice delivery and intra refresh pairs. Every arm first checks that the installed host is idle; exit code 3 is an encoder hang: stop, never retry. `quick` is the whole check in under ten minutes (refused when `estimate --plan quick` says more): quartets at HEVC 10 Mbps on editor, `mixed` and one game clip, AV1 10 Mbps and H.264 20 Mbps on `mixed`, and one 45 s off/on stream pair. Its verdict says it is a quick pass, and a result that only reaches opt-in ships as Experimental. `estimate --plan m2` prints the arms and minutes. `focus_ab_result.py WORK [WORK ...]` (or `focus_ab.py summarize`) combines GPUs and prints the verdict per codec: `default_on`, `opt_in`, `drop`, `undecided` or `incomplete`. The tests are `test_focus_ab.py`.
