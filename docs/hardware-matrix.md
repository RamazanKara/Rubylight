# Hardware matrix

What the A5 probes found on real hardware. Each section names the machine and driver it was measured on. Regenerate with the command under the section.

## Radeon 3D settings (ADLX)

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
| `enhanced_sync.enabled` | `IADLX3DEnhancedSync` | yes | yes | yes | yes: wrote on, read back on, restored off | configurable | off | - | excludes vsync (off now) |
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
- Another process sees a write about 2 s after it lands. A write made just before a process dies can also arrive late, so recovery waits 3 s and checks again before it believes a setting is already back to the original. A process's own view can stay behind for a long time after another process wrote, so checks across processes use fresh processes.
- ADLX is initialised once per process and never terminated: calling `ADLXTerminate` per session crashed the process with an access violation within a few hundred cycles. A later `ADLXInitialize` returns ALREADY_INITIALIZED, which is accepted.

### Crash test

`crash_then_recover` (ignored, hardware): a child process writes `morphological_aa.enabled` through the journal and dies without restoring, once with `exit(3)` and once with `abort()`. A fresh process then runs `recover_on_startup`, which restored the setting ("Restored", journal empty), and new processes read the original from then on, still the same 5 s later. A fake-backend unit test covers the same recovery and a stale-read case.

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
- Telemetry still has its own ADLX init and terminate. If it terminates mid-call while the control layer is reading or writing, ADLX can fault; the control layer keeps ADLX initialised for the process lifetime.
