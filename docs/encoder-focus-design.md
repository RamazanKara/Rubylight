# Encoder focus: design

Status: design, not implemented. Written 2026-10-26 against main `817e1469c`, AMF SDK 1.5.2.0 (`rust/windows/include/AMF/core/Version.h`).

Encoder focus gives the AMF encoder's bits to the parts of the picture where they matter, through AMF's per-block importance map (ROI). The output is an ordinary H.264, HEVC or AV1 stream: every Moonlight client decodes it unchanged, and nothing changes on the wire. `PROTOCOL_VERSION` stays as it is.

## 1. Decision

- **Mode to build: `static`.** Blocks that have not changed for a few frames get a higher importance than blocks that are moving. It works like PA's temporal AQ, without PA's lookahead and latency, and it works for 10-bit HDR, where PA is not available. A fully moving frame and a fully still frame both produce a uniform map, and a uniform map is never attached, so focus only acts on mixed pictures: a HUD or text over motion, a video window on a still desktop, or a settled page after a scroll.
- **`centre` is opt-in and experimental:** static plus a fixed weight on the screen centre and around the cursor. A whole-frame metric cannot show its benefit, so it is measured with centre-crop scores, and it is dropped if it does not earn its place (section 9).
- **The map is built on the GPU in the existing D3D12 conversion submission**, from a per-16x16-block signature and a still-age counter. The CPU copies the last completed map, at most about 130 KB at 4K with H.264 and 8 KB with HEVC or AV1, from a READBACK buffer whose fence has already signalled. It never waits on a fence. The map is therefore one frame old, which suits a measure of how long a block has been still.
- **Support is proven, never assumed.** AMF exposes caps for H.264 and HEVC but none for AV1, and a surface property is accepted whether or not the driver uses it. The startup encoder check therefore runs a functional probe: half the map at importance 10, half at 0, then the encoder's block QP feedback must show the difference. A codec that fails the probe reports `unsupported` with a reason in the native API document, the host log and the session warning when it is requested. There is no silent fallback.
- **The setting is `encoder_focus = off | static | centre`, default `off`**, with per-app and per-device overrides, in the web console and through native API v1 (`stream.settings`, `stream.encoder_focus`). Nightjar toggles it live, without an encoder restart or an IDR. It is opt-in: there is no `auto` value. One is added, resolving per codec to the measured winner, only once a codec measures better (section 9).
- **PyroWave is excluded**, explicitly: it codes every frame on its own, so a still block gains nothing from extra bits in one frame. A PyroWave session reports `unsupported` with the reason `pyrowave`.
- **Default-on only if it measures better** (section 9). Otherwise it ships opt-in and labelled Experimental, or it is dropped.

## 2. What AMF 1.5.2 offers (verified in the vendored headers)

| | H.264 (`VideoEncoderVCE.h`) | HEVC (`VideoEncoderHEVC.h`) | AV1 (`VideoEncoderAV1.h`) |
|---|---|---|---|
| Importance map (per-submission, on the input surface) | `ROIData` (l. 298), 16x16 macroblocks | `HevcROIData` (l. 263), 64x64 | `Av1ROIData` (l. 308), 64x64 |
| Map format | 2D AMFSurface `AMF_SURFACE_GRAY32`, u32 importance from 0 (least) to 10 (most) | same | same |
| Capability | `ROIMap` (l. 370) | `HevcROIMap` (l. 324) | **none** |
| Block QP feedback request (per-submission) | `BlockQpFeedback` (l. 303) | `HevcBlockQpFeedback` (l. 268) | `Av1BlockQIndexFeedback` (l. 312) |
| Feedback result (on the output buffer) | `BlockQpMap` (l. 347), GRAY32 | `HevcBlockQpMap` (l. 310) | `Av1BlockQIndexMap` (l. 353), q-index rather than QP |
| Also per-submission | `PSNRFeedback` | `HevcPSNRFeedback`, `HevcSSIMFeedback`, `HevcStatisticsFeedback` | `Av1PSNRFeedback`, `Av1SSIMFeedback`, `Av1StatisticsFeedback` |

- **There is no input QP map or delta-QP map.** ROI importance is the only per-region input. Per-frame QP limits and VBAQ/CAQ are frame-wide.
- **AMD's encode API docs say the map can only use system memory** ("Currently, the ROI map can only use system memory"). That rules out handing AMF a GPU buffer, and it is why the design uses a READBACK ring rather than GPU-to-AMF zero-copy. If a later AMF accepts `AMF_MEMORY_DX12` maps, the probe can test that and drop the one-frame lag.
- **How the map is applied is documented only vaguely:** "Encoder will further adjust the bits allocation among code blocks based on the importance, on top of the base rate control decisions." The working hypothesis was that only relative importance matters under rate control. Measured on the RX 7900 XT (HEVC 1080p60, 20 Mbps, `vbr_latency`, 2026-10-27), importance is closer to absolute: an all-`k` map moved block QP against no map by +1.1 at 0, -0.7 at 1, -1.6 at 2, -2.8 at 5 and -5.0 at 10, about -0.5 QP per step. Uniform maps are never attached, so only the spread reaches the encoder.
- **Any map lifts the latency-constrained rate control's limit on big frames, on the frame that carries it**, whatever its values (an all-zero map too). Under `vbr_latency` that frame codes like CBR. MaxAUSize does not bind either, with or without a map. A map in the frames after a keyframe, while the rate control settles, made frames of 75 to 145 KB against an 83 KB cap, so no map goes on the 30 frames after a keyframe, recovery picture or first frame (`SETTLE_FRAMES`). From then on a map costs nothing at steady state: an all-zero map was bit-identical to no map. The rest is a scene cut on a frame with a map, which comes out about twice the size of the same cut without one (184 to 291 KB against 92 to 137 KB). The map is the previous frame's (the one-frame lag), and seeing the cut in time would mean a CPU wait for the current frame's map, which is ruled out.
- **AMD documents no per-generation support** for ROI. RDNA 1 (VCN 2), RDNA 2 (VCN 3), RDNA 3 (VCN 4) and RDNA 4 (VCN 5) are all possible, and so is a driver that accepts the property and ignores it. The functional probe decides per GPU, codec and bit depth. Our hardware is RDNA 3 only: the RX 7900 XT here and the Radeon 780M on the laptop. RDNA 1, 2 and 4 rely on the probe and ship labelled Experimental. No GPU is dropped: a GPU that fails the probe keeps streaming exactly as today, with focus reported unsupported.
- **The SimpleROI sample** allocates the map with `AllocSurfaceEx(AMF_MEMORY_HOST, AMF_SURFACE_GRAY32, ceil(w/16 or 64), ceil(h/16 or 64))`, fills plane 0 through `GetNative()` using `GetHPitch()` in bytes, and attaches it with `SetProperty(ROI_DATA, surface)`. It checks no cap.

