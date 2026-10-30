# Decoder flags

[Documentation](index.md)

Nightjar and its two streaming engines (Rust and the classic C engine) share one MediaCodec setup: `MediaCodecHelper.setDecoderLowLatencyOptions` builds the format, `MediaCodecDecoderRenderer.initializeDecoder` walks the fallback ladder. This page lists the low-latency keys Nightjar sends to each decoder, what was measured and what is deliberately not sent. "Both engines" cannot differ here.

## Rules

- A vendor key goes only to the decoder it belongs to, matched by codec-name prefix (`c2.qti`, `c2.mtk`, `OMX.Nvidia`, ...). On Android 12 and newer a decoder that lists its vendor parameters gets the keys it declares, plus the Snapdragon, Tegra and MediaTek sets, which those decoders take by name.
- A decoder that rejects `configure()` gets the next try of the fallback ladder with fewer keys, down to a plain format. The format of every try is built fresh, so nothing carries over. The Snapdragon extras and the performance keys leave after the first tries, so a decoder that dislikes them still ends up with the plain low-latency keys.
- The applied keys and the keys dropped by rejected configures are logged once per stream (`Decoder <name> tuning keys applied: ...; dropped after rejected configures: ...`) and appear in the problem report as "Tuning keys applied" and "Tuning keys dropped after rejected configures". The overlay is unchanged.

## What each decoder gets

SoC names are the ones the source or its comments name.

| Setting | What Nightjar sends |
| --- | --- |
| `low-latency` | Every Android version, try 0. Decoders that do not know the key ignore it. |
| Qualcomm keys on `c2.qti`, `c2.qcom`, `OMX.qcom` | Low-latency, picture order 1, SW fence, output fence and fence type, then `frame-drop.enable` = 1 on tries 0 to 2. `info-misr.disable`, `instant-decode.enable` and `error-correction.conceal` = 1 only to decoders that declare them (Android 12 and newer), on try 0 only, so a rejected one leaves the confirmed keys on the next tries. `picture-order.enable` = 0 is not sent (see the Retroid section). |
| `OMX.qcom` only | `dpb-output-delay.enable` = 0 and `picture-type.enable` = 0, tries 0 to 2. |
| Operating rate and priority on Snapdragon | Operating rate except on the Adreno 620, priority 0 elsewhere (see "Not sent"). |
| MediaTek preset (`c2.mtk`, `OMX.MTK`) | 17 `vendor.mtk` keys with boost and DVFS level 2 and `vendor.mtk.vdec.pq` = 0, by name on Android 8 and newer, tries 0 and 1 (the plain vendor switches get try 2). Try 0 asks for queue depth 2, fetch timeout and guard interval 2 ms and no preload; try 1 relaxes to the full preset (depth 3, 4 ms, 1 frame) without `KEY_LOW_LATENCY`. Each fallback sends the keys of the try before or fewer, with values only relaxing toward the full preset. `cpu.boost.mode` = 2 reaches every MediaTek decoder that declares the key, not only the preset's. No vendor keys at all while "Render through Vulkan" draws the frames: with them the TCL C6K's decoder took no input after one to four frames. |
| MediaTek operating rate and priority | The rate only, behind the performance hints like the Snapdragon rate. No priority: rate plus real-time priority is the pair that crashes decoders after `start()`. |
| NVIDIA Tegra keys (`OMX.Nvidia`, `c2.nvidia`) | `media.low-latency.enable`, `vendor.low-latency.enable`, `disable-output-reorder` and `vendor.nvidia.disable-output-reorder` on tries 0 to 3, and the operating rate behind the performance hints, without the real-time priority. |
| Samsung Exynos preset (`c2.exynos`, `OMX.Exynos`), H.264, HEVC and AV1 | `vendor.rtc-ext-dec-low-latency.enable`, `vendor.rtc-ext-dec-output-queue-depth.value` = 2 and `vendor.sec-dec-output.delay` = 0 by name, tries 0 and 1; the switch alone through try 4 (on Android 12 and newer the declared switch of Exynos H.264 decoders stays through try 4 as well). Earlier builds sent the switch only when the decoder listed it, which Exynos H.264 decoders do and HEVC decoders do not (issue 26). Pixel 6 to 9 (Tensor G1 to G4) are public-listed as exposing `c2.exynos` decoders too, so they take this row, not the Google one. Not measured: no Exynos device here. |
| Google Codec2 (`c2.google`, Pixel/Tensor) | Operating rate, priority 0, and `vendor.google.frame-rate.value` on Android 16 (API 36) and newer, tries 0 and 1 (the frame rate key on try 0), behind the vendor low-latency setting and the performance hints. The rate is the decoder's own maximum for the stream size (at most 960), never above what it declares; the 120 floor applies only to a decoder that declares at least 120 or reports nothing. Not measured: no Tensor device here. |
| RFI (reference frame invalidation) | Exynos, Qualcomm, NVIDIA, Amlogic and Fire OS lists, plus `c2.mtk` and `omx.mtk` for AVC and HEVC and `c2.qcom` for HEVC, the last behind the same GLES 3 check as the other Qualcomm entries. |
| RFI detection by declared vendor switch | The known low-latency switches, and five MediaTek ones (`low-latency.strict.enable`, `early.output.enable`, `no.reorder.enable`, `disable.reorder`, `no.clock.adjust`), for HEVC RFI only, detected and never set. The delay and timeout knobs (`output.delay.frames`, `dpb.output.delay.frames`, `buffer.fetch.timeout.ms`) are no evidence of low latency and are left out; the AV1 whitelist is unchanged. |
| Declared vendor parameters (Android 12 and newer) | The declared keys, reasserted after `start()`, with the exact applied and dropped keys in the log and the problem report. |

