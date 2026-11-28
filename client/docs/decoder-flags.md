# Decoder flags

[Nightjar docs](index.md) › [Reference](index.md#reference)

Nightjar's streaming engine (Rust) uses one MediaCodec setup: `MediaCodecHelper.setDecoderLowLatencyOptions` builds the format, `MediaCodecDecoderRenderer.initializeDecoder` walks the fallback ladder. This page lists the low-latency keys Nightjar sends to each decoder, what was measured and what is deliberately not sent. "Both engines" cannot differ here.

## Rules

- A vendor key goes only to the decoder it belongs to, matched by codec-name prefix (`c2.qti`, `c2.mtk`, `OMX.Nvidia`, ...). On Android 12 and newer a decoder that lists its vendor parameters gets the keys it declares that belong to its vendor, plus the Snapdragon, Tegra and MediaTek sets, which those decoders take by name. Snapdragon's `vendor.qti-ext-*` keys, the output fence included, reach only `omx.qcom`, `c2.qti` and `c2.qcom` decoders: another vendor's decoder that declares them does not implement them.
- A decoder that rejects `configure()` gets the next try of the fallback ladder with fewer keys, down to a plain format. The format of every try is built fresh, so nothing carries over. The Snapdragon extras and the performance keys leave after the first tries, so a decoder that dislikes them still ends up with the plain low-latency keys.
- A decoder that takes no input for 5 seconds after `start()`, which no configure fallback can catch, is reset with the same format. If it hangs again before it has run well since (600 frames over 10 seconds, the measure that refills the recovery attempts) and the vendor low-latency setting added keys to its format, the ladder sets it up again on a new codec without the vendor keys instead of ending the stream: Android's own `low-latency` flag and the performance hint stay, the format the Vulkan renderer gives MediaTek decoders. If that hangs too, the stream ends. Each decoder gets that setup once per stream: one taking over, such as HEVC after a refused Dolby Vision setup, starts with its own keys and gets its own reset first. Once a decoder has run well without them (600 frames over 10 seconds with no hang, failure or recovery since, except a planned restart, such as for a size or HDR change, that succeeds at its first step, and no input wait of half a second running when the check looks), the next streams start it, matched by codec name and MIME type, without them; a decoder taking over records nothing about the one before. An app or system update drops the record (its own preferences file) and tries the keys again.
- The applied keys and the keys dropped by rejected configures are logged once per stream (`Decoder <name> tuning keys applied: ...; dropped after rejected configures: ...`) and appear in the problem report as "Tuning keys applied" and "Tuning keys dropped after rejected configures". Keys dropped after a hang are logged when they go (`<name> took no input for 5000 ms with its vendor low-latency keys, again after a reset; setting it up again without them: ...`), at each setup without them (`Decoder <name> gets no vendor low-latency keys: it took no input with them ...`), at the end of the applied-keys line (`; dropped after a hang: ...`) and in the problem report as "Tuning keys dropped after a hang". The overlay is unchanged.

## What each decoder gets

SoC names are the ones the source or its comments name.

| Setting | What Nightjar sends |
| --- | --- |
| `low-latency` | Every Android version, try 0. Decoders that do not know the key ignore it. |
| Qualcomm keys on `c2.qti`, `c2.qcom`, `OMX.qcom` | Low-latency, picture order 1, SW fence, output fence and fence type, then `frame-drop.enable` = 1 on tries 0 to 2 only with `debug.nightjar.qti_frame_drop=1` (off by default since 2026-11-17: it let the decoder drop a picture the screen never saw, and the pacing rig could not tell it from the setting off). `info-misr.disable`, `instant-decode.enable` and `error-correction.conceal` = 1 only to decoders that declare them (Android 12 and newer), on try 0 only, so a rejected one leaves the confirmed keys on the next tries. `picture-order.enable` = 0 is not sent (see the Retroid section). |
| `OMX.qcom` only | `dpb-output-delay.enable` = 0 and `picture-type.enable` = 0, tries 0 to 2. |
| Operating rate and priority on Snapdragon | Operating rate except on the Adreno 620, priority 0 elsewhere (see "Not sent"). |
| MediaTek preset (`c2.mtk`, `OMX.MTK`) | 15 `vendor.mtk` keys with boost and DVFS level 1 (including the `.value` twins of the fetch timeout and the guard interval, which some MediaTek builds read instead of the plain names), by name on Android 8 and newer, tries 0 and 1 (the plain vendor switches get try 2). AV1 gets only the eight low-latency, boost, DVFS, idle and vsync switches, without queue, wait or preload keys, on both tries: a MediaTek AV1 decoder on a Fire TV showed wrong colours for whole streams with either queue setting. Try 0 asks for queue depth 2, fetch timeout and guard interval 2 ms and no preload; try 1 relaxes to the full preset (depth 3, 4 ms, 1 frame) without `KEY_LOW_LATENCY`. Each fallback sends the keys of the try before or fewer, with values only relaxing toward the full preset. `cpu.boost.mode` = 1 reaches every MediaTek decoder that declares the key, not only the preset's. The preset sends none of `pq`, `nvop.skip`, `skip.mode`, `drop.nonref.frame` and `frame-drop.policy` (since 2026-11-19, as the derflacco fork's `Artemide_Experimental_Async` branch at `32e38f0e` sends none of them and boost and DVFS level 1; its `moonlight-noir` branch still sends level 2 and the five keys). No vendor keys at all while "Render through Vulkan" draws the frames: with them the TCL C6K's decoder took no input after one to four frames. |
| MediaTek operating rate and priority | The rate only, behind the performance hints like the Snapdragon rate. No priority: rate plus real-time priority is the pair that crashes decoders after `start()`. |
| NVIDIA Tegra keys (`OMX.Nvidia`, `c2.nvidia`) | `media.low-latency.enable`, `vendor.low-latency.enable`, `disable-output-reorder` and `vendor.nvidia.disable-output-reorder` on tries 0 to 3, and the operating rate behind the performance hints, without the real-time priority. |
| Samsung Exynos preset (`c2.exynos`, `OMX.Exynos`), H.264, HEVC and AV1 | `vendor.rtc-ext-dec-low-latency.enable`, `vendor.rtc-ext-dec-output-queue-depth.value` = 2 and `vendor.sec-dec-output.delay` = 0 by name, tries 0 and 1; the switch alone through try 4 (on Android 12 and newer the declared switch of Exynos H.264 decoders stays through try 4 as well). Earlier builds sent the switch only when the decoder listed it, which Exynos H.264 decoders do and HEVC decoders do not (issue 26). Pixel 6 to 9 (Tensor G1 to G4) are public-listed as exposing `c2.exynos` decoders too, so they take this row, not the Google one. Not measured: no Exynos device here. |
| Google Codec2 (`c2.google`, Pixel/Tensor) | Operating rate, priority 0, and `vendor.google.frame-rate.value` on Android 16 (API 36) and newer, tries 0 and 1 (the frame rate key on try 0), behind the vendor low-latency setting and the performance hints. The rate is the decoder's own maximum for the stream size (at most 960), never above what it declares; the 120 floor applies only to a decoder that declares at least 120 or reports nothing. Not measured: no Tensor device here. |
| RFI (reference frame invalidation) | Exynos, Qualcomm, NVIDIA, Amlogic and Fire OS lists, plus `c2.mtk` and `omx.mtk` for AVC and HEVC and `c2.qcom` for HEVC, the last behind the same GLES 3 check as the other Qualcomm entries. Never for HEVC on a Fire TV's Amlogic decoders, or when HEVC low-latency flags are omitted under Off or Auto on `c2.amlogic` (see below). |
| RFI detection by declared vendor switch | The known low-latency switches, and five MediaTek ones (`low-latency.strict.enable`, `early.output.enable`, `no.reorder.enable`, `disable.reorder`, `no.clock.adjust`), for HEVC RFI only, detected and never set. The delay and timeout knobs (`output.delay.frames`, `dpb.output.delay.frames`, `buffer.fetch.timeout.ms`) are no evidence of low latency and are left out; the AV1 whitelist is unchanged. |
| Declared vendor parameters (Android 12 and newer) | The declared keys of the decoder's own vendor, reasserted after `start()`, with the exact applied and dropped keys in the log and the problem report. |

