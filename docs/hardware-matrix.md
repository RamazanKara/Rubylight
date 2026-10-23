# Hardware matrix

What we have run Rubylight 3.0 on, and what we expect to work. Every cell has one label. It is a promise about our evidence, not about your PC.

| Label | Meaning |
|---|---|
| **Tested** | It ran on that hardware, on a Windows build and (where a client is involved) a client named in the row, and the evidence is linked. |
| **Expected** | The code path is supported and should work, but we have not run it there. Other AMD generations stay Expected until someone runs them. |
| **Experimental** | Opt-in or preview, and not promised. "ran" means we ran it on that hardware, "not run" means we did not. |
| **Unsupported** | Not offered in 3.0, or the hardware cannot do it. |

Our hardware: an RX 7900 XT (RDNA 3) with a Ryzen 7 5800X3D on Windows 11 25H2, build 26200 (the Radeon probes name 26200.9550); a Radeon 780M laptop (RDNA 3 iGPU), mostly as a client; a Retroid Pocket 6 phone; and an Intel NUC on Debian as the Linux test client. Everything else is Expected at best. Evidence is in [PERFORMANCE.md](../rust/PERFORMANCE.md), the [release notes](../rust/RELEASE_NOTES.md) and the [3.0 plan](rubylight-3.0-plan.md). A 3.0 build in the "Build and date" column means the run was on the 3.0 line. An older build means the code path was not re-run on 3.0.

Written 2026-10-23 against main `79b6c3131`.

## 1. GPU generations

Rubylight 3.0 accepts any AMD Radeon with a working AMF video encoder. There is no generation floor. NVIDIA and Intel stay on [2.6.x](https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0). The host asks AMF for its codecs at startup and offers only what the driver reports, so the codec column below says what we measured, not what a generation should have.

