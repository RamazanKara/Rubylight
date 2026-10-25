# Rubylight

Windows Moonlight game-streaming host in Rust (formerly Butterpollo). Nightjar, the Android client, lives in `C:\src\rubylight-android`.

Read the relevant doc instead of exploring: [docs/building.md](docs/building.md) (build, SDK, options), [rust/README.md](rust/README.md) (workspace, running, entry points), [docs/architecture.md](docs/architecture.md), [rust/release/README.md](rust/release/README.md) (release, hardware harness), [rust/PERFORMANCE.md](rust/PERFORMANCE.md) (measurements). 3.0 work follows [docs/rubylight-3.0-plan.md](docs/rubylight-3.0-plan.md); keep its status table current.

## Layout
- Crates: `butterpollo-core` (`rust/core`, OS-independent protocol, crypto, FEC, input), `butterpollo-windows` (`rust/windows`: capture, GPU, AMF/PyroWave, audio, displays), `butterpollo` (`rust/host`, not `butterpollo-host`: HTTP, RTSP, ENet, stream lifecycle). The driver `rust/drivers/rubylight-vdisplay` is outside the workspace.
- Internal names stay butterpollo (crates, `butterpollo.exe`, `BUTTERPOLLO_*`, `ProgramData\Butterpollo`, service `ApolloService`); only the install dir and branding say Rubylight.

