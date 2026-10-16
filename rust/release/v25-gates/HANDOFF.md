# Rubylight 2.5 (Task B): where the work stopped, 2026-10-16

Not for merge as is: this folder holds the release-gate harness and notes. Remove it from
the branch before the Phase 5 PR. The full task is in [taskb.md](taskb.md).

## Branch

`claude/rubylight-2.5` is the whole stack, oldest first. The phase branches were pushed as
well and point into it:

| Phase | Branch | State |
| --- | --- | --- |
| Task A, fence ring (CAP_FENCE_RING) | `claude/fence-ring` | Coded. It ships in 2.5, not as 2.4.6 |
| 1, AMD only | `claude/amd-only` | Coded; workaround audit in [phase1-workarounds.md](phase1-workarounds.md) (37 rows, none removed) |
| 2, virtual displays only (includes the floor check) | `claude/virtual-only` | Coded |
| 3, D3D12-only host | `claude/d3d12-host` | Coded; no D3D11 device in host runtime code (scan test plus a debug counter) |
| 4, D3D12 driver (IddCx 1.11) | `claude/d3d12-driver` | Coded, including the keyed-mutex ring's removal |
| 5, README, site, film, docs | `claude/rubylight-2.5` (was `claude/positioning`) | Started: docs requirements, troubleshooting, architecture. README, site and film still to do |

The floor is RDNA 2 with a working AMF HEVC encoder, Windows build 26200, and the D3D12 driver
running (`rust/release/floor.json`). The last release for anything below it is 2.4.5.

## Gates

- **Gate 1 passed on the stack before the last two commits:** fmt, clippy and workspace tests;
  driver fmt and clippy; TestSign and unsigned driver packages; web check, build, unit tests
  (44) and browser tests (50).
  - Re-run it on the tip.
  - Run the web unit tests with `--test-concurrency=1`, because parallel Vite cache renames
    hit a Windows lock.
  - The browser tests need `PLAYWRIGHT_MODULE` set to a playwright 1.56.1 install with its
    headless shell.
- **Gate 4:**
  - **Floor refusals passed.** The three forced verdicts (windows, gpu, driver) each exited 1,
    named 2.4.5 and changed nothing.
  - **Upgrade run 1:**
    - The upgrade exited 0, and pairings (12) and the app library (4) were kept.
    - It showed that `virtual_display_driver` and `dd_use_sunshine_virtual_display_driver`
      were never retired.
    - The fix is commit `da98f361` (Retire the virtual display driver choice…).
  - **Gate 4 needs a re-run with a package built from the tip:** the upgrade check and the
    isolated `encoder = nvenc` check.
  - `gate4.ps1` now reads the exit code and message from setup's own log in `%TEMP%`. Setup
    is a GUI-subsystem program.
  - The PC was restored to 2.4.5 with the original profile and the 2.4.5 driver.
- **Not run yet:** gates 2, 3 and 5–8, the old-host WGC check, and the debug-build D3D11
  counter during a stream.
- **Known failure that predates this work:** `pyrowave_decoded_end_to_end` fails at 1080p
  SDR8 4:4:4 with record framing at 1000 Mbps: 42.8/37.3/36.6 dB against a 40 dB bar.
  - Plain framing matches the October 7 table exactly.
  - Record framing's budget has been about 15–20% smaller since 2.4.5.
  - Main fails it too.
  - Decide before the release; the threshold was not loosened.
- **Native suite:** `native_virtual_display_recovery…` failed with "cannot read display mode"
  on this stack and on main. Triage it in gate 2.

## Harness (copied from C:\src\butterpollo-release, paths inside still point there)

