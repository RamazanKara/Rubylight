

<pasted_content id="d20c">
You're working on Rubylight (github.com/RamazanKara/Rubylight) on my Windows dev PC (RX 7900 XT). Work fully autonomously through to a published release. This builds on the fence-ring release (CAP_FENCE_RING); if that isn't on main and tagged yet, stop and report.

## Goal
Rubylight becomes a host for current AMD GPUs that streams only Rubylight virtual displays, with D3D12 for every frame in the host. Delete everything below that floor instead of keeping fallbacks. Then rebuild the README, website and demo film around that.

## Floor (setup refuses below it)
- GPU: an AMD adapter (vendor 0x1002) on which AMF can create the AV1 encoder. That admits RDNA 3 and newer, including RDNA 3 APUs. Detect it by asking AMF, not from a device-ID list.
- Windows 11 23H2 (build 22631) or newer. Phase 4 may raise this.
- Rubylight's display driver installs and starts. Setup already detects when Windows refuses it (Smart App Control, S mode, App Control). That now ends setup with the reason instead of falling back to a physical display.
Below the floor, setup refuses before touching anything, names what it found and what's required, and leaves an existing installation working.

## Work in phases
One PR per phase. Merge a phase only after its gates pass. Release once at the end, including every phase that merged; never ship half a phase.

### Phase 0: find out (no PR)
- Record this PC's Windows build.
- Add a log line to the driver at load: the IddCxGetVersion result, and whether IddCxCheckOsFeatureSupport reports the D3D12 swap chain. Build it, install it test-signed (you have my consent on this PC), and read the result.
- Phase 4 runs only if this PC reports the D3D12 swap chain. Never upgrade Windows yourself.

### Phase 1: AMD only
- Remove NVENC (native, nvenc_legacy, CUDA interop), nvapi.rs and every NVIDIA-only path and test.
- Remove Quick Sync and the FFmpeg-based encoders (hardware and libx264/libx265/libsvtav1). Drop FFmpeg from build.ps1 and the package unless something else still uses it; list what does.
- Remove vendor detection in Automatic (the encoder is AMF) and the Windows 10 code paths.
- Remove workarounds and fallbacks for AMD hardware or drivers below the floor. For each one, cite why it existed (git blame/log, PERFORMANCE*.md, issues) and show that the reason is below the floor. If you can't, keep it and list it.
- Keep H.264, HEVC and AV1 through AMF (clients choose), and keep PyroWave.

### Phase 2: virtual displays only
- Every stream runs on a Rubylight virtual display. Remove Windows Graphics Capture, Desktop Duplication, physical-display streaming and the capture setting, with their probes and tests. Keep the display layouts (extended, exclusive, isolated) and HDR.
- Two devices on one virtual display with different settings: the driver keeps one ring per monitor, so take one capture and fan it out to a conversion and encode per device. Don't open a second capture.
- When the running driver lacks CAP_FENCE_RING (an update waiting for a restart), the host refuses the stream and the console says to restart Windows. Then remove the host's keyed-mutex ring.
- The driver keeps accepting keyed-mutex rings while it stays on D3D11, because a setup rollback can pair an older host with this driver.

### Phase 3: D3D12-only host
- Move PyroWave's interop to D3D12 textures and fences imported into Vulkan, then remove every remaining D3D11 device from the host crates (tests may keep theirs).
- Gate: no ID3D11Device is created anywhere in the host's runtime code. Check it in code, and with a counter in a debug build during a stream.

### Phase 4: D3D12 driver (only if Phase 0 found support)
- Install a WDK whose headers have IddCx 1.11. If none is available, skip this phase and record why.
- Bind an ID3D12Device with IddCxSwapChainSetDevice2, receive pD3d12Surface, report the queue that reads it as the docs require, and signal the ring's written fence from D3D12. Remove the driver's D3D11 path and its keyed-mutex rings.
- Raise the Windows floor to the oldest build you can show reports the feature.
- Verify that the previous release's host still gets a picture through its own WGC fallback.

### Phase 5: positioning (README, website, demo film, docs)
Runs after Phases 1-4. Rubylight's pitch becomes: built for one platform (current Radeon, Rubylight's own virtual display, D3D12 for every frame in the host), and measured end to end.

Measure before you write:
- On this PC, with the same receiver and identical settings, measure Rubylight against the current releases of Vibepollo, Apollo, Sunshine and punktfunk: HEVC/AV1, SDR/HDR, 120 and 240 fps, idle and beside gpu_load.
  - Record each host's picture age (mean and p95) and its Moonlight host-latency field.
  - Leave out a host that doesn't work with the receiver, and say why.
  - Skip a host whose installer would replace Rubylight's driver or can't be removed cleanly.
  - After each one, uninstall it, restore Rubylight, and confirm a Rubylight stream works.
- Verify the "only" claim: for each of those hosts plus Vibeshine and Foundation Sunshine, read the current source and record whether capture, conversion or encode uses D3D11 (file, commit, date).
  - Write "the only D3D12-only host" only if every one of them does, and put the evidence in docs/performance.md.
  - Otherwise use "D3D12 for every frame" without "only".