| Generation | Label | Codecs AMF exposes | Evidence |
|---|---|---|---|
| GCN, Polaris, Vega (Radeon HD 7000 to RX 500, Vega, Radeon VII) | Expected | Not recorded. [Features](features.md) says H.264 and HEVC work on all supported GPUs and AV1 where the encoder has it; we have not checked that on these. | None. |
| RDNA 1 (RX 5000) | Expected | Not recorded. | None. |
| RDNA 2 (RX 6000) | Expected | Not recorded. | Users report daily use on RX 6800 XT and 6900 XT ([README](../README.md#runs-on-your-hardware)). Those are reports, not our runs. |
| RDNA 3 desktop (RX 7000) | Tested on the RX 7900 XT | H.264, HEVC (8 and 10-bit, SDR and HDR) and AV1, all streamed; 4:2:0 only. | [alpha.1 gate](../rust/PERFORMANCE.md#october-22-30-alpha1-hardware-gate), [AMF 4:4:4 probe](../rust/PERFORMANCE.md#october-7-444-from-amf), [`amf-unit-probe-7900xt.json`](../rust/release/fixtures/amf-unit-probe-7900xt.json). Driver 32.0.32015.2008, AMD Software 26.9.2, Windows 11 26200. |
| RDNA 3 iGPU (Radeon 780M, 890M) | Expected | The 780M ran packed 4:4:4 in HEVC and AV1 (see section 3). Nothing else is recorded. | [2.5.12 notes](../rust/RELEASE_NOTES.md#new-in-2512). The Windows build is not named there. 890M handhelds are user reports only. |
| RDNA 4 (RX 9000) | Expected | Not recorded. AMD's maintainers say its encoders are 4:2:0 only, 8 and 10-bit ([PERFORMANCE.md](../rust/PERFORMANCE.md#october-7-444-from-amf)); we did not test it. | Users report daily use on the 9070 and 9070 XT. [rc.14](../rust/RELEASE_NOTES.md#new-in-rc14) fixed an RDNA 4 driver freeze that a forced one-frame encoder queue exposed. We have no RDNA 4 card. |

## 2. Windows

| Windows | Label | Evidence |
|---|---|---|
| Windows 11 25H2, build 26200 | Tested | Every 3.0 gate: [alpha.1](../rust/PERFORMANCE.md#october-22-30-alpha1-hardware-gate), [alpha.2](../rust/PERFORMANCE.md#october-22-30-alpha2-gate), and the Radeon probes on 26200.9550 (section 5). |
| Windows 11, other builds | Expected | No run. The 2.5 branch's build 26200 floor was reverted on 2026-10-22 ([plan](rubylight-3.0-plan.md), decision 1a). |
| Windows 10 | Expected | No run on 3.0. Its code paths were put back in the AMD-only port ([plan](rubylight-3.0-plan.md#status), A1). The per-client virtual display is off by default there ([PERFORMANCE.md](../rust/PERFORMANCE.md#october-14-frame-rates-above-60)), so Windows 10 streams the physical display unless you turn it on. |

## 3. Features

Columns: **RX 7900 XT** is Windows 11 26200 with the driver named in the evidence. **780M** and **Other AMD** are hosts. When the 780M was only the client, its cell stays as the host label.

| Feature | RX 7900 XT | 780M | Other AMD | Evidence | Build and date |
|---|---|---|---|---|---|
| **Capture and displays** | | | | | |
| Windows Graphics Capture, physical display | Tested | Expected | Expected | [alpha.1 gate](../rust/PERFORMANCE.md#october-22-30-alpha1-hardware-gate): HEVC and AV1 1440p120, 4 runs per case against 2.6.0; 30-minute 1080p60 soak | 3.0 `a14f1932b`, 2026-10-22 |
| Desktop Duplication | Tested | Expected | Expected | Same gate, HEVC and AV1 1440p120 | 3.0 `a14f1932b`, 2026-10-22 |
| Physical displays (stream the PC's own monitor) | Tested | Expected | Expected | Same gate, 5120x1440 at 240 Hz source; [alpha.2](../rust/PERFORMANCE.md#october-22-30-alpha2-gate): the physical display's mode and position were unchanged after a killed virtual-display stream | 3.0 `a14f1932b`, `b758a8044`, 2026-10-22 |
| Virtual display driver (per-client display, direct push) | Tested | Expected | Expected | Same gate: driver 0.2.0 installed over 2.6.0 with no restart, 8 of 8 streams passed; alpha.2 crash test | 3.0 `a14f1932b`, 2026-10-22 |
| Keyed-mutex ring | Tested | Expected | Expected | [Fence ring under GPU load](../rust/PERFORMANCE.md#october-22-fence-ring-under-gpu-load), 4 runs per transport and range | 3.0 main, driver 2.6.0.7064, 2026-10-22 |
| Fence ring (the default) | Tested | Expected | Expected | Same section: no worse on any metric; became the default | 3.0 main, driver 2.6.0.7064, 2026-10-22 |
| HDR on a virtual display (HEVC, AV1) | Tested | Expected | Expected | Fence-ring runs above (SDR and HDR, 3.0); [native HDR streams and pixels](../rust/PERFORMANCE.md#native-virtual-hdr-results-october-12), 28 of 28 and 4 of 4 | 3.0 main 2026-10-22; 2.2.9 2026-10-12, Windows 11 Pro 26200 |
| HDR on a physical HDR monitor | Expected | Expected | Expected | None. The HDR runs used a virtual HDR source, physical HDR stayed off, and nothing was calibrated against a panel ([Oct 12](../rust/PERFORMANCE.md#october-12-rubylight-229-released-package-refresh)) | - |
| **Codecs** | | | | | |
| H.264 | Tested | Expected | Expected | [Stream presets](../rust/PERFORMANCE.md#october-23-stream-presets-b3): H.264 1080p60 20 Mbps, 7 runs; [timing record](../rust/PERFORMANCE.md#october-23-per-frame-timing-record-baselines) | 3.0 `10aa9f0be`, 2026-10-23 |
| HEVC | Tested | Expected | Expected | Alpha.1 gate; stream presets (4K60 80 Mbps); timing record | 3.0 `a14f1932b`, 2026-10-22 and `10aa9f0be`, 2026-10-23 |
| AV1 | Tested | Expected | Expected (only where the encoder has AV1) | Alpha.1 gate (1440p120); timing record | 3.0 `a14f1932b`, 2026-10-22 |
| PyroWave | Tested | Expected | Expected | Timing record (PyroWave 1080p60); [PyroWave 144 fps and async encode](../rust/PERFORMANCE.md#october-23-pyrowave-144-fps-pacing-and-async-encode-b4); [device loss tests](../rust/RELEASE_NOTES.md#new-in-300-beta1) | 3.0 alpha.3 to beta.1, 2026-10-23 |
| **Chroma** | | | | | |
| PyroWave 4:4:4 (SDR and HDR) | Tested | Expected | Expected | [Decoded end to end](../rust/PERFORMANCE.md#october-7-pyrowave-decoded-end-to-end); the 3.0 [B4 row](rubylight-3.0-plan.md#status) records every case passing at the 2 Gbps control rate | 3.0 main, 2026-10-23 |
| Packed 4:4:4 over HEVC and AV1 (off by default) | Experimental, not run | Experimental, ran | Experimental, not run | [2.5.12 notes](../rust/RELEASE_NOTES.md#new-in-2512): all four packed modes at about 1440p120 for 10 s each on the 780M, host p95 at most 3.58 ms above matched 4:2:0. Bounded synthetic runs, no sustained end-to-end result | 2.5.12, Windows build not recorded |
| 4:4:4 straight from AMF (H.264, HEVC, AV1) | Unsupported | Unsupported | Unsupported | [4:4:4 from AMF](../rust/PERFORMANCE.md#october-7-444-from-amf): the 7900 XT encodes 4:2:0 only and rejects 4:4:4 input. The other two cells rest on AMD's statement for VCN 5.0 and below, not on our runs | rc era, driver 32.0.31041.1004, 2026-10-07 |
| **Delivery and rate** | | | | | |
| Slice delivery, H.264 and HEVC (off by default) | Experimental, ran | Experimental, not run | Experimental, not run | [Slice delivery 2f](../rust/PERFORMANCE.md#october-23-slice-delivery-b2-step-2f): 4K60 loopback, real Wi-Fi 6E to the 780M laptop as client, Retroid Pocket 6 at 1080p. Misses its 1 ms gate on the phone and in the Wi-Fi emulation | 3.0 `36c3c8ac3`, 2026-10-23 |
| Slice delivery, AV1 and PyroWave | Unsupported | Unsupported | Unsupported | Always sent whole ([beta.1 notes](../rust/RELEASE_NOTES.md#new-in-300-beta1)); AV1 tiles missed the bitrate in the host probe ([`amf-unit-probe-7900xt.json`](../rust/release/fixtures/amf-unit-probe-7900xt.json)) | 3.0 beta.1 |
| Host bitrate adaptation (`host_adaptive_bitrate`, opt-in) | Experimental, ran | Experimental, not run | Experimental, not run | [Link speeds, with the option on](../rust/PERFORMANCE.md#with-host_adaptive_bitrate--true): no collapse above the link, but it cuts too far and does not come back. NUC client, not a phone | 3.0 beta.1, 2026-10-23 |
| Link rate adaptation (`link_rate_adaptation`) | Experimental, not run | Experimental, not run | Experimental, not run | Only the problem is measured ([link speeds](../rust/PERFORMANCE.md#october-23-behaviour-across-client-link-speeds)). Wired into the host on 3.0 main `de0a4cd33`; only smoke runs so far, the acceptance run on the link-speed harness is still to do ([plan](rubylight-3.0-plan.md#status)) | - |
| Loss recovery (reference invalidation, FEC) | Tested | Expected | Expected | Slice-delivery runs at 1% and 5% loss; [Android engine runs](../rust/PERFORMANCE.md#october-23-rust-engine-against-the-c-engine-on-android-b8) at 1% and 5% send loss | 3.0 `36c3c8ac3`, 2026-10-23 |
| Frame rates above 60, and VRR | Tested | Expected | Expected | [Frame rates above 60](../rust/PERFORMANCE.md#october-14-frame-rates-above-60): 1440p at 60 to 240 fps; [116 fps VRR capture](../rust/PERFORMANCE.md#october-7-116-fps-vrr-capture-wgc-and-ddx); alpha.1 gate at 120 fps | 2.3.0, 2026-10-14, Windows 26200; 3.0 `a14f1932b` at 120 fps |
| **Control** | | | | | |
| Radeon control (read, change, restore) | Tested | Expected | Expected | [Radeon 3D settings](#5-radeon-3d-settings-adlx): all 24 configurable settings proven by round trip; [alpha.2 gate](../rust/PERFORMANCE.md#october-22-30-alpha2-gate): Anti-Lag from a phone, crash restore. Other GPUs and drivers: the host asks ADLX for each setting and offers only what it proves | 3.0 alpha.2, AMD Software 26.9.2, Windows 11 25H2 26200.9550, 2026-10-22 |
| Stream presets | Tested | Expected | Expected | [Stream presets](../rust/PERFORMANCE.md#october-23-stream-presets-b3): 13 loopback runs read back the encoder settings each preset sets. No phone run | 3.0 `10aa9f0be`, 2026-10-23 |
| **Audio and input** | | | | | |
| Audio (stereo) | Tested | Expected | Expected | Alpha.1 gate streams include the audio tone check; [audio continuity under CPU load](../rust/PERFORMANCE.md#october-7-release-audio-continuity-under-cpu-load) | 3.0 `a14f1932b`, 2026-10-22; rc.20, 2026-10-07 |
| Surround, microphone, game-only audio | Expected | Expected | Expected | None recorded | - |
| Keyboard and mouse | Tested | Expected | Expected | [Input on the control thread](../rust/PERFORMANCE.md#october-7-input-on-the-control-thread), Windows 11 26200 | rc era, 2026-10-07 |
| Virtual DualShock 4 and DualSense pads | Tested | Expected | Expected | [Native controller checks](../rust/PERFORMANCE.md#native-controller-and-moonlight-62-command-checks), signed driver 0.1.0.39. No network client or live pad | rc era, 2026-10-06 |
| Xbox and Switch Pro profiles, Steam Deck pads, DualSense USB haptics, back grips | Expected | Expected | Expected | None recorded | - |
| **Rendering paths** | | | | | |
| D3D12 compute path (the default) | Tested | Expected | Expected | Every 3.0 gate runs it; compute off against on, [1080p60](../rust/PERFORMANCE.md#1080p-at-60-fps) | 3.0 `a14f1932b`, 2026-10-22 |
| D3D11 path (`gpu_compute_conversion = false`) | Tested | Expected | Expected | [Single-pass D3D11 conversion](../rust/PERFORMANCE.md#october-12-single-pass-d3d11-420-conversion): 1080p60 HEVC SDR and HDR and PyroWave SDR streams, 9 GPU correctness tests | 2.2.8, 2026-10-12. Not re-run on 3.0 |

A D3D12-only host and virtual-display-only streaming are not offered. That work is parked on `claude/d3d12-virtual-only`, and any of it returns beside the existing paths, never instead of them.

## 4. Clients

| Client | Label | Where it ran | Evidence | Build and date |
|---|---|---|---|---|
| Nightjar Android, Rust engine (the default) | Tested | Retroid Pocket 6, Android 13, Wi-Fi 5 GHz | [Rust against C engine](../rust/PERFORMANCE.md#october-23-rust-engine-against-the-c-engine-on-android-b8): HEVC 1080p60 picture age median 6.92 ms, 30-minute soak steady | Nightjar main `6456da5d` and later, 2026-10-23 |
| Nightjar Android, classic (C) engine | Tested | Retroid Pocket 6 | Same section | Same |
| Nightjar Android, couch loop (Home, Play, Radeon panel, End session) | Tested | Retroid Pocket 6 and the Android emulator | [Alpha.2 gate](../rust/PERFORMANCE.md#october-22-30-alpha2-gate) | Nightjar 0.8.0, 3.0 alpha.2, 2026-10-22 |
| Nightjar Android, PyroWave | Tested | Retroid Pocket 6, 1080p60 at 400 Mbps | Same B8 section: end to end median 35.0 ms on Rust against 36.2 ms on C | Nightjar main, 2026-10-23 |
| Nightjar Android, on-screen keyboard | Tested | Retroid Pocket 6, key events read on the host | [B12 row](rubylight-3.0-plan.md#status) | Nightjar main `10f088b1`, 2026-10-23 |
| Nightjar Android, HDR on an HDR phone | Expected | None | The HDR10 end-to-end check is still open in the [plan](rubylight-3.0-plan.md#status), B3 | - |
| Nightjar Android, other phones, tablets and Android TV | Expected | None | The Retroid Pocket 6 is the only phone in our records. The Shield report ([#7](https://github.com/RamazanKara/nightjar/issues/7)) still needs the reporter's data | - |
| Nightjar desktop, Windows | Experimental | Radeon 780M laptop over LAN and Wi-Fi 6E | A 6-minute 1080p60 HEVC stream: end to end median 7.06 ms, p99 10.29 ms ([B9 row](rubylight-3.0-plan.md#status)); slice-delivery runs; [PyroWave decode stage split](../rust/PERFORMANCE.md#october-23-pyrowave-desktop-decode-stage-split-brief-1) on loopback | Nightjar main `60cf18bd`, 3.0 alpha.3 to beta.1, 2026-10-23 |
| Nightjar desktop, Linux | Experimental | The desktop app was not run. `nightjar-cli` ran on the NUC (Debian 13, 1 GbE, VAAPI decode) | [Link speeds](../rust/PERFORMANCE.md#october-23-behaviour-across-client-link-speeds) | 3.0 beta.1, 2026-10-23 |
| Moonlight 6.2.0 (stock), Windows | Tested | Radeon 780M laptop over Wi-Fi, 1080p60 HEVC SDR, two 165 s sessions | [Performance overview](performance.md#october-12-229-measurements) and [Oct 12](../rust/PERFORMANCE.md#october-12-single-pass-d3d11-420-conversion) | 2.2.8 and 2.2.9, 2026-10-12. Not re-run on 3.0 |
| moonlight-common-c client, Linux (stands in for stock Moonlight on another PC) | Tested | NUC on wired LAN: PIN pairing, then 60 s of HEVC 1080p60, every frame decoded | [Alpha.1 gate](../rust/PERFORMANCE.md#october-22-30-alpha1-hardware-gate) | 3.0 alpha.1, 2026-10-22 |
| Moonlight on phones (Android, iOS) and other stock clients | Expected | None | "Not tested yet: stock Moonlight on a phone" in the alpha.1, alpha.2 and alpha.3 notes | - |

## 5. Radeon 3D settings (ADLX)

What the A5 probes found on real hardware. Each section names the machine and driver it was measured on. Regenerate with the command under the section.

The ADLX 3D settings services (`rust/windows/src/radeon`), probed and then written on this PC. The probe (`radeon_probe`) reads every value and range ADLX offers and never calls a setter, so its "Writable" column comes from ADLX's own `IsSupported` and `GetRange` answers. The "Proven writable" column comes from A5 step 2: the ignored hardware test `prove_configurable_settings_round_trip` wrote a different valid value to every setting through the journaled write path, read it back, restored the original, and a probe afterwards matched the one before.

Regenerate the probe: `cargo run -p butterpollo-windows --example radeon_probe` (JSON) or `-- --markdown` (the tables below, without the proven column). Re-prove the writes (changes real settings briefly, host must be idle): `cargo test -p butterpollo-windows --lib radeon::hardware -- --ignored --nocapture --test-threads=1`.

| | |
|---|---|
| GPU | AMD Radeon RX 7900 XT (ADLX id 11520) |
| AMD Software | 26.9.2, AMD Software: Adrenalin Edition |
| Driver version | 32.0.32015.2008 (Adrenalin string `26.20.15.02-260921a1-204590C-AMD-Software-Adrenalin-Edition`) |
| ADLX runtime | 1.5.0.124 (`ADLXQueryFullVersion`); the host asks for 1.0.5.30, the headers are SDK 2.0.0.125 |
| 3D settings services | `IADLX3DSettingsServices3` (the newest of Services1 to 3) |
| Windows | 11 Pro 25H2, build 26200.9550 (the registry `ProductName` still says "Windows 10 Pro") |
| Probed | 2026-10-22 |

How to read the columns:

- **Supported**: ADLX `IsSupported` (or a successful interface query) for the setting.
- **Readable**: the getter returned a value.
- **Writable**: what ADLX offers. `yes` when it reports the setting supported with a usable range (min below max, step above 0), or it is a plain toggle or action. `no` when unsupported, the range is empty, or ADLX has no setter. Enums are `yes` only where a write has been proven (`PROVEN_CHOICES` in `model.rs`), because ADLX cannot say which options a driver accepts.
- **Proven writable**: the result of the round-trip write on this PC. `yes: wrote X, read back X, restored Y` is a measured round trip. "needs <group>.enabled on in the same request" means the driver refuses the value (ADLX_FAIL, or ADLX_NOT_ACTIVE for AFMF) while the feature is off, so a request must list the toggle first. `not written` and `no` carry their reason.
- **State**: the native-api state (`docs/native-api.md`). `configurable` is a setting whose write round-trip is proven (or a plain toggle, range or action ADLX offers). `read_only` is readable but not written: the probe could not prove a write, or writing it could not be undone. A failed `IsSupported` or a not-found style code becomes `unsupported`. Other failures become `temporarily_unavailable`. `needs_external_setup` is still not produced: ADLX gives no way to tell "off, turn it on in Adrenalin" from "off".
- Dependencies are the ones the ADLX headers state, with the other side's current value.

### AMD Radeon RX 7900 XT (id 11520)

| Setting | ADLX interface | Supported | Readable | Writable | Proven writable | State | Current value | Range or options | Notes |
|---|---|---|---|---|---|---|---|---|---|
| `anti_lag.enabled` | `IADLX3DAntiLag` | yes | yes | yes | yes: wrote on, read back on, restored off | configurable | off | - | excludes chill (off now) |
| `anti_lag.level` | `IADLX3DAntiLag1` | no | no | no | no: ADLX says unsupported | unsupported | - | anti_lag, anti_lag_next | Anti-Lag 2 is an in-game SDK integration and is not exposed by ADLX; this selects the driver's Anti-Lag or Anti-Lag Next; interface: ADLX_UNKNOWN_INTERFACE; excludes chill (off now) |
| `chill.enabled` | `IADLX3DChill` | yes | yes | yes | yes: wrote on, read back on, restored off | configurable | off | - | excludes anti_lag (off now); excludes boost (off now) |
| `chill.min_fps` | `IADLX3DChill` | yes | yes | yes | yes: wrote 76, read back 76, restored 75 (needs chill.enabled on in the same request) | configurable | 75 fps | 30..300 step 1 | the driver refuses writes while chill.enabled is off; list chill.enabled first in the same request; excludes anti_lag (off now); excludes boost (off now) |
| `chill.max_fps` | `IADLX3DChill` | yes | yes | yes | yes: wrote 141, read back 141, restored 140 (needs chill.enabled on in the same request) | configurable | 140 fps | 30..300 step 1 | the driver refuses writes while chill.enabled is off; list chill.enabled first in the same request; excludes anti_lag (off now); excludes boost (off now) |
| `boost.enabled` | `IADLX3DBoost` | yes | yes | yes | yes: wrote on, read back on, restored off | configurable | off | - | excludes chill (off now); excludes rsr (off now) |
| `boost.min_resolution` | `IADLX3DBoost` | yes | yes | yes | yes: wrote 84, read back 84, restored 67 (needs boost.enabled on in the same request) | configurable | 67 % | 50..84 step 17 | the driver refuses writes while boost.enabled is off; list boost.enabled first in the same request; excludes chill (off now); excludes rsr (off now) |
| `image_sharpening.enabled` | `IADLX3DImageSharpening` | yes | yes | yes | yes: wrote on, read back on, restored off | configurable | off | - | excludes rsr (off now) |
| `image_sharpening.sharpness` | `IADLX3DImageSharpening` | yes | yes | yes | yes: wrote 90, read back 90, restored 80 (needs image_sharpening.enabled on in the same request) | configurable | 80 % | 10..100 step 10 | the driver refuses writes while image_sharpening.enabled is off; list image_sharpening.enabled first in the same request; excludes rsr (off now) |
| `enhanced_sync.enabled` | `IADLX3DEnhancedSync` | yes | yes | yes | yes: wrote on, read back on, restored off (first run; the host now refuses it while vsync is on, see Caveats) | configurable | off | - | excludes vsync (off now) |
| `vsync.enabled` | `IADLX3DWaitForVerticalRefresh` | yes | yes | no | no: ADLX has no setter; it follows vsync.mode | read_only | on | - | derived from vsync.mode; ADLX has no setter for it; excludes enhanced_sync (off now) |
| `vsync.mode` | `IADLX3DWaitForVerticalRefresh` | yes | yes | yes | yes: wrote always_off, read back always_off, restored off_unless_app_specifies | configurable | off_unless_app_specifies | always_off, off_unless_app_specifies, on_unless_app_specifies, always_on | excludes enhanced_sync (off now) |
| `frame_rate_target.enabled` | `IADLX3DFrameRateTargetControl` | yes | yes | yes | yes: wrote on, read back on, restored off | configurable | off | - |  |
| `frame_rate_target.fps` | `IADLX3DFrameRateTargetControl` | yes | yes | yes | yes: wrote 61, read back 61, restored 60 (needs frame_rate_target.enabled on in the same request) | configurable | 60 fps | 15..1000 step 1 | the driver refuses writes while frame_rate_target.enabled is off; list frame_rate_target.enabled first in the same request |
| `afmf.enabled` | `IADLX3DAMDFluidMotionFrames` | yes | yes | yes | yes: wrote on, read back on, restored off | configurable | off | - |  |
| `afmf.algorithm` | `IADLX3DAMDFluidMotionFrames1` | no | no | no | no: ADLX says unsupported | unsupported | - | auto, enhanced, standard | IsSupportedAlgorithm failed: ADLX_NOT_SUPPORTED; read: ADLX_NOT_SUPPORTED |
| `afmf.search_mode` | `IADLX3DAMDFluidMotionFrames1` | yes | yes | yes | yes: wrote standard, read back standard, restored auto (needs afmf.enabled on in the same request) | configurable | auto | auto, standard, high | the driver refuses writes while afmf.enabled is off; list afmf.enabled first in the same request |
| `afmf.performance_mode` | `IADLX3DAMDFluidMotionFrames1` | yes | yes | yes | yes: wrote quality, read back quality, restored auto (needs afmf.enabled on in the same request) | configurable | auto | auto, quality, performance | the driver refuses writes while afmf.enabled is off; list afmf.enabled first in the same request |
| `afmf.fast_motion_response` | `IADLX3DAMDFluidMotionFrames1` | yes | yes | yes | yes: wrote blended_frames, read back blended_frames, restored repeat_frames (needs afmf.enabled on in the same request) | configurable | repeat_frames | repeat_frames, blended_frames | the driver refuses writes while afmf.enabled is off; list afmf.enabled first in the same request |
| `rsr.enabled` | `IADLX3DRadeonSuperResolution` | yes | yes | no | not written: turning RSR on makes ADLX switch GPU scaling on, a display setting the host cannot read or restore | read_only | off | - | not written: ADLX turns GPU scaling on with RSR, and GPU scaling is a display setting the host can neither read nor restore; excludes boost (off now); excludes image_sharpening (off now); requires gpu_scaling |
| `rsr.sharpness` | `IADLX3DRadeonSuperResolution` | yes | yes | yes | yes: wrote 76, read back 76, restored 75 | configurable | 75 % | 0..100 step 1 | excludes boost (off now); excludes image_sharpening (off now); requires gpu_scaling |
| `anti_aliasing.mode` | `IADLX3DAntiAliasing` | yes | yes | yes | yes: wrote enhance_app_settings, read back enhance_app_settings, restored use_app_settings | configurable | use_app_settings | use_app_settings, enhance_app_settings, override_app_settings |  |
| `anti_aliasing.level` | `IADLX3DAntiAliasing` | yes | yes | no | not written: reads "invalid" while the mode leaves AA to the app, and ADLX refuses to write that back, so it could not be restored | read_only | invalid | 2x, 2x_eq, 4x, 4x_eq, 8x, 8x_eq | not written: while the mode leaves AA to the application the level reads 'invalid', and ADLX refuses to write that back, so a change could not be restored |
| `anti_aliasing.method` | `IADLX3DAntiAliasing` | yes | yes | yes | yes: wrote adaptive_multisampling, read back adaptive_multisampling, restored multisampling | configurable | multisampling | multisampling, adaptive_multisampling, supersampling |  |
| `morphological_aa.enabled` | `IADLX3DMorphologicalAntiAliasing` | yes | yes | yes | yes: wrote on, read back on, restored off | configurable | off | - |  |
| `anisotropic_filtering.enabled` | `IADLX3DAnisotropicFiltering` | yes | yes | yes | yes: wrote on, read back on, restored off | configurable | off | - |  |
| `anisotropic_filtering.level` | `IADLX3DAnisotropicFiltering` | yes | yes | no | not written: reads "invalid" while filtering is left to the app, and ADLX refuses to write that back, so it could not be restored | read_only | invalid | 2x, 4x, 8x, 16x | not written: while filtering is left to the application the level reads 'invalid', and ADLX refuses to write that back, so a change could not be restored |
| `tessellation.mode` | `IADLX3DTessellation` | yes | yes | yes | yes: wrote use_app_settings, read back use_app_settings, restored amd_optimized | configurable | amd_optimized | amd_optimized, use_app_settings, override_app_settings |  |
| `tessellation.level` | `IADLX3DTessellation` | yes | yes | yes | yes: wrote off, read back off, restored 64x | configurable | 64x | off, 2x, 4x, 6x, 8x, 16x, 32x, 64x |  |
| `reset_shader_cache.run` | `IADLX3DResetShaderCache` | yes | n/a | yes | action, never written (destructive) | configurable | - | - | Action: nothing to read; the probe does not invoke it |
| `image_sharpen_desktop.enabled` | `IADLX3DImageSharpenDesktop` | no | no | no | no: ADLX says unsupported | unsupported | - | - | ADLX IsSupported is false; requires image_sharpening (off now) |
| `fidelityfx_super_resolution.enabled` | `IADLX3DFidelityFXSuperResolution` | yes | yes | yes | yes: wrote off, read back off, restored on | configurable | on | - |  |
| `fidelityfx_frame_gen.enabled` | `IADLX3DFidelityFXFrameGenUpgrade` | no | no | no | no: ADLX says unsupported | unsupported | - | - | ADLX IsSupported is false |
| `fidelityfx_frame_gen.ratio` | `IADLX3DFidelityFXFrameGenUpgrade` | no | no | no | no: ADLX says unsupported | unsupported | - | - | ADLX IsSupported is false |

Totals: 34 entries, 25 `configurable` (24 settings and the `reset_shader_cache.run` action), 4 `read_only`, 5 `unsupported`; none is `temporarily_unavailable` or `needs_external_setup`. All 24 `configurable` settings and the AFMF, vsync, AA and tessellation enums round-tripped; the action is never written.

### Final five-state mapping

| State | What puts a setting there | On this PC |
|---|---|---|
| `configurable` | ADLX reports it supported and writable, and a write was proven (or it is a toggle, range or action ADLX offers) | 24 settings plus the action |
| `read_only` | readable, but a write is not offered or could not be undone | `vsync.enabled` (no setter), `rsr.enabled` (GPU scaling side effect), `anti_aliasing.level` and `anisotropic_filtering.level` (read `invalid` while the app decides) |
| `needs_external_setup` | not produced: ADLX cannot tell "off, turn it on in Adrenalin" from "off". `rsr.enabled` requires GPU scaling, which ADLX does not expose, and is the first candidate when a display API reads it | none |
| `unsupported` | `IsSupported` false, or the interface is missing from this runtime | `anti_lag.level`, `afmf.algorithm`, `image_sharpen_desktop.enabled`, `fidelityfx_frame_gen.enabled`, `fidelityfx_frame_gen.ratio` |
| `temporarily_unavailable` | any other ADLX failure (a not-active or busy code) | none |

Write errors map to `invalid_value` (validated before anything is written), `not_writable`, `driver_rejected` (ADLX refused; this request's earlier writes are undone), `radeon_unavailable` and `journal_error`.

### What the write proof found

- Ranged and enum settings of a feature that is off (`chill.*_fps`, `boost.min_resolution`, `image_sharpening.sharpness`, `frame_rate_target.fps`, `afmf.search_mode`, `afmf.performance_mode`, `afmf.fast_motion_response`) are refused alone (ADLX_FAIL, or ADLX_NOT_ACTIVE for AFMF) and accepted when `<group>.enabled` is on in the same request. The write path does not add the toggle itself: the request has to list it.
- A write of the value a setting already holds returns ADLX_ALREADY_ENABLED. That is treated as success.
- `rsr.enabled` and `anti_aliasing.level` and `anisotropic_filtering.level` stay unwritten (reasons in the table). `rsr.sharpness` round-tripped on its own.
- Turning a feature on can switch its exclusive partners off (`chill` and `anti_lag`, `boost` and `rsr`, and so on). The journal records those driver side effects as separate entries and restore puts them back.
- A fresh process sees a write about 130 ms after it lands. A long-lived process never sees another process's write (see "ADLX view refresh"), so the host reads and writes through short-lived helper processes.
- ADLX is initialised once per process and never terminated: calling `ADLXTerminate` per session crashed the process with an access violation within a few hundred cycles. A later `ADLXInitialize` returns ALREADY_INITIALIZED, which is accepted.

### ADLX view refresh

Measured on the RX 7900 XT (AMD Software 26.9.2, `measure_view_refresh`, ignored, hardware). A child process wrote `morphological_aa.enabled`; the parent, which had read it before, tried to see the new value:

| Attempt in the long-lived process | Result |
| --- | --- |
| A new ADLX session (re-query the system services) | still the old value after 12 s |
| `ADLXInitializeWithIncompatibleDriver` again | returns 2 (already initialised); still the old value after 12 s |
| Waiting | still the old value after 12 s |
| `ADLXTerminate` and initialise again | crashes the process (0xC0000005); not an option |
| A new process | sees the write in about 130 ms |

A write of the value the stale process believes is current is dropped. The earlier "about 2 s lag" was the parent's stale view, not driver latency. So nothing in-process refreshes the view, and the host does the guard reads, original reads and writes in a short-lived helper process (`butterpollo.exe --radeon-helper`, hidden; one JSON request on stdin, one `RADEON-HELPER-RESPONSE` line on stdout). Startup recovery also writes the original back for entries the guard calls already-original, and the old fixed 3 s wait is gone. It runs on a dedicated worker thread with a time limit, so it cannot block host startup; later writes and session restores wait for it.

### Write and restore through the worker (A3 brief 4)

`worker_writes_and_restores_anti_lag` (ignored, hardware; host idle): starts the worker with the helper-process backend and a scratch journal, waits for recovery to end, reads the panel (`panel`), writes `anti_lag.enabled` through `apply` for a made-up session, reads it back from a fresh helper process, then `restore_session`s it and reads again. Run on this PC, 2026-10-22: AMD Radeon RX 7900 XT (id 11520), driver 26.10.1, `anti_lag.enabled` off, written on, read back on, restored to off. This is the path the native API endpoints use; the endpoints themselves are tested against a fake backend only, so no HTTP request touched the GPU. Command: `cargo test -p butterpollo-windows --lib radeon::hardware::worker_writes_and_restores_anti_lag -- --ignored --nocapture --test-threads=1`.

### Crash test

`crash_then_recover` (ignored, hardware): a child process writes `morphological_aa.enabled` through the journal and dies without restoring, once with `exit(3)` and once with `abort()`. A fresh process then runs recovery inside the window where a long-lived process still sees the stale value (recovery took 612 to 636 ms); it restored the setting ("Restored", journal empty), and new processes read the original from then on. Fake-backend unit tests cover the same recovery, a stale-read case, and restoring a field whose feature is off (the group is switched on temporarily, journaled first, the field restored, then the toggle). Both hardware tests ended with an empty probe diff from a fresh process (34 settings compared).

### Which account

The installed host runs as SYSTEM in the console session; a development host runs as the signed-in user. Every proof above first ran as the user, so the account was tested separately (`cross_account_server` as SYSTEM in the console session, started the way the service starts the host, and `cross_account_round_trip` as the user; both ignored, hardware).

- A SYSTEM helper process does not see the user's values of the three AFMF mode fields. After the user (or SYSTEM) wrote `afmf.search_mode = standard` with AFMF on, a fresh SYSTEM helper kept reading `auto` for 3 s and more, while a fresh user helper read `standard`; a SYSTEM write of `auto` was then dropped (the user still read `standard`), as with the stale view in "ADLX view refresh". `afmf.enabled` and the other groups tried before the run stopped (Anti-Lag, Chill, Boost, Image Sharpening, vsync, frame rate target) read the same from both accounts. The other groups were not measured as an un-impersonated SYSTEM.
- So the helper runs as the signed-in user: a SYSTEM host starts it with the console user's primary token (`WTSQueryUserToken`, `CreateProcessAsUserW`, request on stdin, answer on stdout and stderr pipes, inside the usual kill-on-close job), so nothing in it ever runs as SYSTEM, and it answers with an error if it finds itself SYSTEM anyway. It does not impersonate: a SYSTEM process that loads the driver's DLLs while impersonating a user resolves their paths through that user's device map, which a standard user can redirect, so the DLLs would load into a SYSTEM process. The settings are the user's, not global. An elevated administrator host that is not SYSTEM already is the user, and starts the helper as itself. With nobody signed in the host cannot start the helper and says so.
- Proof with a SYSTEM peer in the console session answering helper requests: `cross_account_round_trip` wrote every configurable setting that is not refused (36 writes in both directions) once through the SYSTEM peer and read it back as the user, then the other way round. All writes read back identically and every full read agreed. Each of the 390 helpers the peer started logged its own token: all ran as the user's SID, none as SYSTEM (the test asserts it). All restored, final probe diff empty (34 settings). The earlier run with impersonation counted 46 writes; the AMD driver was updated between the runs (26.20.15.02 to 26.20.15.07) and the set of writable settings changed with it.
- `crash_then_recover` with a SYSTEM parent (both children SYSTEM, console session) restored the write after `exit(3)` and after `abort()` (recovery 547 ms and 537 ms), and again as the user. The recovery of an `afmf.*` mode field was not exercised that way: it needs its feature switched on in the same request.
- With nobody signed in (the login screen) the helper fails loudly, so startup recovery waits and retries every minute until a user is there.
- The journal records the owner's SID with every entry. Recovery and restore only touch entries of the user who is signed in now and log the others loudly ("belong to another user") instead of writing one user's values into another user's settings; those stay until their user returns. An entry written by an older build has no SID and is left the same way.

### Mismatches with the spec's ids (`docs/native-api.md`)

- The spec's id list is covered, with extras: `vsync.enabled` (a read-only mirror of `vsync.mode`), `anti_lag.level`, `afmf.*` beyond `enabled`, `chill.min_fps`, `anti_aliasing.*`, `morphological_aa.enabled`, `anisotropic_filtering.*`, `tessellation.*`, `reset_shader_cache.run` and the FidelityFX set. Clients already render unknown ids generically.
- `vsync.mode` is the writable id (`configurable`; `always_off`, `off_unless_app_specifies`, `on_unless_app_specifies`, `always_on` all valid options). `vsync.enabled` has no ADLX setter and is `read_only`; clients should write `vsync.mode`.
- The spec example shows `afmf.enabled` as `needs_external_setup` and `rsr.enabled` as `unsupported`. On this GPU `afmf.enabled` is `configurable` and its write is proven. `rsr.enabled` is `read_only`, not `unsupported`: it reads fine but is deliberately not written (GPU scaling). `rsr.sharpness` is `configurable`. The spec text should say so.
- `anti_lag.level` is `unsupported`: the runtime (1.5.0.124) predates `IADLX3DAntiLag1`, so Anti-Lag Next cannot be selected through ADLX. Anti-Lag 2 is an in-game SDK feature and ADLX does not expose it at all.
- `afmf.algorithm` is `unsupported` on this driver; the other three AFMF options are present and proven.
- The spec example shows `driver: "25.9.1"`; the probe reports `26.9.2` (AMD Software) next to the Windows driver version `32.0.32015.2008`. Report the AMD Software version.

### Caveats

- "Proven" is one round trip per setting on one GPU and one driver, with one alternative value (the next valid one). Other values in a range, other GPUs, older drivers and the 1.x ADLX runtimes will differ: re-run the hardware tests there before relying on a row.
- Dependency notes come from sentences in the ADLX headers and from the write proof. ADLX does not expose a dependency query, so the list is not exhaustive.
- Nothing in a normal stream writes these settings yet; the write, journal and restore core is exercised only by the tests (session wiring is later work).
- AFMF on the Radeon 780M (driver 26.10.1): `afmf.enabled` wrote, read back and restored through the journaled controller, but a borderless D3D11 flip-model test app showed no sign of frame generation (it kept its own pacing at 119.7 fps on a 120 Hz panel) and WGC and Desktop Duplication captured no generated picture. Whether AFMF output reaches a capture is therefore unresolved, and no AFMF latency is claimed; see [PERFORMANCE.md](../rust/PERFORMANCE.md#october-23-420-against-packed-444-on-text-and-ui-and-afmf-capture-b3-b7-measured-on-the-780m).
- Telemetry and the control layer share one ADLX owner (`adlx_system.rs`): one initialisation, one lock, never terminated.
- Not measured: whether enabling Enhanced Sync while `vsync.enabled` is on flips vsync. Until a hardware run measures it, the write is refused (`invalid_value`, naming vsync; turn `vsync.mode` to `always_off` first), so `enhanced_sync.enabled` is no longer round-tripped by the proof run. `vsync.mode` has no pre-write journal entry because the value after a flip is unknown, so only the post-write diff covers it.
- The journal's mutex (`Global\Butterpollo.RadeonJournal.*`) is created with a descriptor that gives SYSTEM, Administrators and its creator only SYNCHRONIZE and MUTEX_MODIFY_STATE (wait and release); a process of another account cannot take it. A journal owner that started before the last boot counts as dead without opening its process, so a reused pid cannot keep a stale journal from being recovered.
- A predicted partner (a setting the driver is expected to switch when another is written) stays in the journal even when the read-back shows it unchanged; restore then reports it as already original instead of forgetting it.
- A helper that hangs inside the driver is killed, and abandoned if it still does not exit 3 s later. While an abandoned helper is alive no new one starts (the request fails with "previous helper still hung"), so a hung driver cannot pile up processes or race the next write. `Worker::apply` and `restore_session` give up after 150 s (an apply that has not started by then is dropped, so it cannot land after its caller was told it failed). Each write costs about 600 ms (helper start, read, write, verify), so applying must stay off the stream-start path.
- Telemetry keeps its ADLX system handle across a driver reset (the shared owner never re-initialises); settings IO is unaffected because it runs in fresh helper processes.
