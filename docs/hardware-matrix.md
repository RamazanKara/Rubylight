# Hardware matrix

What the A5 probes found on real hardware. Each section names the machine and driver it was measured on. Regenerate with the command under the section.

## Radeon 3D settings (ADLX)

Read-only probe of the ADLX 3D settings services (`rust/windows/src/radeon`). It reads every value and range ADLX offers and never calls a setter, so "writable" comes from ADLX's own `IsSupported` and `GetRange` answers, not from a write.

Regenerate: `cargo run -p butterpollo-windows --example radeon_probe` (JSON) or `-- --markdown` (the tables below).

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
- **Writable**: `yes` when ADLX reports the setting supported with a usable range (min below max, step above 0), or it is a plain toggle or action. `unverified` when the setting is an enum: ADLX has no query for which options a driver accepts, so a write could still be refused. `no` when unsupported or the range is empty.
- **State**: the native-api state (`docs/native-api.md`). `yes` becomes `configurable`. `unverified` becomes `read_only` until a write is proven (A5 step 2). A failed `IsSupported` or a not-found style code becomes `unsupported`. Other failures become `temporarily_unavailable`. `needs_external_setup` is not produced yet: ADLX gives no way to tell "off, turn it on in Adrenalin" from "off".
- Dependencies are the ones the ADLX headers state, with the other side's current value.

### AMD Radeon RX 7900 XT (id 11520)

| Setting | ADLX interface | Supported | Readable | Writable | State | Current value | Range or options | Notes |
|---|---|---|---|---|---|---|---|---|
| `anti_lag.enabled` | `IADLX3DAntiLag` | yes | yes | yes | configurable | off | - | excludes chill (off now) |
| `anti_lag.level` | `IADLX3DAntiLag1` | no | no | no | unsupported | - | anti_lag, anti_lag_next | Anti-Lag 2 is an in-game SDK integration and is not exposed by ADLX; this selects the driver's Anti-Lag or Anti-Lag Next; interface: ADLX_UNKNOWN_INTERFACE; excludes chill (off now) |
| `chill.enabled` | `IADLX3DChill` | yes | yes | yes | configurable | off | - | excludes anti_lag (off now); excludes boost (off now) |
| `chill.min_fps` | `IADLX3DChill` | yes | yes | yes | configurable | 75 fps | 30..300 step 1 | excludes anti_lag (off now); excludes boost (off now) |
| `chill.max_fps` | `IADLX3DChill` | yes | yes | yes | configurable | 140 fps | 30..300 step 1 | excludes anti_lag (off now); excludes boost (off now) |
| `boost.enabled` | `IADLX3DBoost` | yes | yes | yes | configurable | off | - | excludes chill (off now); excludes rsr (off now) |
| `boost.min_resolution` | `IADLX3DBoost` | yes | yes | yes | configurable | 67 % | 50..84 step 17 | excludes chill (off now); excludes rsr (off now) |
| `image_sharpening.enabled` | `IADLX3DImageSharpening` | yes | yes | yes | configurable | off | - | excludes rsr (off now) |
| `image_sharpening.sharpness` | `IADLX3DImageSharpening` | yes | yes | yes | configurable | 80 % | 10..100 step 10 | excludes rsr (off now) |
| `enhanced_sync.enabled` | `IADLX3DEnhancedSync` | yes | yes | yes | configurable | off | - | excludes vsync (off now) |
| `vsync.enabled` | `IADLX3DWaitForVerticalRefresh` | yes | yes | yes | configurable | on | - | excludes enhanced_sync (off now) |
| `vsync.mode` | `IADLX3DWaitForVerticalRefresh` | yes | yes | unverified | read_only | off_unless_app_specifies | always_off, off_unless_app_specifies, on_unless_app_specifies, always_on | ADLX has no query for the accepted options; the list is the SDK enum, so a write may still be refused for some of them; excludes enhanced_sync (off now) |
| `frame_rate_target.enabled` | `IADLX3DFrameRateTargetControl` | yes | yes | yes | configurable | off | - |  |
| `frame_rate_target.fps` | `IADLX3DFrameRateTargetControl` | yes | yes | yes | configurable | 60 fps | 15..1000 step 1 |  |
| `afmf.enabled` | `IADLX3DAMDFluidMotionFrames` | yes | yes | yes | configurable | off | - |  |
| `afmf.algorithm` | `IADLX3DAMDFluidMotionFrames1` | no | no | no | unsupported | - | auto, enhanced, standard | IsSupportedAlgorithm failed: ADLX_NOT_SUPPORTED; read: ADLX_NOT_SUPPORTED |
| `afmf.search_mode` | `IADLX3DAMDFluidMotionFrames1` | yes | yes | unverified | read_only | auto | auto, standard, high | enum options not queryable |
| `afmf.performance_mode` | `IADLX3DAMDFluidMotionFrames1` | yes | yes | unverified | read_only | auto | auto, quality, performance | enum options not queryable |
| `afmf.fast_motion_response` | `IADLX3DAMDFluidMotionFrames1` | yes | yes | unverified | read_only | repeat_frames | repeat_frames, blended_frames | enum options not queryable |
| `rsr.enabled` | `IADLX3DRadeonSuperResolution` | yes | yes | yes | configurable | off | - | excludes boost (off now); excludes image_sharpening (off now); requires gpu_scaling (not probed) |
| `rsr.sharpness` | `IADLX3DRadeonSuperResolution` | yes | yes | yes | configurable | 75 % | 0..100 step 1 | same dependencies as `rsr.enabled` |
| `anti_aliasing.mode` | `IADLX3DAntiAliasing` | yes | yes | unverified | read_only | use_app_settings | use_app_settings, enhance_app_settings, override_app_settings | enum options not queryable |
| `anti_aliasing.level` | `IADLX3DAntiAliasing` | yes | yes | unverified | read_only | invalid | 2x, 2x_eq, 4x, 4x_eq, 8x, 8x_eq | enum options not queryable; ADLX reports raw 0 ("invalid"), probably because the mode is use_app_settings |
| `anti_aliasing.method` | `IADLX3DAntiAliasing` | yes | yes | unverified | read_only | multisampling | multisampling, adaptive_multisampling, supersampling | enum options not queryable |
| `morphological_aa.enabled` | `IADLX3DMorphologicalAntiAliasing` | yes | yes | yes | configurable | off | - |  |
| `anisotropic_filtering.enabled` | `IADLX3DAnisotropicFiltering` | yes | yes | yes | configurable | off | - |  |
| `anisotropic_filtering.level` | `IADLX3DAnisotropicFiltering` | yes | yes | unverified | read_only | invalid | 2x, 4x, 8x, 16x | enum options not queryable; raw 0 ("invalid"), probably because filtering is off |
| `tessellation.mode` | `IADLX3DTessellation` | yes | yes | unverified | read_only | amd_optimized | amd_optimized, use_app_settings, override_app_settings | enum options not queryable |
| `tessellation.level` | `IADLX3DTessellation` | yes | yes | unverified | read_only | 64x | off, 2x, 4x, 6x, 8x, 16x, 32x, 64x | enum options not queryable |
| `reset_shader_cache.run` | `IADLX3DResetShaderCache` | yes | n/a | yes | configurable | - | - | Action: nothing to read; the probe does not invoke it |
| `image_sharpen_desktop.enabled` | `IADLX3DImageSharpenDesktop` | no | no | no | unsupported | - | - | ADLX IsSupported is false; requires image_sharpening (off now) |
| `fidelityfx_super_resolution.enabled` | `IADLX3DFidelityFXSuperResolution` | yes | yes | yes | configurable | on | - |  |
| `fidelityfx_frame_gen.enabled` | `IADLX3DFidelityFXFrameGenUpgrade` | no | no | no | unsupported | - | - | ADLX IsSupported is false |
| `fidelityfx_frame_gen.ratio` | `IADLX3DFidelityFXFrameGenUpgrade` | no | no | no | unsupported | - | - | ADLX IsSupported is false |