### Per-device exceptions

- **Amlogic HEVC on Fire TV** (`omx.amlogic`, `c2.amlogic`): no reference frame invalidation. The Fire TV Cube 3 stopped rendering seconds into an HEVC stream from a host that marks long-term references; without RFI the stream asks for one reference frame and recovers from loss with a keyframe. Older `omx.amlogic` decoders on other devices keep RFI, and `c2.amlogic` decoders on other devices keep it when HEVC low-latency flags are set to On (see below).
- **Amlogic Codec2 HEVC outside Fire TV** (`c2.amlogic`): the HEVC low-latency flags and HEVC RFI are left out while the HEVC low-latency setting is on Automatic, since with them the decoder configures and then shows about one frame a second. Fire TV keeps the flags: its Fire TV 3 shows no frames without low-latency decoding.
- **Xiaomi FX Mini LED Fire TV** (`AFTNE4E43F`), MediaTek decoders: Android low-latency mode starts off, once and again after Reset all settings, because the picture can break into coloured blocks with it. Turned back on, the stream asks for one reference frame, asks for a keyframe when decode time climbs and when the climb ends (and at least every 30 seconds), and spaces out frames that arrive right behind each other.

Also sent, and not listed above: `vdec-lowlatency`, the Kirin and Amlogic keys, `FEATURE_LowLatency`, adaptive playback keys, colour range, `KEY_FRAME_RATE`, decoder blocklists and whitelists, and the SPS and RFI quirks inherited from Moonlight.