Also sent, and not listed above: `vdec-lowlatency`, the Kirin and Amlogic keys, `FEATURE_LowLatency`, adaptive playback keys, colour range, `KEY_FRAME_RATE`, decoder blocklists and whitelists, and the SPS and RFI quirks inherited from Moonlight.

### No separate lowest-delay mode

Nightjar's default render loop already takes the shortest path: frame pacing defaults to `latency` (`PreferenceConfiguration.DEFAULT_FRAME_PACING`), the renderer thread takes the newest decoder output and drops older ones (`wantsCatchUp`), there is no async codec path, and the present scheduler plans the release on the vsync grid. A 50 ms blocking dequeue returns as soon as a frame is ready, so a shorter wait would change nothing but CPU use. A second switch would only duplicate the frame pacing option, so there is none.

## Not sent

- **Qualcomm operating rate and priority on every Snapdragon.** Moonlight saw reliable decoder crashes with them on the Snapdragon 765G (Adreno 620), and a crash after `start()` is outside the fallback ladder. Nightjar sends the operating rate on Snapdragon except the Adreno 620, priority 0 elsewhere.
- **MediaTek `vendor.mtk.vdec.ultra-low-latency`.**
- **`KEY_OPERATING_RATE` on other non-Qualcomm decoders** (Exynos, Amlogic): Moonlight found crashes there.
- **A no-output watchdog** that flushes the decoder after a second without output: that is stream recovery, not a latency flag, and Nightjar has its own.

## Retroid Pocket 6 (Snapdragon 8 Gen 2)

One combined check on 2026-10-24 for the Snapdragon part: the 1080p60 HEVC fixture through an isolated Rubylight host on the same PC, classic C engine, `c2.qti.hevc.decoder.low_latency`, about 88 s per run, the first 2 s dropped. Medians in ms (95th percentile in brackets); n is 4800 to 5200 frames. "Before" is the build without any of this work (two runs), "Vendor lists" the build with the Tegra, MediaTek and `low-latency` changes plus the logging (two runs), "All keys" this page's full set (one run).

| Build | Decode | Client total | End to end |
| --- | --- | --- | --- |
| Before, run 1 | 1.49 (2.10) | 2.48 (3.72) | 8.01 (11.50) |
| Before, run 2 | 1.49 (2.05) | 2.55 (3.81) | 7.95 (11.41) |
| Vendor lists, run 1 | 1.48 (2.06) | 2.53 (3.82) | 8.20 (11.65) |
| Vendor lists, run 2 | 1.49 (2.04) | 2.54 (3.85) | 8.08 (11.42) |
| All keys | 1.43 (2.08) | 2.27 (3.44) | 7.53 (10.73) |

