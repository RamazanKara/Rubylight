# Release tooling

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

## Code signing

The [Code signing policy](../../docs/code-signing.md) contains the application draft, privacy disclosure and maintainer activation steps. Signing stays disabled while `SIGNPATH_ORGANIZATION_ID` is absent. Official builds use `-SkipTrueHdr`; the proprietary [TrueHDR add-on](../../docs/building.md#optional-unsigned-truehdr-add-on) is built separately and stays unsigned.

With the variable set, releases always build in their own workflow run on GitHub-hosted runners. The `sign` job signs the package's own binaries using the `payload` artifact configuration, rebuilds the internal SHA-256 manifest and portable ZIP with `repackage.ps1`, then signs the assembled installer using the `installer` configuration. Both requests use `release-signing` and require manual SignPath approval (up to an hour each). The installer must be signed after appending its payload; the extractor understands the certificate table, and the installed uninstaller keeps the complete signed file.

The publish job uses only the signed artifact when signing is enabled and computes `SHA256SUMS` last. A rejected, timed-out or failed signing request prevents publication. Ordinary CI remains unsigned; when the variable is absent, release build reuse and publication work as before.

## Host check

The only part GitHub's runners cannot do needs the Radeon release workstation, with the installed host idle:

```powershell
pwsh rust/release/check.ps1                # the newest release
pwsh rust/release/check.ps1 v2.2.2 -NoInstall -NoUpload
```

It downloads the release and checks it against `SHA256SUMS`, streams H.264, HEVC, HEVC VRR, AV1, HEVC with keyframe requests (16, one every 0.5 s, while the picture moves at the stream rate; the host's per-claim trace must show no unchanged picture encoded again) and PyroWave (SDR and HDR 4:4:4 at 1080p60) for 12 seconds each through the packaged host on an isolated profile with the independent moonlight-common-c client, runs the pairing and launch protocol checks (`e2e.py`, `protocol.py`), runs the display self-test as SYSTEM and installs over the running host (`elevated.ps1`), then records the results with `validation.py`, uploads them as `VALIDATION.json` and links them from the release text. The stream checks start a second host on ports 48518-48544 and do not touch displays, HDR or audio. The receivers and probes are kept in `C:\src\butterpollo-release\fixtures` and only rebuild what changed.

Machine settings live outside the repository in `C:\src\butterpollo-release\settings.ps1`, which is dot-sourced: the build environment for the test receivers and probes (MSYS2 UCRT64 on `PATH`, `BUTTERPOLLO_PYROWAVE_ROOT`, ...) and `BUTTERPOLLO_TEST_PYTHON` (a Python with `requests` and `cryptography`). Work files go to `C:\src\butterpollo-release\<version>`. Not `%LOCALAPPDATA%`: a packaged (MSIX) app such as the Claude desktop app sees a private copy of that folder, and the SYSTEM self-test could not find a package unpacked there.

### No prompts

- **UAC.** Run `rust/release/elevation.ps1` once from an elevated PowerShell. It copies `elevated.ps1` and `elevated-task.ps1` to `C:\ProgramData\ButterpolloRelease` (writable only by administrators and SYSTEM) and registers the on-demand task `ButterpolloReleaseElevated`, which runs them with highest privileges for you. `check.ps1` starts that task instead of asking, as long as the installed scripts match the checkout; after they change it asks once more and says to run `elevation.ps1` again. The trade-off: any program running as you can start the task, which installs the setup and runs the self-test from `C:\src\butterpollo-release\<version>` with administrator rights. `elevation.ps1 -Remove` uninstalls it.
- **Firewall.** The test hosts listen on 127.0.0.1 only, so Windows Firewall has nothing to ask about.

## Real-client smoke test

After the install, from the measurement laptop (Moonlight-qt paired as client "lap", whose Extended layout streams a virtual display, with `motion_probe` drawing on it from the host):

```powershell
pwsh rust/release/smoke.ps1 -Version v2.2.2
```

It streams the Desktop app for 30 seconds per codec at 1968x2184, HDR, 120 fps, 80 Mbps with AV1 and HEVC, closes Moonlight and reads its "Global video stats". A run passes with at least 115 fps received and a host processing average under 5 ms. The result goes to the release as `REAL-CLIENT.json` when the GitHub CLI is signed in on the laptop. If the laptop is offline the release has no `REAL-CLIENT.json`: the smoke test was skipped.
