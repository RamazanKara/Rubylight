# Performance and validation

[Docs](README.md) · [How compute works](architecture.md) · [Troubleshooting](troubleshooting.md) · [Full measurement record](../rust/PERFORMANCE.md)

Rubylight's performance work targets fresh pictures and lower picture age, especially when the GPU is busy. Start with the [current 2.2.9 measurements](#october-12-229-measurements), then use the component tests to see where the time goes. The [matched October 8 comparison](#next-to-vibepollo-20) retains its original builds and results.

**On this page:** [Metrics](#what-the-numbers-mean) · [Host latency between hosts](#comparing-host-latency-between-hosts) · [2.2.9 measurements](#october-12-229-measurements) · [Host comparison](#next-to-vibepollo-20) · [Wi-Fi](#on-a-real-client-over-wi-fi) · [Radeon compute](#isolating-radeon-compute) · [GPU pressure](#radeon-under-pressure) · [Measure your setup](#measure-your-setup)

## What the numbers mean

| Metric | What it measures |
| --- | --- |
| **Render-to-decode delay / picture age** | Time from a timestamped rendered picture through independent decoding and readback. The matched host comparisons use loopback on the same PC, so they include composition, capture, encoding and client decoding, but exclude remote network transit and display time. |
| **Render-to-received** | Read the fixture's arrival boundary: the October 12 hardware receiver estimates completed frame assembly/enqueue, excluding decoder-queue waiting and decoding. The historical October 8 method below describes arrival at decode start. Both include network transit and clock-mapping uncertainty; neither measures display latency. Do not compare their absolute values with render-to-decode measurements. |
| **95th / 99th percentile (p95 / p99)** | The slower end of the sample: 95% / 99% of observations are at or below this delay. The matched host-comparison table averages the per-run metrics rather than pooling samples. |
| **Fresh pictures per second** | Distinct pictures received, counted from changing picture IDs. Repeated frames do not increase this count. |
| **Encoder time** | Read the probe's timer boundaries. In the native AMF stream, the timer starts before capacity checking and GPU conversion submission, and ends when `QueryOutput` returns the matching encoded frame. Pending capture-copy and conversion dependencies are included; this is not hardware compression time alone. |
| **Present to sent** (console; `present_to_sent_ms` in the `stream timings` log line) | New pictures, from the capture's present stamp to the last packet handed to the socket, with the stages in between. It includes the capture, the wait for the claim and the send that Moonlight's host latency leaves out. Static repeats have no present and are not counted. See [Comparing host latency between hosts](#comparing-host-latency-between-hosts). |
| **`host_mean_ms`, `host_p95_ms`, `host_p99_ms`, `host_max_ms`** | The duration reported in Moonlight's frame header; the session API calls these `host_processing_*`. It runs from the claim, when the stream thread takes the newest picture to encode, to packetization, or the separate sender's dequeue, which includes sender-capacity and queue waiting: where punktfunk's runs. A repeat's claim is reconstructed from the encoder duration. From the evening of October 15 until 2.4.0 it started at the present stamp instead. Capture, the wait for the claim, packetization, sending and client work are excluded; **Present to sent** covers them. Only the frame-header value is rounded to 0.1 ms. |

None of these timings measures button press to visible response on a client display. Compare hosts within the same fixture, and read picture age alongside fresh-picture throughput.

## Comparing host latency between hosts

Every host reports a "host latency", but each times a different part of a frame's trip, so two hosts' figures compare only when they start and end at the same points. A new picture passes these steps: Windows presents the composed desktop; the host gets the picture (capture); it waits for its turn to be encoded (the claim); it is converted and encoded; it is packetized and its packets are sent.

| Figure | Starts | Ends | Leaves out |
| --- | --- | --- | --- |
| Moonlight's host processing latency from **Sunshine** | The capture's present stamp: Desktop Duplication's `LastPresentTime` or the WGC frame's time (`frame_timestamp` in `src/stream.cpp`) | The send thread, before packetization | Packetization and sending |
| Moonlight's host processing latency from **Vibepollo** (since April 2026) | When its capture took the frame (`host_processing_timestamp`; the present stamp when there is none) | The same | Present to pickup, packetization and sending |
| Moonlight's host processing latency from **Rubylight** (console: **Host processing**) | The claim: the stream thread takes the newest picture to encode, as punktfunk's tick takes its newest frame (from the evening of October 15 until 2.4.0: the present stamp) | Packetization, or the separate sender's dequeue | Capture, the wait for the claim, packetization and sending |
| Rubylight's host timing reports (control message 0x5536, for a client that asks; Nightjar shows them as host latency) | The capture's present stamp; on a Rubylight display, the driver's acquire | The frame's last packet handed to the socket, with queue, encode and send split out as punktfunk's | Nothing on the host |
| Rubylight's **Present to sent** (console; `present_to_sent_ms` in the log) | The capture's present stamp, as Sunshine's | The last packet handed to the socket | Nothing on the host, for new pictures. WGC's stamp is late, see below |
| punktfunk's native `host_us` (v0.44, `native/stream/send.rs`) | The compositor's present stamp that its virtual display driver passes with the frame; the submit time for a repeat or an implausible stamp | The frame's last paced packet sent | Nothing on the host |
| punktfunk's Moonlight field (v0.44, `gamestream/stream.rs`: `cap_at` to the packetizer thread) | The encode loop's tick, before it takes the newest frame | The packetizer thread, before packetization | Present to tick, packetization and sending. Its Windows driver encodes before the tick, so the encode too |

Set Rubylight's **Present to sent**, or the host timing a client shows from its reports, beside punktfunk's `host_us`: same start, same end, same stages. One difference remains on a virtual display: IddCx's `PresentDisplayQPCTime`, which punktfunk takes as the present stamp, is the frame's scan-out (6.8 ms after the driver acquired it on a 120 Hz display here), so punktfunk's figure starts later or falls back to its submit time; Rubylight starts at the acquire and reads higher by that gap. Rubylight's Moonlight figure starts and ends where punktfunk's does, so the two compare too, with one exception: when punktfunk's Windows driver encodes, the encode happens before its tick and its figure leaves it out, while Rubylight's includes the encode.

Measured on the RX 7900 XT, October 15, with an isolated host streaming 2560×1440 from the 240 Hz monitor to a receiver on the same PC (two runs per case; [method, every rate and the virtual display](../rust/PERFORMANCE.md#october-15-where-host-latency-goes)). WGC here is the helper process the service uses:

| 2560×1440 at 120 fps, mean ms | Moonlight shows | Present to sent (p99) | Of which capture |
| --- | ---: | ---: | ---: |
| HEVC, Desktop Duplication, idle | 3.3 | 4.3 (7.5) | 0.7 |
| HEVC, WGC, idle | 3.2 | 4.4 (8.4) | 0.7 + about 2 hidden |
| AV1, Desktop Duplication, idle | 3.0 | 3.8 (4.7) | 0.6 |
| AV1, WGC, idle | 2.9 | 3.8 (8.5) | 0.5 + about 2 hidden |
| HEVC, Desktop Duplication, GPU load | 3.5 | 5.6 (10.3) | 0.6 |
| HEVC, WGC, GPU load | 3.2 | 7.5 (14.6) | 1.5 + more hidden |
| AV1, Desktop Duplication, GPU load | 3.8 | 6.4 (9.8) | 0.6 |
| AV1, WGC, GPU load | 3.0 | 6.2 (11.2) | 1.0 + more hidden |

- **Idle, Present to sent is 0.8–2.4 ms above what Moonlight shows**, about 1 ms at 120 fps. Most of both is the encode (2.6–3.1 ms). Capture adds about 0.6 ms with Desktop Duplication, the wait for the claim 0–1.7 ms (most at 60 fps) and packetization 0.1–0.2 ms; on loopback sending adds almost nothing. On a real network the paced send adds about the frame's size over the pacing rate.
- **WGC's stamp is late.** For 47–99% of idle frames, WGC's frame time was later than the frame's arrival at the host. Rubylight then uses the arrival, so WGC's capture stage reads low. Against Desktop Duplication, the receiver's render-to-received times put the hidden part at about 2 ms idle on the 240 Hz monitor and about 1 ms on a virtual display. Add it to WGC's Present to sent before comparing with another host.
- **Under GPU load the time goes to waiting.** The wait for the claim grows to 1–4.5 ms at 60 and 120 fps (0.2–1.6 ms at 240), and WGC's pictures arrive later. Desktop Duplication hands the host a picture before DWM's GPU work on it has finished ([October 12 diagnosis](../rust/PERFORMANCE_WORK.md#av1-host-processing-p95-diagnosis-october-12)); under load that work waits behind the game's, and Desktop Duplication's encode stage grows by up to 1.1 ms instead. Moonlight's figure stays near 3–4 ms because it covers none of the waiting before the claim.

## October 15: 2.4.0 to an Intel receiver

Rubylight 2.4.0, installed, streamed the Desktop app on a per-device virtual display (extended layout, captured through Rubylight's display driver) over gigabit Ethernet to the [independent receiver](../rust/tests/moonlight_client.c) on an Intel NUC (i5-8259U, Iris Plus 655), decoding on Quick Sync through VA-API. The host was the RX 7900 XT while a game ran on it; the virtual desktop itself was still, so most frames sent were repeats.

| 1920×1080 at 60 fps, 20 Mbps | Frames received / decoded | Failures | Moonlight host processing, mean / p95 | Receiver decode, mean |
| --- | ---: | ---: | ---: | ---: |
| HEVC, 30 s | 681 / 681 | 0 | 2.6 / 3.4 ms | 17.9 ms |
| H.264, 15 s | 385 / 385 | 0 | 2.9 / 3.6 ms | 16.5 ms |
| HEVC 10-bit HDR (BT.2020/PQ), 15 s | 378 / 378 | 0 | 2.9 / 4.6 ms | 20.4 ms |

- **Capture through the driver takes 0.2 ms** (p95 0.3 ms), from the driver's acquire to the host holding the frame, against about 0.9 ms through WGC on a virtual display ([October 15](../rust/PERFORMANCE.md#october-15-where-host-latency-goes)). Each stream had its first picture from the driver's retained frame about 0.5 s after launch, and the HDR stream kept running through a swap-chain replacement when the display switched to HDR.
- **What is left is the encode.** For new pictures on the final build: capture 0.22 ms, convert 0.26 ms, AMF HEVC encode 2.43 ms (p95 3.12 ms), packetization 0.03 ms. Moonlight's host processing, which starts at the claim as punktfunk's does, read 2.69 ms mean and 3.63 ms p95 for them; present to sent read 3.54 ms, the difference being capture and the wait for the claim.
- **The release check passed** on the published installer: H.264, HEVC, AV1, HEVC with VRR, with keyframe requests, at 2560×1440 and 120 fps (plain and launched as Artemis's Warp does) and PyroWave SDR and HDR 4:4:4 all held their rate (60.05–60.12 and 120.12–120.13 fps) through an isolated host on loopback, with no decode failures. The protocol checks, the display self-test and the install passed too. Results: [`VALIDATION.json`](https://github.com/RamazanKara/Rubylight/releases/download/v2.4.0/VALIDATION.json).
- **Limits.** The table's runs used the first 2.4.0 build, whose Moonlight field started a new picture at its present stamp; most frames were repeats, which start at the claim either way. One run per case, beside a game, on a still desktop: the waits for the claim are those of rare new pictures, not of a moving stream. The NUC has no AV1 decoder, so AV1 was not part of this check.

## October 12: 2.2.9 measurements

Native virtual-HDR capture passed **28/28 stream and timing checks** across DDX and WGC, plus **4/4 HDR pixel checks**. The table below gives the current **DDX** results: arithmetic means of two runs' statistics, measured from a timestamped rendered picture through local hardware decoding. It excludes remote network transit and display time.

| Rubylight 2.2.9, October 12 | Picture age mean / p95 / p99 (ms) | Fresh pictures/s | Host mean (ms) |
| --- | ---: | ---: | ---: |
| Under GPU load, 1968×2184 HEVC HDR 120 fps | **14.34 / 16.03 / 18.15** | 119.99 | 3.35 |
| Under GPU load, 1968×2184 AV1 HDR 120 fps | **13.65 / 18.00 / 18.95** | 119.98 | 5.25 |
| Under GPU load, 1920×1080 HEVC HDR 60 fps | **10.64 / 18.27 / 18.58** | 60.00 | 2.24 |
| Idle, 1968×2184 HEVC HDR 120 fps | 9.65 / 10.87 / 12.16 | 119.99 | 3.30 |
| Idle, 1968×2184 AV1 HDR 120 fps | 10.50 / 11.26 / 11.61 | 120.00 | 3.03 |
| Idle, 2560×1440 HEVC HDR 120 fps | 9.38 / 9.89 / 10.19 | 120.00 | 2.97 |
| Idle, 1920×1080 HEVC HDR 60 fps | 10.09 / 10.56 / 10.76 | 60.00 | 2.14 |

![Rubylight 2.2.9 October 12 and unchanged Vibepollo 2.0 October 8 mean and p95 picture age under GPU load, with different physical desktop conditions.](media/gaming-latency.svg)

**Different dates and physical desktop conditions.** The RX 7900 XT and 120 Hz virtual HDR source are retained; the physical desktop is now **5120×1440 SDR at 240 Hz**, versus **2560×1440 at 120 Hz** on October 8. The loaded native rows request 80 Mbps and use the 60 fps-capped synthetic GPU load. Current Rubylight and historical competitor bars do not establish a matched improvement. The original matched October 8 tables below remain unchanged. [Native runs, source validation and capture scope →](../rust/PERFORMANCE_WORK.md#native-virtual-hdr-headline-streams-october-12)

The AV1 host p95 was **8.4 ms in both morning repeats**. A [25-run follow-up](../rust/PERFORMANCE_WORK.md#av1-host-processing-p95-diagnosis-october-12) traced the slow frames to unfinished DWM graphics work behind DDX's producer-ready GPU fence, before compute copying and compression. Same-condition released-binary controls reproduced a slow tail on rc.24 as well as 2.2.9, and changing the load cadence shifted the slow-frame phase with it. An [earlier-signal experiment](../rust/PERFORMANCE_WORK.md#dwm-dependency-and-early-signal-experiment) and a [post-publication frame hold](../rust/PERFORMANCE_WORK.md#ddx-post-publication-hold-experiment) did not consistently improve picture age or shorten the slow producer waits, so neither was adopted. The original measurements above remain intact; the follow-up identifies a capture-dependency delay rather than a demonstrated persistent release regression.

The separate SDR VRR capture sweep has **20 valid requested-mode phases** and **12 unavailable phases** whose observed Windows refresh rates differed from the requested modes. Their timings are excluded rather than relabelled. The historical private HDR/RTSS source is unavailable.

The [October 12, 2026 release record](../rust/PERFORMANCE.md#october-12-rubylight-229-released-package-refresh) includes **81/81 repeat-frame encoder cases**, CPU/GPU conversion, FEC, packet, wait/input/ENet and PyroWave checks. The [32 current-default AMF quality cases](../rust/PERFORMANCE_WORK.md#current-amf-default-quality) and [116 settings variants](../rust/PERFORMANCE_WORK.md#current-amf-quality-settings-comparison) all encoded and scored **180 frames per case**. Passing means successful execution and scoring, not a minimum quality score. Settings readback was verified in 106 variants; it was unavailable for ten high-motion-boost requests. HDR quality here uses SDR clips converted to HDR, not native HDR capture.

The [AMF property sweeps](../rust/PERFORMANCE.md#retained-amf-property-sweeps-october-12) passed **1,018/1,030 probes**: 30/30 low-latency, 280/280 rate-control and 708/720 split-frame cases. All 12 failures used HEVC or AV1 4K SDR at 60 fps/80 Mbps with preanalysis enabled: ten did not complete the measured frames, and two reported 181 completed frames for 180 timing samples. Passing checks completion and accounting; it does not establish a latency target or prove that every optional setting applied. These synthetic encoder and clip-quality probes exclude capture, network, decoding and display timing.

The stream refresh passed **36/36 short release streams** and **12/12 static-keyframe checks**. Both **4K120 HEVC** runs and both **5120×1440 at 240 FPS** saturation attempts failed the unchanged stream gates. The separate [audio/load/outage batch](../rust/PERFORMANCE_WORK.md#audio-under-load-and-80-ms-outage-recovery) passed **6/11 full-stream checks**, while **all 11 kept continuous audio with zero source underruns**. [Per-run measurements and failures →](../rust/PERFORMANCE_WORK.md#rubylight-229-short-release-streams-2026-10-12)

Those earlier short-stream batches used the physical **5120×1440 SDR display at 240 Hz**; their HDR output is SDR conversion. Conventional streams use a loopback software decoder, and PyroWave uses Vulkan decode with CPU readback. Their scope differs from the new native-HDR results above and the historical host comparison below.

In the [eight 90-second loss checks](../rust/PERFORMANCE_WORK.md#ninety-second-control-and-20-ms-outage-streams), all four controls passed and all four injected-outage runs failed the normal gap gates. [Both 120-second audio streams](../rust/PERFORMANCE_WORK.md#two-minute-decoded-audio-continuity-and-video-windows) passed, with **24/24 continuous audio windows**. The short soak passed **64/67 cases**: three GPU-load cases failed fresh-picture delivery, while all 52 reconnect/churn cases, eight codec switches and helper recovery passed. Host working set, handle and thread growth checks passed. The [GPU-memory follow-up](../rust/PERFORMANCE_WORK.md#gpu-memory-counter-follow-up-october-12) reproduced the rising process counter as a reporting artifact: all 30 HEVC reconnects passed, while RX 7900 XT dedicated usage returned from **2.664 GB before launch to 2.659 GB after 30 seconds idle**. No sustained VRAM growth was observed in that focused test; it does not establish every path as leak-free. [Detailed soak results and scope →](../rust/PERFORMANCE_WORK.md#short-soak-switching-reconnects-and-wgc-helper-recovery)

The real Radeon 780M Wi-Fi receiver passed **20/20 stream checks**: 12 HDR-output conversion runs and eight SDR control/recovery runs, including four injected-outage cases. **19/20 calibrated timing records** passed exact source-ID/timestamp validation; AV1 HDR 1080p60 run 2 remains unavailable because three barcode timestamps disagreed with the source ledger. This independent D3D11VA receiver measures render-to-assembly/enqueue timing, excluding decoder queueing, decoding and display. [Per-repeat Wi-Fi timing, recovery and clock scope →](../rust/PERFORMANCE_WORK.md#real-wi-fi-hardware-receiver-streams-october-12)

Official **Moonlight 6.2.0** completed two **165-second 1080p60 HEVC SDR sessions requesting 80 Mbps** on the Radeon 780M laptop over Wi-Fi. Both reported **59.99 FPS rendering**; host counters measured **60.002 / 59.998 FPS** over separate **119.329 / 119.304-second** windows. Client decode averages were **0.59 / 0.50 ms**, queueing **0.02 / 0.02 ms**, and rendering **0.48 / 0.47 ms**. These endpoints and windows differ: do not add them or infer full picture age. Both wrappers failed their immediate exit check; later checks confirmed natural exit, with settings and pairings preserved. [Stock results and cleanup evidence →](../rust/PERFORMANCE_WORK.md#stock-moonlight-620-over-wi-fi-october-12)

## Next to Vibepollo 2.0

**In the October 8, 2026 rc.24 comparison, p95 picture age under game-like GPU load was 43% lower in HEVC and 48% lower in AV1.** Average picture age fell by 7.4 ms for both codecs. These matched historical results are separate from the current chart above.

### Test setup

| Kept the same | Recorded setup |
| --- | --- |
| Hardware | AMD Radeon RX 7900 XT; host and independent hardware-decoding client on the same PC. |
| Stream | 1968×2184, 120 fps HDR, 80 Mbps requested; native AMF at ultra-low latency. The 1440p idle check requested 50 Mbps. |
| Capture and displays | Desktop Duplication on both hosts; Extended layout, 120 Hz virtual HDR display, physical monitor at 2560×1440/120 Hz. |
| GPU load | Synthetic `gpu_load 45 1000 60 200`, capped at 60 fps to leave room for the client's decoder. This is a repeatable game-like load, not a benchmark of a retail game. |
| Builds and repeats | Rubylight 2.0.0-rc.24 and the pinned Vibepollo 2.0 baseline; two alternating runs per host and codec, October 8, 2026. |
| Measurement | Rendered timestamp through independent decoding/readback over loopback. Each cell averages the corresponding per-run metrics. |

### Results

Picture age, average / p95 / p99. Host processing is shown separately and is not the basis for the percentage improvement: the implementations' host counters do not cover exactly the same work. The comparison isolates the hosts from a remote network, but includes the local client's decoding. Rubylight's current default capture is WGC; these tests deliberately use DDX for both hosts.

| | Rubylight rc.24 | Vibepollo 2.0 |
| --- | ---: | ---: |
| **Under GPU load**, 1968×2184 AV1 HDR 120 fps | **17.15 / 18.96 / 22.18 ms**, host 3.1 ms | 24.53 / 36.77 / 38.05 ms, host 7.4 ms |
| **Under GPU load**, 1968×2184 HEVC HDR 120 fps | **19.61 / 21.56 / 23.10 ms**, host 3.4 ms | 26.96 / 37.62 / 39.25 ms, host 8.3 ms |
| No GPU load, 1968×2184 AV1 HDR 120 fps | **17.42 / 18.10 / 18.62 ms** | 18.88 / 19.54 / 19.90 ms |
| No GPU load, 1968×2184 HEVC HDR 120 fps | **17.81 / 18.94 / 19.56 ms** | 19.97 / 20.43 / 20.87 ms |
| No GPU load, 2560×1440 HEVC HDR 120 fps | 17.09 / 17.99 / 18.46 ms | 17.19 / 17.68 / 17.93 ms |

At 1968×2184, average picture age was 1.5–2.2 ms lower without the load and **7.4 ms lower for both codecs under load**. The p95 reduction was 16.06 ms (43%) for HEVC and 17.81 ms (48%) for AV1. Rubylight delivered 117–120 fresh pictures a second; Vibepollo delivered 103–118 in HEVC and 106–111 in AV1. At 1440p without the load, the two were effectively equal; at 1080p60 they were close. [All rows, including 1080p60 →](../rust/PERFORMANCE_WORK.md#rc24-against-vibepollo-20-on-the-same-gpu)

**Scope and limitations.** Two runs per cell on one Radeon establish this comparison, not a guarantee for every GPU, game or client. The capped load avoids starving the loopback client's hardware decoder. At this AV1 size, AMD produces a padded 1984×2186 coded frame; the source record documents the size-check limitation. Network transit, client display refresh and input-to-display response require separate measurements.

### Earlier stress comparison

An earlier whole-host comparison on October 4 showed the same pattern at a heavier load. It used **Vibepollo 2.0** and an early Rubylight build with the same settings: native AMF at ultra-low latency, DDX and realtime GPU priority on both. 1080p60 HEVC HDR, 20 Mbps requested, controlled GPU load; arithmetic means of three alternating runs per host on October 4, 2026.

| Under controlled GPU load | Vibepollo 2.0 | Rubylight |
| --- | ---: | ---: |
| Average render-to-decode delay | 96.4 ms | **42.4 ms** |
| Mean per-run 95th-percentile delay | 137.0 ms | **56.5 ms** |
| Fresh pictures per second | 23.9 | **51.4** |
| Game frame rate beside the host | 176.7–177.4 fps | 173.8–174.9 fps |

The idle comparison measured 16.0 ms versus 13.8 ms. Most of the loaded gap is not the compute path: with compute off, Rubylight still delivered about 57 fresh pictures a second beside the same load in an earlier batch, and the [same-build comparison](#isolating-radeon-compute) puts the compute gain at 7.5 ms. Vibepollo handed its native AMF encoder about 24 frames a second, and the encoder logged that its output had not caught up. That encoder came from Rubylight's author ([Vibepollo #342](https://github.com/Nonary/Vibepollo/pull/342)).

[Baseline revision, workload and recorded runs →](../rust/PERFORMANCE.md#against-vibepollo-20)

## On a real client over Wi-Fi

The [October 12, 2026 2.2.9 hardware-receiver matrix](../rust/PERFORMANCE_WORK.md#real-wi-fi-hardware-receiver-streams-october-12) completed 20/20 ordinary receiver checks with 19 valid calibrated timing records. It uses a physical SDR source, including SDR-to-HDR output conversion, and an assembly/enqueue arrival boundary. Its 1.177 ms clock-fit diagnostic is not a confidence interval or proven error bound. The following October 8 values retain their original source, method and date; no new before/after delta is inferred.

The rc.24 host on Ethernet streamed to a client on **5 GHz Wi-Fi**, decoding in hardware. The client's clock was synchronised to the host with 200 ms pings (offset error about ±0.6 ms), so each rendered picture's age is measured on arrival. HDR on every row, one 30-second run per row on October 8, 2026.

| Render to received, average / p95 / p99 | Fresh pictures per second | Host time |
| --- | ---: | ---: |
| 1968×2184 AV1, 120 fps, 80 Mbps: **13.54 / 14.33 / 14.93 ms** | 120.0 | 3.0 ms |
| 1968×2184 HEVC, 120 fps, 80 Mbps: 14.13 / 14.97 / 15.90 ms | 120.0 | 3.7 ms |
| 2560×1440 AV1, 120 fps, 50 Mbps: 13.34 / 14.15 / 16.12 ms (repeat 13.28 / 14.07 / 15.63 ms) | 120.0 | 2.8 ms |
| 2560×1440 HEVC, 120 fps, 50 Mbps: 13.67 / 14.69 / 19.87 ms | 120.0 | 3.1 ms |
| 1920×1080 AV1, 60 fps, 20 Mbps: 12.91 / 15.05 / 17.89 ms | 60.6 | 2.2 ms |
| 1920×1080 HEVC, 60 fps, 20 Mbps: 12.99 / 14.15 / 16.62 ms | 60.5 | 2.4 ms |

"Received" is when the client starts decoding a fully received picture; hardware decoding adds well under a millisecond. Every row received all pictures, with zero decode failures.

With Moonlight on the same Wi-Fi link:

- **AV1 1440p120:** 119.7 fps, no loss, no frozen picture.
- **AV1 loss recovery:** with the host dropping 20 ms of video every 1.5 s for 90 s at 50 Mbps, all **59 of 59 lost frames were recovered without a keyframe**, at 117.7 fps and 2.7 ms host processing.
- **Moonlight's own statistics:** host processing 2.6–3.6 ms, network 1–3 ms, decode 0.3–0.7 ms, at 118–121 fps.

[Method, clock sync and every run →](../rust/PERFORMANCE_WORK.md#october-8-real-client-picture-age-over-wi-fi-rc24-laptop)

## Isolating Radeon compute

This separate test changes one setting in the **same Rubylight build**: compute off versus compute on. DDX, 1080p60 HEVC HDR, controlled GPU load; arithmetic means of two runs per path on October 4, 2026.

| Render-to-decode delay | Compute off | Compute on |
| --- | ---: | ---: |
| Average | 41.0 ms | **33.5 ms** |
| Mean per-run 95th percentile | 54.4 ms | **42.3 ms** |

This isolates the benefit of moving frame preparation onto compute. The whole-host result above includes the combined implementation. [Queue placement and synchronization](architecture.md#the-radeon-frame-path) explain the change.

[Same-build runs and component probes →](../rust/PERFORMANCE.md#1080p-at-60-fps)

## Radeon under pressure

Two separate October 7 tests on the RX 7900 XT target frame preparation and encoder queueing under pressure. Each compares the change with the code before it; the timings cover different boundaries and should not be added together.

| | Before | After |
| --- | ---: | ---: |
| **PyroWave repeat-frame processing under synthetic GPU load**, 1080p HDR 4:4:4 | 5.7 ms | **0.55 ms** |
| Same PyroWave processing probe, paced at 120 fps | 4.62 ms | **0.71 ms** |
| **Encoder saturated** (5120×1440 HEVC, 240 fps requested), present to send | 42.7 ms | **11.1 ms** |
| Encoder saturated, 99th percentile | 45.9 ms | **13.3 ms** |

The PyroWave probe measures repeat-frame processing through colour conversion and encoding; it excludes capture, transport, decoding and display. Moving conversion onto Radeon compute removed waiting behind the synthetic load's graphics work. Idle repeat-frame processing stayed at 0.47 ms, and the output planes were byte-identical. This is not a 0.55 ms end-to-end stream result.

The encoder-queue test compared rc.18 with rc.19 at 150 Mbps, with two alternating runs each. Limiting pictures in flight to two reduced stale frames waiting for the saturated encoder; throughput stayed at 220 fps in both builds.

[PyroWave runs →](../rust/PERFORMANCE.md#october-7-pyrowave-conversion-on-the-compute-queue) · [Encoder queue runs →](../rust/PERFORMANCE.md#october-7-two-frames-in-the-encoder)

**Earlier AMF low-latency measurements.** With the default ultra-low-latency usage, the driver already applies its internal low-latency mode and AV1's lowest latency; forcing them changed neither encode time nor output size, so they stay on Driver default. [When forcing them helps →](configuration.md#capture-and-video)

## Input under load

Every client's input passes through one host thread. Recent changes give virtual controllers a thread of their own, let the input thread keep its multimedia priority boost, and send the keyboard and mouse input of one network pass to Windows in one call. October 7, 2026:

| | Before | After |
| --- | ---: | ---: |
| Mouse move behind a controller update, CPU-bound load on every core, worst case | 287 ms | **2.0 ms** |
| Input packet to the input thread beside time-critical threads on every core, median | 3.5 ms | **17 µs** |

Smaller changes take a few tens of microseconds off every input packet (the network acknowledgement now goes out after the input is applied) and remove 2–18 ms pauses at a stream's first input and while its display is created or renamed.

[Method and all runs →](../rust/PERFORMANCE.md#october-7-input-on-the-control-thread)

## WGC capture and pacing

Automatic capture prefers WGC and supported Radeon streams use compute by default. Guarded source-phase pacing waits for a predicted fresh update when the capture history supports it.

In the earlier controlled local 720p60 AV1 comparison, guarded pacing delivered **59.862–60.000 fresh pictures/s**, versus **57.650**, and reduced estimated source-presentation-to-software-decode age by about **3.8 ms on average**. The default-path SDR check recorded **59.999 fresh pictures/s**.

These WGC results use a different fixture from the DDX comparisons above. The full record includes slower sources, 120 FPS operation, helper recovery and GPU saturation.

[Pacing comparisons](../rust/PERFORMANCE.md#guarded-ab-stress-and-compatibility-checks) · [Default-path checks](../rust/PERFORMANCE.md#hdr-state-correction-and-current-sdr-validation)

## HDR and PyroWave validation

The table retains the earlier validation runs linked below. The [October 12 release results](#october-12-229-measurements) record the new native-HDR checks and the failed 4K120 stream attempts separately.

| Check | Recorded result | What was inspected |
| --- | --- | --- |
| **Native HEVC and AV1 HDR** | **5,173 / 5,173 frames decoded** across four runs | 720p60 FP16 virtual-HDR capture, decoded BT.2020/PQ, reference colours and display restoration. |
| **PyroWave HDR 4:4:4 transport** | **2,357 / 2,357 frames decoded**, zero video/audio decode errors | 1080p/120 encrypted transport, profile/HDR control state and independent vendor decoding. |
| **Moonlight** | Ten codec/reconnect sessions | H.264, HEVC, AV1, HEVC HDR and AV1 HDR, two connections each at 720p60. |
| **HDR over Wi-Fi** | All six HDR streams (1968×2184, 1440p120, 1080p60; HEVC and AV1) negotiated 10-bit and decoded in hardware | 10-bit P010, BT.2020 and PQ on every row. |
| **4K HDR** | HDR streaming at 3840×2160, 120 Hz, HEVC | The client receives the HDR state once per stream ([#11](https://github.com/RamazanKara/Rubylight/issues/11)). |

The native HDR tests include four reference-frame pixel dumps. Their per-channel mean error is below 0.51 ten-bit code values.

PyroWave streams carry 10-bit 4:4:4, validated here with encrypted 1080p/120 transport and independent decoding.

[Native HDR evidence](../rust/PERFORMANCE.md#final-native-virtual-hdr-pixels-excluding-physical-panel-calibration) · [PyroWave transport](../rust/PERFORMANCE.md#vibepollo-20-pyrowave-transport)

## CPU and correctness work

Video error correction uses **21–29% less CPU time** than the original C++ implementation on the recorded video blocks, with byte-identical parity. [FEC sources and reproduction](../rust/PERFORMANCE.md#controlled-comparison-with-the-original-c-fec) keep this component result separate from stream latency.

The [Windows workflow](../.github/workflows/rust-windows.yml) runs the Rust and release-script tests, web-console checks, formatting and Clippy for relevant code changes. Documentation-only changes run the [Pages build](../.github/workflows/pages.yml). These checks validate software and documentation; they do not replace hardware measurements. [Build and validation guide](building.md#development-checks).

## Measure your setup

Start with the [troubleshooting guide](troubleshooting.md) to collect the actual capture backend, codec, stream settings, GPU/driver, network type and client statistics. Compare one setting at a time with the same source motion and game load.

For development, the [full record](../rust/PERFORMANCE.md#reproduce-on-another-machine) documents capture/encode probes, picture-ID fixtures and independent decoding. Use a release build and record display restoration for fixtures that change the display layout.

To compare static-screen recovery, build the independent receiver, set `BUTTERPOLLO_TEST_IDR_PROBE=10` and run `python rust/tests/interop.py C:\path\to\artifacts hevc 1920 1080 60 25 20000 1` against each [isolated test host](../rust/PERFORMANCE.md#reproduce-on-another-machine). Keep the desktop still: no motion probe, `BUTTERPOLLO_TEST_REQUIRE_MOTION` or `BUTTERPOLLO_TEST_MIN_FPS`. After `BUTTERPOLLO_TEST_WARMUP_SECONDS` (default 2), `LiRequestIdrFrame` requests are at least 1.5 seconds apart, with one outstanding request. Allow an explicit duration of at least warm-up + N × 1.5 seconds. `IDR_PROBE` summarizes request-to-fully-assembled-IDR latency; `<BUTTERPOLLO_TEST_TIMING_CSV>.idr.csv` (default `idr-probe.csv`) records request, arrival and decode-completion times on Moonlight's local monotonic clock. Missing values are −1 and incomplete probes fail. Use identical fixture settings for both hosts; unset the probe variable for normal behavior.
