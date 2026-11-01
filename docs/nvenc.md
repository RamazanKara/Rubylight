# NVENC

[Rubylight](../README.md) · [Docs](README.md) · [Architecture](architecture.md) · [Hardware matrix](hardware-matrix.md)

Rubylight encodes with AMF on an AMD GPU and with NVENC on an NVIDIA GPU. With `encoder = auto`, the host picks the encoder from the vendor of the GPU that captures. NVENC is called through NVIDIA's driver ABI (`rust/windows/src/nvenc.rs`), API 11 to 13, on the compute conversion's D3D12 device or the capture's D3D11 device. Its settings are the `nvenc_*` keys ([configuration](configuration.md)). Intel and software encoding stay removed; those PCs use [Rubylight 2.6.x](https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0).

## Parity with AMF

Each row lists a feature the AMF path has and how NVENC provides it. Fake-driver tests in `rust/windows/src/nvenc/tests.rs` cover every NVENC row except the GPU scheduling row, whose tests are in `gpu_priority.rs`.

| Feature | AMF | NVENC | Source and notes |
|---|---|---|---|
| Codecs | H.264, HEVC, AV1 | H.264, HEVC, AV1 | The codec check refuses a codec the driver does not list, before the stream starts (Foundation Sunshine). AV1 needs API 12.0 (NVIDIA driver 522.25 or newer). A driver past its session limit (`NV_ENC_ERR_OUT_OF_MEMORY` or `NV_ENC_ERR_INCOMPATIBLE_CLIENT_KEY` on open) is reported as that, naming the apps that usually hold the sessions (out of memory also names exhausted GPU memory), and no other driver API or device is tried. |
| 10-bit and HDR10 | Yes | Yes | API 13 embeds mastering display and content light metadata in the bitstream. Older drivers send it only on the control channel and log that. |
| HDR10+, HDR Vivid, Dolby Vision RPU | Yes | Yes | Both take the D3D11 conversion's luminance statistics and use the same splice (`amf::dynamic`). On a sliced frame the metadata goes before the first slice and the RPU after the last. |
| 4:4:4 | Packed 4:4:4 over 4:2:0 | Packed 4:4:4 over 4:2:0 | Native 4:4:4 is refused with an error: both conversions write NV12 or P010 only, which a 4:4:4 session cannot read. The CUDA interop for 10-bit 4:4:4 (`nvenc/cuda.rs`) waits for a conversion that writes 4:4:4. |
| Rate control | Yes | CBR (default), VBR (`nvenc_rate_control`) | A one-frame VBV, with `nvenc_vbv_increase`. VBR and its bounds follow Foundation Sunshine. |
| Bitrate changes | Yes | Yes | Reconfigured in place with frames in flight, without a reset or a keyframe, in both directions, where the driver reports `NV_ENC_CAPS_SUPPORT_DYN_BITRATE_CHANGE`. Without it, or when the driver refuses the change in place (logged as a warning), a change waits for the frames in flight, resets the encoder and forces a keyframe. |
| Reference frame invalidation | Yes | Yes | Used where the driver reports it and keeps more than one reference. Otherwise a lost frame is recovered with a keyframe. |
| Intra refresh | Yes | Yes | Off while slice delivery runs, as on AMF. |
| Frame cap | Writes the per-codec maximum frame size | Targets the one-frame VBV and lowers the VBV to a cap below it | Changes within a tenth are skipped, so a steady link never reconfigures. Lowering the VBV needs no reset, no keyframe and no wait for frames in flight. Keyframes are targeted too (`lowDelayKeyFrameScale` 1 and a one-frame VBV, as Sunshine), so there is no keyframe cap scale. The driver does not guarantee the bound: a frame over twice the cap logs a warning, at most every 10 s. |
| Scene-cut re-encode | Yes | Yes | An inter frame past the full-FEC bound that is alone in flight is submitted again as a keyframe from the kept input, with the same frame number. The rule is AMF's (`reencodes_cut`). A failed re-encode sends the inter frame and raises the same `encoder_reencode` event. |
| Slice delivery (`video_slice_delivery`) | Slice output mode | Subframe readback | `enableSubFrameWrite` and `reportSliceOffsets` require synchronous mode, so only these streams poll without completion events. A frame is complete when the lock reports `hwEncodeStatus` 2 (NVIDIA's developer forum); the slice count alone is not trusted. Slices leave early only while their offsets rise strictly and fit the bytes written, and the newest slice waits for the next lock, since it may be the last; otherwise the frame waits to finish and the log warns once. The first four frames' locks are logged at debug level. HEVC split encoding cannot run beside it, so a GPU with several NVENC engines keeps split encoding and sends whole frames (set `nvenc_split_encode = disabled` to stream slices). A driver without `NV_ENC_CAPS_SUPPORT_SUBFRAME_READBACK` warns and sends whole frames. No AV1, as on AMF. |
| Split-frame encoding | Not engaged by the driver | `nvenc_split_encode` (auto, forced, 2 to 4 strips) | Fixed strip counts and the slice-mode rule follow Foundation Sunshine. |
| Frame budget guard | Not needed | `nvenc_frame_budget_guard` | Lowers a preset whose estimated encode time misses the frame interval. The per-preset calibration comes from Foundation Sunshine. |
| Adaptive quantization | VBAQ and pre-analysis | Spatial AQ on by default, temporal AQ opt-in | Spatial AQ by default follows Foundation Sunshine and NVIDIA's game-streaming guidance. |
| GPU scheduling priority | High | Realtime, falling back to high when it is denied | Under hardware-accelerated GPU scheduling, `nvenc_realtime_hags` (on by default) keeps realtime. AMD stays at high: realtime would also raise AMF's and the game's queues. |
| Couch menu overlay | Yes | Yes | Drawn by the D3D11 conversion. |
| D3D12 compute conversion | Yes | Yes | See [D3D12 conversion](#d3d12-conversion). A native D3D12 session waits for the conversion's fence on the GPU (driver 522.25 or newer). Without it NVENC converts on the D3D11 graphics queue, which realtime scheduling covers. |
| Encoder focus (ROI map) | Yes | QP delta map | The map comes from the D3D12 compute conversion; the probe needs driver 531.61 or newer. See [Encoder focus](#encoder-focus). |

## D3D12 conversion

With `gpu_compute_conversion` on (the default), NVENC takes the compute queue's output as AMF does. The session opens on the compute queue's `ID3D12Device` (`Session::open` with an output fence):

- **The GPU waits, not the CPU.** For the D3D12 interface `nvEncMapInputResource` "does not provide synchronization guarantee" (`nvEncodeAPI.h`). Each picture's input is an `NV_ENC_INPUT_RESOURCE_D3D12` whose `inputFencePoint` waits for the conversion's fence at its value.
- **Output goes to READBACK buffers.** Each conversion target gets a buffer of twice an uncompressed picture (at most 16 MiB), registered as `NV_ENC_OUTPUT_BITSTREAM` and passed as an `NV_ENC_OUTPUT_RESOURCE_D3D12`. Its `outputFencePoint` signals the session's own fence.
- **Completion is read from the output fence.** The poll locks a frame once that fence reaches the frame's value and never waits on the CPU. A frame the fence never reaches fails at the completion timeout.
- **What the D3D12 interface leaves out.** It has no weighted prediction, so `nvenc_weighted_prediction` warns and stays off. Slice delivery returns whole frames there; set `gpu_compute_conversion = false` for either.
- **Driver floor.** The D3D12 interface needs API 12.0, NVIDIA driver 522.25 or newer. An older driver, a missing D3D12 device or a refused session warns (`encoder_conversion`) and converts on the D3D11 graphics queue, as AMF falls back. Native 4:4:4 is refused (see 4:4:4 above).

## Encoder focus

Focus works as on AMF: the D3D12 compute conversion builds the importance map from its still-age detector (`compute::Converter::latest_map`), and the same rules pick the frames that carry it ([`amf::focus`](../rust/windows/src/amf/focus.rs)). No map goes on a keyframe, a recovery picture, the encoder's first frame or the 30 frames after them, or when the map is uniform or not newer than the last one used. Without the D3D12 conversion, focus is off with the same reason as on AMF.

- **Delta map.** NVENC takes the map as a per-picture `qpDeltaMap` (`NV_ENC_QP_MAP_DELTA`): one signed offset per 16x16 macroblock for H.264, per 32x32 CTB for HEVC (the only CTB size NVENC codes) and per 64x64 superblock for AV1 (`nvenc::focus_deltas`). Turning focus on switches `qpMapMode` with a reconfigure, without a reset or a keyframe. The emphasis map is H.264-only, so it is not used.
- **Same weight as AMF.** Each importance step is half a QP (two q-index steps on AV1), the effect AMF's map measured on the RX 7900 XT. The offsets are centred on the picture's mean importance, so the map moves bits between blocks and leaves the frame's QP to the rate control and its one-frame VBV.
- **Startup check.** The probe encodes half-and-half maps, as on AMF, on a session of its own with the delta map and per-block statistics, so the codec check never depends on either. GeForce drivers cap the sessions open at once, so the probe closes the stream's session first and opens it again after, never two at a time. It reads each block's QP from `NV_ENC_OUTPUT_STATS_BLOCK` and needs an effect of 2 QP (8 q-index on AV1). Block statistics need API 12.1, NVIDIA driver 531.61 or newer (`NV_ENC_CAPS_OUTPUT_BLOCK_STATS`); without them focus reports that ROI cannot be verified.
- **Verification during a stream.** `encoder_focus_verify` turns block statistics on when the encoder is made, and `qp_delta` then reports the QP difference between the least and most important blocks, as on AMF.

The D3D11 conversion builds no map, so focus needs the D3D12 conversion on NVENC too.

## Not yet run on NVIDIA hardware

The rows above pass the fake-driver tests. These behaviours follow NVIDIA's headers and other hosts' code and still need a first run on an NVIDIA GPU:

- Subframe completion: `hwEncodeStatus` 2 as the end of a picture, and the slice offsets a partial lock reports. The debug lines of the first four frames settle both.
- Keyframes within the one-frame VBV, which the driver does not guarantee.
- Rate control changes applied with frames in flight.
- The D3D12 session: fence points, READBACK output and completion by fence.
- Encoder focus: the delta map's effect per codec, `qpMapMode` switched by a reconfigure, and the block statistics the probe reads.