## 3. Focus sources evaluated

| Source | Verdict | Why |
|---|---|---|
| OS dirty rects (Desktop Duplication `GetFrameDirtyRects`, WGC dirty regions) | Rejected | Games redraw the whole frame, so dirty rects are always full-screen. WGC dirty regions need Windows 11 24H2, and Windows 10 must keep working. Our driver's swap-chain path (`ReleaseAndAcquireBuffer2`) passes no dirty rects. It would only cover the desktop case, through three different capture paths. |
| GPU block diff in the existing conversion pass | **Chosen** | Capture-agnostic: the driver, WGC and Desktop Duplication all feed the same converter. It works for games, because a HUD is still even when the scene moves, and for desktops, Windows 10 and HDR. It is computed where the pixels already are, with no extra submission or wait. |
| Screen centre and cursor neighbourhood | Opt-in experimental (`centre`) | A heuristic with no full-frame objective gain: it moves bits away from the edges by design. Kept only if centre-crop quality improves (section 9). |
| PA temporal AQ / perceptual AQ (`PATemporalAQMode`, `PAPerceptualAQMode`, `PAActivityMap`) | Rejected as a source | PA is off by default because it cost 25 ms per 4K60 frame. It is forced only for QVBR/HQVBR/HQCBR, and only for 8-bit NV12. The host never sets TAQ or PAQ (driver default NONE). Static focus is the lookahead-free stand-in for TAQ, so it does not duplicate PA. With PA on, a chosen mode runs and the `encoder focus` line logs `preanalysis=true`. |
| VBAQ / AV1 CAQ | Kept as is | On for HEVC (`HevcEnableVBAQ`) and AV1 (`Av1AQMode=1`), off for H.264, where it measured worse (`docs/configuration.md`, `amd_vbaq`). It works within a frame on texture variance, while focus works across frames on stillness. Both stay on in both arms of the A/B, which is the shipping configuration. |
| AV1 screen-content tools, high-motion boost, frame QP limits | Unrelated | Frame-wide, and settings of their own already exist. |
| Block QP / q-index feedback | Used for verification | It proves that the driver honoured the map, in the probe and in measurement runs (`encoder_focus_verify`). |

## 4. Modes and levels

`encoder_focus` (string; default `off`, which an unset or empty value also means; app and device overrides like the other `amd_*` keys):

- `off`: no map, no signature work. The conversion runs the existing `yuv420_cs` pipeline unchanged.
- `static`: importance rises with each block's still-age.
- `centre` (alias `center`): static, plus centre and cursor weighting.

An unknown value, `auto` included, is an error at session start (warning `encoder_focus`, effective `off` with the reason `invalid_value`). It is never ignored silently.

Hidden tuning key, for measurement only and absent from the console: `encoder_focus_levels = "moving=2,still=6,ramp=4,centre=4,ctb_still=1.0"`, with all importances clamped to 0..10.

- A block's age counts the consecutive frames in which its signature matched, saturating at 255.
- Importance is `moving` at age 0 and climbs linearly to `still` at age `ramp` or more.
- `centre` adds up to `centre` inside an ellipse covering the middle half of the frame, and within 128 px of the cursor while the cursor is in the picture. The result is capped at 10.
- For HEVC and AV1, a 64x64 block counts as still when at least `ctb_still` of its sixteen 16x16 blocks are still (1.0 means all of them).
- These defaults are starting points, fixed by step M1.

Hidden verification key: `encoder_focus_verify = true` requests block QP feedback on one frame per second. It logs the mean QP of high- versus low-importance blocks in the 5 s `encoder focus` line. It is for measurement only: feedback may cost encoder time.

## 5. GPU map pipeline (no added latency)

The converter (`rust/windows/src/compute.rs`) already records the RGB-to-YUV dispatch per slot (4 slots, a fence each) and never CPU-waits. AMF waits on the conversion fence through D3D12 interop. Focus adds the following, all in the same command list as the conversion:

1. **`yuv420_focus_cs`**, a second entry point compiled from the same `yuv420_cs` body behind a define, so the `off` pipeline is byte-for-byte the shader it is today.
   - Each 8x8 thread group already covers one 16x16 luma block, matching an H.264 macroblock. Each thread hashes its four luma codes and one chroma pair, after the `noise_bits` mask (default 0, meaning exact), mixed with its position. The hashes are combined with `InterlockedAdd` in group-shared memory.
   - After `GroupMemoryBarrierWithGroupSync`, thread 0 compares the result with the block's signature in a persistent `uint2` UAV `{signature, age}` and writes back the new signature and the age (age + 1 if equal, 0 if not).
   - Luma and chroma output must stay bit-identical to `yuv420_cs`, and a test enforces it.
2. **`focus_map_cs`**, one thread per codec block (16x16 for H.264, 64x64 for HEVC and AV1).
   - It reads the ages and applies the mode and levels from root constants: mode, levels, grid, cursor position and visibility.
   - It writes u32 importances into a map buffer laid out at the codec grid's width, and folds min and max into a 2-word header with `InterlockedMin` and `InterlockedMax`.
   - A UAV barrier separates it from the conversion dispatch.
3. **`CopyBufferRegion`** of the header and map into this slot's READBACK buffer, which is persistently mapped. The slot's existing fence signal covers it.
4. **CPU, in `encode_gpu` before `SubmitInput`:**
   - Take the newest slot older than the current frame whose fence has completed (`Compute::completed`, which does not wait).
   - If its header shows min = max, the map is uniform: attach nothing.
   - Otherwise allocate an AMF host GRAY32 surface of the grid size, copy the rows honouring `GetHPitch()`, and `SetProperty` the codec's ROI name on the input surface. AMF owns and releases it with the input. A pool is an optimisation to add only if the timing record shows the allocation.
   - If no completed slot is newer than the last map used, attach nothing, and count the frame as `stale`.

Cost bounds checked in measurement:

- GPU memory: at 4K, 259 KB of signatures and ages, plus at most 4 x 130 KB of readback.
- GPU time: one group-shared reduction per 16x16 block and a dispatch of 510 to 32,400 threads.
- CPU time: one copy of at most 130 KB, usually 2 to 8 KB.

