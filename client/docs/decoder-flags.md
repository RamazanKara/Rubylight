# Decoder flags: Nightjar against Artemis and moonlight-noir

Nightjar and its two streaming engines (Rust and the classic C engine) share one MediaCodec setup: `MediaCodecHelper.setDecoderLowLatencyOptions` builds the format, `MediaCodecDecoderRenderer.initializeDecoder` walks the fallback ladder. This page compares that setup with two Moonlight forks, both GPL-3.0, and records what was ported, what was measured and what was left alone:

- [Artemis](https://github.com/ClassicOldSong/moonlight-android) by ClassicOldSong, whose vendor key lists Nightjar borrows from.
- [moonlight-noir](https://github.com/derflacco/moonlight-android) by derflacco, a fork of Artemis that adds MediaTek keys, reference frame invalidation (RFI) for more decoders, Snapdragon extras and the "Prefer Lower Delays" (LFR) mode. Its `MediaCodecHelper` and `MediaCodecDecoderRenderer` were diffed against Artemis's, so the table below covers everything it changed, not only what its README lists.

The code comments next to each ported key credit the fork they come from, and the [license and credits](index.md#license-and-credits) section names both.

## Rules the ports follow

- A vendor key goes only to the decoder it belongs to, matched by codec-name prefix (`c2.qti`, `c2.mtk`, `OMX.Nvidia`, ...). On Android 12 and newer a decoder that lists its vendor parameters gets the keys it declares, plus the Snapdragon, Tegra and MediaTek sets, which those decoders take by name.
- A decoder that rejects `configure()` gets the next try of the fallback ladder with fewer keys, down to a plain format. The format of every try is built fresh, so nothing carries over. The Snapdragon extras and the performance keys of the forks leave after the first tries, so a decoder that dislikes them still ends up with the plain low-latency keys.
- The applied keys and the keys dropped by rejected configures are logged once per stream (`Decoder <name> tuning keys applied: ...; dropped after rejected configures: ...`) and appear in the problem report as "Tuning keys applied" and "Tuning keys dropped after rejected configures". The overlay is unchanged.

## Comparison

"Both" means Nightjar's two engines, which cannot differ here. SoC names are the ones the source or its comments name. Rows where the forks and Nightjar agree are left out.

| Setting | Artemis | moonlight-noir | Nightjar before | Nightjar now |
| --- | --- | --- | --- | --- |
| `low-latency` | Every Android version, try 0 | Same | Android 11 and newer | Every Android version, try 0 |
| Qualcomm keys on `c2.qti`, `OMX.qcom` | low-latency, picture order 1, SW fence, output fence, fence type | Same, then `picture-order.enable` = 0 and `frame-drop.enable` = 1 on every try | The five keys | The five keys, then `frame-drop.enable` = 1 on tries 0 to 2, and the `c2.qcom` prefix. `picture-order` = 0 is not ported (see the Retroid section). From Moonlight V+: `info-misr.disable`, `instant-decode.enable` and `error-correction.conceal` = 1, only to decoders that declare them (Android 12 and newer), on try 0 only so a rejected one leaves the confirmed keys on the next tries; V+ sends them by name |
| `OMX.qcom` only | None | `dpb-output-delay.enable` = 0, `picture-type.enable` = 0 | None | The same two, tries 0 to 2 |
| Operating rate and priority on Snapdragon | Operating rate, except where upstream saw crashes | Both, always | Operating rate except Adreno 620, else priority | Same as before (see "Left alone") |
| MediaTek preset (`c2.mtk`, `OMX.MTK`) | 17 `vendor.mtk` keys, boost level 1 | Same, boost and DVFS level 2, `vendor.mtk.vdec.pq` = 0, `ultra-low-latency` removed | `low-latency.mode`, `cpu.boost.mode` and the `.value` forms | The whole preset with the fork's values, by name on Android 8 and newer, tries 0 and 1 (the plain vendor switches get try 2). From Moonlight V+: try 0 asks for queue depth 2, fetch timeout and guard interval 2 ms and no preload; try 1 relaxes to the full preset (depth 3, 4 ms, 1 frame) without `KEY_LOW_LATENCY`. V+ tiers by try the other way round (full preset first, then depth 2, then 1); here each fallback sends the keys of the try before or fewer, with values only relaxing toward the full preset. `cpu.boost.mode` = 2 reaches every MediaTek decoder that declares the key, not only the preset's |
| MediaTek operating rate and priority | Operating rate on its RFI lists | Both | Priority only | The rate only, behind the performance hints like the Snapdragon rate. The priority is not sent to MediaTek, where the fork sends it: rate plus real-time priority is the pair that crashes decoders after `start()` |
| NVIDIA Tegra keys (`OMX.Nvidia`, `c2.nvidia`) | Four keys: `media.low-latency.enable`, `vendor.low-latency.enable`, `disable-output-reorder`, `vendor.nvidia.disable-output-reorder` | Same, plus operating rate and priority | None | The four keys on tries 0 to 3, and the operating rate behind the performance hints (as Artemis), without the real-time priority |
| Samsung Exynos preset (`c2.exynos`, `OMX.Exynos`), H.264, HEVC and AV1 | The switch (`vendor.rtc-ext-dec-low-latency.enable`); the rest not compared | Same. Moonlight X (MoreOrLessSoftware, `mlsoft-x` branch, GPL-3.0) adds `vendor.rtc-ext-dec-output-queue-depth.value` = 2 and `vendor.sec-dec-output.delay` = 0 | The switch, and on Android 12 and newer only when the decoder lists it, which Exynos H.264 decoders do and HEVC decoders do not (issue 26) | The three keys by name, tries 0 and 1; the switch alone through try 4 (on Android 12 and newer the declared switch of Exynos H.264 decoders stays through try 4 as well). Pixel 6 to 9 (Tensor G1 to G4) are public-listed as exposing `c2.exynos` decoders too, so they take this row, not the Google one; we could not confirm that against Moonlight X. Experimental: no Exynos device here, so the effect is not measured |
| Google Codec2 (`c2.google`, Pixel/Tensor) | Not compared | Not compared. Moonlight X sets operating rate (the decoder's reported maximum for the stream size, at least 120 and the stream's frame rate, at most 960), priority 0, and `vendor.google.frame-rate.value` on Android 16 (API 36) and newer | Priority 0 behind the performance hints | The same three, tries 0 and 1 (the frame rate key on try 0), behind the vendor low-latency setting and the performance hints. The rate is the decoder's own maximum for the stream size (at most 960), never above what it declares; the 120 floor applies only to a decoder that declares at least 120 or reports nothing. Experimental: no Tensor device here |
| RFI (reference frame invalidation) | Exynos, Qualcomm, NVIDIA, Amlogic and Fire OS lists | Adds `c2.mtk` and `omx.mtk` for AVC and HEVC, `c2.qcom` for HEVC | `omx.mtk` and `c2.mtk` only on Fire OS and GX6 PowerVR | The fork's additions, behind the same GLES 3 check as the Qualcomm entries |
| RFI detection by declared vendor switch | Known low-latency switches | Adds eight MediaTek switches | Known switches | Five of them (`low-latency.strict.enable`, `early.output.enable`, `no.reorder.enable`, `disable.reorder`, `no.clock.adjust`), for HEVC RFI only, detected and never set. The delay and timeout knobs (`output.delay.frames`, `dpb.output.delay.frames`, `buffer.fetch.timeout.ms`) are no evidence of low latency and are left out; the AV1 whitelist is unchanged |
| LFR "Prefer Lower Delays" | None | Off by default. Latest-frame output path, frame pacing "lowest latency", synchronous codec, short dequeue wait | Not needed (see below) | Not added as a setting |
| Declared vendor parameters (Android 12 and newer), reasserting keys after `start()`, applied and dropped keys logged | Not used or logged | Post-start `low-latency` through `setParameters`, key audit log | Declared keys, reassert, overlay | Same, plus the exact applied and dropped keys in the log and report |

Settings identical in all three and not listed: `vdec-lowlatency`, the Exynos, Kirin and Amlogic keys, `FEATURE_LowLatency`, adaptive playback keys, colour range, `KEY_FRAME_RATE`, decoder blocklists and whitelists, the SPS and RFI quirks inherited from Moonlight.

### LFR and what Nightjar already does

moonlight-noir's LFR is a render-loop mode: with it on, the app picks "lowest latency" frame pacing, turns the async codec off, waits at most 1 ms for decoder output, presents the newest frame at once and drops at most one old frame. It is off by default in the fork. Nightjar's default is already that mode: frame pacing defaults to `latency` (`PreferenceConfiguration.DEFAULT_FRAME_PACING`), the renderer thread takes the newest decoder output and drops older ones (`wantsCatchUp`), there is no async codec path, and the present scheduler plans the release on the vsync grid. A 50 ms blocking dequeue returns as soon as a frame is ready, so the fork's 1 ms wait changes nothing but CPU use. Adding a second switch that selects the setting Nightjar already uses by default would only duplicate the frame pacing option, so none was added. The fork's single-drop rule is covered by the catch-up logic.

## Left alone

- **Qualcomm operating rate and priority on every Snapdragon.** The fork sets both in all cases. Upstream Moonlight saw reliable decoder crashes with them on the Snapdragon 765G (Adreno 620), and a crash after `start()` is outside the fallback ladder. Nightjar keeps its rule: operating rate on Snapdragon except Adreno 620, priority 0 elsewhere.
- **MediaTek `vendor.mtk.vdec.ultra-low-latency` = 0.** Commented out in the fork, so it is not sent.
- **AV1.** The fork widens its AV1 RFI test and returns "no AV1" from the decoder check. Nightjar's AV1 handling stays as is.
- **The fork's key audit and post-start `setParameters`.** Debug logging that Nightjar replaces with the applied and dropped lists and its own reassert after `start()`.
- **`KEY_OPERATING_RATE` on other non-Qualcomm decoders** (Exynos, Amlogic): upstream found crashes, and no fork sets it there unconditionally.
- **The no-output watchdog** in Artemis (flush after 1.2 s without output): stream recovery, not a latency flag, and Nightjar has its own.

## Retroid Pocket 6 (Snapdragon 8 Gen 2)

One combined check on 2026-10-24 for the Snapdragon part: the 1080p60 HEVC fixture through an isolated Rubylight host on the same PC, classic C engine, `c2.qti.hevc.decoder.low_latency`, about 88 s per run, the first 2 s dropped. Medians in ms (95th percentile in brackets); n is 4800 to 5200 frames. "Before" is the build without any of this work (two runs), "Artemis keys" the build with the Tegra, MediaTek and `low-latency` changes plus the logging (two runs), "All ported" this page's full set (one run).

| Build | Decode | Client total | End to end |
| --- | --- | --- | --- |
| Before, run 1 | 1.49 (2.10) | 2.48 (3.72) | 8.01 (11.50) |
| Before, run 2 | 1.49 (2.05) | 2.55 (3.81) | 7.95 (11.41) |
| Artemis keys, run 1 | 1.48 (2.06) | 2.53 (3.82) | 8.20 (11.65) |
| Artemis keys, run 2 | 1.49 (2.04) | 2.54 (3.85) | 8.08 (11.42) |
| All ported | 1.43 (2.08) | 2.27 (3.44) | 7.53 (10.73) |

- The Snapdragon keys Artemis sets on `c2.qti` were already set by Nightjar, so the "Artemis keys" build sends the identical key set and measures the same as before, within the run-to-run spread of about 0.1 ms.
- The fork's Snapdragon extras are one key on `c2.qti`: `frame-drop.enable` = 1 (applied, no rejected configure, nothing dropped). The "all ported" run is not worse; with one run, its 0.4 ms lower end to end is within what a second run could change, so it is not claimed as a gain.
- **`picture-order.enable` = 0 was not ported.** The fork sets it after the keys above. With it, `c2.qti.hevc.decoder.low_latency` accepted the configure and then produced no output at all (28 frames in, none out, `DecoderHungException` after 5 s). The fallback ladder cannot catch that, since the configure succeeded. Without the key the same build streams normally.
- Not measured: the other Snapdragon generations, MediaTek, NVIDIA and Amlogic (see below), and AV1 on this device.

## Experimental

Nightjar's own devices are a Retroid Pocket 6 (Snapdragon), an emulator and a Linux NUC. Only the Snapdragon keys can be measured here, so everything below is labelled Experimental: it follows the forks and is covered by unit tests (`DecoderTuningKeysTest`, a test per decoder name for the keys and for RFI), not by a run on that hardware:

- **MediaTek**: the whole preset, boost and DVFS level 2, `pq` = 0, operating rate on `c2.mtk` and `OMX.MTK` (behind the performance hints, no priority), RFI for `c2.mtk` and `omx.mtk`, the five detect-only switches. `cpu.boost.mode` = 2 goes to every MediaTek decoder that declares it.
- **NVIDIA Tegra**: the four keys on `OMX.Nvidia` and `c2.nvidia`, and the operating rate behind the performance hints.
- **`low-latency` before Android 11** on every decoder. Decoders that do not know the key ignore it, and a decoder that rejects the format loses it on the next try.
- **`c2.qcom` prefix** for Snapdragon decoders under that name, including RFI for HEVC.
- **`OMX.qcom` extras** (`dpb-output-delay`, `picture-type`): old Snapdragons only.
- **`frame-drop.enable` on Snapdragons other than the 8 Gen 2.** Only the Retroid's 8 Gen 2 was run; the key is sent to every Qualcomm decoder on tries 0 to 2.

Each is covered by a test per decoder name, and by one for the fallback ladder (the keys drop out of later tries), the vendor switch (off means none sent) and the Android 12 declared-parameter path.

## Fire TV Cube (Amlogic): a configure failure

Issue 21 reports every format failing `configure()` with error -5 on a Fire TV Cube, where Moonlight and Artemis work. Reading Artemis and moonlight-noir against Nightjar for Amlogic and Fire OS found no Amlogic key that either sends and Nightjar does not: both send `vendor.low-latency.enable` and `vdec-lowlatency` on the Amlogic OMX decoder, both whitelist `omx.amlogic` for HEVC on Fire OS, Artemis sends the operating rate on Android 8 and newer Amlogic (through its reference-invalidation list) where Nightjar sends `KEY_PRIORITY` = 0, but both only on the first tries, and the ladder drops them before the plain format. moonlight-noir changes nothing for Amlogic. Two things differ in what reaches `configure()` on Android 9:

- **A null HDR entry.** Nightjar wrote a null `KEY_HDR_STATIC_INFO` into every SDR format below Android 10. Artemis never writes the key without metadata. Fixed: the key is only cleared when the format has it.
- **Diagnostics inside the configure `try`.** `getInputFormat()` and the overlay diagnostics ran there, so a throw from either counted as a rejected configure on every try. Fixed: they log a warning and no longer fail the decoder.

Neither is confirmed against a Fire TV, so the issue stays open until its reporter tries a build. Other differences worth checking if the failure remains: `KEY_MAX_INPUT_SIZE`, which Nightjar sets above about 125 Mbps and neither fork sets (it is not part of the ladder), and the mandatory `vdec-lowlatency` for HEVC, which Nightjar limits to the Fire TV 3 (`AFTN`) while Artemis sends it to every Amazon device.
