# Device results

[Documentation](index.md)

Decode time differs a lot between Android chips. Snapdragon is well covered by Nightjar's low-latency decoder settings, but MediaTek Dimensity, Samsung Exynos, Google Tensor and the Amlogic, MediaTek and Pentonic chips in TV boxes and TVs have far fewer measurements. This page explains how to measure a device with Nightjar's own tools, how to read the results and how to send them, so that per-chip changes are based on numbers instead of guesses.

Nothing on this page sends data automatically. You decide whether to share your results and where.

## Run the tools

You need no PC for either tool.

1. **Decoder benchmark.** Nightjar times each hardware decoder once per app version on Android 10 and later, and again after changing **HEVC low-latency flags**. It decodes a short 1920×1080, 60 FPS game-like clip in H.264, HEVC and AV1 with the same low-latency settings a stream uses, one frame at a time, and keeps the median decode time per frame. It runs quietly in the background when the home screen opens, but only after a first stream, because the decoder choice depends on the GPU that the first stream records. Once it has run, the help text of **Settings → Video → Video codec** shows the measured times.
2. **Latency benchmark.** Open **Settings → Advanced → Latency benchmark** and tap **Start 10-second test**. Touch the screen, move a mouse or press controller buttons during the test so that it also collects input samples. Close other apps first and keep the device plugged in, as power saving changes the timing.
3. Tap **Share results** on the same screen. Nightjar shows the exact text it would share, then lets you **Copy** it, **Share** it through Android's share sheet, or **Open report form**, which copies it and opens the public GitHub form for device results in your browser.

Run the latency test before you tap **Share results**; otherwise the text includes the decoder benchmark only. If the decoder benchmark has not run yet, stream once, return to the home screen, wait a minute and open the latency benchmark again.

## Read the results

The text is JSON with these parts:

| Field | Meaning |
| --- | --- |
| `format`, `version` | Always `nightjar-device-results`, and the format version (currently 1). |
| `app` | Nightjar version and build number that produced the text. |
| `device` | Manufacturer, model and codename, SoC maker and model (Android 12 and later), hardware and board names, Android version and API level, and the GPU name. |
| `decoderBenchmark.measured` | `false` when this app version has not measured this device yet. |
| `decoderBenchmark.clip` | Size and frame rate of the test clip, for example `1920x1080@60`. |
| `medianDecodeMs` | Median time from queuing one frame to getting it back from the decoder, in milliseconds. Lower is better. A 120 FPS frame lasts 8.3 ms, so decode times near that leave no headroom. |
| `featureLowLatency` | Whether the decoder advertises Android's standard low-latency mode. `null` means not recorded. |
| `vendorLowLatencyKeys` | The vendor low-latency switches the decoder declares. These describe capabilities; disabling vendor or HEVC low-latency flags can leave them unset during a stream. An empty list means the decoder declares none; `null` means unknown (Android 11 or older, or recorded by an older version). |
| `latencyTest.panelHz` | The refresh rate the display reports. |
| `audioSampleRate`, `audioFramesPerBuffer` | Audio output properties the device reports. They are capabilities, not measured audio delay. |
| `frameIntervalMs` | Time between display frame callbacks. It should be close to 1000 ÷ `panelHz`; a high `p99` means missed frames. |
| `frameCallbackDelayMs` | How late Android delivers each frame callback. |
| `inputDispatchAgeMs` | Age of touch, mouse and controller events when the app receives them. `null` if you gave no input during the test. |
| `timerWakeupDelayMs` | How late a 100 ms timer wakes up. High values point to aggressive power saving. |

Each latency metric has `samples`, `mean`, `p95` and `p99` in milliseconds (`p95` is the slowest 5%). The latency test measures this device only; it does not include the network, the host or the panel's own lag. For a full stream, use **Measure input latency** and **Why did my stream stutter?** in the stream menu (see [video and latency](video-and-latency.md#automatic-bitrate-and-diagnostics) and [stream health](stream-health.md)).

## What is left out