Totals: 34 settings, 19 `configurable`, 10 `read_only`, 5 `unsupported`; none is `temporarily_unavailable` or `needs_external_setup`.

### Mismatches with the spec's ids (`docs/native-api.md`)

- The spec's id list is covered, with extras: `vsync.enabled` (the spec has only `vsync.mode`; ADLX has both a toggle and a mode, and the toggle is the supported-and-writable one), `anti_lag.level`, `afmf.*` beyond `enabled`, `chill.min_fps`, `anti_aliasing.*`, `morphological_aa.enabled`, `anisotropic_filtering.*`, `tessellation.*`, `reset_shader_cache.run`, `image_sharpen_desktop.enabled` and the FidelityFX pair. Clients already render unknown ids generically.
- The spec example shows `afmf.enabled` as `needs_external_setup` and `rsr.enabled` as `unsupported`. On this GPU ADLX reports both supported and readable, so the probe says `configurable` for both. RSR also needs GPU scaling on, which the probe does not read, so a write may still change nothing visible. The spec text should not promise either state for these two until step 2 tries a write.
- `vsync.mode` is `read_only` here, not configurable, because its options cannot be queried. `vsync.enabled` covers the common case.
- `anti_lag.level` is `unsupported`: the runtime (1.5.0.124) predates `IADLX3DAntiLag1`, so Anti-Lag Next cannot be selected through ADLX. Anti-Lag 2 is an in-game SDK feature and ADLX does not expose it at all.
- `afmf.algorithm` is `unsupported` on this driver; the other three AFMF options are present.
- The spec example shows `driver: "25.9.1"`; the probe reports `26.9.2` (AMD Software) next to the Windows driver version `32.0.32015.2008`. Report the AMD Software version.

### Caveats

- The probe cannot prove a write works. `writable: yes` means ADLX offers the setting with a usable range, nothing more. Step 2 of A5 must write and read back each setting that the first version exposes, then restore it.
- Dependency notes come from sentences in the ADLX headers. ADLX does not expose a dependency query, so the list is not exhaustive.
- Measured on one GPU and one driver. Other GPUs, older drivers and the 1.x ADLX runtimes will differ: run the probe there before relying on any row.
