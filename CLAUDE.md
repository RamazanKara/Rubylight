# Rubylight

Windows game-streaming host for Moonlight clients, in Rust (formerly Butterpollo). The Android client, Nightjar, is a separate repo with its own CLAUDE.md (`C:\src\rubylight-android`).

The docs are thorough, so read the relevant section instead of exploring: [docs/building.md](docs/building.md) (build, SDK env, options), [rust/README.md](rust/README.md) (workspace, running, entry points), [docs/architecture.md](docs/architecture.md), [rust/release/README.md](rust/release/README.md) (release and hardware harness), [rust/PERFORMANCE.md](rust/PERFORMANCE.md) (measurement history).

3.0 work follows [docs/rubylight-3.0-plan.md](docs/rubylight-3.0-plan.md). Keep its status table current.

## Layout
- `rust/core` (`-p butterpollo-core`): protocol, crypto, FEC, input, state. OS-independent.
- `rust/windows` (`-p butterpollo-windows`): capture, GPU conversion, AMF/PyroWave codecs, audio, input, displays. `rust/host` (`-p butterpollo`, not `butterpollo-host`): HTTP/TLS, RTSP, ENet, UDP, stream lifecycle. `rust/setup`: installer. `rust/vulkan-layer`: HDR layer.
- `rust/drivers/rubylight-vdisplay`: IddCx virtual display driver, outside the Cargo workspace.
- `rust/web`: Svelte console. `site/`: Pages site. `third-party/`: submodules.
- `rust/release`: release tooling and hardware harness. `rust/tests`: interop and hardware fixtures.
- Internal names stay butterpollo (crates, `butterpollo.exe`, `BUTTERPOLLO_*`, `ProgramData\Butterpollo`, service `ApolloService`). Only the install dir and branding say Rubylight.

## Build and check
- Work in a Windows checkout under `C:\src`, never `\\wsl.localhost` (rustfmt and Edit fail there). This overrides the global WSL rule.
- Fresh clone: `git submodule update --init --recursive`.
- `%LOCALAPPDATA%` is virtualized inside the Claude desktop app (MSIX); msiexec and Vite break on it. Always pass real paths. Full gate (fmt, tests, clippy, release build), about 19 min cold, run in the background:
  `.\rust\build.ps1 -FetchDependencies -Dependencies C:\src\butterpollo-release\<ver>\sdk -TargetDirectory C:\src\butterpollo-release\<ver>\target *> <log>`, then grep the log for `^test result|FAILED|^error`.
  Package (about 3 min on a warm target): add `-SkipTests -Package`. Keep `-FetchDependencies` even with `-Dependencies` set. Use release builds for any measurement.
