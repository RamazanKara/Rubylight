# Performance and validation

[Docs](README.md) · [How compute works](architecture.md) · [Troubleshooting](troubleshooting.md) · [Full measurement record](../rust/PERFORMANCE.md)

Rubylight's performance work targets fresh pictures and lower picture age. This guide collects the main results; each table links to the fixture, exact runs and scope behind it. Two kinds of measurement appear: **end-to-end runs to a client over Wi-Fi**, and **same-PC runs**, where an independent decoder on the host PC isolates the host from the network so two builds or two hosts can be compared precisely.

## What the numbers mean

| Metric | What it measures |
| --- | --- |
| **Render-to-decode delay / picture age** | Time from a timestamped rendered picture to independent decoding, including desktop composition, capture, encoding and transport. The Wi-Fi runs include real network transit; the same-PC runs use loopback, which isolates the host from the network. |
| **95th percentile (p95)** | The slower end of the sample: 95% of observations are at or below this delay. |
| **Fresh pictures per second** | Distinct pictures received, counted from changing picture IDs. Repeated frames do not increase this count. |
| **Encoder time** | The encoder's submission-to-completion interval. It covers one part of the frame journey. |
| **`host_mean_ms`, `host_p95_ms`, `host_p99_ms`, `host_max_ms`** | Claim to the pre-packetization sample, matching Moonlight's host latency. The session API calls these `host_processing_*`. Capture age and final-packet sending are separate. |

Each table below names its metric, capture path, source and client.

## On a real client over Wi-Fi

The host on Ethernet streamed to a client on **5 GHz Wi-Fi**, decoding in hardware. The client's clock was synchronised to the host with 200 ms pings (offset error about ±0.6 ms), so each rendered picture's age is measured on arrival. HDR on every row, one 30-second run per row on October 8, 2026.

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

## Next to Vibepollo 2.0

**On the same GPU, with identical settings,** Rubylight and the pinned Vibepollo 2.0 build alternated in one batch on October 8 (two runs per cell): native AMF at ultra-low latency, Desktop Duplication capture, a virtual HDR display, hardware decoding in the client. Picture age, average / p95 / p99:

| | Rubylight | Vibepollo 2.0 |
| --- | ---: | ---: |
| **Beside a game**, 1968×2184 AV1 HDR 120 fps | **17.15 / 18.96 / 22.18 ms**, host 3.1 ms | 24.53 / 36.77 / 38.05 ms, host 7.4 ms |
| **Beside a game**, 1968×2184 HEVC HDR 120 fps | **19.61 / 21.56 / 23.10 ms**, host 3.4 ms | 26.96 / 37.62 / 39.25 ms, host 8.3 ms |
| No game, 1968×2184 AV1 HDR 120 fps | **17.42 / 18.10 / 18.62 ms** | 18.88 / 19.54 / 19.90 ms |
| No game, 1968×2184 HEVC HDR 120 fps | **17.81 / 18.94 / 19.56 ms** | 19.97 / 20.43 / 20.87 ms |
| No game, 2560×1440 HEVC HDR 120 fps | 17.09 / 17.99 / 18.46 ms | 17.19 / 17.68 / 17.93 ms |