Buffers and both pipelines are created with the converter whenever the encoder can use focus, so a live switch only changes which pipeline is dispatched.

## 6. Interplay rules

| Situation | Rule |
|---|---|
| IDR, LTR recovery picture, first frame after encoder recreate | No map attached, on that frame and the 30 after it while the rate control settles (section 2). Recovery pictures stay exactly as today (size cap, pacing). Ages keep counting. These frames count as `skipped_recovery`. |
| Repeat frames (`!fresh`) | Converted as today; every block counts as still, which gives a uniform map, so no map. |
| Link-rate adaptation and runtime bitrate | The map is relative, so it needs no change. M2 measures 5 and 10 Mbps at 1080p60 (0.04 and 0.08 bits per pixel). A bits-per-pixel floor below which core forces a neutral map is added only if the 5 Mbps arm shows a loss. |
| Recovery frame cap / max AU size | Unchanged, but it does not bind a frame with a map: a scene cut on such a frame is about twice its size without focus (section 2). M3's `slice_delivery_overflows` and frame sizes show what that costs in a stream. |
| Slice delivery (`video_slice_delivery`) | The map is per frame, and slicing is unchanged. ROI can make slices uneven, so M3 compares `slice_delivery_overflows` between off and on. Any increase blocks default-on. |
| Intra refresh | No special case; M3 has one arm with intra refresh on. |
| LTR | No special case. |
| Split-frame (dual VCN, HEVC/AV1 on the RX 7900 XT) | The probe runs with the stream's own encoder properties. M0 also verifies at 1080p and 4K with split-frame on Automatic. If split-frame ignores the map, focus reports `unsupported` with the reason `split_frame`; split-frame is never turned off on its own. |
| Pre-analysis on | A chosen mode runs, and the `encoder focus` line logs `preanalysis=true`. |
| 10-bit HDR (P010) | Same shader path with the `ten_bit` flag. Probed separately for HEVC and AV1 10-bit. |
| Packed 4:4:4 (`experimental_packed_444`) | Unsupported in v1, reason `packed_444`: chroma packed into the luma plane makes the block grid weight the wrong pixels. |
| D3D11 conversion path (D3D12 conversion unavailable) | Unsupported, reason `d3d12_conversion_unavailable`. |
| PyroWave | Excluded, reason `pyrowave`. |
| Windows 10, WGC, Desktop Duplication, driver capture, physical displays | All work: focus depends only on the D3D12 converter and AMF. |

## 7. Support detection and loud failure

- **Startup encoder check** (`rust/host/src/state.rs`, the loop that probes H.264, HEVC, HEVC 10, AV1 and AV1 10). After a codec reports Available, the same probe encoder runs `probe_focus()`:
  1. Read the cap: `ROIMap` for H.264, `HevcROIMap` for HEVC; AV1 has none. A cap of false is final: unsupported, reason `codec_unsupported`, message "the driver reports no ROI map".
  2. After 4 warm-up frames without a map, encode 3 frames of a deterministic textured image, statistically the same on both halves, with the left half at 10 and the right half at 0, then 3 frames with the halves swapped, each with QP feedback requested.
  3. Read `BlockQpMap`, `HevcBlockQpMap` or `Av1BlockQIndexMap` from each output buffer. The RX 7900 XT's driver returns these maps in a broken layout: rows that are a pitch apart in its memory come out packed as if they were a grid width apart, so block (x, y) sits at flat index `y * pitch / 4 + x`, and rows past `width * height / (pitch / 4)` are lost (the pitch is 1024 bytes). At 1080p that leaves 2 of 17 HEVC and AV1 rows and 32 of 68 H.264 rows, with some blocks 0. `unstride` lays such a grid out again, and blocks without a QP count for neither half.
  4. The map's effect is half the difference of the two arms' right-minus-left QP differences; a difference the picture itself makes cancels. Pass when the effect is at least 2 QP (8 q-index). A no-map control arm with a 1 QP limit was dropped: with only 48 HEVC blocks readable at 1080p, its 3-frame mean reached 0.8 QP on a uniform picture.
  5. No feedback map: unsupported, message "the driver returned no block QP map, so ROI cannot be verified".
- The results are stored per codec and bit depth: `Verified{delta}`, `Unsupported(reason, message)` or `NotProbed`. They are logged in one line, `encoder focus check: h264=verified(+4.1) hevc=verified(+3.6) hevc10=... av1=unsupported(no block QP map)`.
- **At session start**, core `encoder_focus::resolve` combines the requested mode, codec, bit depth, PyroWave, packed 4:4:4, D3D12 conversion, PA and the probe result.
  - The effective state is logged: `encoder focus: requested=static effective=static codec=hevc grid=30x17 levels=...`.
  - A requested mode that is not possible logs a WARN and sets the session warning `encoder_focus` with the reason, and the effective state is `off`.
  - `off`, the default, is logged at info.
- **Every 5 s while focus is on**, an `encoder focus` line reports `mode`, `maps`, `uniform`, `stale` and `skipped_recovery`, and `qp_delta` with `encoder_focus_verify`. It is a separate line, so `timing_report.py` parsers of `stream timings` are not affected.

## 8. Configuration, native API and clients

### Host configuration and web console

- `encoder_focus` joins the "Encoder tuning" entries of `override_allowed` (`rust/core/src/config.rs`). The app and device editors offer it automatically.
- Console: a select in group "AMD AMF" (`rust/web/src/lib/schema/video.ts`, next to `amd_vbaq`), labelled **Encoder focus**:
  - options: Off (default), Still areas, Still areas and centre; the description says Experimental;
  - details: "Gives more bits to parts of the picture that stay still, such as text and HUDs, and fewer to motion. The stream stays a normal video stream.";
  - a Choice line in `rust/host/src/console/settings.rs`.
- `docs/configuration.md` gets a paragraph that links the measurement once it exists.

### Native API v1 (additive; `API_VERSION` stays 1)

- Feature strings in `rubylight_native_api::feature`:
  - `STREAM_SETTINGS = "stream.settings"`: the document below exists;
  - `ENCODER_FOCUS = "stream.encoder_focus"`: the document carries `encoder_focus`.
- Both are listed for every paired caller on a host with this build, whatever the probe said. The setting's `state` says whether focus works.

`GET /api/native/v1/stream/settings` (paired; needs the `launch` permission):