### No separate lowest-delay mode

Nightjar's default render loop already takes the shortest path: frame pacing defaults to `latency` (`PreferenceConfiguration.DEFAULT_FRAME_PACING`), the renderer thread takes the newest decoder output and drops older ones (`wantsCatchUp`), there is no async codec path, and the present scheduler plans the release on the vsync grid. A 50 ms blocking dequeue returns as soon as a frame is ready, so a shorter wait would change nothing but CPU use. A second switch would only duplicate the frame pacing option, so there is none.

## Not sent

- **Qualcomm operating rate and priority on every Snapdragon.** Moonlight saw reliable decoder crashes with them on the Snapdragon 765G (Adreno 620), and a crash after `start()` is outside the fallback ladder. Nightjar sends the operating rate on Snapdragon except the Adreno 620, priority 0 elsewhere.
- **MediaTek `vendor.mtk.vdec.ultra-low-latency`.**
- **`KEY_OPERATING_RATE` on other non-Qualcomm decoders** (Exynos, Amlogic): Moonlight found crashes there.
- **A no-output watchdog** that flushes the decoder after a second without output: that is stream recovery, not a latency flag, and Nightjar has its own.

## Retroid Pocket 6 (Snapdragon 8 Gen 2)

One combined check on 2026-10-24 for the Snapdragon part: the 1080p60 HEVC fixture through an isolated Rubylight host on the same PC, on the classic C engine (since removed), `c2.qti.hevc.decoder.low_latency`, about 88 s per run, the first 2 s dropped. Medians in ms (95th percentile in brackets); n is 4800 to 5200 frames. "Before" is the build without any of this work (two runs), "Vendor lists" the build with the Tegra, MediaTek and `low-latency` changes plus the logging (two runs), "All keys" this page's full set (one run).

| Build | Decode | Client total | End to end |
| --- | --- | --- | --- |
| Before, run 1 | 1.49 (2.10) | 2.48 (3.72) | 8.01 (11.50) |
| Before, run 2 | 1.49 (2.05) | 2.55 (3.81) | 7.95 (11.41) |
| Vendor lists, run 1 | 1.48 (2.06) | 2.53 (3.82) | 8.20 (11.65) |
| Vendor lists, run 2 | 1.49 (2.04) | 2.54 (3.85) | 8.08 (11.42) |
| All keys | 1.43 (2.08) | 2.27 (3.44) | 7.53 (10.73) |

