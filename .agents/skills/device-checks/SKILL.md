---
name: device-checks
description: Run and rerun checks on real devices (this PC, the laptop, the NUC, Android handheld, TV and phone), collect the results and clean up.
---

# Device Checks

Use this skill when a check needs real hardware; prove what you can with unit, fake-device and interop tests first. Addresses, serials and logins come from the operator's machine notes; never add them to the repository. For host isolation, ports, locks and exit code 3, follow the [isolated-e2e skill](../isolated-e2e/SKILL.md).

## Hardware batch (`tools/hw/catalog.yaml`, `tools/hw/run.ps1`)

1. Plan first: `tools\hw\run.ps1 -Batch <device> -DryRun` checks preconditions and prints the commands. Devices: `desktop`, `laptop`, `laptop-host`, `nuc`, `retroid`, `user` (manual steps, never run).
2. Run: `tools\hw\run.ps1 -Batch <device>`; override catalog `vars` with `-Var name=value` (check `release` and `package`).
3. Preconditions (`idle`, `isolated-ports`, `system`, `gpu-lock`) are checked per case; a failed one stops the batch and the rest are `NOT RUN`. Wait or fix the cause; never delete a lock.
4. **Exit 3 = AMD encoder hang (`HANG`): investigate, don't retry.**
5. Rerun only failed or blocked cases: `tools\hw\run.ps1 -Batch <device> -Only <id>,<id>`. `-Only` also runs `pending` cases, so name only those with a real command.
6. A `REVIEW` result is not a pass: read the output against the case's `pass`.

## This PC

- Published release: `pwsh rust/release/check.ps1 [v<version>]` downloads, streams, self-tests, uploads results and **installs over the running host** (`-NoInstall -NoUpload` skip those). Run it only with the installed host idle.
- A build under test: an isolated test host (`rust/release/e2e.py`), never the installed one.

## Laptop (`laptop.py` or ssh)

- Full check of a pushed branch: `tools\remote-check.ps1 -Host laptop -Branch <branch> [-Wait]`. Exit 4 = busy, 5 = nobody or another user at the console.
- Laptop as host, this PC as client: `pwsh tools\hw\laptop-host.ps1 -Out <dir>` (exit 4 = busy or not idle).
- The ssh shell is German Windows PowerShell 5.1 in session 0, elevated. Inline `$_` gets mangled: `scp` a script and run it with `powershell -NoProfile -ExecutionPolicy Bypass -File <script>`. Keep scripts ASCII.
- GUI, capture or program-starting tests need an Interactive scheduled task in the console user's session; copy the pattern in `tools/remote-check/start.ps1`.

## NUC (`ssh nuc`, Linux)

- Catalog cases: `-Batch nuc`. Nightjar under gamescope: `tools\hw\nuc-gamescope.ps1 -Out <dir>`.

## Android handheld, TV, phone (adb)

1. Connect: `adb devices`; over Wi-Fi `adb connect <ip>:<port>`, first time `adb pair <ip>:<pairing-port>` with the code the user reads from the device. Pass `-s <serial>` when more than one is attached.
2. Build: `python tools/build_lock.py --lane gradle --label nightjar-apk -- cmd /c client\gradlew.bat -p client --no-daemon --max-workers=2 assembleNonRootDebug`
3. Install: `adb -s <serial> install -r client/app/build/outputs/apk/nonRoot/debug/app-nonRoot-debug.apk` (package `com.butterpollo.client.debug`; build with `-PdebugIdSuffix=.<tag>` for a side-by-side copy).
4. Evidence: `adb -s <serial> logcat -v threadtime` to a file; `adb -s <serial> exec-out screencap -p` for screenshots.
5. Scripted cases: `-Batch retroid` (see also `tools/hw/host_gone.py`). Gradle unit tests run on the PC and prove nothing about a device.
6. Restore any `setprop` or display setting you changed. Don't uninstall or clear the user's release app.

## Windows Firewall prompts

- Windows asks once per program path, so a new folder each run prompts again and blocks a remote run.
- Run test binaries from a fixed, already-allowed folder: a warm target pool slot (build with `python tools/dev.py cargo -- ...`, which prints the slot folder; never a private `target/`), the laptop host's `-LaptopPackage` folder, or for Nightjar desktop `tools\nightjar-test-client.ps1 -From <exe-or-zip> [-Slot a|b]`.
- Prefer loopback in tests. If a prompt is unavoidable, name the exact exe path before asking the user to allow it; never ask them to click through prompts mid-batch without it.

## Collect and clean up

1. Batch results: `C:\src\butterpollo-release\hw\<date>\<device>\` with `summary.md`, and per case `output.log`, collected files and `stream-timings.txt`. A rerun gets a new folder; report both.
2. Copy remote results back with `scp` before deleting anything; `remote-check.ps1` copies its own to `-Out`.
3. Delete only the scratch folders and scheduled tasks this run created (local and remote, including `/data/local/tmp` files). Keep firewall-allowed folders, profiles with pairings and other runs' work; check that nothing still runs from a folder first.
4. Report per case: device, build SHA, result, and what was not run.
