# Release tooling

Keep pending changes under `## Unreleased` in `rust/RELEASE_NOTES.md`. The version-bump tool moves that section into the new release automatically, also when `--notes` supplies additional release text.

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
| Publish `Rubylight <version>` (a prerelease for `-rc.N`) with the installer, the portable ZIP and `SHA256SUMS`. The text is `body.md` with the `## New in rc.N` section; links relative to `rust/` point at the tagged source. A release that already exists is left alone | `publish` job, `notes.py` |

Tag a commit whose main run is green and the release is out in about a minute. A failed run can be re-run from the Actions page. To release again after a fix, bump to the next version.

## Host check

The only part GitHub's runners cannot do needs the Radeon release workstation, with the installed host idle:

```powershell
pwsh rust/release/check.ps1                # the newest release
pwsh rust/release/check.ps1 v2.2.2 -NoInstall -NoUpload
```

It downloads the release and checks it against `SHA256SUMS`, streams H.264, HEVC, HEVC VRR, AV1, HEVC with keyframe requests (16, one every 0.5 s, while the picture moves at the stream rate; the host's per-claim trace must show no unchanged picture encoded again), HEVC at 2560x1440 and 120 FPS with 80 Mbps, decoded with D3D11VA, plain and launched at 60 FPS as Artemis's Warp frame pacing does (both recorded as skipped when the display refreshes below 120 Hz) and PyroWave (SDR and HDR 4:4:4 at 1080p60) for 12 seconds each through the packaged host on an isolated profile with the independent moonlight-common-c client, runs the pairing and launch protocol checks (`e2e.py`, `protocol.py`), runs the display self-test as SYSTEM and installs over the running host (`elevated.ps1`), then records the results with `validation.py`, uploads them as `VALIDATION.json` and links them from the release text. The stream checks start a second host on ports 48518-48544 and do not touch displays, HDR or audio. The receivers and probes are kept in `C:\src\butterpollo-release\fixtures` and only rebuild what changed.

A stream that shows an AMD video engine hang (`gpu_hang.py`: an encoder call of 2 s or more, the stall watch's encoding stall unresolved after 5 s, or a new display driver watchdog report in `C:\Windows\LiveKernelReports`) stops at once, records `gpu_hang` in its `result.json` and exits with code 3; `check.ps1` then stops without the usual retry, and `soak.py` skips its remaining cases. Stopping does not prevent the GPU reset Windows has already begun; it keeps a retry or the next case off a GPU that is resetting. Reboot if Device Manager shows the Radeon with Code 31.

Machine settings live outside the repository in `C:\src\butterpollo-release\settings.ps1`, which is dot-sourced: the build environment for the test receivers and probes (MSYS2 UCRT64 on `PATH`, `BUTTERPOLLO_PYROWAVE_ROOT`, ...) and `BUTTERPOLLO_TEST_PYTHON` (a Python with `requests` and `cryptography`). Work files go to `C:\src\butterpollo-release\<version>`. Not `%LOCALAPPDATA%`: a packaged (MSIX) app such as the Claude desktop app sees a private copy of that folder, and the SYSTEM self-test could not find a package unpacked there.

If `check.ps1` stops while building the test receivers and probes, delete the stale `fixtures\moonlight-client-build` folder (the CMake build dir from `rust/tests/build-moonlight-client.ps1`) and run it again.

### No prompts

- **UAC.** Run `rust/release/elevation.ps1` once from an elevated PowerShell. It copies `elevated.ps1` and `elevated-task.ps1` to `C:\ProgramData\ButterpolloRelease` (writable only by administrators and SYSTEM) and registers the on-demand task `ButterpolloReleaseElevated`, which runs them with highest privileges for you. `check.ps1` starts that task instead of asking, as long as the installed scripts match the checkout; after they change it asks once more and says to run `elevation.ps1` again. The trade-off: any program running as you can start the task, which installs the setup and runs the self-test from `C:\src\butterpollo-release\<version>` with administrator rights. `elevation.ps1 -Remove` uninstalls it. The WebRTC end-to-end harness (`rust/tests/webrtc_e2e.cjs`) uses the same task: it also installs `rust/tests/webrtc_client/elevated_tasks.ps1` as `webrtc-e2e-elevated.ps1` and the built `system_launch` example as `system_launch.exe` (`-SystemLaunch <path>`; the harness's default build is taken when it is there). The script creates and runs the one-shot SYSTEM task in which that installed launcher starts only `C:\tests\webrtc-host\butterpollo.exe`, stops what that host leaves, and opens the firewall to it from the local subnet while a run lasts. The staged host and the DLLs beside it in `C:\tests\webrtc-host` run as SYSTEM and stay writable by you, so anything running as you can replace them before a run: that is the trade-off of installing it. The harness refuses to start, and says to run `elevation.ps1` again, while the installed scripts differ from the checkout.
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

`--rate-kbps 0` relays without shaping, the control. `udp_shaper --selftest` checks the shaper on loopback first: what it forwards must stay within 2% of the queue model fed with the sender's own send times, and back-to-back departures within 2% of the link's spacing. Its link thread runs at game priority, so other load on the machine does not slow the link.

## Timing record

`timing_report.py` joins a host timing record (`BUTTERPOLLO_TIMING_RECORD`) with a receiver's `frames.csv` or a Nightjar latency CSV and prints per-stage median, p95 and p99 with the join mode (`clock_sync`, `shared_clock` or `min_owd`) and its error bound, `--json PATH` for a file and `--check-bound` to compare a nightjar-cli clock sync with the shared QPC. `e2e.py --record` writes the record and `frames.csv` into the case and the stage table into `result.json` as `timing`. The formats are in [docs/timing-record.md](../../docs/timing-record.md); the tests are `test_timing_report.py` with fixtures in `fixtures/timing/`.

## Encoder focus A/B

`focus_ab.py` measures encoder focus (`encoder_focus`, `encoder_focus_levels`, `encoder_focus_verify`) against off, per section 9 of [docs/encoder-focus-design.md](../../docs/encoder-focus-design.md). `quality` (M2, or `--plan m4` on the laptop) encodes 180-frame fixtures from `chroma_fixtures.py` (the text scenes and `mixed`, a still HUD and editor over moving pictures, each with a `regions.json`) and the two game clips through `amf_quality.exe` in ABBA quartets with bitrate matching, and scores VMAF, PSNR-Y and SSIM on the full frame, the still, moving and HUD regions and the settled frames; `--centre` adds the centre arms at HEVC 10 Mbps. `levels` (M1) ranks the tuning candidates. `timing` (M3, or `--plan m4`) runs `e2e.py --record` ABBA streams on an isolated host, plus slice delivery and intra refresh pairs. Every arm first checks that the installed host is idle; exit code 3 is an encoder hang: stop, never retry. `estimate --plan m2` prints the arms and minutes. `focus_ab_result.py WORK [WORK ...]` (or `focus_ab.py summarize`) combines GPUs and prints the verdict per codec: `default_on`, `opt_in`, `drop`, `undecided` or `incomplete`. The tests are `test_focus_ab.py`.
