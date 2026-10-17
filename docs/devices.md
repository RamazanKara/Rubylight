# Device results

[Documentation](index.md)

Decode time differs a lot between Android chips. Snapdragon is well covered by Nightjar's low-latency decoder settings, but MediaTek Dimensity, Samsung Exynos, Google Tensor and the Amlogic, MediaTek and Pentonic chips in TV boxes and TVs have far fewer measurements. This page explains how to measure a device with Nightjar's own tools, how to read the results and how to send them, so that per-chip changes are based on numbers instead of guesses.

Nothing on this page sends data automatically. You decide whether to share your results and where.

## Run the tools

You need no PC for either tool.

1. **Decoder benchmark.** Nightjar times each hardware decoder once per app version on Android 10 and later, and again after changing **Disable HEVC low-latency flags**. It decodes a short 1920×1080, 60 FPS game-like clip in H.264, HEVC and AV1 with the same low-latency settings a stream uses, one frame at a time, and keeps the median decode time per frame. It runs quietly in the background when the home screen opens, but only after a first stream, because the decoder choice depends on the GPU that the first stream records. Once it has run, the help text of **Settings → Video → Video codec** shows the measured times.
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

**Posted results are public.** Anyone can read the issue, and the maintainers copy the results text into the [device results page](https://ramazankara.github.io/nightjar/devices.html) on the Nightjar website. Remove any line you do not want to publish before you post. To take results down, comment on the issue or open a new one.

### For maintainers

Save the results text from the issue to a file and run `node scripts/add-device-results.mjs results.txt --issue <number> --name "<device>"` (add `--dry-run` first to see the entry). The script accepts format version 1 only, keeps only the fields documented above, and refuses text that looks like an address, e-mail address, ID, token, file path or markup. It appends the entry to `site/data/device-results.json`, which the site renders. Then update the table below, close the issue and publish the site.

## Devices

Measured values are median decode times from the decoder benchmark. "To measure" means no results have been sent yet.

| Device | SoC | Android | H.264 | HEVC | AV1 | Vendor keys declared | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Phone with Dimensity 9000 | MediaTek | To measure | To measure | To measure | To measure | To measure | Upstream reports about 30 ms decode. |
| Phone with a recent Dimensity (8000 or 9000 series) | MediaTek | To measure | To measure | To measure | To measure | To measure | |
| Samsung Galaxy with Exynos 2200 or newer | Samsung Exynos | To measure | To measure | To measure | To measure | To measure | |
| Google Pixel 8 or newer | Google Tensor | To measure | To measure | To measure | To measure | To measure | Shares the Exynos low-latency key. |
| Phone with Snapdragon 8 Gen 2 or newer | Qualcomm | To measure | To measure | To measure | To measure | To measure | Reference for the other rows. |
| Google TV Streamer | MediaTek | To measure | To measure | To measure | To measure | To measure | Upstream reports audio delay on Android TV 14. |
| Chromecast with Google TV | Amlogic | To measure | To measure | To measure | To measure | To measure | |
| Xiaomi Mi Box S | Amlogic | To measure | To measure | To measure | To measure | To measure | Upstream reports HEVC and AV1 problems. |
| Amazon Fire TV Stick 4K Max | MediaTek | To measure | To measure | To measure | To measure | To measure | |
| TV with a MediaTek Pentonic chip | MediaTek Pentonic | To measure | To measure | To measure | To measure | To measure | Upstream reports half of the frames dropped by the display compositor. |
| NVIDIA Shield TV | NVIDIA Tegra X1 | To measure | To measure | To measure | To measure | To measure | |

## HEVC low-latency compatibility

If HEVC video stutters or freezes, try **Settings → Advanced → Decoder → Disable HEVC low-latency flags**. It is off by default and works on any device. Enabling it omits both Android and vendor low-latency flags for HEVC on the next stream and in the decoder benchmark. The Debug overlay reports low-latency options as off. H.264, AV1 and reference-frame invalidation keep their normal behavior; decoder performance hints keep their separate setting and fall back to an untuned format if rejected.

Changing this switch invalidates previous decoder benchmark results. Return to the home screen after streaming to measure again. If it does not improve playback, turn it off to restore the normal decoder options. Some older Amlogic devices need low-latency options to output frames at all, so Nightjar does not infer this choice from a chip or model name.

The [Moonlight HEVC fix](https://github.com/Viktsolovevwork278/moonlight-android-hevc-fix/releases/tag/v1.0.0) reports slideshow-like visible playback on the Xiaomi TV Stick MiTV-AYFR0 with Android TV 14 and `c2.amlogic.hevc.decoder`, despite normal FPS statistics. It reports that omitting the options fixes HEVC 1920×1080 at 60 FPS with Sunshine. A [separate Homatics S905X5M report](https://github.com/sven253/artemis-moonlight-android-hevc-fix#hevc-low-latency-options) identifies Android’s low-latency flag as problematic on that firmware. These reports do not establish a workaround for every Amlogic chip; Nightjar hardware measurements are still needed using the tools above.

## Candidate per-chip changes

Nightjar already sets Android's standard low-latency flag and every vendor low-latency key a decoder declares (Qualcomm, Exynos and Tensor, Kirin, Amlogic, MediaTek Codec2 and NVIDIA keys), plus the MediaTek and Amazon `vdec-lowlatency` option on their older OMX decoders, subject to the [HEVC compatibility switch](#hevc-low-latency-compatibility) and other decoder settings. The decoder benchmark honors the HEVC switch too. The ideas below still need measurements before adding further chipset tuning.

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