- The "Vendor lists" build sends the same key set on `c2.qti` as before, and measures the same, within the run-to-run spread of about 0.1 ms.
- The Snapdragon extra on `c2.qti` is one key: `frame-drop.enable` = 1, sent only with `debug.nightjar.qti_frame_drop=1` since 2026-11-17 (it was on by default in this table's runs: applied, no rejected configure, nothing dropped; the 2026-11-17 pacing rig measured it against off and found no difference in display holds, so it is off). The "All keys" run is not worse; with one run, its 0.4 ms lower end to end is within what a second run could change, so it is not claimed as a gain.
- **`picture-order.enable` = 0 is not sent.** With it, `c2.qti.hevc.decoder.low_latency` accepted the configure and then produced no output at all (28 frames in, none out, `DecoderHungException` after 5 s). The fallback ladder cannot catch that, since the configure succeeded. Without the key the same build streams normally.
- Not measured: the other Snapdragon generations, MediaTek, NVIDIA and Amlogic (see below), and AV1 on this device.

## Not measured here

Nightjar's own Android devices are a Retroid Pocket 6 and a Galaxy Z Fold7 (both Snapdragon), a TCL C6K TV (MediaTek) and an emulator. The keys below have not been timed on their hardware; each is covered by unit tests (`DecoderTuningKeysTest`, a test per decoder name for the keys and for RFI):

- **MediaTek**: the whole preset, boost and DVFS level 1, no `pq` or skip and drop keys, operating rate on `c2.mtk` and `OMX.MTK` (behind the performance hints, no priority), RFI for `c2.mtk` and `omx.mtk`, the five detect-only switches. `cpu.boost.mode` = 1 goes to every MediaTek decoder that declares it.
- **NVIDIA Tegra**: the four keys on `OMX.Nvidia` and `c2.nvidia`, and the operating rate behind the performance hints.
- **`low-latency` before Android 11** on every decoder. Decoders that do not know the key ignore it, and a decoder that rejects the format loses it on the next try.
- **`c2.qcom` prefix** for Snapdragon decoders under that name, including RFI for HEVC.
- **`OMX.qcom` extras** (`dpb-output-delay`, `picture-type`): old Snapdragons only.
- **`frame-drop.enable` on Snapdragons other than the 8 Gen 2.** Only the Retroid's 8 Gen 2 was run; with `debug.nightjar.qti_frame_drop=1` the key goes to every Qualcomm decoder on tries 0 to 2 (off by default).

Each is covered by a test per decoder name, and by one for the fallback ladder (the keys drop out of later tries), the vendor switch (off means none sent) and the Android 12 declared-parameter path.

## Fire TV Cube (Amlogic): a configure failure

Issue 21 reported every format failing `configure()` with error -5 on a Fire TV Cube. Nightjar sends `vendor.low-latency.enable` and `vdec-lowlatency` on the Amlogic OMX decoder, whitelists `omx.amlogic` for HEVC on Fire OS and sends `KEY_PRIORITY` = 0 on Android 8 and newer Amlogic, only on the first tries, and the ladder drops them before the plain format. Two things reached `configure()` on Android 9 that should not have:

- **A null HDR entry.** Nightjar wrote a null `KEY_HDR_STATIC_INFO` into every SDR format below Android 10. Fixed: the key is only cleared when the format has it.
- **Diagnostics inside the configure `try`.** `getInputFormat()` and the overlay diagnostics ran there, so a throw from either counted as a rejected configure on every try. Fixed: they log a warning and no longer fail the decoder.

With both fixed, the reporter's Fire TV Cube connected and streamed on Nightjar 3.0.1, and the issue was closed. If a configure failure comes back on another Amlogic device, check `KEY_MAX_INPUT_SIZE`, which Nightjar sets above about 125 Mbps (it is not part of the ladder), and the mandatory `vdec-lowlatency` for HEVC, which Nightjar limits to the Fire TV 3 (`AFTN`).

## Credits

The vendor key lists follow [Artemis](https://github.com/ClassicOldSong/moonlight-android) by ClassicOldSong, the [moonlight-noir fork](https://github.com/derflacco/moonlight-android) by derflacco, [Moonlight X](https://github.com/MoreOrLessSoftware/moonlight-android/tree/mlsoft-x) by MoreOrLessSoftware and [Moonlight V+](https://github.com/qiin2333/moonlight-vplus) by qiin2333 and contributors, all GPL-3.0. The code comments next to each key credit its source, and the [license and credits](index.md#license-and-credits) section names them all.