## Build and check
- Work in a Windows checkout under `C:\src`, never `\\wsl.localhost`; this overrides the global WSL rule. Fresh clone: `git submodule update --init --recursive`.
- `%LOCALAPPDATA%` is virtualized here (MSIX) and breaks msiexec and Vite: pass real paths.
- Core loop, no dev shell: `cargo +1.98.1-x86_64-pc-windows-msvc test -p butterpollo-core --locked --target-dir C:\src\rl-target`. Otherwise bare `cargo` fails ("linker `gcc` not found"): `build.ps1` sets the GNU toolchain and SDK env for its own shell.
- `build.ps1` and the driver need the VS dev shell: `$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products '*' -property installationPath; Import-Module "$vs\Common7\Tools\Microsoft.VisualStudio.DevShell.dll"; Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64'`
- Full gate (fmt, tests, clippy, release build; in the background, never two at once): `.\rust\build.ps1 -FetchDependencies -Dependencies C:\src\butterpollo-release\<ver>\sdk -TargetDirectory C:\src\butterpollo-release\<ver>\target *> <log>`, then grep the log for `^test result|FAILED|^error`. Package: add `-SkipTests -Package`, keeping `-FetchDependencies`. Measure only release builds.
- Other CI checks: `cd rust\web; npm ci --no-audit --no-fund; npm run check; node --test "*.test.mjs"; npm run build` (kill stray node/vite first: "Port 24678 is already in use"), and `python -m unittest discover -s rust/release -p test_release.py` (and `test_gpu_hang.py`).
- Driver: build and install per its README (`build-package.ps1`, then `install-local.ps1` elevated, host idle, unsigned package). May need a restart.
- Disk fills up: share `C:\src\rl-target` across worktrees and delete stale worktree `target\` folders.
- `core.autocrlf=input`: keep LF (`open(..., newline='')`); edit with the Edit tool, not patch scripts.
- Heredocs and PowerShell here-strings break here: write multi-line scripts and commit messages to a file (`python f.py || exit 1`, `git commit -F f`). Join rebase and push with `&&`, never `;` or a pipe, and grep for `^<<<<<<<` before `git add`.
- Parallel agents each get their own worktree. Never `git stash` (shared across worktrees); move changes with `git diff --binary` and `git apply -3`.

## CI and release
- CI: `.github/workflows/rust-windows.yml` (windows-2022, 2× minutes) on pushes to main touching `rust/**`, Cargo or toolchain files; docs-only commits skip it. Always pass `-R RamazanKara/Rubylight` to `gh`.
- Versions: patch bump for fixes, minor for features; fewer, well-tested releases. Commit style: `fix(stream): ...`, no model names.
- Release (details in rust/release/README.md): changes go under `## Unreleased` in `rust/RELEASE_NOTES.md`. Run `python rust/release/bump.py X.Y.Z`, fill in `## New in X.Y.Z` with a "Tested." paragraph of our own measurements, check it with `python rust/release/notes.py vX.Y.Z --out body.md`, commit, push main.
- Tag the exact SHA whose main run is green, not HEAD: `git tag -a vX.Y.Z -m "Rubylight X.Y.Z" <sha> && git push origin vX.Y.Z`. Expect 3 assets (setup exe, zip, SHA256SUMS), then run `rust/release/check.ps1 vX.Y.Z` on this PC.
- Never delete a published release or move a tag unless the user says so.

## Product rules
- Every fix gets a regression test. Failures are loud: no silent fallback such as AMF to software.
- Don't change `PROTOCOL_VERSION`; stay compatible with Nonary's Moonlight.
- No added latency. Prefer client-reported hints (`pyrowaveLinkMbps`, `pyrowavePaceMbps`) to host probes. Revert changes that measure worse.
- `main` (3.0) is AMD-only: only NVENC, QSV and software encoders are gone (NVIDIA and Intel stay on 2.6.x, no backports unless the user asks). Every AMD GPU with AMF, Windows 10, physical displays, WGC and Desktop Duplication, D3D11 and the keyed-mutex ring stay; new paths go beside them (plan decision 1a). Never write "RX 7000 or newer" or drop hardware support silently.
- Claims: only our own measurements; no first/only/fastest without evidence. Check `rust/PERFORMANCE.md` and the release notes before calling something unmeasured or reviving an old branch. Never install or measure competing hosts (Vibepollo, Apollo, Sunshine, punktfunk); cite "Against Vibepollo 2.0" in `rust/PERFORMANCE.md`. Don't mention punktfunk.
- Keep the credits and fork history in `AGENTS.md`, `NOTICE` and the README intact.
- GitHub issues: reply yourself with `gh issue comment`, short and human. Never post on Reddit.

## Hardware testing
This PC is the dev box, the release test host and the user's gaming PC (RX 7900 XT). Host log: `C:\ProgramData\Butterpollo\config\logs\butterpollo.log`.
- Before any install, restart or stream: `(Invoke-WebRequest http://127.0.0.1:47989/serverinfo).Content` must show RustHostSessionCount and RustHostPendingSessionCount at 0. Never interrupt a stream.
- While the user is gaming: no GPU benchmarks, streams, virtual displays, layout changes, virtual controllers or `check.ps1`, and build with `-j 4`, unless the user says testing is fine.
- Test hosts use an isolated profile and ports (`butterpollo.exe --config-dir C:\tests\<p> --port 48123 --bind 127.0.0.1`; the harness uses 48518-48544) with `virtual_display_layout = extended`, and `dd_configuration_option = disabled` and `virtual_display_mode = disabled` unless the run needs a virtual display (as SYSTEM). Check that the user is idle first: the pattern window covers the monitor.
- Driver and virtual-display tests need SYSTEM: use `rust/release/elevated.ps1` (`schtasks /RU SYSTEM`; setup in rust/release/README.md).
- Exit code 3 from `e2e.py` or `check.ps1` means an AMD encoder hang: stop and investigate, never retry.
- Restart only with the host idle, nothing in flight and the work able to resume unattended; batch it last. Never upgrade Windows.
- Run streams and soaks in the background; read `result.json` and the 5 s `stream timings` lines in the host log, not full logs.
- Clients: the user runs phone tests (read the host log after). The laptop "lap" (`ramaz@192.168.4.12`, use PowerShell ssh) runs `smoke.ps1`; the NUC (`ssh nuc`) stands in for phones while the user is away.