- Bare `cargo` fails ("linker `gcc` not found"); `build.ps1` sets the GNU toolchain (`+1.98.1-x86_64-pc-windows-gnu`, `C:\msys64\ucrt64`) and SDK env for its own shell only.
- Fast core loop, about 10 s warm, no dev shell needed: `cargo +1.98.1-x86_64-pc-windows-msvc test -p butterpollo-core --locked --target-dir C:\src\rl-target`.
- VS dev shell, needed by `build.ps1` and the driver (only Build Tools are installed, hence `-products '*'`): `$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products '*' -property installationPath; Import-Module "$vs\Common7\Tools\Microsoft.VisualStudio.DevShell.dll"; Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64'`
- Other CI checks: `cd rust\web; npm ci --no-audit --no-fund; npm run check; node --test "*.test.mjs"; npm run build`, and `python -m unittest discover -s rust/release -p test_release.py` (and `test_gpu_hang.py`). Kill stray node/vite first ("Port 24678 is already in use").
- Driver: `cd rust\drivers\rubylight-vdisplay; . .\driver-env.ps1; .\build-package.ps1`. Install with `install-local.ps1` from an elevated shell, host idle, using the unsigned package (the script signs it; a `-TestSign` package is rejected). May need a restart.
- Disk: C: has filled up before. Share one target dir (`C:\src\rl-target`) across worktrees and delete stale worktree `target\` folders.
- "dubious ownership" on dirs an elevated shell created: `git config --global --add safe.directory <path>`.
- `core.autocrlf=input`: scripted edits must keep LF (`open(..., newline='')`). Edit files with the Edit tool, not patch scripts.
- Write multi-line scripts and commit messages to a file and run them (`git commit -F <file>`). Heredocs and PowerShell here-strings break here.
- Parallel agents: each gets its own worktree. Never `git stash` (shared across worktrees); move changes with `git diff --binary` and `git apply -3`.

## CI and release
- CI: `.github/workflows/rust-windows.yml` (windows-2022, 2× minutes) on pushes to main touching `rust/**`, Cargo or toolchain files. Docs-only commits skip it.
- Always pass `-R RamazanKara/Rubylight` to `gh`.- Versions: patch bump for fixes, minor for features. Fewer, well-tested releases.
- Release: changes go under `## Unreleased` in `rust/RELEASE_NOTES.md`. Run `python rust/release/bump.py X.Y.Z`, fill in `## New in X.Y.Z` including a "Tested." paragraph with our own measurements, and check it with `python rust/release/notes.py vX.Y.Z --out body.md`. Commit, push main, then `git tag -a vX.Y.Z -m "Rubylight X.Y.Z" && git push origin vX.Y.Z`.
  - Tag only after main's run for that commit is green: the tag run then reuses its build and finishes in under a minute. Tagging while main still runs builds everything twice.
  - Expect 3 assets: setup exe, zip and SHA256SUMS. Then run `rust/release/check.ps1 vX.Y.Z` on this PC.
- Never delete a published release or move a tag unless the user says so.
- Commit style: `fix(stream): ...`, no model names.

## Product rules
- Every fix gets a regression test. Failures are loud: no silent fallback such as AMF to software.
- Don't change `PROTOCOL_VERSION`; stay compatible with Nonary's Moonlight.
- No added latency. Prefer client-reported hints (`pyrowaveLinkMbps`, `pyrowavePaceMbps`) to host probes. Revert changes that measure worse.
- `main` is the 3.0 line and AMD-only: NVENC, QSV and software encoders are gone. NVIDIA and Intel users stay on 2.6.x, with no backports unless the user asks. Nothing else is dropped: every AMD GPU with AMF, Windows 10, physical displays, WGC and Desktop Duplication, the D3D11 path and the keyed-mutex ring stay supported. The 2.5 floor, virtual-only and D3D12-only work was reverted on 2026-10-22 and is parked on `claude/d3d12-virtual-only`. Bring any of it back only as an addition beside the existing paths (plan decision 1a). Never write "RX 7000 or newer", and never drop hardware support silently.
- Claims: only our own measurements; no first/only/fastest without evidence. Never install or measure competing hosts (Vibepollo, Apollo, Sunshine, punktfunk); cite "Against Vibepollo 2.0" in `rust/PERFORMANCE.md`. Don't mention punktfunk.
- Before calling something unmeasured or reviving an old branch, check `rust/PERFORMANCE.md` and the release notes.
- Keep the credits and fork history in `AGENTS.md`, `NOTICE` and the README intact.
- GitHub issues: reply yourself with `gh issue comment`, short and human. Never post on Reddit.

## Hardware testing
This PC is the dev box, the release test host and the user's gaming PC (RX 7900 XT). Host log: `C:\ProgramData\Butterpollo\config\logs\butterpollo.log`.
- Idle check before any install, restart or stream: `(Invoke-WebRequest http://127.0.0.1:47989/serverinfo).Content` must show RustHostSessionCount and RustHostPendingSessionCount at 0. Never interrupt a stream.
- When the user says he's gaming: no GPU benchmarks, streams, virtual displays, layout changes, virtual controllers or `check.ps1`, and build with `-j 4`, unless he says testing is fine.
- Test hosts use an isolated profile and ports (`butterpollo.exe --config-dir C:\tests\<p> --port 48123 --bind 127.0.0.1`; the harness uses 48518-48544). Always set `virtual_display_layout = extended`: the default `exclusive` turns the physical monitor off.
- Driver and virtual-display tests need SYSTEM, not just admin. Use the `schtasks /RU SYSTEM` pattern in `rust/release/elevated.ps1`. Run `rust/release/elevation.ps1` once elevated to register `ButterpolloReleaseElevated`; until then expect UAC prompts.
- `check.ps1` dot-sources `C:\src\butterpollo-release\settings.ps1` (MSYS2 UCRT64 on PATH, `BUTTERPOLLO_PYROWAVE_ROOT`, ...). It was deleted in a cleanup, so recreate it first. If check.ps1 stops at "test receivers and probes", delete the stale `fixtures\moonlight-client-build`.
- Exit code 3 from `e2e.py` or `check.ps1` means an AMD encoder hang. Stop and investigate, never retry. A reboot needs the user's OK. Never upgrade Windows.
- Run streams and soaks in the background; read `result.json` and the 5 s `stream timings` lines in the host log, not full logs.
- Real-client tests on phones are done by the user; read the host log after. The laptop "lap" runs `smoke.ps1`; the NUC test client is `ssh nuc`.