At 1968×2184 Rubylight delivers the picture 1.5–2.2 ms sooner with no game running and 7.4–7.8 ms sooner beside one, with about 16–18 ms less at the 95th and 99th percentiles. Beside the game Vibepollo also delivered fewer fresh pictures (103–118 a second against 117–120). At 1440p the two are equal with no game, and at 1080p60 they are close. [All rows, including 1080p60 →](../rust/PERFORMANCE_WORK.md#rc24-against-vibepollo-20-on-the-same-gpu)

An earlier whole-host comparison on October 4 showed the same pattern at a heavier load. It used **Vibepollo 2.0** and an early Rubylight build with the same settings: native AMF at ultra-low latency, DDX and realtime GPU priority on both. 1080p60 HEVC HDR, 20 Mbps requested, controlled GPU load; arithmetic means of three alternating runs per host on October 4, 2026.

| Under controlled GPU load | Vibepollo 2.0 | Rubylight |
| --- | ---: | ---: |
| Average render-to-decode delay | 96.4 ms | **42.4 ms** |
| Mean per-run 95th-percentile delay | 137.0 ms | **56.5 ms** |
| Fresh pictures per second | 23.9 | **51.4** |
| Game frame rate beside the host | 176.7–177.4 fps | 173.8–174.9 fps |

The idle comparison measured 16.0 ms versus 13.8 ms. Most of the loaded gap is not the compute path: with compute off, Rubylight still delivered about 57 fresh pictures a second beside the same load in an earlier batch, and the [same-build comparison](#isolating-radeon-compute) puts the compute gain at 7.5 ms. Vibepollo handed its native AMF encoder about 24 frames a second, and the encoder logged that its output had not caught up. That encoder came from Rubylight's author ([Vibepollo #342](https://github.com/Nonary/Vibepollo/pull/342)).

[Baseline revision, workload and recorded runs →](../rust/PERFORMANCE.md#against-vibepollo-20)

## Isolating Radeon compute

This separate test changes one setting in the **same Rubylight build**: compute off versus compute on. DDX, 1080p60 HEVC HDR, controlled GPU load; arithmetic means of two runs per path on October 4, 2026.

| Render-to-decode delay | Compute off | Compute on |
| --- | ---: | ---: |
| Average | 41.0 ms | **33.5 ms** |
| Mean per-run 95th percentile | 54.4 ms | **42.3 ms** |

This isolates the benefit of moving frame preparation onto compute. The whole-host result above includes the combined implementation. [Queue placement and synchronization](architecture.md#the-radeon-frame-path) explain the change.

[Same-build runs and component probes →](../rust/PERFORMANCE.md#1080p-at-60-fps)

## Radeon under pressure

Two changes target Radeon cards under pressure, measured against the code before them. Idle delay on the fixture above stayed at 14.9 ms.

| | Before | After |
| --- | ---: | ---: |
| **PyroWave beside a GPU-heavy game**, per 1080p120 HDR 4:4:4 frame | 5.7 ms | **0.55 ms** |
| PyroWave beside the game, paced at 120 fps | 4.62 ms | **0.71 ms** |
| **Encoder saturated** (5120×1440 HEVC at 240 fps), game frame to packet | 42.7 ms | **11.1 ms** |
| Encoder saturated, 99th percentile | 45.9 ms | **13.3 ms** |

PyroWave's colour conversion now runs on the Radeon compute queue instead of waiting behind the game on the graphics queue; idle it takes 0.47 ms either way, and the planes are byte-identical. When the encoder cannot keep up, the host now claims a new picture only while fewer than two wait in the encoder, so the frames it sends are fresh; the encoder delivered 220 fps either way.

[PyroWave runs →](../rust/PERFORMANCE.md#october-7-pyrowave-conversion-on-the-compute-queue) · [Encoder queue runs →](../rust/PERFORMANCE.md#october-7-two-frames-in-the-encoder)

**AMF's low-latency switches were checked too.** With the default ultra-low-latency usage, the driver already applies its internal low-latency mode and AV1's lowest latency; forcing them changed neither encode time nor output size, so they stay on Driver default. [When forcing them helps →](configuration.md#capture-and-video)

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

In the controlled local 720p60 AV1 comparison, guarded pacing delivered **59.862–60.000 fresh pictures/s**, versus **57.650**, and reduced estimated source-presentation-to-software-decode age by about **3.8 ms on average**. The default-path SDR check recorded **59.999 fresh pictures/s**.

These WGC results use a different fixture from the DDX comparisons above. The full record includes slower sources, 120 FPS operation, helper recovery and GPU saturation.

[Pacing comparisons](../rust/PERFORMANCE.md#guarded-ab-stress-and-compatibility-checks) · [Default-path checks](../rust/PERFORMANCE.md#hdr-state-correction-and-current-sdr-validation)

## HDR and PyroWave validation

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

Every push to main runs **589 Rust tests**, the release-script, web console and video-settings tests, formatting and Clippy with warnings denied. [Release notes](../rust/RELEASE_NOTES.md)

## Measure your setup

Start with the [troubleshooting guide](troubleshooting.md) to collect the actual capture backend, codec, stream settings, GPU/driver, network type and client statistics. Compare one setting at a time with the same source motion and game load.

For development, the [full record](../rust/PERFORMANCE.md#reproduce-on-another-machine) documents capture/encode probes, picture-ID fixtures and independent decoding. Use a release build and record display restoration for fixtures that change the display layout.

To compare static-screen recovery, build the independent receiver, set `BUTTERPOLLO_TEST_IDR_PROBE=10` and run `python rust/tests/interop.py C:\path\to\artifacts hevc 1920 1080 60 25 20000 1` against each [isolated test host](../rust/PERFORMANCE.md#reproduce-on-another-machine). Keep the desktop still: no motion probe, `BUTTERPOLLO_TEST_REQUIRE_MOTION` or `BUTTERPOLLO_TEST_MIN_FPS`. After `BUTTERPOLLO_TEST_WARMUP_SECONDS` (default 2), `LiRequestIdrFrame` requests are at least 1.5 seconds apart, with one outstanding request. Allow an explicit duration of at least warm-up + N × 1.5 seconds. `IDR_PROBE` summarizes request-to-fully-assembled-IDR latency; `<BUTTERPOLLO_TEST_TIMING_CSV>.idr.csv` (default `idr-probe.csv`) records request, arrival and decode-completion times on Moonlight's local monotonic clock. Missing values are −1 and incomplete probes fail. Use identical fixture settings for both hosts; unset the probe variable for normal behavior.
