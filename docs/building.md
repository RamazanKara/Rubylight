# Build Rubylight

[Rubylight docs](README.md) › [For developers](README.md#for-developers) · [Getting started](getting-started.md) · [Configuration](configuration.md)

The Windows host, service, launcher and installer are written in Rust. [rust/build.ps1](../rust/build.ps1) performs the complete build and packaging; [.github/workflows/rust-windows.yml](../.github/workflows/rust-windows.yml) defines the CI environment. The [Rust developer guide](../rust/README.md) describes the workspace and its crates.

To install a release instead, see [Getting started](getting-started.md). The [archived C++ build guide](https://github.com/RamazanKara/Rubylight/blob/2.0.0-rc.23/docs/legacy/building-cpp.md) covers the earlier CMake host, whose sources live at the 2.0.0-rc.23 tag.

## Windows prerequisites

Windows x64 with PowerShell, Git, Python 3, Rustup, Node.js 22, MSYS2, and Visual Studio with the C++ workload. The display and virtual gamepad drivers also need the Windows Driver Kit (CI installs `Microsoft.WindowsWDK.10.0.26100`) and LLVM for their bindings (`winget install LLVM.LLVM`). In an MSYS2 UCRT64 shell, install these packages, matching CI:

```sh
pacman -S --needed git mingw-w64-ucrt-x86_64-gcc mingw-w64-ucrt-x86_64-clang mingw-w64-ucrt-x86_64-cmake mingw-w64-ucrt-x86_64-ninja mingw-w64-ucrt-x86_64-vulkan-headers mingw-w64-ucrt-x86_64-opus mingw-w64-ucrt-x86_64-nasm make
```

The default MSYS2 path is `C:\msys64`; pass `-MsysRoot` if yours differs. The host uses the GNU Rust target; the display and virtual gamepad drivers use the MSVC toolchain pinned by each workspace's `rust-toolchain.toml`.

Clone the repository with its submodules and install the pinned toolchains in PowerShell:

```powershell
git clone https://github.com/RamazanKara/Rubylight.git
cd Rubylight
git submodule update --init --recursive

rustup toolchain install 1.98.1-x86_64-pc-windows-gnu --profile minimal --component rustfmt --component clippy
rustup target add x86_64-pc-windows-msvc --toolchain 1.98.1-x86_64-pc-windows-gnu
rustup toolchain install 1.98.1-x86_64-pc-windows-msvc --profile minimal
```

The pinned version is defined by [rust-toolchain.toml](../rust-toolchain.toml) and the build script; `Cargo.lock` pins Rust dependencies.

## Build and package

Run the full build from the repository root in a **Visual Studio x64 developer PowerShell** so the driver builds find the Microsoft C++ toolchain and the Windows Driver Kit:

```powershell
.\rust\build.ps1 -FetchDependencies -Package
```

The script builds the pinned PyroWave SDK and FFmpeg 8.1.3's AC3 and E-AC3 codecs (LGPL, `rust/tools/build_ffmpeg.sh`, kept when the dependency folder already holds the same build), and extracts `nefconc.exe` and the Playnite plugin from the pinned Vibepollo 2.0.0 release. Rubylight's own display and virtual gamepad drivers are built from source in this repository. Archive hashes and the PyroWave and FFmpeg build identities are checked. CMake builds the external PyroWave SDK; Cargo builds the host and drivers.

The build checks Rust formatting, runs ordinary workspace tests and Clippy with warnings denied, then builds the release executables, performance probes and the display driver. Packaging also builds the Svelte console and includes runtime libraries, drivers, the Playnite plugin, artwork, licenses, the optional Moonlight source patch and a SHA-256 file manifest. Building a package does not install it. While the service's version is unchanged, the package carries the previous release's `butterpollo-service.exe`, checked against that release's `SHA256SUMS`, so the file stays the same across releases ([the rule](../rust/release/README.md#the-service-executable)); a local build without network keeps the fresh one. Packaging also downloads and verifies the pinned, signed usbip-win2 0.9.8.1 installer (about 26 MB), cached in the dependencies folder, and includes it with its BSD license. It never executes that installer. `rust/drivers/usbip/bundle.ps1` can check this small packaging step independently of a host build.

Default outputs are under `%LOCALAPPDATA%\ButterpolloRust\target`:

| Output | Contents |
| --- | --- |
| `release\` | Compiled executables, runtime DLLs and examples. |
| `butterpollo-rust-release\` | Portable package with console assets and dependencies. |
| `butterpollo-rust-release.zip` | ZIP of the portable package. |
| `butterpollo-setup-<version>.exe` | Installer carrying that package; version comes from the workspace manifest. |

CI publishes the installer and the archive as `rubylight-setup-<version>.exe` and `rubylight-<version>-windows-x64.zip`. SDK downloads use `%LOCALAPPDATA%\ButterpolloRust\sdk`. Choose a separate build location with `-TargetDirectory` and a dependency cache with `-Dependencies`.

Useful options:

| Option | Purpose |
| --- | --- |
| `-FetchDependencies` | Download, verify and build the pinned SDKs and driver packages missing from the dependency folder. |
| `-Package` | Build the portable package and installer (needs the `Build` stage). |
| `-DebugBuild` | Build debug executables for development. Use release builds for streaming measurements. |
| `-PyrowaveRoot` | Reuse an existing PyroWave SDK. Its identity must match the pinned revision and patches. |
| `-FfmpegRoot` | Reuse an existing `rust/tools/build_ffmpeg.sh` build. Its `build-info.txt` must name the pinned source and the LGPL. |
| `-DriverRoot <path>` | Take `nefconc.exe` and the Playnite plugin from a Vibepollo 2.0.0 tools folder instead of fetching the pinned release. |
| `-SkipTests` | Skip workspace tests and Clippy for a local iteration. Formatting still runs; this is not the release validation path. |
| `-Stage Test\|Lint\|Build` | Run one stage: tests, Clippy, or the release build (with `-Package`). CI runs the three as parallel jobs; the default `All` runs them in order. |
| `-Jobs <n>` | Parallel jobs for Cargo, CMake, PyroWave and FFmpeg (default 4). |
| `-Nvidia` | Build the NVIDIA component's archive (the NGX adapter with NVIDIA's runtime DLLs) instead of the host; see below. |
| `-NvidiaRoot <path>` | Reuse an extracted NVIDIA RTX Video SDK 1.1.0 instead of fetching it (only with `-Nvidia`). |
| `-MsvcSdk <path>` | Use an xwin SDK layout containing `crt/lib/x86_64`, `sdk/lib/um/x86_64` and `sdk/lib/ucrt/x86_64` for the NVIDIA adapter instead of the developer shell's libraries. |

### The NVIDIA component

RTX HDR and RTX Video enhancement call NVIDIA's proprietary NGX library, so no signed file contains them ([code signing policy](code-signing-policy.md#the-nvidia-component)). Their code in the host is always built; what they need comes in the NVIDIA component, which the console downloads from the host's own release at the user's request:

```powershell
.\rust\build.ps1 -FetchDependencies -Nvidia
```

With `-Nvidia` the script builds the component and no host: it fetches the pinned NVIDIA RTX Video SDK 1.1.0 (SHA-256 `abf4f34e2b5a618e355b0d5a0365d8ecc3db4396e756e4c850a867e1ae2ed69e`) from NVIDIA's NGC, checks formatting, lints, tests and builds the adapter `rust/truehdr-runtime` (`butterpollo_truehdr.dll`, MSVC target, outside the workspace), checks that NVIDIA's `nvngx_truehdr.dll`, `nvngx_vsr.dll` and the SDK licence are exactly the files `rust/core/src/nvidia_component.rs` pins, and writes `rubylight-nvidia-component-<version>.zip` with the four files at its top level, then reads every file back from it. NVIDIA's files go out only beside the adapter, never on their own. CI's `nvidia` job builds it in a developer shell and `nvidia-assets` attaches it to the already published release with its own `SHA256SUMS-nvidia`. The release never waits on it: if it fails, or NVIDIA's download is down, only the component comes later, by running that job again. It is attached only while the repository variable `PUBLISH_NVIDIA_BUILD` is `true` (Settings, Secrets and variables, Actions, Variables); otherwise the `nvidia` job's archive stays a workflow artifact (`butterpollo-rust-windows-x64-nvidia`, kept 30 days). `nvidia-assets` also attaches the signed installer as `rubylight-nvidia-setup-<version>.exe`, the name hosts of the former NVIDIA build (4.3.2 to 4.4.0) update from.

To try an archive before a release, `BUTTERPOLLO_TEST_NVIDIA_COMPONENT=<zip> cargo test -p butterpollo -- --ignored the_built_archive` installs it through the host's own download code with the real pins.

## SDK environment

Cargo alone cannot build the Windows crates: their build scripts generate bindings from the pinned PyroWave and Vulkan headers. The AMF headers are vendored in `rust/windows/include/AMF`. The build script sets these variables for the session it runs in; set them yourself to run `cargo` in another shell.

| Variable | Read by | Points at |
| --- | --- | --- |
| `BUTTERPOLLO_PYROWAVE_ROOT` | `rust/windows/build.rs` | The pinned PyroWave SDK built by `rust/tools/build_pyrowave.sh`. |
| `BUTTERPOLLO_VULKAN_INCLUDE` | `rust/windows/build.rs` | Vulkan 1.4 headers, `C:\msys64\ucrt64\include` in MSYS2. |
| `LIBCLANG_PATH` | bindgen | MSYS2's `ucrt64\bin`, which holds `libclang.dll`. |
| `BUTTERPOLLO_DRIVER_ROOT` | `rust/build.ps1` | Default for `-DriverRoot`. |
| `BUTTERPOLLO_FFMPEG_ROOT` | `rust/build.ps1` | Default for `-FfmpegRoot`. |
| `NV_RTX_VIDEO_SDK` | `rust/build.ps1`, `rust/truehdr-runtime/build.rs` | NVIDIA's RTX Video SDK, only for the NVIDIA component; default for `-NvidiaRoot`. |
| `RUBYLIGHT_NGX_APPLICATION_ID` | `rust/truehdr-runtime/build.rs` | The NGX application ID the adapter gives NVIDIA's NGX; 0, the SDK's generic ID, when unset. |
| `BUTTERPOLLO_MSVC_ROOT` | `rust/build.ps1` | Default for `-MsvcSdk`. |
| `BUTTERPOLLO_TEST_FFMPEG_ROOT` | the ignored FFmpeg tests | The FFmpeg build's `bin` folder; `rust/build.ps1` sets it and runs them. |

`butterpollo-core` needs none of them and builds and tests on any OS with plain `cargo test -p butterpollo-core`.

## Development checks

After the build script has configured the SDK paths in the same PowerShell session, the ordinary Rust checks are:

```powershell
cargo +1.98.1-x86_64-pc-windows-gnu fmt -- --check
cargo +1.98.1-x86_64-pc-windows-gnu test --workspace --locked
cargo +1.98.1-x86_64-pc-windows-gnu clippy --workspace --all-targets --locked -- -D warnings
```

The service is a separate workspace: `rust/build.ps1` runs the same three commands for it with `--manifest-path rust/service/Cargo.toml`.

### Faster local builds

Use `tools\dev.ps1 check -DryRun` to preview the checks selected for this
worktree's changes, then `tools\dev.ps1 check` to run them. Changes are
compared against the merge base with `-Base` (default `origin/main`), plus
uncommitted files; `-Base HEAD` checks only uncommitted changes. Each build
stage takes its own lane (below), so stages of different worktrees run side by
side. Pure Python suites take none. A failure stops later checks by default;
`-KeepGoing` runs the remaining independent checks.
Every invocation writes a separate run folder, by default under
`C:\src\butterpollo-release\unified\dev-check\<worktree>` (`-LogDir` chooses
another), with one log per check and `summary.json` with the commit, worktree,
selected paths, results and each stage's lock wait (`wait`). The Cargo stages build in a warm slot of the
target pool `C:\src\rl-target-pool` (`-Pool` chooses another; `-TargetDir` one folder instead). These are
development results, not release approval; uncommitted changes may be included.

For the workflow's own Python tests, install the small pinned test dependency
once with `python -m pip install -r tools/requirements-test.txt`. The
`Developer workflow checks` CI runs these suites on Windows without an SDK,
host installation or GPU work.

Local web checks reuse an npm install only after a successful `npm ci` with
matching manifests, Node/npm versions, npm configuration and installed lock
file. `-FreshDependencies` forces a clean install when investigating dependency
problems. CI and release packaging still install dependencies afresh.

#### Build locks and lanes

A lock for the whole check made a client-only branch wait behind a host build.
So each stage takes its own lane (`tools/build_lock.py`) in the lock directory:

| Stage | Lane |
| --- | --- |
| host, service, client-protocol, interop (Cargo) | one of the Cargo slots, and a warm target slot |
| app (Gradle) | the Gradle lane `D:\CodexArtifacts\locks\gradle-lane`, the one manual Gradle runs take with `--lock-directory` |
| web | the web lane |

The Cargo slot count caps the Cargo stages at once: CPU threads per `-j 4`
stage and memory above a 16 GB reserve per stage peak (7 GB of commit charge
measured for a host stage on 2026-11-18), whichever is fewer. That is 4 on this
PC (16 threads, 64 GB) and 2 on the laptop (16 threads, 31 GB);
`RUBYLIGHT_CARGO_SLOTS` overrides it. A Cargo, target or web lane is a file
held open without sharing: Windows closes it when its owner exits, so a crashed
stage never leaves one taken. The dev check also ends its builds when it is
killed, so none keeps building in a slot it no longer holds. The Gradle lane is
a `BUILD-LOCK`-style file, shared with manual Gradle runs: a crashed owner
leaves it behind, the next app stage names it, and exclusive owners stop
waiting for it once its owner is gone (its `pid=` has exited, its `start=`
differs, or, in a lock without `start=`, its pid's process started after the
file was written); remove it after checking for surviving Gradle processes.
`rust\build.ps1` and the release checks wait for a live stage as long as it
runs, naming it every 10 minutes, rather than failing the gate.

`BUILD-LOCK` stays exclusive. `rust\build.ps1`, the release checks and
`tools/build_lock.py` without `--lane` take it before SDK setup, tests or
packaging and then wait until every lane is given back; a stage that sees it,
or a live release `PRIORITY` ticket, gives its lanes back and waits. `-Jobs`
defaults to 4 and sets the Cargo, CMake, PyroWave and FFmpeg build limits.
Wrap other heavy commands with `tools/build_lock.py`, with a lane when the
work is one stage's:

```powershell
python tools/build_lock.py --lane gradle --label nightjar-apk -- cmd /c client\gradlew.bat -p client --no-daemon --max-workers=2 assembleNonRootDebug
python tools/build_lock.py --status
```

A warm target slot remembers the checkout that last built in it
(`.rubylight-checkout`, and the slot's `<n>.owner` that `C:\src\rl-quick.ps1`
writes too). A stage prefers its own checkout's slot, then another checkout's
least recently used one. When another checkout built there last, the
fingerprints of this repository's own packages are removed first: Cargo judges
a path package fresh when none of its files is newer than its last build and
names its artifacts the same in every checkout of a version, so a checkout
whose files are older would get the other checkout's build (the
`MAX_BITRATE_KBPS` failures of 2026-11-18 in the shared `C:\src\rl-target`).
Registry and git dependencies stay built.

The shared directory defaults to `C:\src\butterpollo-release` on Windows.
Set `RUBYLIGHT_LOCK_DIRECTORY` consistently for all build callers when using
another machine directory; pass the same `-LockDirectory` to release checks.
With a private lock directory (tests), the target pool and the Gradle lane
move beside it too, so a test never touches the machine's slots.
The developer checker also accepts `-LockDirectory` and `-LockTimeout` (seconds,
default 1800). The wrapper accepts `--lock-directory`, `--timeout` (seconds,
default 1800) and `--label`, the owner name waiting builds see.
A nested command inherits the lease rather than waiting for itself. Do not
start independent parallel builds inside a single lease.

`BUILD-LOCK` never expires by age. Windows keeps a handle that prevents a live lease
being deleted by an older waiter. If a runner is forcibly terminated, its file
remains: check the named owner **and surviving child processes** before removing
it manually. A timeout reports the holder and leaves it untouched. Use separate
Orca worktrees for overlapping edits; one coordinator should integrate their
changes and verify the combined commit before pushing main.

#### Other lanes: GitHub CI and the laptop

Pushes to `wip/**` branches run the host, Nightjar and tools workflows with
main's path filters (docs-only pushes run nothing; only main saves the
compiled-dependency caches). `tools\dev.ps1 ci wip/<topic>` pushes nothing: it
watches the runs of the branch's head on origin and prints the result;
`-Dispatch host,android,macos,tools|all` starts them on the branch when the last
push touched only docs, since GitHub filters each push by its own changes.

`tools\remote-check.ps1 -Host laptop -Branch wip/<topic>` runs `tools\dev.ps1
check -KeepGoing` for a pushed branch on the laptop and copies the run folder
back. Over ssh the laptop's shell is session 0 and elevated, where host tests
that start programs fail, so the check runs through an Interactive scheduled
task in the console user's session. It refuses while the laptop's
`BUILD-LOCK`, `LAPTOP-LOCK`, a release ticket, a lane or another remote check
is busy (`-Wait` waits).

#### Merges

Nightjar's desktop string tables merge with git's union driver, so two
branches that append entries keep both; an entry both change comes out twice,
which the table test rejects. Generated docs (`client/docs/settings.md`,
`client/docs/devices.md`) use the driver of `tools/generated_docs.py`: register
it once per clone with `python tools/generated_docs.py install`; after a merge
that touched their sources, run `python tools/generated_docs.py`. The dev check
fails while one is stale.

The dev and test profiles keep only line tables for the workspace crates (panics and backtraces still show file and line) and no debug info for dependencies, which keeps test target folders small (a cold test build of the core, Windows and host crates: 10 GB instead of 25). Set `CARGO_PROFILE_DEV_DEBUG=full` for a debugger session. Release builds keep their own profile. Two optional tools cut local build time further:

- **sccache** caches compiled crates across target folders and checkouts: `cargo install sccache --locked --no-default-features`, then set `RUSTC_WRAPPER=sccache`, `SCCACHE_DIR` (not under `%LOCALAPPDATA%` if it is virtualized) and `CARGO_INCREMENTAL=0`.
- **lld** links the test binaries faster than GNU ld: install `mingw-w64-ucrt-x86_64-lld` in MSYS2 and set `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS=-Clink-arg=-fuse-ld=lld`. Not `RUSTFLAGS`: it would replace the display driver's `+crt-static`. Rust flags are part of every artifact's fingerprint and cache key, so set them the same way for every build or each change rebuilds everything.

Keep target folders between runs instead of deleting them. Two checkouts may reuse one target folder one after the other but never at the same time, and the second must make Cargo rebuild its own packages first: Cargo compares modification times, so a checkout whose files are older than the folder's artifacts would test the other checkout's build. `tools\dev.ps1 check` does this itself (`claim_target` in `tools/dev.py`); for a hand-run Cargo command, touch the sources or use a folder of its own.

The display driver (`rust/drivers/rubylight-vdisplay`) is outside the main workspace; the build script builds it separately with the Windows Driver Kit. The NVIDIA adapter (`rust/truehdr-runtime`) is outside it too; only `-Nvidia` checks and builds it, for `x86_64-pc-windows-msvc`. Nightjar (`client/`) keeps its own workspace in `client/protocol`; the host takes only its `rubylight-protocol` crate, by path.

Hardware and environment-dependent tests are selected separately. Their fixtures can exercise capture, controllers or displays; choose a fixture for the intended machine and preserve its restoration checks. [PERFORMANCE.md](../rust/PERFORMANCE.md) records measured workloads.

## Web console

The current interface is the Svelte app in [rust/web](../rust/web). Its commands, starting from the repository root, are:

```powershell
cd rust/web
npm ci --no-audit --no-fund
npm run check
npm run build
npm run dev
```

CI runs `npm run check`; packaging runs the production build and copies `dist` into `assets/web`. The package script uses a separate web build folder so a WSL checkout's Linux `node_modules` cannot be reused as Windows dependencies.

The development server proxies `/api` to `https://localhost:47990` by default. Set `BUTTERPOLLO_HOST` before starting it to point at an isolated development profile. Requests go to that real host, including actions taken in the console. See [vite.config.ts](../rust/web/vite.config.ts) for the proxy configuration.

## Website

[site/](../site) builds the project website at https://ramazankara.github.io/Rubylight/: a landing page plus these guides, rendered from the repository's Markdown. [.github/workflows/pages.yml](../.github/workflows/pages.yml) publishes it whenever `site/`, `docs/`, `README.md` or the Markdown in `rust/` changes on `main`. Until **Settings → Pages → Source** is set to GitHub Actions, it builds and checks the site but skips the deployment. To preview it, from the repository root:

```sh
python3 -m pip install -r site/requirements.txt
python3 site/build.py
```

Open `_site/index.html`. The build fails on a broken link in a rendered page, README.md or the landing page: a file that doesn't exist, or a `#fragment` that names no heading or id in its target. The landing page's measurements are written into [site/index.html](../site/index.html); when a newer measurement replaces one, update it there too.