- Write "D3D12 end to end" only if Phase 4 shipped. Otherwise write "D3D12 from the display driver's hand-off to the encoder".

README order:
1. One line saying what Rubylight is and who it's for, then the requirements: RX 7000 or newer, the Windows floor, a Moonlight client.
2. Headline picture age against the other hosts on the same hardware, as a chart with versions and dates.
3. The three clocks: Moonlight's host-latency field, present to sent, and picture age. Show what each covers, with every host's field beside its picture age. Base it on docs/performance.md "Comparing host latency between hosts".
4. The frame path as a diagram: game, DWM, Rubylight's display driver, fence ring, D3D12 conversion, AMF, network. Label each stage with its measured time.
5. Why only AMD and virtual displays: what specializing removed (code, paths, hand-offs) and what it bought.
6. Who it's not for, and their last version.
7. Get started, features, testimonials (trimmed), then credits.
Keep the credits section and the Vibepollo fork history exactly as they are (AGENTS.md, NOTICE). The tone is confident and factual, with no digs at other hosts; Vibepollo is upstream. Remove NVIDIA, Intel and physical-display mentions outside the credits, and replace the "Next: Direct3D 12" paragraph with what shipped.

Website (site/): the same story and numbers. Build it with site/build.py and check that the Pages workflow passes.

Demo film:
- Update the scenes in docs/media/render_launch_film.py to follow one frame through the new path with the real stage times, then show the picture-age comparison, then end with the requirements.
- Re-render demo.mp4, demo.gif, the teaser and site/poster.jpg with docs/media/render_demo.py.
- Render a storyboard and look at every scene for legible text and correct numbers.

Docs: update docs/getting-started.md, features.md, troubleshooting.md, configuration.md and architecture.md to match what shipped.

## Settings migration (Phases 1-2)
- Saved settings naming removed encoders or capture modes (nvenc*, qsv, quicksync, software, capture = wgc/ddx) migrate to AMF on the virtual display, with a one-time console warning. They never cause a startup failure.
- Remove the dropped options from the web console (rust/web).
- Add a floor marker to the release and teach the updater to skip releases whose floor the PC doesn't meet. Older hosts will still offer this release; the setup refusal is the safeguard.

## Gates for Phases 1-4 (after every phase; never loosen a gate, threshold or test)
1. cargo fmt, cargo clippy --workspace --all-targets -D warnings, cargo test --workspace, the driver workspace's clippy/fmt/build-package.ps1 -TestSign, and the web console's build and tests.
2. The native ignored tests from rust/README.md. Remove a test only together with the code it tests, and list it in the PR.
3. Run the vd_e2e matrix (HEVC/AV1, SDR/HDR, 60/120/240 fps; scripts in C:\src\butterpollo-release\idd-push) twice on main before Phase 1 as the baseline, and twice on each candidate. Pass only if:
   - no receiver check that passed on the baseline fails;
   - picture age and present to sent (mean and p95) are no worse than the baseline beyond its run-to-run spread;
   - the driver's dropped/overwritten counters are no higher.
4. Setup on this PC:
   - an upgrade from the current release keeps settings, pairings and the app library;
   - a profile using removed encoders and capture modes migrates with the warning;
   - forced below-floor verdicts (GPU, Windows, driver refused) each refuse and change nothing. Use a test-only switch that release builds don't contain.
5. Phase 2 and later: two clients on one virtual display, one SDR HEVC and one HDR AV1, both get a picture for 5 minutes.
6. Phase 3 and later: PyroWave streams pass its native tests and an e2e run.
7. A 30-minute 120 fps stream beside windows/examples/gpu_load.rs. Pass only if the stream holds, and WUDFHost's handle count and the GPU's dedicated memory are flat at the end.
8. No remaining references to removed encoders, capture modes or physical-display streaming in code, config, console or docs, outside the release notes and credits.

## Gates for Phase 5
- Every number in the README, site and film links to its measurement and matches the rust/PERFORMANCE.md tables; check each one.
- Every link resolves.
- No "first", "only" or "fastest" claim without the evidence above.
- The storyboard is checked, and the Pages workflow passes.

## Release
- Merge Phase 5 together with the version bump, just before tagging, so the README and site never describe an unreleased version.
- Bump the minor version (2.5.0 if nothing has shipped since).
- Release notes in the existing voice:
  - lead with who this release is for and who it drops (non-AMD GPUs, Radeon before RX 7000, Windows below the floor, PCs that refuse the driver, physical-display streaming);
  - say that the last version for them is the current one, and that setup refuses and leaves their installation working.
- Add a dated rust/PERFORMANCE.md section with the before/after and competitor tables, and record Phase 0's finding in PERFORMANCE_WORK.md.
- Push the tag, wait for the workflow, and confirm the release and its assets.

## Stop rules
If a phase's gates fail or can't run, don't merge that phase. Leave its PR open with your findings and release only the phases that merged. If Phase 5 can't merge, the release still ships with the README's requirements and claims corrected to match what shipped. Never force-push main or move a tag.

## Report
When done: per phase, merged or not, lines removed, the measurements (including the competitor table), and links to the PRs and the release.
</pasted_content id="d20c">