```json
{
  "revision": 4,
  "session": { "streaming": true, "owner": true, "codec": "hevc", "ten_bit": false, "pyrowave": false },
  "settings": [
    {
      "id": "encoder_focus",
      "group": "encoder",
      "kind": "choice",
      "state": "configurable",
      "value": "off",
      "source": "host",
      "effective": "off",
      "options": [
        { "value": "inherit", "label": "Host setting" },
        { "value": "off", "label": "Off" },
        { "value": "static", "label": "Still areas" },
        { "value": "centre", "label": "Still areas and centre" }
      ],
      "reason": null,
      "message": null
    }
  ]
}
```

- `settings[]` entries reuse `radeon::Setting` (flattened) and add:
  - `source`: `host`, `device` or `app`, the layer the value comes from;
  - `effective`: what the encoder runs now. Absent without a session or when the caller does not own it.
- `state`:
  - `configurable`;
  - `read_only`: reason `app_override` when the running app sets the key itself, or `permission`;
  - `unsupported`: reason `pyrowave`, `codec_unsupported` (the message carries the probe detail), `packed_444`, `d3d12_conversion_unavailable`, `split_frame` or `no_amf`;
  - `temporarily_unavailable`: reason `not_probed` while the startup check runs.
- Without a session, `state` describes the host's support for the device's next stream. `unsupported` applies only when no codec verified.

`POST /api/native/v1/stream/settings` with `{"changes":[{"id":"encoder_focus","value":"static"}]}`:

- It writes the caller's own device override in the store behind Devices and `/api/clients/update`. `inherit` removes it.
- If the caller owns the streaming session, the new value takes effect from the next frame (session atomic; no encoder restart, no IDR).
- It answers with the document above.
- Errors, using existing codes:
  - `not_paired`;
  - `permission_required` (no `launch`);
  - `invalid_value` (unknown id or value);
  - `not_writable` (state not `configurable`; the error carries the reason).
- A host without the route answers 404 through the existing `unknown` fallback, and clients then hide the setting.

### Couch-menu hook

- `rust/host/src/native_api/stream_settings.rs` exposes:
  - `pub(crate) fn read(state: &Shared, device: &str) -> StreamSettings`;
  - `pub(crate) fn write(state: &Shared, device: &str, changes: &[Change]) -> Result<StreamSettings, Failure>`.
- The host-drawn couch menu (`docs/couch-menu-design.md`, in progress) calls them with the streaming device's id, with the same rules and persistence as the HTTP route. It can render every entry of `settings[]` generically by `kind`, so later stream-scoped settings show up there without menu changes.

### Nightjar (Android phone, TV, desktop preview)

- Shown only when `NativeCapabilities.has("stream.encoder_focus")`. The gate is never the host brand or version. Labels are host-neutral: "Encoder focus", "Still areas", "Still areas and centre".
- **In-stream menu:**
  - Android `StreamMenu` native page and desktop Session page.
  - One choice row. The subtitle shows the effective mode ("Running: Still areas") or the host's reason text when not configurable, with the row disabled.
  - Polled once a second while the panel is shown, like the Radeon panel. Writes go through POST.
- **Host stream settings** (`HostStreamSettings`, per host, outside a stream): the same row, which writes the device override.
- **No local preference key**: the host stores the value, so `SettingsKeysTest` is untouched.

## 9. Measurement plan

All runs follow the CLAUDE.md hardware rules:

- The RX 7900 XT is the user's gaming PC. GPU runs (M0 to M3) only start when the user says testing is fine, or is verifiably idle.
- Before each batch, `(Invoke-WebRequest http://127.0.0.1:47989/serverinfo).Content` must show `RustHostSessionCount` and `RustHostPendingSessionCount` at 0.
- Test hosts use an isolated profile: `butterpollo.exe --config-dir C:\tests\focus --port 48123 --bind 127.0.0.1`, `virtual_display_layout = extended`, `dd_configuration_option = disabled`, `virtual_display_mode = disabled`.
- Exit code 3 from `e2e.py` or `check.ps1` means an AMD encoder hang: stop and investigate, never retry.
- Runs go in the background. Read `result.json`, the JSONL rows and the 5 s lines, not full logs.

