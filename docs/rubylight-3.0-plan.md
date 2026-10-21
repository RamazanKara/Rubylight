# Rubylight 3.0 and Nightjar 3.0 plan

Working plan for the 3.0 cycle, written 2026-10-21. It replaces the "Definitive Master Engineering Specification" draft. Agents execute from this file: keep the status table at the end current and keep the file short. How agents work (delegation, briefs, commits, hardware rules) is set by `CLAUDE.md` and the global working rules. Nothing here overrides them.

## Goal

Rubylight 3.0 is an AMD-only Windows host meant as the successor to AMD Link. You open Nightjar, pick a game, play, and change Radeon settings from the couch. Nightjar 3.0 exposes all of it on Android, with Windows and Linux as a preview, all on one Rust engine. Nightjar stays host-neutral: Rubylight features appear only when `HostCapabilities` reports them, and stock Moonlight keeps working against the host.

"Better" means measured on our hardware against Rubylight 2.6.0: lower picture age, steadier pacing, higher fidelity. Results go into `rust/PERFORMANCE.md`. No first, only or fastest claims without our own numbers.

## Starting point (audit, 2026-10-21)

| Area | State | Evidence |
|---|---|---|
| AMD-only host | Done on the unmerged `claude/rubylight-2.5` branch: 63 commits, 241 files, +17.8k/−30.7k lines. It removes NVENC, QSV, software and FFmpeg encoders. The floor is RDNA 2, a working AMF HEVC encoder, Windows 26200 and the IddCx 1.11 D3D12 driver. It also makes the host D3D12-only, streams on virtual displays only and adds the fence ring. Gates 1 and 4 passed but need a re-run on a new tip. Gates 2, 3 and 5–8 never ran. Main is 96 commits ahead, and the `claude/floor` tip is not in the branch. | `rust/release/v25-gates/HANDOFF.md` and `rust/release/floor.json` on that branch |
| Non-AMD encoders on main | NVENC (native) and QSV (through FFmpeg) still ship | `rust/windows/src/nvenc.rs`, `rust/windows/src/encoder.rs:424` |
| AMF | H.264, HEVC and AV1, HDR10, intra refresh, rate control. Packed 4:4:4 over 4:2:0 exists but is experimental and off by default. | `rust/windows/src/amf/`, `gpu_color/packed444_*`, `rust/RELEASE_NOTES.md` |
| Slices and tiles | 2 or 4 slices or tiles left per-frame encode time unchanged on the RX 7900 XT (about 6.3 and 5.2 ms at 3840×2160), and the driver never engaged split-frame. Every run used whole-frame output. AMF 1.5.2, which we build against, has `OutputMode`/`HevcOutputMode` = SLICE and `AV1OutputMode` = TILE. Those modes return each unit as soon as it is encoded. They are **untested**. | `rust/PERFORMANCE.md` "October 7: AMF split-frame encoding", AMF `VideoEncoderVCE.h:232`, `VideoEncoderHEVC.h:215`, `VideoEncoderAV1.h:245` |
| Timing | Clock sync (0x5537), present timing, network pacing, the pace-based AMF frame cap, the 1000 Hz VRR EDID on the virtual display, and 5 s `stream timings` log lines. Nothing records per-frame stage timing across host and client. | `rust/core/src/clock_sync.rs`, `rust/windows/src/present_timing.rs`, `docs/control-messages.md` |
| PyroWave | D3D11 and Vulkan on main, D3D12 on 2.5. `pyrowave_decoded_end_to_end` fails 1080p SDR 4:4:4 record framing on both (42.8/37.3/36.6 dB against a 40 dB bar). Still open from the audit: async encode was never evaluated, and no test causes a real device loss. | `docs/pyrowave-d3d12-audit-2026-10-18.md` |
| Radeon | ADLX loads, but only for read-only performance telemetry. No code reads or writes Anti-Lag, AFMF, Chill, Boost, RIS or other 3D settings. | `rust/windows/src/telemetry.rs:93` |
| Library | The host imports installed games from Steam, Epic, Battle.net, EA, Ubisoft and GOG. Nightjar shows the Moonlight app grid with artwork and search, but has no favourites, recents or per-game profiles. | `rust/core/src/library.rs:10`, Nightjar `AppView.java` |
| Session | Disconnect reaping exists. Neither side separates Disconnect, End session and Close game. | `rust/host/src/display_session.rs:1741` |
| Client endpoints | Paired-client HTTPS on the streaming port already exists (`/api/hardware-metrics`). The admin API on `port + 1` uses scoped tokens. | `docs/api.md`, `docs/hardware-metrics.md` |
| Autostart | `ApolloService` plus the installer work. Only hardening is needed. | `rust/windows/src/service.rs`, `rust/setup/src/install.rs` |
| Linux host | No code at all. Release notes exclude Linux hosting. | `rust/RELEASE_NOTES.md:879` |
| Nightjar app | On `origin/main` (0.7.18), the Android app is Java on XML Views: decoding through Java MediaCodec with the low-latency keys, Choreographer pacing and `setFrameRate`, plus TV remote focus work. Games can be exported to Beacon, ES-DE, Daijisho and Pegasus. The overlay shows stage timings and exports CSV. 1,400+ unit tests. The local checkout `C:\src\rubylight-android` is 129 commits behind, so pull before any work. | Nightjar `docs/FRONTENDS.md`, `docs/PARITY-MATRIX.md` |
| Nightjar Rust engine | The `protocol/` workspace holds 41.7k lines of Rust. `nightjar-core` (19k) is a Rust port of moonlight-common-c's receive paths, RTSP, ENet, pairing and HTTP, and it depends on this repo's `butterpollo-core` through a pinned git rev. `nightjar-android` is a JNI crate, and Android runs streams on it only behind a beta setting. The C engine and its 13 patches are still the default. `nightjar-desktop` is Windows and Linux on egui 0.37, SDL3 and D3D11 (13.8k lines): unreleased and under active development. Then `nightjar-cli` (replaces our C test client) and `difftest`. Packed 4:4:4 reconstruction with HDR output already exists on the client. | Nightjar `protocol/*/Cargo.toml` |
| Release tooling | `bump.py` and `notes.py` accept only `-rc.N`. CI already publishes any `-*` tag as a prerelease. | `rust/release/bump.py:21`, `rust/release/notes.py:24`, `.github/workflows/rust-windows.yml:320` |
| Latency today | Against Vibepollo 2.0 under GPU load: average render-to-decode 42.4 ms against 96.4 ms, p95 56.5 against 137.0. The 3.0 baseline is 2.6.0, not Vibepollo. | `docs/performance.md:157` |