| File | What it does |
| --- | --- |
| `run.ps1` | One vd_e2e case. Its label is `<codec>[-hdr]-<fps>-<base\|fence\|v25>-r<N>[-s<secs>]`. v25 reads `v25\pkg` and `v25\tools.txt` |
| `cases.ps1` | The 12-case matrix, run as SYSTEM, waiting for no build and CPU below 20%: `-Runs <dir> -Build <b> -Round <n> [-Reverse]` |
| `compare_v25.py` | Gate 3: runs-v25-1/2 against runs-base1/2 (picture age and present to sent, mean and p95, per-rate noise) |
| `baseline-2.4.5.zip` | The 2.4.5 baseline passes (`runs-base1/2`) that `compare_v25.py` reads: result.json, frames.csv, and the needed log lines. Unzip it into the fence-ring folder |
| `two_clients.py` | Gate 5: SDR HEVC plus HDR AV1 on one shared virtual display for 5 minutes |
| `gate4.ps1` | Gate 4: forced floor refusals, then the upgrade, then the migration of an isolated profile |
| `native.ps1` | Gate 2: the ignored native tests of one tree |
| `sample.ps1` | Gate 7: samples WUDFHost handles and GPU dedicated memory every 10 s |
| `vd_e2e.py`, `stages.py` | The isolated virtual-display e2e stream and the stage parser |
| `d3d11-audit.md` | Phase 5 evidence. All six audited hosts use D3D11. The dimizago Vibepollo fork is unverified, so write "among the audited hosts" or re-check before writing "only" |

## Build notes

- Shells started from the Claude desktop app see a virtualized `%LOCALAPPDATA%`. Pass
  `rust/build.ps1 -FetchDependencies -Dependencies <real dir> -TargetDirectory <real dir>
  -PyrowaveRoot <real dir> -SkipTests -Package` real paths. With the defaults, msiexec fails
  with 1619 and the Vite build rejects the path.
- For the test package, run `python rust/release/bump.py 2.5.0` in a scratch worktree, without
  committing.
- For the floor-test setup:
  1. `cargo +1.98.1-x86_64-pc-windows-gnu build -p butterpollo-setup --release --features floor-test`
     into its own target directory.
  2. `rust/release/package.ps1` with that stub and a copy of the distribution.
- `RUBYLIGHT_SETUP_FLOOR_TEST=windows|gpu|driver` forces a verdict.
- Setup refuses downgrades. To repeat the upgrade gate:
  1. Uninstall with `uninstall.exe --uninstall --quiet` (this keeps the profile).
  2. Restore the profile backup.
  3. Install `rubylight-setup-2.4.5.exe --quiet`.

## Kept from the local-only branches (reviewed and deleted 2026-10-16)

`claude/pyrowave-plane-views` (`ff246352`) is the only local work that was neither in main
nor measured and dropped.
- **What it does:** a PyroWave SDK patch that keeps the encoder's three plane image views
  between frames.
- **Gain:** 0.15–0.25 ms of CPU per frame, as Punktfunk measured on an RTX 5070 Ti; not yet
  measured here.
- **Before taking it:**
  - Port it onto Phase 3's D3D12-imported planes. Planes are keyed by image id, so a rebuilt
    plane must still drop its view.
  - Update the build-info patch list and the CI SDK cache key.
  - Measure it on the RX 7900 XT.

Everything else was already in main under other hashes, or was measured and dropped (see
PERFORMANCE.md and PERFORMANCE_WORK.md):
- the driver-side NV12 conversion;
- the half-period pacer;
- batched display apply;
- the link-speed upkeep (never shown to help).

## Next steps

1. Re-run gate 1 on the tip.
2. Build the package from the tip and re-run gate 4.
3. Run gates 2, 3 (`cases.ps1`, forward and `-Reverse`, then `compare_v25.py`), 5, 6, 7 and 8.
4. Write Phase 5 with the measured numbers. Measure Rubylight only, and cite the documented
   Vibepollo 2.0 comparison in rust/PERFORMANCE.md. Keep the credits and fork history exactly.
5. Open one PR per phase, stacked.
6. Merge Phase 5 with `bump.py 2.5.0 --notes …`, tag v2.5.0, then run `check.ps1 v2.5.0`.