- The "Vendor lists" build sends the same key set on `c2.qti` as before, and measures the same, within the run-to-run spread of about 0.1 ms.
- The Snapdragon extra on `c2.qti` is one key: `frame-drop.enable` = 1 (applied, no rejected configure, nothing dropped). The "All keys" run is not worse; with one run, its 0.4 ms lower end to end is within what a second run could change, so it is not claimed as a gain.
- **`picture-order.enable` = 0 is not sent.** With it, `c2.qti.hevc.decoder.low_latency` accepted the configure and then produced no output at all (28 frames in, none out, `DecoderHungException` after 5 s). The fallback ladder cannot catch that, since the configure succeeded. Without the key the same build streams normally.
- Not measured: the other Snapdragon generations, MediaTek, NVIDIA and Amlogic (see below), and AV1 on this device.

## Not measured here

Nightjar's own devices are a Retroid Pocket 6 (Snapdragon), a TCL C6K TV (MediaTek), an emulator and a Linux NUC. The keys below have not been timed on their hardware; each is covered by unit tests (`DecoderTuningKeysTest`, a test per decoder name for the keys and for RFI):

- **MediaTek**: the whole preset, boost and DVFS level 2, `pq` = 0, operating rate on `c2.mtk` and `OMX.MTK` (behind the performance hints, no priority), RFI for `c2.mtk` and `omx.mtk`, the five detect-only switches. `cpu.boost.mode` = 2 goes to every MediaTek decoder that declares it.
- **NVIDIA Tegra**: the four keys on `OMX.Nvidia` and `c2.nvidia`, and the operating rate behind the performance hints.
- **`low-latency` before Android 11** on every decoder. Decoders that do not know the key ignore it, and a decoder that rejects the format loses it on the next try.
- **`c2.qcom` prefix** for Snapdragon decoders under that name, including RFI for HEVC.
- **`OMX.qcom` extras** (`dpb-output-delay`, `picture-type`): old Snapdragons only.
- **`frame-drop.enable` on Snapdragons other than the 8 Gen 2.** Only the Retroid's 8 Gen 2 was run; the key is sent to every Qualcomm decoder on tries 0 to 2.

Each is covered by a test per decoder name, and by one for the fallback ladder (the keys drop out of later tries), the vendor switch (off means none sent) and the Android 12 declared-parameter path.

## Fire TV Cube (Amlogic): a configure failure

Issue 21 reports every format failing `configure()` with error -5 on a Fire TV Cube. Nightjar sends `vendor.low-latency.enable` and `vdec-lowlatency` on the Amlogic OMX decoder, whitelists `omx.amlogic` for HEVC on Fire OS and sends `KEY_PRIORITY` = 0 on Android 8 and newer Amlogic, only on the first tries, and the ladder drops them before the plain format. Two things reached `configure()` on Android 9 that should not have:

- **A null HDR entry.** Nightjar wrote a null `KEY_HDR_STATIC_INFO` into every SDR format below Android 10. Fixed: the key is only cleared when the format has it.
- **Diagnostics inside the configure `try`.** `getInputFormat()` and the overlay diagnostics ran there, so a throw from either counted as a rejected configure on every try. Fixed: they log a warning and no longer fail the decoder.

Neither is confirmed on a Fire TV, so the issue stays open until its reporter tries a build. Other things worth checking if the failure remains: `KEY_MAX_INPUT_SIZE`, which Nightjar sets above about 125 Mbps (it is not part of the ladder), and the mandatory `vdec-lowlatency` for HEVC, which Nightjar limits to the Fire TV 3 (`AFTN`).

## Credits

The vendor key lists follow [Artemis](https://github.com/ClassicOldSong/moonlight-android) by ClassicOldSong, the [moonlight-noir fork](https://github.com/derflacco/moonlight-android) by derflacco, [Moonlight X](https://github.com/MoreOrLessSoftware/moonlight-android/tree/mlsoft-x) by MoreOrLessSoftware and [Moonlight V+](https://github.com/qiin2333/moonlight-vplus) by qiin2333 and contributors, all GPL-3.0. The code comments next to each key credit its source, and the [license and credits](index.md#license-and-credits) section names them all.