## Decisions

Kept from the draft:

1. **The host is AMD-only.** NVIDIA and Intel users stay on 2.6.x. 3.0 refuses to install on unsupported hardware with a clear message (the 2.5 floor refusals), and the release notes put the floor first. No backports to 2.6 unless the user asks.
2. **Moonlight compatibility is frozen.** Pairing, session control and H.264/HEVC/AV1 streaming keep working with stock Moonlight (Nonary's). No new Moonlight-only features. `PROTOCOL_VERSION` does not change.
3. **Native features ride versioned extensions** on what already exists: 0x55xx control messages, RTSP hints such as `pyrowaveLinkMbps`, and paired-client HTTPS endpoints. No new transport and no new authentication.
4. **Measure, don't assert.** A hardware feature that hasn't run on our hardware is "awaiting validation" in the docs and the status table.
5. **Radeon controls show real state only:** configurable, read-only, needs external setup, unsupported, or temporarily unavailable. No toggle that does nothing.
6. **Sessions have explicit states,** and Disconnect, End session and Close game are different actions. A disconnect never kills the game.

Changed from the draft:

7. **No repo merge.** Code is already shared: `nightjar-core` builds against `butterpollo-core` through a pinned git rev. Native API types go into `rust/core`, the host serves them, and Nightjar bumps the pin when a host slice lands. A merge would add Nightjar's signing, store listing and Gradle builds to a CI that runs on Windows runners costing twice as much. Merge only if pin bumps turn into real friction.
8. **No Linux host in 3.0.** No code exists. AMF on Linux needs AMD's proprietary stack, so a Mesa host means a new VA-API or Vulkan Video encoder, PipeWire or gamescope capture, and different virtual displays. It also needs an AMD Linux test machine we don't have. It becomes a 3.x track.
9. **One client engine.** New client features (the native API, Radeon panel data, session actions, timing) are written once in `nightjar-core` and used by Android through JNI and by the desktop client. 3.0 makes the Rust engine the Android default once it is measured no worse than the C engine. A later release deletes moonlight-common-c and its patches from Android. This is where "Rust-first" pays off.
10. **Nightjar for Windows and Linux ships with 3.0 as a preview.** It exists and is moving fast. It becomes a full client when it passes the same couch-loop gate as Android.
11. **No UI rewrite.** Android keeps Java and Views and builds on the existing TV focus work. Desktop keeps egui. "Unified" means the same screens, names, order and session actions on both, not shared UI code. Bring in Compose or another framework only if a specific screen is blocked, and write that reason down here.
12. **Tile/slice delivery is a spike with a kill criterion,** not a mandate. The data above says slices alone gain nothing; only per-unit output might.

## Release train

`v3.0.0-alpha.N` → `v3.0.0-beta.N` → `v3.0.0-rc.N` → `v3.0.0`. Prerelease tags publish as GitHub prereleases. Nightjar jumps from 0.7.x to 3.0 prereleases, numbered separately (`3.0.0-alpha.N`), with the same signing key and an increasing `versionCode`. Every milestone below ends with its gate, and nothing moves to the next milestone until the gate passes.

### alpha.1: AMD-only base (host only)

Goal: ship the 2.5 work on top of current main. Nightjar 0.7.x and stock Moonlight keep working unchanged.

- **A0 Release tooling.** `bump.py` and `notes.py` accept `-alpha.N` and `-beta.N`, with cases in `test_release.py`. Check `check.ps1` too. (mechanic)
- **A1 Port 2.5 one phase at a time,** in this order: amd-only, floor (check whether the `claude/floor` tip is still needed), d3d12-driver, d3d12-host, idd-push, virtual-only, fence-ring. Use one brief per phase and run the full `build.ps1` gate after each one. Leave out the gate-harness commit `45a9d2630`. Resolve conflicts in favour of main's newer console, library and chroma work.
- **A2 Floor experience.** The installer and the host explain a refusal in plain words: GPU, driver or Windows build, and what to do next. Release notes and README state the floor.

Gate: the full build gate and CI pass. The HANDOFF.md gates re-run on the alpha tip. `check.ps1` and a 30-minute soak pass with no encoder hang. `check.ps1` latency does not regress against 2.6.0, and the numbers go into `PERFORMANCE.md`. Stock Moonlight pairs and streams. An upgrade from 2.6.0 works.

### alpha.2: the couch loop (host and Nightjar)

Goal: one end-to-end story on the RX 7900 XT and the user's phone. Open Nightjar, pick an installed game from the host library, play, open the overlay, change a Radeon setting, then end the session. The setting is restored afterwards.

- **A3 Native API v1 (host).** Paired-client HTTPS endpoints on the streaming port, using the same authentication as `/api/hardware-metrics`. They cover capabilities (API version and feature list), the library (stable game, store and install ids plus artwork URLs, from the `rust/core/src/library.rs` model), session actions, Radeon state, and Radeon writes. Anything that writes needs a per-client "host control" permission, which is off by default and granted in the console. Request and response types live in `rust/core`. Write the spec in `docs/native-api.md`. The client side goes into `nightjar-core` after a pin bump.
- **A4 Session state machine (host).** Idle → Preparing → Launching → Streaming → Suspended → Stopping → Idle, with cleanup that is safe to repeat.
  - Disconnect keeps the game and the virtual display alive for a grace period.
  - End session tears down the stream and the display but leaves the game running.
  - Close game also ends the game process.
  - Reconnecting to a suspended session resumes it.
  - Add a regression test for each transition and for crash recovery.
- **A5 Radeon Control Center v1 (host).** First run a probe that records, for each ADLX 3D setting on the installed driver (Anti-Lag, Chill, Boost, Image Sharpening, Enhanced Sync, frame rate target, AFMF, RSR, whatever ADLX reports), whether it is supported, readable and writable. Commit the results to `docs/hardware-matrix.md`.
  - Expose read and write only for the settings the probe proves.
  - Writes are global. The host journals the original values to disk before changing anything and restores them at session end, and again on the next start after a crash.
  - Telemetry stays on its existing path, never per video packet.
- **A6 Nightjar console shell (Android, Views).** Data comes from `nightjar-core` over JNI.
  - Home: Continue playing, a library across paired hosts, and hosts.
  - Game detail: Play or Resume, host, streaming profile.
  - Navigation is controller-first, with predictable Back and focus restored on return.
  - Hosts without the capability get the classic app grid.
- **A7 Nightjar in-stream overlay.** Resume, Disconnect, End session, Close game, connection info, and the Radeon panel with its five states. Measure client frame-time p99 with the overlay open and closed: it must not get worse.

Gate: the alpha.1 gate again, plus Nightjar's CI. Tests cover old Nightjar against the new host and new Nightjar against the old host. The user runs the couch loop on their phone, and we read the host log afterwards. A crash test (kill the host mid-session) leaves no Radeon setting or display changed.

### alpha.3: timing record (measure only)

- **A8 Per-frame stage timestamps.**
  - Host: game present, capture, encode submit, encode complete, send.
  - Nightjar: the overlay already records client stages and exports CSV. Extend it to cover receive, decoder input, decoder output and present.
  - Join both sides with the 0x5537 clock offset into one per-session record. A `rust/release` script reports median, p95 and p99 for each stage.
  - Record baselines for H.264, HEVC, AV1 and PyroWave at the default settings. No behaviour change.

This milestone gives the beta milestones their numbers. It may ship together with alpha.2 if it is ready.

### Betas

Each item is a milestone with its own gate, and the order can change. A change that measures worse is reverted.

- **B1 Frame pacing and VRR.**
  - Host: frames follow the game's present cadence through the high-refresh virtual display.
  - Nightjar: a deadline-based present scheduler, a bounded decoder queue aiming for one frame of residency, `setFrameRate` matched to the content, and dropping late frames instead of queueing them.
  - Gate: on the user's phone, A8's present-interval jitter p99 and median picture age both improve over alpha.
- **B2 Incremental slice/tile delivery.**
  - Step 1 (host probe, one brief): turn on SLICE output for HEVC and TILE output for AV1 at 4K and record the time to the first and the last unit, against whole-frame output.
  - Kill if the driver still returns whole frames, or if the first unit arrives less than 1 ms before the full frame.
  - Step 2, only if step 1 passes: send each unit as it arrives, with FEC per unit. Nightjar feeds MediaCodec with partial-frame input.
  - Gate: median picture age improves by at least 1 ms, with no regression at 1% packet loss.
  - Track the four client levels separately: partial input, early decode start, partial output, and partial-frame presentation. Android is expected to give only the first.
- **B3 Visual fidelity and profiles.**
  - Add three named presets over the existing settings: Lowest latency, Balanced and Reference quality. Manual overrides stay.
  - Packed 4:4:4 encode (host) and reconstruction (Nightjar) both exist. Make it the Reference-quality default only after measurements on text and UI fixtures.
  - Check HDR10 end to end on an HDR phone.
  - AMF knobs such as pre-analysis, QP maps and queue depth change only with `amf_sweep` evidence.
- **B4 PyroWave 3.0.** Fix the 4:4:4 PSNR failure. Evaluate async encode. Add a real device-loss test. Rerun 144 fps pacing (reverted in 2.5.9) with A8 data. Cover HDR and chroma.
- **B5 Library and per-game profiles.**
  - Favourites, recently played, and an artwork cache that works offline.
  - A per-game streaming profile and Radeon profile: Competitive, Smoothness, Quality or Custom.
  - Launch handling for games that are already running and for launcher prompts.
  - Microsoft Store and Xbox games where they can be detected. No DRM or login bypass.
- **B6 Console UX completion.** Settings, host management and pairing follow the new shell. A TV and 10-foot layout, UI scaling, contrast, reduced motion, and German and English strings.
- **B7 Frame generation.** Measure whether IddCx push and WGC capture see AFMF-generated frames, and what that does to latency. Expose it as a Radeon control only with a measured latency note.
- **B8 Rust engine as the Android default.** Close the gaps in `PARITY-MATRIX.md`. Then compare picture age, decode time, loss recovery and a 30-minute soak against the C engine on the user's phone. Flip the default when it is no worse. Keep the C engine one release as a fallback setting, then delete it.
- **B9 Nightjar desktop couch loop.** Home, library, Radeon panel, overlay and session actions on Windows (D3D11) and Linux, all through `nightjar-core`. Coordinate with the desktop work already in flight: pull first and claim files in the brief.

### rc and 3.0.0

- `docs/hardware-matrix.md` labels every feature Tested, Expected, Experimental or Unsupported. Tested means it ran on our RX 7900 XT with Windows 26200 and a named phone. RDNA 2 and RDNA 4 stay "Expected" unless someone runs them.
- A two-hour soak, upgrade from 2.6.x, uninstall, and recovery after reinstalling the virtual display driver.
- README, site and docs updated. Credits and fork history stay intact.

## 3.x tracks (not in 3.0)

Each track starts with a spike and needs the user's go-ahead.

- **Linux host.** Needs an AMD Linux test machine. Spike: PipeWire/DMA-BUF or gamescope capture plus VA-API HEVC on Mesa, measured against the Windows path.
- **Monorepo.** Only if bumping the `butterpollo-core` pin turns into real friction.
- **Nightjar desktop out of preview,** if it didn't pass the B9 gate in time.
- **Smaller tracks:** multi-client policy beyond one owned session, Wake-on-LAN, NAT traversal, battery and thermal adaptation.

## Risks

| Risk | Mitigation |
|---|---|
| The 2.5 port conflicts with 96 newer commits (console, library, chroma) | Port phase by phase, run the gate after each phase, keep main's newer work |
| D3D12-only and virtual-display-only remove fallbacks some users rely on | Say so first in the alpha notes; refusal and error messages name the cause |
| We have one test GPU | RDNA 2 and RDNA 4 stay "Expected"; the matrix records exactly what ran |
| ADLX writes are global and depend on the driver version | The probe gates each setting; journal and restore; crash test in the alpha.2 gate |
| Codex works in parallel on both repos (Nightjar desktop above all) | Pull and check `origin/main` before every slice; briefs name the files they own |
| Android runs two engines until B8 lands | New client features go into `nightjar-core` only; the C engine gets fixes only |
| AMD encoder hangs (exit code 3) | Stop and investigate, never retry (CLAUDE.md) |
| Usage budget | One brief per slice, sized to about 100 turns; Sonnet implements, Opus reviews, Haiku runs gates; one session per milestone |

## Not carried over from the draft

- The role and persona section and the "never stop" directive. `CLAUDE.md` and the global rules decide how agents work and when they stop.
- Eight separate project records. This file, `rust/PERFORMANCE.md`, `rust/RELEASE_NOTES.md`, `floor.json` and `docs/hardware-matrix.md` cover them, and git history is the removed-code inventory.
- Milestone 0 as an open-ended audit. The audit is the table above.
- A monorepo, a UI-framework evaluation, and Linux hosts in 3.0. See decisions 7, 8 and 11.

## Status

| Slice | State | Notes |
|---|---|---|
| A0 release tooling | done | `bump.py` accepts alpha/beta and labels them with the full version; the elevated task accepts them; CI tag trigger covers `v3.*` |
| A1 port 2.5 (7 phases) | pending | |
| A2 floor experience | pending | |
| A3 native API v1 | pending | |
| A4 session state machine | pending | |
| A5 Radeon Control Center v1 | pending | |
| A6 Nightjar console shell | pending | |
| A7 Nightjar overlay | pending | |
| A8 timing record | pending | |
| B1–B9 | not started | |