| Step | What | Where | Gate |
|---|---|---|---|
| M0 probe characterisation | `focus_native_tests` (ignored) for H.264, HEVC, HEVC 10, AV1, AV1 10. Records the caps, whether feedback exists, the half/half QP delta, and uniform 0/6/10 against no map (frame size and mean QP), at 640x480 and at 1080p/4K with stream properties (split-frame Automatic). Saved to `rust/release/fixtures/amf-focus-probe-7900xt.json` and `-780m.json`. | 7900 XT idle, then the 780M | No codec verifies: stop and drop the feature. Uniform map worse than no map: keep "never attach uniform maps" (already the design) and note it. |
| M1 level sweep | Single runs at HEVC 1080p60 10 Mbps on the `mixed` and `editor` fixtures. `still` in {4, 6, 8} x `ramp` in {2, 8}; `ctb_still` in {1.0, 0.75}. Pick the best 1-2. | 7900 XT | Pick the best by full-frame VMAF, then by the settled-phase result. |
| M2 quality A/B (ABBA) | `focus_ab.py`: arms A1 (off), B1 (on), A2 (off), B2 (on, request rescaled to A1/B1's actual bitrate, as `amf_sweep.py` `quartet` does). Bitrates in the next table. Fixtures: `chroma_fixtures.py` editor, browser and sheet (hold 30, scroll 30, hold 30), the new `mixed` fixture, and the TMNT HD and GoldenEye XBLA clips; 180 frames each. Scores: libvmaf (default model), PSNR-Y and SSIM, with the corrected graph (`trim=end_frame=180`, `shortest=1:repeatlast=0`; `rust/PERFORMANCE_WORK.md`, current AMF quality settings comparison). Full frame, plus the still region against the moving region (crop masks from the fixture manifest), plus the settled phase (frames 60-89). | 7900 XT, about 45 min | Decision rules below. |
| M3 timing A/B | `e2e.py` streams with `BUTTERPOLLO_TIMING_RECORD=1`, 90 s per arm, ABBA, at 1080p60 HEVC 20 Mbps and 4K60 HEVC 40 Mbps, plus one arm each with slice delivery and with intra refresh. `timing_report.py` gives convert, encode and host_total p50/p99. The 5 s lines give present_to_sent p99, capture_age_at_submit and `slice_delivery_overflows`; the `encoder focus` lines give stale and uniform. | 7900 XT on the isolated host | Same rules. |
| M4 second GPU | M2 subset (HEVC 1080p60 10 Mbps: mixed and editor) and M3 at 1080p60 via `smoke.ps1`. | Laptop 780M (`ramaz@192.168.4.12`) | It must agree with the 7900 XT in direction. |

M2 bitrates at 1080p60:

| Codec | Bitrates |
|---|---|
| HEVC | 5, 10 and 20 Mbps |
| AV1 | 10 and 20 Mbps |
| H.264 | 20 Mbps |
| HEVC 10-bit | 20 Mbps at 1440p, on the converted-HDR fixture |

`centre` is measured as one extra B arm at HEVC 10 Mbps, scored on the centre crop (the middle 50%) and the periphery.

**Decision rules**, per codec. Noise is the larger of |A1 − A2| and |B1 − B2|. Each rule must hold on both GPUs.

**Default-on** (`AUTO` entry set to `static`), if all of these hold:

- **Quality gain:** `mixed` and the text fixtures gain at least +0.5 VMAF mean at 10 Mbps and +0.2 at 20 Mbps, each at least 2x noise.
- **No fixture worse:** none drops more than 0.2 in mean VMAF or 0.5 in 5th-percentile frame VMAF.
- **Game clips:** they drop no more than 0.2.
- **Moving region:** it drops no more than 1.0.
- **Timing:**

| Metric | Limit |
|---|---|
| Convert p99 | No more than +0.05 ms |
| Encode p99 | No more than +0.1 ms |
| present_to_sent p99 | Within noise (no more than +0.2 ms) |
| `slice_delivery_overflows` | No increase |
| `stale` | Under 1% of frames |

**Opt-in**, labelled Experimental:

- **Quality:** text and `mixed` gain, but games lose up to 0.5, or the gain shows only at 10 Mbps or below.
- **Timing:** the timing limits still hold.

**Drop** (remove `static` and the shader variant, keep this document):

- no gain beyond noise anywhere; or
- a timing regression that cannot be fixed. Per CLAUDE.md, revert changes that measure worse.

**`centre`** stays opt-in only if:

- the centre crop gains at least +0.5 VMAF;
- full-frame VMAF loses no more than 0.5.

Otherwise the mode is removed.

The result goes into `rust/PERFORMANCE.md` as a new dated section. The release notes' "Tested." paragraph quotes only these numbers.

## 10. Risks

1. **The driver accepts the property and ignores it.** The functional probe catches this. A driver might honour ROI at 640x480 but not with split-frame, or at stream resolutions; M0 tests that, and `encoder_focus_verify` can confirm it in a stream.
2. **The importance semantics are undocumented.** Measured on the RX 7900 XT: importance acts close to absolute, and any map lifts `vbr_latency`'s limit on big frames for the frame it is on, with VBAQ on or off (section 2). The settle period removes the overshoot after keyframes. Scene cuts on a frame with a map stay about twice their size without focus, which is why focus stays opt-in and Experimental until M3 shows the stream cost.
3. **The gain may be small.** Under low-latency CBR the rate controller rebalances, and HEVC/AV1's 64x64 grid is coarse for small HUD elements; the `ctb_still` sweep covers this. H.264's 16x16 grid may gain the most. The drop rule handles a feature that does not pay.
4. **Bits move away from motion.** Fast games could look worse in the moving area; M2's moving-region score and its game-clip limit guard against it.
5. **The one-frame-old map** gives a block that just started moving one frame of still importance. That costs bits on new content, which is arguably useful, and it resolves the next frame.
6. **Encoder time:** AMF could upload the map synchronously. M3's encode p99 measures it, and the timing rules reject it.
7. **Slices become uneven**, which raises overflows and turns slice delivery off. M3 measures it, and it blocks default-on.
8. **Hash collisions** mark a changed block as still: about 2^-32 per block per frame, harmless.
9. **The shader edit touches the hot path.** The off path keeps the old pipeline object; a test enforces bit-identical luma and chroma output, and M3 measures convert time.
10. **Coverage of untested generations:** RDNA 1, 2 and 4 depend on the probe alone and are labelled Experimental. Any GPU that fails keeps today's behaviour, loudly reported.
11. **Cross-repo coupling:** Nightjar pins `rubylight-native-api` by git rev, so brief C waits for brief B to be pushed.

## 11. Implementer briefs

Order:

1. A1, then A2, with the same implementer, continued through SendMessage.
2. M-harness, in parallel with A (its files are disjoint). The M runs follow after A2, by a runner, with the user idle.
3. B, after M0 passes, because a failed M0 drops the feature.
4. C, after B is pushed.

Every brief:

- works in its own worktree under `C:\src`, sharing the target dir only when not in parallel;
- keeps LF line endings, uses the Edit tool, and never `git stash`;
- writes commit messages to a file and runs `git commit -F`;
- commits small and pushes after its checks pass;
- adds a regression test for every fix.

The GPU tests in A follow the hardware rules in section 9.

### A1: core policy, GPU still-age detector and map (host encoder, part 1)

Files (only these):

- new `C:\src\rubylight-rel\rust\core\src\encoder_focus.rs`;
- `C:\src\rubylight-rel\rust\core\src\lib.rs` (`pub mod encoder_focus;`);
- `C:\src\rubylight-rel\rust\core\src\session.rs`;
- `C:\src\rubylight-rel\rust\windows\src\compute.rs`;
- new `C:\src\rubylight-rel\rust\windows\src\compute_focus_tests.rs`;
- `C:\src\rubylight-rel\rust\windows\src\shaders\color.hlsl`;
- `C:\src\rubylight-rel\rust\windows\build_shaders.rs`.

Steps:

1. **Core module `encoder_focus`:**
   - `Requested {Off, Static, Centre}`, with `parse()` accepting `""`/`off`, `static`, `centre`/`center` and returning an error otherwise (`auto` included). Off is wire value 0, so a zeroed session atomic is off.
   - `Mode {Static, Centre}`.
   - `Levels::parse("moving=2,still=6,ramp=4,centre=4,ctb_still=1.0,noise_bits=0")`, clamped, with these defaults.
   - `Reason`, with stable wire strings: `pyrowave`, `codec_unsupported`, `packed_444`, `d3d12_conversion_unavailable`, `split_frame`, `no_amf`, `not_probed`, `invalid_value`.
   - `Support {Verified{delta: f32}, Unsupported{reason, message}, NotProbed}`.
   - `Context {codec, ten_bit, pyrowave, packed_444, d3d12, preanalysis, support}`.
   - `resolve(requested, &Context) -> Resolved {On(Mode), Off{reason: Option<Reason>}}`.
   - `grid(codec, coded_w, coded_h) -> (u32, u32)`: 16x16 for codec 0, otherwise 64x64.
   - `importance(age, centre_weight, mode, &Levels) -> u32`, a CPU reference of the shader.
   - `wire()` and `from_wire()` for a `u8` session atomic.
   - Unit tests for every function.
2. **`Session`:**
   - `pub encoder_focus: AtomicU8`, the requested mode, written by the native API and read per frame;
   - `pub encoder_focus_state: Mutex<FocusState {effective, reason, message}>`, written by the encoder thread only on change.
   - Initialise both from the merged config (`encoder_focus`) where the session is built, at `session.rs` around l. 2550.
3. **Shader:**
   - `yuv420_focus_cs`, the `yuv420_cs` body behind `#define FOCUS`, plus the group-shared hash and age update in a `RWStructuredBuffer<uint2>`.
   - `focus_map_cs`: one thread per codec block. It reads root constants (mode, levels, grid, cursor x/y/visible, `ctb_still`, `noise_bits`), writes importances and does `InterlockedMin`/`InterlockedMax` into a header.
   - Register both in `build_shaders.rs`. The existing `yuv420_cs` blob must not change.
4. **`Converter`:**
   - `enable_focus(codec_block)` creates the age UAV, the map buffer, 4 persistently mapped READBACK buffers and both pipelines. It is called at encoder creation when focus can be used.
   - `set_focus(Option<(Mode, Levels, cursor)>)` picks the pipeline per `convert()`.
   - In `convert()`, when on: the focus variant, a UAV barrier, `focus_map_cs`, and `CopyBufferRegion` to the slot's readback, all before the existing fence signal.
   - `latest_map(&mut self, newer_than: u64) -> Option<MapView {value, uniform: bool, rows: &[u32], width, height}>` must use only `completed()`, never a wait.
   - Reset ages on resize or recreate.
5. **Tests** in `compute_focus_tests.rs`. GPU tests are `#[ignore]`.
   - Converted luma and chroma are bit-identical with focus on and off for an SDR and a P010 source.
   - 5 identical frames give ages 0..4; changing one 16x16 block resets only that block.
   - The HEVC 64x64 aggregation follows `ctb_still`.
   - `focus_map_cs` output equals `encoder_focus::importance` for a synthetic age field.
   - A never-signalled fence makes `latest_map` return `None` without blocking.

Test commands:

- `cargo +1.98.1-x86_64-pc-windows-msvc test -p butterpollo-core --locked --target-dir C:\src\rl-target encoder_focus`
- `C:\src\rl-quick.ps1 -Repo <worktree> -Crates butterpollo-core,butterpollo-windows -TargetDir C:\src\rl-target-focus`
- GPU tests, only with the user idle and in the `build.ps1` environment (see `rust/README.md` l. 98): `cargo test -p butterpollo-windows --locked compute_focus -- --ignored --test-threads=1 --nocapture`

Acceptance:

- All tests pass and clippy is clean.
- The `yuv420_cs` shader blob hash is unchanged.
- No CPU fence wait was added (show the `latest_map` code).
- Diff under about 500 lines.

### A2: AMF attach, probe, capability and stream wiring (host encoder, part 2)

Files (only these):

- `C:\src\rubylight-rel\rust\windows\src\amf.rs`;
- new `C:\src\rubylight-rel\rust\windows\src\amf\focus.rs`;
- new `C:\src\rubylight-rel\rust\windows\src\amf\focus_native_tests.rs`;
- `C:\src\rubylight-rel\rust\windows\src\encoder.rs`;
- `C:\src\rubylight-rel\rust\windows\examples\amf_quality.rs`;
- `C:\src\rubylight-rel\rust\host\src\stream.rs`;
- `C:\src\rubylight-rel\rust\host\src\state.rs`.

Steps:

1. **`amf/focus.rs`:**
   - `roi_name(codec)` returns `ROIData`, `HevcROIData` or `Av1ROIData`.
   - `cap_name(codec)` returns `ROIMap`, `HevcROIMap` or `None`.
   - `qp_feedback(codec)` and `qp_map(codec)` use the names in section 2.
   - `attach_for(idr, recover, first_after_create, uniform) -> bool`: only false/false/false/false attaches.
   - `attach(context, surface, view)` follows SimpleROI: `AllocSurfaceEx(AMF_MEMORY_HOST, AMF_SURFACE_GRAY32, w, h)`, then a row copy with `GetHPitch()`, then `SetProperty` with an interface variant. Get the refcount right: AMF keeps its own reference, and we release ours.
   - `probe(encoder) -> Support` as specified in section 7 (cap, then 3 half/half frames and 3 swapped ones, then the QP map effect; the threshold is a named constant).
2. **`amf.rs`:**
   - A `focus` field holding the mode, the levels and the last-used value, plus `set_focus(Option<Mode>)`.
   - At creation, call `converter.enable_focus()` when the compute path exists, and record why focus is unavailable otherwise (D3D11 path, packed 4:4:4).
   - In `encode_gpu`, after `prepare_surface` and before `SubmitInput`, call `latest_map` and `attach_for`, then `attach`.
   - Count `maps`, `uniform`, `stale` and `skipped_recovery`.
   - Add the focus line to `log_effective`, and honour `encoder_focus_verify`: one frame per second gets feedback and a QP delta.
   - Add `pub fn probe_focus(&mut self) -> Support`.
3. **`encoder.rs`:** `Encoder::set_focus(Option<Mode>)` and `focus_stats()`. Both are no-ops for PyroWave, which reports the reason `pyrowave`.
4. **`state.rs`:**
   - In the codec probe loop, after the Available outcome, call `probe_focus()` per codec and bit depth.
   - Store `focus_support: Mutex<[Support; 5]>` with a getter `focus_support(codec, ten_bit)`.
   - Log the one-line summary from section 7.
5. **`stream.rs`:**
   - At start, resolve through `encoder_focus::resolve`, store `encoder_focus_state`, and set the session warning `encoder_focus` (WARN) when a requested mode cannot run.
   - Per frame, read `s.encoder_focus` and call `active.set_focus` on change. Pass the cursor position for `centre`.
   - Emit the 5 s `encoder focus` line.
6. **`amf_quality.rs`:** print `focus_mode`, `focus_maps`, `focus_uniform`, `focus_stale` and, with verify on, `focus_qp_delta` in the final JSON line.
7. **`focus_native_tests.rs`** (`#[ignore]`):
   - the probe per codec and bit depth;
   - uniform 0/6/10 against no map;
   - half/half at 1080p with stream properties;
   - writes the M0 JSON when `BUTTERPOLLO_FOCUS_PROBE_OUT` is set.

Test commands:

- `C:\src\rl-quick.ps1 -Repo <worktree> -Crates butterpollo-core,butterpollo-windows,butterpollo -TargetDir C:\src\rl-target-focus`
- GPU tests, user idle only: `cargo test -p butterpollo-windows --locked focus_native -- --ignored --test-threads=1 --nocapture`
- Isolated host smoke, user idle only, idle-check first: `butterpollo.exe --config-dir C:\tests\focus --port 48123 --bind 127.0.0.1` with `encoder_focus = static`, then `python rust/release/e2e.py` (90 s, in the background). Grep the host log for `encoder focus`.

Acceptance:

- A unit test covers `attach_for` (no map on IDR, recovery, first frame or uniform maps).
- The probe summary line appears at startup on the 7900 XT.
- A session with `encoder_focus = static` logs `effective=static` and non-zero `maps` in the 5 s line.
- Each of these logs a WARN and sets the session warning with its reason, and the stream runs as today:
  - a PyroWave session;
  - packed 4:4:4;
  - `encoder_focus = bogus`.
- `amf_quality.exe` with `encoder_focus = static` reports `focus_maps > 0`.
- No change to recovery pictures: the existing recovery and LTR tests pass.
- Diff under about 500 lines.

### B: host config, native API, web console and docs

Starts after A1 is on main and M0 passes. Files (only these):

- `C:\src\rubylight-rel\rust\core\src\config.rs`;
- `C:\src\rubylight-rel\rust\core\native-api\src\lib.rs`;
- new `C:\src\rubylight-rel\rust\core\native-api\src\stream.rs`;
- new fixtures `C:\src\rubylight-rel\rust\core\native-api\tests\fixtures\stream_settings.json` and `stream_settings_write.json`;
- `C:\src\rubylight-rel\rust\core\native-api\tests\fixtures.rs`;
- `C:\src\rubylight-rel\rust\host\src\native_api.rs`;
- new `C:\src\rubylight-rel\rust\host\src\native_api\stream_settings.rs`;
- `C:\src\rubylight-rel\rust\host\src\nvhttp\native_tests.rs`;
- `C:\src\rubylight-rel\rust\host\src\console\settings.rs`;
- `C:\src\rubylight-rel\rust\web\src\lib\schema\video.ts`;
- `C:\src\rubylight-rel\rust\web\SettingsSchema.test.mjs`;
- `C:\src\rubylight-rel\docs\configuration.md`;
- `C:\src\rubylight-rel\docs\native-api.md`;
- `C:\src\rubylight-rel\rust\RELEASE_NOTES.md` (under `## Unreleased`).

Steps:

1. Add `encoder_focus` to `override_allowed` under "Encoder tuning", and extend the test `overrides_allow_stream_input_display_and_encoder_keys_but_not_host_settings`.
2. **native-api crate:**
   - `feature::STREAM_SETTINGS` and `feature::ENCODER_FOCUS`;
   - `stream::{StreamSettings {revision, session: Option<SessionFacts>, settings: Vec<StreamSetting>}, StreamSetting {#[serde(flatten)] base: radeon::Setting, source, effective}, StreamSettingsWrite {changes: Vec<radeon::Change>}}`;
   - fixtures with serde round-trip tests, using the JSON in section 8.
3. **Host route module `stream_settings.rs`:**
   - `read` and `write` as in section 8 (the couch-menu hook: `pub(crate)`, documented).
   - Device overrides go through the same store as `/api/clients/update`.
   - Live apply sets `session.encoder_focus` when the caller owns the streaming session.
   - States and reasons come from `state.focus_support` and `session.encoder_focus_state`.
   - Errors: `not_paired`, `permission_required`, `invalid_value`, `not_writable`.
   - Register `GET` and `POST` on `/stream/settings` in `router()`, and add both features to the capabilities list.
4. **Web:** add a select entry next to `amd_vbaq`, with the label and details from section 8 (the Experimental tag on `centre`), and a Choice line in `console/settings.rs`.
5. **Docs:**
   - a `configuration.md` paragraph (what it does, PyroWave excluded, opt-in and off by default, the link to the measurement);
   - the `native-api.md` endpoint, features, states and reasons;
   - a release-notes Unreleased entry.

Test commands:

- `cargo +1.98.1-x86_64-pc-windows-msvc test -p butterpollo-core --locked --target-dir C:\src\rl-target config`
- `C:\src\rl-quick.ps1 -Repo <worktree> -Crates butterpollo-core,butterpollo -TargetDir C:\src\rl-target-focusapi`. This includes `rubylight-native-api` if it is not a workspace member: add `-p rubylight-native-api` in that case.
- `cd rust\web; npm ci --no-audit --no-fund; npm run check; node --test "*.test.mjs"; npm run build`. Kill stray node or vite first.

Acceptance, with route tests in `native_tests.rs`:

- **Reading the settings:**
  - the owner's GET shows `effective`;
  - GET without a session omits `effective`.
- **Owner write:** POST applies live (the atomic changes) and persists the device override. `inherit` removes it.
- **Write refusals:**
  - a non-owner POST persists the override and does not touch the session;
  - no `launch` permission gives 403 `permission_required`;
  - a bad value gives 400 `invalid_value`;
  - a PyroWave session or unsupported codec gives state `unsupported` with its reason, and a POST gives 409 `not_writable`.
- **App override:** state `read_only` with the reason `app_override`.
- **Capabilities:** they list `stream.settings` and `stream.encoder_focus`.
- **Unchanged:** `PROTOCOL_VERSION` and `API_VERSION` (`git diff` shows neither).
- **Checks:** the web checks pass.

### C: Nightjar (Android phone and TV, desktop preview)

Starts after B is pushed. Repo `C:\src\rubylight-android`. Files (only these):

- `C:\src\rubylight-android\protocol\client\Cargo.toml` (`rubylight-native-api` rev set to B's commit) and `protocol\Cargo.lock`;
- `protocol\client\src\http\native.rs` (`stream_settings()`, `write_stream_settings()`);
- `protocol\android\src\native_api.rs` (ops `stream_settings` and `stream_settings_write`);
- new `protocol\desktop\src\stream_settings.rs`;
- `protocol\desktop\src\session_page.rs`;
- `protocol\desktop\src\strings.rs` and `strings\de.rs`;
- new `app\src\main\java\com\limelight\nativeapi\NativeStreamSettings.java`;
- `app\src\main\java\com\limelight\nativeapi\NativeApiClient.java`;
- `app\src\main\java\com\limelight\nativeapi\NativeCapabilities.java` (constants `STREAM_SETTINGS`, `ENCODER_FOCUS`);
- new `app\src\main\java\com\limelight\console\StreamSettingsController.java`, modelled on `NativeMenuController` (1 s poll while shown, one background thread);
- `app\src\main\java\com\limelight\ui\StreamMenu.java`;
- `app\src\main\java\com\limelight\preferences\HostStreamSettings.java`;
- `app\src\main\res\values\strings.xml`, plus `values-de` and the other locales the repo keeps in step;
- tests: new `app\src\test\java\com\limelight\nativeapi\NativeStreamSettingsTest.java`, `NativeApiFixtures.java`, new `app\src\test\java\com\limelight\console\StreamSettingsControllerTest.java`.

Steps:

1. Rust client calls and JNI ops, with tests modelled on the existing `radeon_write` tests in `protocol/android/src/native_api.rs`.
2. A Java parser for the document. Reuse `NativeRadeon`'s choice and state parsing where it is public; otherwise mirror it. Unknown states become UNKNOWN and are shown read-only.
3. **In-stream row:** it appears only when `NativeCapabilities.has(ENCODER_FOCUS)` and the document lists `encoder_focus`.
   - The subtitle shows the effective mode, or the reason text with the row disabled.
   - A choice writes POST, and the answer replaces the state.
   - A 404 hides the row and stops polling.
4. **`HostStreamSettings`:** the same row outside a stream. It writes the device override.
5. **Desktop Session page:** the same row, beside the Radeon panel.
6. **Strings:** host-neutral, with no host or GPU brand: "Encoder focus", "Still areas", "Still areas and centre", "Off", "Host setting".

Test commands:

- `cargo test --manifest-path C:\src\rubylight-android\protocol\Cargo.toml -p nightjar-core native`
- `cargo test --manifest-path C:\src\rubylight-android\protocol\Cargo.toml -p nightjar-android native_api`
- `cargo test --manifest-path C:\src\rubylight-android\protocol\Cargo.toml -p nightjar-desktop stream_settings`
- `.\gradlew.bat :app:testDebugUnitTest --tests "com.limelight.nativeapi.*" --tests "com.limelight.console.StreamSettingsControllerTest" --tests "com.limelight.preferences.SettingsKeysTest"`

Acceptance:

- B's fixtures parse on both the Rust and Java sides.
- The row is hidden on a host without the feature or route, and shown greyed with the reason when `unsupported`.
- A write updates the effective subtitle after the next poll.
- No preference key is added or renamed (`SettingsKeysTest` is unchanged and passes).
- applicationId and signing are untouched.
- The user runs a phone check against a host with B and reads the host log for `encoder focus` changes. Phone parts stay pending until then; the NUC can stand in for the desktop row.

### M: measurement harness and runs

**Harness** (implementer, can run in parallel with A). Files:

- new `C:\src\rubylight-rel\rust\release\focus_ab.py` (driver);
- new `focus_ab_result.py` (pure parsing, scoring and decision logic);
- new `test_focus_ab.py`;
- `chroma_fixtures.py`: a new `mixed` kind, a still editor desktop with a moving 640x360 clip window and a still HUD bar over a clip, plus a region manifest.

The driver:

- takes every path as an argument: `--amf-quality`, `--ffmpeg`, `--clips`, `--work`;
- runs ABBA quartets with bitrate rescaling like `amf_sweep.py quartet`;
- scores libvmaf, PSNR and SSIM with the corrected graph, plus crop masks and the settled phase;
- writes `runs.jsonl` and `summary.json` with the decision of section 9;
- turns an encoder exit code it does not know into a hard stop.

Test commands:

- `python -m unittest discover -s rust/release -p test_focus_ab.py`
- `python -m unittest discover -s rust/release -p test_chroma_quality.py`

Acceptance:

- The decision logic is unit-tested on synthetic rows: default-on, opt-in and drop.
- `estimate` prints the run count and runtime.

**Runs** (runner, after A2 is on main, user idle). Run M0 to M4 as in section 9, each batch in the background.

- Stop on exit code 3 or any unknown encoder failure.
- Return the summary tables and the decision.
- The main session then sets `AUTO` in `encoder_focus.rs` and writes the `rust/PERFORMANCE.md` section.

## 12. Sources

- AMF SDK 1.5.2.0 headers in `rust/windows/include/AMF/components/VideoEncoderVCE.h`, `VideoEncoderHEVC.h`, `VideoEncoderAV1.h` and `PreAnalysis.h` (line numbers above).
- AMD AMF docs: https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/doc/AMF_Video_Encode_API.md, https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/doc/AMF_Video_Encode_HEVC_API.md, https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/doc/AMF_Video_Encode_AV1_API.md
- SimpleROI sample: https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/public/samples/CPPSamples/SimpleROI/SimpleROI.cpp
- Host code read for this design:
  - `rust/windows/src/amf.rs` (`create_attempt`, `encode_gpu`, `prepare_surface`, `log_effective`);
  - `rust/windows/src/amf/gpu.rs`;
  - `rust/windows/src/compute.rs` (`Converter`, `convert`);
  - `rust/windows/src/shaders/color.hlsl`;
  - `rust/core/src/encoder_policy.rs` (PA and VBAQ rules);
  - `rust/core/src/session.rs`;
  - `rust/host/src/state.rs` (codec probe);
  - `rust/host/src/stream.rs` (per-frame encode);
  - `rust/host/src/native_api.rs` and `native_api/radeon.rs`;
  - `rust/core/native-api/src/*`.
- Nightjar:
  - `app/src/main/java/com/limelight/nativeapi/*`;
  - `console/NativeMenuController.java`;
  - `ui/StreamMenu.java`;
  - `protocol/android/src/native_api.rs`;
  - `protocol/desktop/src/radeon.rs`.
