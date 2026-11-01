# NVENC

[Rubylight](../README.md) · [Docs](README.md) · [Architecture](architecture.md) · [Hardware matrix](hardware-matrix.md)

Rubylight encodes with AMF on an AMD GPU and with NVENC on an NVIDIA GPU. With `encoder = auto`, the host picks the encoder from the vendor of the GPU that captures. NVENC is called through NVIDIA's driver ABI (`rust/windows/src/nvenc.rs`), API 11 to 13, on the capture's D3D11 device. Its settings are the `nvenc_*` keys ([configuration](configuration.md)). Intel and software encoding stay removed; those PCs use [Rubylight 2.6.x](https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0).

## Parity with AMF

Each row lists a feature the AMF path has and how NVENC provides it. Fake-driver tests in `rust/windows/src/nvenc/tests.rs` cover every NVENC row except the GPU scheduling row, whose tests are in `gpu_priority.rs`.

| Feature | AMF | NVENC | Source and notes |
|---|---|---|---|
| Codecs | H.264, HEVC, AV1 | H.264, HEVC, AV1 | The codec check refuses a codec the driver does not list, before the stream starts (Foundation Sunshine). |
| 10-bit and HDR10 | Yes | Yes | API 13 embeds mastering display and content light metadata in the bitstream. Older drivers send it only on the control channel and log that. |
| HDR10+, HDR Vivid, Dolby Vision RPU | Yes | Yes | Both take the D3D11 conversion's luminance statistics and use the same splice (`amf::dynamic`). On a sliced frame the metadata goes before the first slice and the RPU after the last. |
| 4:4:4 | Packed 4:4:4 over 4:2:0 | Packed 4:4:4 and native 4:4:4 where the driver lists it | 10-bit 4:4:4 input goes through CUDA interop (`nvenc/cuda.rs`). |
| Rate control | Yes | CBR (default), VBR (`nvenc_rate_control`) | A one-frame VBV, with `nvenc_vbv_increase`. VBR and its bounds follow Foundation Sunshine. |
| Bitrate changes | Yes | Yes | Reconfigured in place. A higher bitrate resets the encoder and forces a keyframe. |
| Reference frame invalidation | Yes | Yes | Used where the driver reports it and keeps more than one reference. Otherwise a lost frame is recovered with a keyframe. |
| Intra refresh | Yes | Yes | Off while slice delivery runs, as on AMF. |
| Frame cap | Writes the per-codec maximum frame size | Holds frames to the one-frame VBV and lowers the VBV to a cap below it | Changes within a tenth are skipped, so a steady link never reconfigures. Lowering the VBV needs no reset and no keyframe. NVENC holds keyframes to the VBV (`lowDelayKeyFrameScale`), so it needs no keyframe cap scale. |
| Scene-cut re-encode | Yes | Yes | An inter frame past the full-FEC bound that is alone in flight is submitted again as a keyframe from the kept input, with the same frame number. The rule is AMF's (`reencodes_cut`). A failed re-encode sends the inter frame and raises the same `encoder_reencode` event. |
| Slice delivery (`video_slice_delivery`) | Slice output mode | Subframe readback | `enableSubFrameWrite` and `reportSliceOffsets` require synchronous mode, so only these streams poll without completion events. A frame is complete once every slice the session set is written (`sliceMode` 3). `hwEncodeStatus` is not read because the SDK leaves its values undocumented. HEVC split encoding cannot run beside it, so a GPU with several NVENC engines keeps split encoding and sends whole frames (set `nvenc_split_encode = disabled` to stream slices). A driver without `NV_ENC_CAPS_SUPPORT_SUBFRAME_READBACK` warns and sends whole frames. No AV1, as on AMF. |
| Split-frame encoding | Not engaged by the driver | `nvenc_split_encode` (auto, forced, 2 to 4 strips) | Fixed strip counts and the slice-mode rule follow Foundation Sunshine. |
| Frame budget guard | Not needed | `nvenc_frame_budget_guard` | Lowers a preset whose estimated encode time misses the frame interval. The per-preset calibration comes from Foundation Sunshine. |
| Adaptive quantization | VBAQ and pre-analysis | Spatial AQ on by default, temporal AQ opt-in | Spatial AQ by default follows Foundation Sunshine and NVIDIA's game-streaming guidance. |
| GPU scheduling priority | High | Realtime, falling back to high when it is denied | Under hardware-accelerated GPU scheduling, `nvenc_realtime_hags` (on by default) keeps realtime. AMD stays at high: realtime would also raise AMF's and the game's queues. |
| Couch menu overlay | Yes | Yes | Drawn by the D3D11 conversion. |
| D3D12 compute conversion | Yes | No | See [D3D12 conversion](#d3d12-conversion). NVENC converts on the D3D11 graphics queue. Realtime scheduling covers that queue. |
| Encoder focus (ROI map) | Yes | No | The map comes from the D3D12 compute conversion. See [Encoder focus](#encoder-focus). Focus reports `nvenc` as its reason. |

## D3D12 conversion

AMF can take the compute queue's output directly: a D3D12 resource that a D3D12 fence completes. NVENC cannot take it in this release, for these reasons:

- **The D3D11 session never waits on the compute fence.** NVENC runs on the capture's D3D11 device. `nvEncMapInputResource` guarantees that graphics work submitted on the input has finished before encoding. A D3D12 fence on another device is not graphics work on that input, so the session never waits for it.
- **A D3D12 session has to wait on the fence itself.** For the D3D12 interface the same call "does not provide synchronization guarantee" (`nvEncodeAPI.h`). The wait must be passed in, which takes a second session type opened on the compute queue's `ID3D12Device`:
  - Each picture's input is an `NV_ENC_INPUT_RESOURCE_D3D12` whose `inputFencePoint` waits for the compute fence at the conversion's value.
  - Output goes to READBACK buffers the application allocates, registered as `NV_ENC_OUTPUT_BITSTREAM` and passed as `NV_ENC_OUTPUT_RESOURCE_D3D12` with an output fence.
  - `NV_ENC_INITIALIZE_PARAMS::bufferFormat` must be set.
  - Completion is read from the output fence, not from events.
- **The workarounds cost latency or the graphics queue.**
  - Waiting on the CPU for the compute fence before handing a shared texture to the D3D11 session adds a thread wake-up to every frame.
  - A D3D11 queue wait followed by a copy puts work back on the graphics queue that compute conversion is meant to avoid.

The D3D12 session is not built in this change. Without an NVIDIA GPU there is nothing to check its fence handling against.

## Encoder focus

AMF attaches focus as an ROI map that the D3D12 compute conversion builds from its still-age detector (`compute::Converter::latest_map`). NVENC converts on D3D11, which builds no map. The NVENC side of focus is in the SDK:

- **Delta map.** `NV_ENC_RC_PARAMS::qpMapMode = NV_ENC_QP_MAP_DELTA` takes a per-picture `qpDeltaMap` of signed QP offsets: one per 16x16 macroblock for H.264, one per CTB for HEVC and one per 64x64 superblock for AV1.
- **Emphasis map.** `NV_ENC_QP_MAP_EMPHASIS` is H.264-only and cannot run with spatial or temporal AQ, so the delta map is the one to use.

Focus can follow once NVENC consumes the D3D12 conversion. Until then the console and the log give `nvenc` as the reason focus is off.
