# Build Rubylight

[Documentation](README.md) · [Getting started](getting-started.md) · [Configuration](configuration.md)

The Windows host, service, launcher and installer are written in Rust. [rust/build.ps1](../rust/build.ps1) performs the complete build and packaging; [.github/workflows/rust-windows.yml](../.github/workflows/rust-windows.yml) defines the CI environment. The [Rust developer guide](../rust/README.md) describes the workspace and its crates.

To install a release instead, see [Getting started](getting-started.md). The [archived C++ build guide](https://github.com/RamazanKara/Rubylight/blob/2.0.0-rc.23/docs/legacy/building-cpp.md) covers the earlier CMake host, whose sources live at the 2.0.0-rc.23 tag.

## Windows prerequisites

Windows x64 with PowerShell, Git, Rustup, Node.js 22, MSYS2, and Visual Studio with the C++ workload and the Windows Driver Kit (for the display driver). In an MSYS2 UCRT64 shell, install these packages, matching CI:

```sh
pacman -S --needed git mingw-w64-ucrt-x86_64-gcc mingw-w64-ucrt-x86_64-clang mingw-w64-ucrt-x86_64-cmake mingw-w64-ucrt-x86_64-ninja mingw-w64-ucrt-x86_64-vulkan-headers mingw-w64-ucrt-x86_64-opus mingw-w64-ucrt-x86_64-nasm make
```

The default MSYS2 path is `C:\msys64`; pass `-MsysRoot` if yours differs. The host uses the GNU Rust target.

Clone the repository with its submodules and install the pinned toolchains in PowerShell:

```powershell
git clone https://github.com/RamazanKara/Rubylight.git
cd Rubylight
git submodule update --init --recursive

rustup toolchain install 1.98.1-x86_64-pc-windows-gnu --profile minimal --component rustfmt --component clippy
rustup target add x86_64-pc-windows-msvc --toolchain 1.98.1-x86_64-pc-windows-gnu
```

The pinned version is defined by [rust-toolchain.toml](../rust-toolchain.toml) and the build script; `Cargo.lock` pins Rust dependencies.

## Build and package

Run the full build from the repository root in a **Visual Studio x64 developer PowerShell** so the display driver build finds the Microsoft C++ toolchain and the Windows Driver Kit:

```powershell
.\rust\build.ps1 -FetchDependencies -Package
```

The script builds the pinned PyroWave SDK and FFmpeg 8.1.2's AC3 and E-AC3 codecs (LGPL, `rust/tools/build_ffmpeg.sh`, kept when the dependency folder already holds the same build), and extracts the signed gamepad driver package, `nefconc.exe` and the Playnite plugin from the pinned Vibepollo 2.0.0 release. Rubylight's own display driver is built from source in this repository. Archive hashes, the PyroWave and FFmpeg build identities and the gamepad driver's signatures are checked. CMake builds the external PyroWave SDK; Cargo builds the host.

The build checks Rust formatting, runs ordinary workspace tests and Clippy with warnings denied, then builds the release executables, performance probes and the display driver. Packaging also builds the Svelte console and includes runtime libraries, drivers, the Playnite plugin, artwork, licenses, the optional Moonlight source patch and a SHA-256 file manifest. Building a package does not install it. Packaging also downloads and verifies the pinned, signed usbip-win2 0.9.8.1 installer (about 26 MB), cached in the dependencies folder, and includes it with its BSD license. It never executes that installer. `rust/drivers/usbip/bundle.ps1` can check this small packaging step independently of a host build.

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
| `-DebugBuild` | Build debug executables for development. Use release builds for streaming measurements. |
| `-PyrowaveRoot` | Reuse an existing PyroWave SDK. Its identity must match the pinned revision and patches. |
| `-FfmpegRoot` | Reuse an existing `rust/tools/build_ffmpeg.sh` build. Its `build-info.txt` must name the pinned source and the LGPL. |
| `-DriverRoot <path>` | Take the signed gamepad driver package from a Vibepollo installation's `drivers` folder instead of the pinned release. |
| `-SkipTests` | Skip workspace tests and Clippy for a local iteration. Formatting still runs; this is not the release validation path. |
| `-Stage Test\|Lint\|Build` | Run one stage: tests, Clippy, or the release build (with `-Package`). CI runs the three as parallel jobs; the default `All` runs them in order. |

## SDK environment

Cargo alone cannot build the Windows crates: their build scripts generate bindings from the pinned PyroWave and Vulkan headers. The AMF headers are vendored in `rust/windows/include/AMF`. The build script sets these variables for the session it runs in; set them yourself to run `cargo` in another shell.

| Variable | Read by | Points at |
| --- | --- | --- |
| `BUTTERPOLLO_PYROWAVE_ROOT` | `rust/windows/build.rs` | The pinned PyroWave SDK built by `rust/tools/build_pyrowave.sh`. |
| `BUTTERPOLLO_VULKAN_INCLUDE` | `rust/windows/build.rs` | Vulkan 1.4 headers, `C:\msys64\ucrt64\include` in MSYS2. |
| `LIBCLANG_PATH` | bindgen | MSYS2's `ucrt64\bin`, which holds `libclang.dll`. |
| `BUTTERPOLLO_DRIVER_ROOT` | `rust/build.ps1` | Default for `-DriverRoot`. |
| `BUTTERPOLLO_FFMPEG_ROOT` | `rust/build.ps1` | Default for `-FfmpegRoot`. |
| `BUTTERPOLLO_TEST_FFMPEG_ROOT` | the ignored FFmpeg tests | The FFmpeg build's `bin` folder; `rust/build.ps1` sets it and runs them. |

`butterpollo-core` needs none of them and builds and tests on any OS with plain `cargo test -p butterpollo-core`.

## Development checks

After the build script has configured the SDK paths in the same PowerShell session, the ordinary Rust checks are:

```powershell
cargo +1.98.1-x86_64-pc-windows-gnu fmt -- --check
cargo +1.98.1-x86_64-pc-windows-gnu test --workspace --locked
cargo +1.98.1-x86_64-pc-windows-gnu clippy --workspace --all-targets --locked -- -D warnings
```

### Faster local builds

The dev and test profiles keep only line tables for the workspace crates (panics and backtraces still show file and line) and no debug info for dependencies, which keeps test target folders small (a cold test build of the core, Windows and host crates: 10 GB instead of 25). Set `CARGO_PROFILE_DEV_DEBUG=full` for a debugger session. Release builds keep their own profile. Two optional tools cut local build time further:

- **sccache** caches compiled crates across target folders and checkouts: `cargo install sccache --locked --no-default-features`, then set `RUSTC_WRAPPER=sccache`, `SCCACHE_DIR` (not under `%LOCALAPPDATA%` if it is virtualized) and `CARGO_INCREMENTAL=0`.
- **lld** links the test binaries faster than GNU ld: install `mingw-w64-ucrt-x86_64-lld` in MSYS2 and set `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS=-Clink-arg=-fuse-ld=lld`. Not `RUSTFLAGS`: it would replace the display driver's `+crt-static`. Rust flags are part of every artifact's fingerprint and cache key, so set them the same way for every build or each change rebuilds everything.

Keep target folders between runs instead of deleting them. Two checkouts may reuse one target folder one after the other but never at the same time, and the second must touch its sources first: Cargo compares modification times, so a checkout whose files are older than the folder's artifacts would test the other checkout's build.

The display driver (`rust/drivers/rubylight-vdisplay`) is outside the main workspace; the build script builds it separately with the Windows Driver Kit. Nightjar (`client/`) keeps its own workspace in `client/protocol`; the host takes only its `rubylight-protocol` crate, by path.

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

Open `_site/index.html`. The build fails on a link to a file that doesn't exist. The landing page's measurements are written into [site/index.html](../site/index.html); when a newer measurement replaces one, update it there too.