The text never contains PC names, addresses, MAC addresses, serial numbers, the build fingerprint, account data or anything about your streams. Model, board and GPU names pass through the same redaction as [problem reports](PRIVACY.md#sharing-a-problem-or-crash-report): known PC names, IP and MAC addresses, e-mail addresses, IDs and long tokens are replaced with placeholders such as `[PC]` or `[address]`. Decoder names and vendor keys are kept only when they look like plain identifiers. Read the text before you share it; you can remove any line you do not want to publish.

## Send results

Tap **Open report form** in the **Share results** dialog, or open the [device results form](https://github.com/RamazanKara/nightjar/issues/new?template=device-results.yml) yourself. The link names the form only and carries no device data. Enter the device's name, paste the copied text into **Results text**, and add anything the tools cannot see:

- whether the device was plugged in and how warm it was;
- for a TV or TV box, the output resolution and refresh rate set in Android;
- for a stream on the same device, the codec, resolution, frame rate and the decode timing from the Debug overlay (see [performance overlay](video-and-latency.md#performance-overlay)).

**Posted results are public.** Anyone can read the issue, and the results text is copied into the [device results page](https://ramazankara.github.io/nightjar/devices.html) on the Nightjar website. Remove any line you do not want to publish before you post. To take results down, comment on the issue or open a new one.

### For maintainers

Nothing to do for a normal report. When a "Device results" issue is opened or edited, the [Device results workflow](../.github/workflows/device-results.yml) checks the form (the Publishing box must be ticked), validates the results text with the rules below, adds the entry to `site/data/device-results.json`, regenerates the measured table in this page, copies the data file to the `gh-pages` branch for the website, comments and closes the issue. It leaves the issue open when "Stream on this device" has text, and replies once with the specific problems when a check fails; editing the issue runs it again, and an edit of an issue that is already in the table replaces its entry. A `note` field on an entry in the data file adds a maintainer comment to its Notes cell.

Manual fallback, for example when the workflow is down: save the results text from the issue to a file and run `node scripts/add-device-results.mjs results.txt --issue <number> --name "<device>" --docs docs/devices.md` (add `--dry-run` first to see the entry, `--replace` to update an issue that is already in the table). The script accepts format version 1 only, keeps only the fields documented above, and refuses text that looks like an address, e-mail address, ID, token, file path or markup. It appends the entry to `site/data/device-results.json`, which the site renders, and regenerates the table between the `device-results` markers below with `scripts/device-docs.mjs`; CI fails when the table and the data disagree. Then close the issue and copy the data file to `data/device-results.json` on the `gh-pages` branch.

## Devices

Measured values are median decode times in milliseconds from the decoder benchmark (1920×1080, 60 FPS clip, decoded on the device, no host or network involved). "To measure" means no results have been sent yet. The full results text of each report, including the latency test, is on the [device results page](https://ramazankara.github.io/nightjar/devices.html) and in the linked issue.

<!-- device-results:start -->
<!-- Generated from site/data/device-results.json by scripts/device-docs.mjs. Edit the data, not this table. -->
| Device | SoC | Android | H.264 | HEVC | AV1 | Vendor keys declared | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Samsung Galaxy S25 Ultra | Snapdragon 8 Elite (SM8750) | 16 | 0.60 | 0.55 | 0.39 | picture order, output fence | Nightjar 0.7.13, 120 Hz. [#11](https://github.com/RamazanKara/nightjar/issues/11) |
| OnePlus 13 | Snapdragon 8 Elite (SM8750) | 16 | 0.88 | 0.73 | 1.58 | picture order, output fence | Nightjar 0.7.6, 120 Hz. [#6](https://github.com/RamazanKara/nightjar/issues/6) |
| Alldocube iPlay 70 Mini Ultra | Snapdragon 7+ Gen 3 (SM7675) | 14 | 1.35 | 1.10 | — | picture order, output fence | Nightjar 0.7.13, 144 Hz. [#10](https://github.com/RamazanKara/nightjar/issues/10) |
| NVIDIA Shield Pro | darcy | 11 | 1.83 | 2.06 | — | — | Nightjar 3.1.0, 59.94 Hz. [#32](https://github.com/RamazanKara/nightjar/issues/32) |
| iQOO 7 | Snapdragon 865 or 870 (SM8250) | 14 | 2.42 | 2.83 | — | None | Nightjar 0.8.0, 120 Hz. [#25](https://github.com/RamazanKara/nightjar/issues/25) |
| OPPO Reno 10 Pro+ | Snapdragon 8+ Gen 1 (SM8475) | 16 | 4.48 | 4.26 | — | picture order | Nightjar 0.8.0; no latency test. Slower than the Snapdragon 870 phone (#25); being looked into. [#23](https://github.com/RamazanKara/nightjar/issues/23) |
| Realme GT 7T | MediaTek MT6899 | 16 | 4.87 | 4.27 | 8.31 | CPU boost | Nightjar 0.7.18, 120 Hz. [#18](https://github.com/RamazanKara/nightjar/issues/18) |
| Samsung Galaxy A37 | Samsung s5e8845 | 16 | 11.17 | 11.53 | — | `rtc-ext-dec-low-latency` (H.264 only) | Nightjar 0.8.0, 120 Hz. [#24](https://github.com/RamazanKara/nightjar/issues/24) |
| Samsung Galaxy A37 | Samsung s5e8845 | 16 | 12.26 | 11.55 | — | `rtc-ext-dec-low-latency` (H.264 only) | Nightjar 3.0.0, 120 Hz. No change from 0.8.0. 3.1.0 sends more Exynos keys; retest pending. [#26](https://github.com/RamazanKara/nightjar/issues/26) |
| Samsung  a37 | Samsung s5e8845 | 16 | 10.63 | 12.35 | — | `rtc-ext-dec-low-latency` (H.264 only) | Nightjar 3.2.1; no latency test. [#40](https://github.com/RamazanKara/nightjar/issues/40) |
| Samsung Galaxy S24+ (Exynos) | Samsung s5e9945 | 17 | 12.79 | 12.72 | 10.53 | `rtc-ext-dec-low-latency` (H.264 only) | Nightjar 3.2.3, 120 Hz. [#43](https://github.com/RamazanKara/nightjar/issues/43) |
<!-- device-results:end -->

Still to measure:

| Device | SoC | Android | H.264 | HEVC | AV1 | Vendor keys declared | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Phone with Dimensity 9000 | MediaTek | To measure | To measure | To measure | To measure | To measure | Upstream reports about 30 ms decode. |
| Samsung Galaxy with Exynos 2200 or newer | Samsung Exynos | To measure | To measure | To measure | To measure | To measure | |
| Google Pixel 8 or newer | Google Tensor | To measure | To measure | To measure | To measure | To measure | Shares the Exynos low-latency key. |
| Google TV Streamer | MediaTek | To measure | To measure | To measure | To measure | To measure | Upstream reports audio delay on Android TV 14. |
| Chromecast with Google TV | Amlogic | To measure | To measure | To measure | To measure | To measure | |
| Xiaomi Mi Box S | Amlogic | To measure | To measure | To measure | To measure | To measure | Upstream reports HEVC and AV1 problems. |
| Amazon Fire TV Stick 4K Max | MediaTek | To measure | To measure | To measure | To measure | To measure | |
| TV with a MediaTek Pentonic chip | MediaTek Pentonic | To measure | To measure | To measure | To measure | To measure | Upstream reports half of the frames dropped by the display compositor. |
| NVIDIA Shield TV | NVIDIA Tegra X1 | To measure | To measure | To measure | To measure | To measure | |

## HEVC low-latency compatibility

If HEVC video stutters or freezes, try setting **Settings → Advanced → Low-latency decoding → Customize → HEVC low-latency flags** to Off. It is Auto by default and works on any device. On Auto, the newer Amlogic HEVC decoders (`c2.amlogic`) don't get these flags, except on Fire TV: they configure fine and then show about one frame a second. The older `omx.amlogic` decoders and every Fire TV keep them. On gives them the flags anyway, for firmware that has fixed this. Off omits optional Android and vendor low-latency flags for HEVC on the next stream and in the decoder benchmark, and disables HEVC reference-frame invalidation. The low-latency master switch shows **Partly off** while this override limits enabled options. H.264 and AV1 keep their normal behavior; decoder performance hints keep their separate setting and can fall back if rejected.

Changing this switch invalidates previous decoder benchmark results. Return to the home screen after streaming to measure again. If it does not improve playback, turn it off to restore the normal decoder options. Fire TV 3 (AFTN) on Fire OS 6 retains the required `vdec-lowlatency` compatibility flag even with tuning disabled and during fallback, because its Amlogic OMX decoder otherwise produces no HEVC frames. The Debug overlay reports that retained flag; other decoders report low-latency options as off with this override.

The [Moonlight HEVC fix](https://github.com/Viktsolovevwork278/moonlight-android-hevc-fix/releases/tag/v1.0.0) reports slideshow-like visible playback on the Xiaomi TV Stick MiTV-AYFR0 with Android TV 14 and `c2.amlogic.hevc.decoder`, despite normal FPS statistics. It reports that omitting the options fixes HEVC 1920×1080 at 60 FPS with Sunshine. A [separate Homatics S905X5M report](https://github.com/sven253/artemis-moonlight-android-hevc-fix#hevc-low-latency-options) identifies Android’s low-latency flag as problematic on that firmware. These reports do not establish a workaround for every Amlogic chip; Nightjar hardware measurements are still needed using the tools above.

## Candidate per-chip changes

Nightjar already sets Android's standard low-latency flag and every vendor low-latency key a decoder declares (Qualcomm, Exynos and Tensor, Kirin, Amlogic, MediaTek Codec2 and NVIDIA keys), plus the MediaTek and Amazon `vdec-lowlatency` option on their older OMX decoders, subject to the [HEVC compatibility switch](#hevc-low-latency-compatibility) and other decoder settings. The decoder benchmark honors the HEVC switch too. The ideas below still need measurements before adding further chipset tuning. The first two ship as **Chipset tuning** in Advanced settings, off by default: `vdec-lowlatency` on `c2.mtk` decoders, and on Exynos decoders the operating rate they declare for the stream size next to the realtime priority hint. Both are sent on the first configure try only.

| Chip family | Idea to measure | How to tell |
| --- | --- | --- |
| MediaTek Dimensity (Codec2 decoders) | Whether `vdec-lowlatency`, today sent only to older OMX decoders, also helps `c2.mtk` decoders; whether the CPU boost key changes decode time when the device is cool and when it is warm. | `medianDecodeMs` with and without, several runs each. |
| Samsung Exynos and Google Tensor | Whether a maximum operating rate behaves better than the realtime priority hint used today. It has crashed decoders on other chips, so it may only be tried as an opt-in. | `medianDecodeMs`, plus the Debug overlay decode timing during a 120 FPS stream. |
| Amlogic TV boxes | Whether the HEVC reference-frame and low-latency workarounds that apply to Fire TV also suit Google TV and Xiaomi boxes. | `medianDecodeMs` for HEVC and AV1; whether the stream shows frames at all. |
| MediaTek Pentonic TVs | Whether a different frame pacing mode avoids the compositor dropping frames. This is a display problem, not a decoder one. | `frameIntervalMs` in the latency test and estimated display drops in the Debug overlay. |
| Android TV audio | Audio delay on Android TV 14. These tools do not measure audio delay. | A camera or an audio-video sync test against the host. |

Any change that comes out of these measurements ships as an opt-in setting limited to the matching decoders, off by default. It becomes the default only after the decoder benchmark and the stutter report show a gain without freezes on several devices of that family.

## Format versions

The shared text keeps the `format` name and version so that results stay comparable. New fields may be added within version 1, so readers should ignore fields they do not know. Renaming or removing a field, or changing a unit, raises the version.
